//! Desktop view helpers. Runtime evaluation uses FlowNet directly.
use crate::model::*;
use anyhow::{Result, ensure};
pub use flow_core::graph::*;
use serde_json::Value;
impl Node {
    pub fn spec(&self) -> flow_core::domain::NodeSpec {
        let type_id = if self.flow.node_type.is_empty() {
            format!(
                "local.{}",
                serde_json::to_value(self.kind).unwrap().as_str().unwrap()
            )
        } else {
            self.flow.node_type.clone()
        };
        let mut flow = self.flow.clone();
        flow.values.clear();
        flow.properties.remove("_alert");
        flow.properties
            .insert("command".into(), serde_json::json!(self.command));
        flow.properties.remove("cwd");
        if !self.cwd.as_os_str().is_empty() {
            flow.properties
                .insert("cwd".into(), serde_json::json!(self.cwd));
        }
        if self.kind == NodeKind::Agent {
            flow.properties
                .entry("writable".into())
                .or_insert(serde_json::json!(self.writable));
        }
        flow_core::domain::NodeSpec {
            id: self.id.clone().into(),
            type_id,
            flow,
        }
    }
}
impl State {
    pub fn graph(&self) -> flow_core::domain::FlowNet {
        flow_core::domain::FlowNet {
            nodes: self.nodes.iter().map(Node::spec).collect(),
            connections: self.connections.clone(),
            types: self.types.clone(),
            casts: self.casts.clone(),
            node_types: self.node_types.clone(),
        }
    }
}
pub fn type_id(node: &Node) -> &str {
    if !node.flow.node_type.is_empty() {
        return &node.flow.node_type;
    }
    match node.kind {
        NodeKind::Constant => "constant",
        NodeKind::Display => "display",
        NodeKind::Alert => "alert",
        NodeKind::BreakSignal => "break_signal",
        NodeKind::MergeSignal => "merge_signal",
        NodeKind::Conditional => "conditional",
        NodeKind::Vox => "vox",
        NodeKind::Python => "python",
        NodeKind::Window => "window",
        NodeKind::Extension => "extension",
        _ => "workflow",
    }
}
pub fn pins(state: &State, node: &Node, output: bool) -> Vec<Pin> {
    let spec = node.spec();
    flow_core::graph::pins_for(
        &spec,
        state.node_types.iter().find(|d| d.id == spec.type_id),
        output,
    )
}
pub fn constant_type(node: &Node) -> Type {
    flow_core::graph::constant_type(&node.spec())
}
pub fn property(state: &State, node: &Node, name: &str) -> Option<Value> {
    node.flow.properties.get(name).cloned().or_else(|| {
        let spec = node.spec();
        state
            .node_types
            .iter()
            .find(|d| d.id == spec.type_id)
            .and_then(|d| d.properties.get(name))
            .and_then(|p| p.default.clone())
    })
}
pub fn validate(state: &State) -> Result<()> {
    flow_core::graph::validate(&state.graph())
}
pub fn connect(state: &mut State, edge: Connection) -> Result<()> {
    let a = state
        .node(&edge.source.node)
        .ok_or_else(|| anyhow::anyhow!("Source missing"))?;
    let b = state
        .node(&edge.target.node)
        .ok_or_else(|| anyhow::anyhow!("Target missing"))?;
    ensure!(
        a.workspace == b.workspace,
        "Connections must remain inside a workflow"
    );
    let mut graph = state.graph();
    flow_core::graph::connect(&mut graph, edge)?;
    state.connections = graph.connections;
    Ok(())
}
pub fn check_value(state: &State, value: &Value, ty: &Type) -> Result<()> {
    flow_core::graph::check_value(&state.graph(), value, ty)
}
pub fn can_cast(state: &State, from: &Type, to: &Type) -> bool {
    flow_core::graph::can_cast(&state.graph(), from, to)
}
pub fn pin_label(node: &Node, pin: &Pin, output: bool) -> String {
    match (pin.name.as_str(), output) {
        ("env", _) => "Env".into(),
        ("in", false) => "Run".into(),
        ("out", true) if !node.kind.primitive() => "Completed".into(),
        ("out", true) => "Continue".into(),
        ("failed", true) => "Failed".into(),
        ("output", true) => "Output text".into(),
        ("on_exit", true) => "On exit".into(),
        ("on_session_finish", true) => "Session finished".into(),
        _ => pin.name.replace('_', " "),
    }
}
pub fn pin_description(pin: &Pin, output: bool) -> &'static str {
    match (pin.name.as_str(), output) {
        ("env", false) => {
            "Environment for this run; unconnected inputs inherit the row environment"
        }
        ("env", true) => "Complete environment; connect to a node's Env input",
        ("in", false) => "Start this step when an event arrives",
        ("out", true) => "Emits after successful completion",
        ("failed", true) => "Emits when this step fails; connect a recovery step",
        ("output", true) => "Text produced by this step; read on demand",
        ("on_exit", true) => "Emits when the application process exits; exit_code is UInt8",
        ("on_session_finish", true) => "Agent session ended; ERROR describes any failure",
        _ if pin.ty.is_signal() => "Event connection",
        _ => "Data value; evaluated on demand",
    }
}
