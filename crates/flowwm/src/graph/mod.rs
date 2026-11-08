//! Typed pins, extension definitions, and the Vox graph evaluator.
mod bindings;
mod runtime;
mod types;
mod worker;

pub use runtime::{Activation, Evaluation, evaluate, signal_environment};
pub use types::*;
pub use worker::{evaluate_isolated, run_worker};

use crate::model::{Node, NodeKind, State};
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
    /// Last published output properties. Never used to cache pure computations.
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
    if !node.flow.node_type.is_empty() {
        return &node.flow.node_type;
    }
    match node.kind {
        NodeKind::Constant => "constant",
        NodeKind::Display => "display",
        NodeKind::BreakSignal => "break_signal",
        NodeKind::MergeSignal => "merge_signal",
        NodeKind::Conditional => "conditional",
        NodeKind::Vox => "vox",
        NodeKind::Python => "python",
        NodeKind::Extension => "extension",
        _ => "workflow",
    }
}

pub fn pins(state: &State, node: &Node, output: bool) -> Vec<Pin> {
    let signal = || Type::trigger();
    let event = || Type::Signal {
        args: BTreeMap::new(),
        open: true,
    };
    let mut pins = if let Some(def) = state.node_types.iter().find(|d| d.id == type_id(node)) {
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
            (_, false) => vec![Pin::new("in", event())],
            (_, true) => vec![
                Pin::new("out", event()),
                Pin::new("failed", event()),
                Pin::new("output", Type::String),
            ],
        }
    };
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
pub fn pin_label(node: &Node, pin: &Pin, output: bool) -> String {
    match (pin.name.as_str(), output) {
        ("in", false) => "Run".into(),
        ("out", true) if !node.kind.primitive() => "Completed".into(),
        ("out", true) => "Continue".into(),
        ("failed", true) => "Failed".into(),
        ("output", true) => "Output text".into(),
        ("on_session_finish", true) => "Session finished".into(),
        _ => pin.name.replace('_', " "),
    }
}
pub fn pin_description(pin: &Pin, output: bool) -> &'static str {
    match (pin.name.as_str(), output) {
        ("in", false) => "Start this step when an event arrives",
        ("out", true) => "Emits after successful completion",
        ("failed", true) => "Emits when this step fails; connect a recovery step",
        ("output", true) => "Text produced by this step; read on demand",
        ("on_session_finish", true) => "Agent session ended; ERROR describes any failure",
        _ if pin.ty.is_signal() => "Event connection",
        _ => "Data value; evaluated on demand",
    }
}

pub fn property(state: &State, node: &Node, name: &str) -> Option<Value> {
    node.flow.properties.get(name).cloned().or_else(|| {
        state
            .node_types
            .iter()
            .find(|d| d.id == type_id(node))
            .and_then(|d| d.properties.get(name))
            .and_then(|p| p.default.clone())
    })
}

pub fn validate(state: &State) -> Result<()> {
    validate_registry(state)?;
    for node in &state.nodes {
        if node.kind == NodeKind::Constant
            && let Some(value) = node.flow.properties.get("value")
        {
            check_value(state, value, &constant_type(node))?;
        }
        ensure!(
            node.kind != NodeKind::Extension
                || state.node_types.iter().any(|d| d.id == type_id(node)),
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
        if let Some(def) = state.node_types.iter().find(|d| d.id == type_id(node)) {
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
        ensure!(
            source.workspace == target.workspace,
            "Cross-workspace connection"
        );
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
        state: &'a State,
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
                visit(state, &source.id, active, done)?;
            }
        }
        active.remove(id);
        done.insert(id);
        Ok(())
    }
    let mut done = HashSet::new();
    for node in &state.nodes {
        visit(state, &node.id, &mut HashSet::new(), &mut done)?;
    }
    Ok(())
}

pub fn connect(state: &mut State, connection: Connection) -> Result<()> {
    let target = state
        .node(&connection.target.node)
        .context("Missing target node")?;
    ensure!(
        !target.status.active(),
        "Stop the target before changing its inputs"
    );
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
