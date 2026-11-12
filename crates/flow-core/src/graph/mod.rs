//! Typed pins, extension definitions, and the Vox graph evaluator.
mod bindings;
mod runtime;
mod types;

pub use runtime::{Activation, Evaluation, evaluate, signal_environment};
pub use runtime::{EvaluationContext, ScriptEvaluator, evaluate_with};
pub use types::*;

use crate::domain::{FlowNet, NodeSpec as Node};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub type Record = BTreeMap<String, Value>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Flow {
    #[serde(default = "editable_default")]
    pub editable_in_graph: bool,
    /// Extension node definition, or a builtin ID such as `constant`.
    #[serde(default)]
    pub node_type: String,
    #[serde(default)]
    pub properties: Record,
    /// Additional pins, including exposed properties and typed script bindings.
    #[serde(default)]
    pub inputs: Vec<Pin>,
    #[serde(default)]
    pub outputs: Vec<Pin>,
    /// Per-frame values, overlaid exclusively from EvaluationContext.
    #[serde(default)]
    pub values: Record,
}

fn editable_default() -> bool {
    true
}
impl Default for Flow {
    fn default() -> Self {
        Self {
            editable_in_graph: true,
            node_type: String::new(),
            properties: Default::default(),
            inputs: vec![],
            outputs: vec![],
            values: Default::default(),
        }
    }
}
pub fn infer_type(value: &Value) -> Type {
    match value {
        Value::Bool(_) => Type::Bool,
        Value::Number(n) if n.is_i64() => Type::Int,
        Value::Number(n) if n.is_u64() => Type::UInt,
        Value::Number(_) => Type::Float,
        Value::String(_) => Type::String,
        Value::Array(_) => Type::List {
            item: Box::new(Type::Any),
        },
        Value::Object(_) => Type::record(),
        _ => Type::Any,
    }
}
pub fn constant_type(node: &Node) -> Type {
    node.flow
        .outputs
        .iter()
        .find(|p| p.name == "value")
        .map(|p| p.ty.clone())
        .unwrap_or_else(|| {
            node.flow
                .properties
                .get("value")
                .map(infer_type)
                .unwrap_or(Type::Any)
        })
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pin {
    pub name: String,
    pub ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}
impl Pin {
    pub fn new(name: &str, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
            property: None,
            default: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub node: String,
    pub pin: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Connection {
    pub source: Endpoint,
    pub target: Endpoint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Property {
    pub ty: Type,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub output: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NodeDefinition {
    #[serde(default)]
    pub behavior: crate::providers::Behavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<crate::providers::WindowInterface>,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub inputs: Vec<Pin>,
    #[serde(default)]
    pub outputs: Vec<Pin>,
    #[serde(default)]
    pub properties: BTreeMap<String, Property>,
    /// Vox expression/body, with `inputs` and `args` record bindings. Returns a record.
    pub source: String,
}

pub fn type_id(node: &Node) -> &str {
    node.type_id.strip_prefix("local.").unwrap_or(&node.type_id)
}

pub fn pins(state: &FlowNet, node: &Node, output: bool) -> Vec<Pin> {
    pins_for(
        node,
        state.node_types.iter().find(|d| d.id == node.type_id),
        output,
    )
}
/// Inspect a single node without cloning a graph or requiring its other instances.
pub fn pins_for(node: &Node, definition: Option<&NodeDefinition>, output: bool) -> Vec<Pin> {
    let signal = || Type::trigger();
    let event = || Type::Signal {
        args: BTreeMap::new(),
        open: true,
    };
    let mut pins = if let Some(def) = definition {
        if output {
            def.outputs.clone()
        } else {
            def.inputs.clone()
        }
    } else {
        match (type_id(node), output) {
            ("constant", false) => vec![],
            ("constant", true) => vec![Pin::new("value", constant_type(node))],
            ("display", false) => vec![Pin {
                default: Some(Value::Null),
                ..Pin::new("value", Type::Any)
            }],
            ("display", true) => vec![Pin::new("value", Type::Any)],
            ("alert", false) => vec![
                Pin::new("in", event()),
                Pin {
                    property: Some("message".into()),
                    default: Some(json!("Needs your attention")),
                    ..Pin::new("message", Type::String)
                },
                Pin {
                    property: Some("critical".into()),
                    default: Some(json!(false)),
                    ..Pin::new("critical", Type::Bool)
                },
            ],
            ("alert", true) => vec![Pin::new("out", event())],
            ("break_signal", false) => vec![Pin::new("in", event())],
            ("break_signal", true) => {
                vec![Pin::new("out", signal()), Pin::new("args", Type::record())]
            }
            ("merge_signal", false) => vec![
                Pin::new("in", event()),
                Pin {
                    default: Some(json!({})),
                    ..Pin::new("args", Type::record())
                },
            ],
            ("merge_signal", true) => vec![Pin::new("out", event())],
            ("conditional", false) => vec![
                Pin::new("in", event()),
                Pin {
                    default: Some(json!(false)),
                    ..Pin::new("condition", Type::Bool)
                },
            ],
            ("conditional", true) => vec![Pin::new("true", event()), Pin::new("false", event())],
            ("vox" | "python", false) => vec![Pin::new("in", event())],
            ("vox" | "python", true) => {
                vec![Pin::new("out", event()), Pin::new("value", Type::Any)]
            }
            ("window", true) => vec![Pin::new(
                "on_exit",
                Type::Signal {
                    args: BTreeMap::from([("exit_code".into(), Type::UInt8)]),
                    open: false,
                },
            )],
            (_, false) => vec![Pin::new("in", event())],
            (_, true) => vec![
                Pin::new("out", event()),
                Pin::new("failed", event()),
                Pin::new("output", Type::String),
            ],
        }
    };
    if output
        && type_id(node) == "constant"
        && let Some(value) = pins.iter_mut().find(|p| p.name == "value")
    {
        value.ty = constant_type(node);
    }
    for pin in if output {
        &node.flow.outputs
    } else {
        &node.flow.inputs
    } {
        if let Some(existing) = pins.iter_mut().find(|p| p.name == pin.name) {
            *existing = pin.clone();
        } else {
            pins.push(pin.clone());
        }
    }
    pins
}

/// Human names leave persisted endpoint IDs stable for extensions and saved graphs.
pub fn property(state: &FlowNet, node: &Node, name: &str) -> Option<Value> {
    node.flow.properties.get(name).cloned().or_else(|| {
        state
            .node_types
            .iter()
            .find(|d| d.id == node.type_id)
            .and_then(|d| d.properties.get(name))
            .and_then(|p| p.default.clone())
    })
}

pub fn validate(state: &FlowNet) -> Result<()> {
    validate_registry(state)?;
    for node in &state.nodes {
        if type_id(node) == "constant"
            && let Some(value) = node.flow.properties.get("value")
        {
            check_value(state, value, &constant_type(node))?;
        }
        ensure!(
            matches!(
                type_id(node),
                "constant"
                    | "display"
                    | "alert"
                    | "break_signal"
                    | "merge_signal"
                    | "conditional"
                    | "vox"
                    | "python"
                    | "window"
                    | "shell"
                    | "agent"
                    | "human"
                    | "gate"
                    | "metric"
            ) || state.node_types.iter().any(|d| d.id == node.type_id),
            "Unknown node type: {}",
            type_id(node)
        );
        for output in [false, true] {
            let mut names = HashSet::new();
            for pin in pins(state, node, output) {
                validate_name(&pin.name)?;
                ensure!(names.insert(pin.name.clone()), "Duplicate pin {}", pin.name);
                validate_type(state, &pin.ty, 0)?;
                if let Some(value) = &pin.default {
                    check_value(state, value, &pin.ty)?;
                }
                if let Some(name) = &pin.property {
                    validate_name(name)?;
                    ensure!(!pin.ty.is_signal(), "A property pin must carry data");
                }
            }
        }
        if let Some(def) = state.node_types.iter().find(|d| d.id == node.type_id) {
            for (name, prop) in &def.properties {
                if let Some(value) = node.flow.properties.get(name) {
                    check_value(state, value, &prop.ty)?;
                }
                if prop.output
                    && let Some(value) = node.flow.values.get(name)
                {
                    check_value(state, value, &prop.ty)?;
                }
            }
        }
        for pin in pins(state, node, true)
            .into_iter()
            .filter(|p| !p.ty.is_signal())
        {
            if let Some(value) = node
                .flow
                .values
                .get(pin.property.as_ref().unwrap_or(&pin.name))
            {
                check_value(state, value, &pin.ty)?;
            }
        }
    }
    let mut edges = HashSet::new();
    let mut data_inputs = HashSet::new();
    for edge in &state.connections {
        ensure!(
            edges.insert((&edge.source, &edge.target)),
            "Duplicate connection"
        );
        let source = state
            .node(&edge.source.node)
            .context("Missing source node")?;
        let target = state
            .node(&edge.target.node)
            .context("Missing target node")?;
        let from = pins(state, source, true)
            .into_iter()
            .find(|p| p.name == edge.source.pin)
            .context("Missing output pin")?;
        let to = pins(state, target, false)
            .into_iter()
            .find(|p| p.name == edge.target.pin)
            .context("Missing input pin")?;
        ensure!(
            from.ty.is_signal() == to.ty.is_signal(),
            "Signal and data pins cannot be connected"
        );
        ensure!(
            can_cast(state, &from.ty, &to.ty),
            "Cannot cast {:?} to {:?}",
            from.ty,
            to.ty
        );
        if !to.ty.is_signal() {
            ensure!(
                data_inputs.insert(&edge.target),
                "A data input accepts one connection"
            );
        }
    }
    fn visit<'a>(
        state: &'a FlowNet,
        id: &'a str,
        active: &mut HashSet<&'a str>,
        done: &mut HashSet<&'a str>,
    ) -> Result<()> {
        if done.contains(id) {
            return Ok(());
        }
        ensure!(active.insert(id), "Data connections contain a cycle");
        for edge in state.connections.iter().filter(|e| e.target.node == id) {
            let source = state.node(&edge.source.node).unwrap();
            if pins(state, source, true)
                .iter()
                .any(|p| p.name == edge.source.pin && !p.ty.is_signal())
            {
                visit(state, &source.id.0, active, done)?;
            }
        }
        active.remove(id);
        done.insert(id);
        Ok(())
    }
    let mut done = HashSet::new();
    for node in &state.nodes {
        visit(state, &node.id.0, &mut HashSet::new(), &mut done)?;
    }
    Ok(())
}

pub fn connect(state: &mut FlowNet, connection: Connection) -> Result<()> {
    if state.connections.contains(&connection) {
        return Ok(());
    }
    state.connections.push(connection);
    if let Err(error) = validate(state) {
        state.connections.pop();
        return Err(error);
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name.bytes().enumerate().all(|(i, c)| c == b'_'
                || c.is_ascii_alphabetic()
                || (i > 0 && c.is_ascii_digit())),
        "Invalid property or argument name: {name}"
    );
    Ok(())
}
