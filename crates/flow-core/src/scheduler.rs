//! Provider-aware delivery frames using the existing Vox traversal and data cache.
//! Hosts execute returned activations through the registry and feed completion signals
//! into a fresh frame. Reading data cannot start an invoked node or emit a signal.
use crate::{
    domain::*,
    graph::*,
    model::{Node, State},
    providers::*,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

struct Bindings {
    registry: Registry,
    scripts: Arc<dyn ScriptEvaluator>,
}
impl Bindings {
    fn definition(&self, node: &Node) -> Option<&Definition> {
        self.registry.definition(type_id(node)).ok().or_else(|| {
            let kind = serde_json::to_value(node.kind).ok()?;
            self.registry
                .definition(&format!("local.{}", kind.as_str()?))
                .ok()
        })
    }
}
impl ScriptEvaluator for Bindings {
    fn behavior(&self, node: &Node) -> Option<Behavior> {
        self.definition(node).map(|d| d.behavior.clone())
    }
    fn compute(&self, node: &Node, bindings: &Record, args: &Value) -> Option<Result<Value>> {
        let definition = self.definition(node)?;
        if definition.behavior != Behavior::Lazy {
            return None;
        }
        Some((|| {
            let mut properties: Record =
                serde_json::from_value(bindings.get("properties").cloned().unwrap_or(json!({})))?;
            properties.insert(
                "data_inputs".into(),
                bindings.get("inputs").cloned().unwrap_or(json!({})),
            );
            self.registry.provider(&definition.owner)?.read(Invocation {
                id: format!("data:{}", node.id).into(),
                workflow: node.workspace.clone().into(),
                node: node.id.clone().into(),
                type_id: definition.id.clone(),
                properties,
                args: args.clone(),
                generation: 0,
            })
        })())
    }
    fn evaluate(
        &self,
        node: &Node,
        bindings: &Record,
        args: &Value,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        self.scripts.evaluate(node, bindings, args, cancel)
    }
}

pub fn evaluate(
    mut state: State,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
    registry: Registry,
    scripts: Arc<dyn ScriptEvaluator>,
) -> Result<Evaluation> {
    // Registered native nodes carry schemas even when no Vox source is installed.
    for definition in registry.definitions() {
        if state
            .nodes
            .iter()
            .any(|n| n.flow.node_type == definition.id)
            && !state.node_types.iter().any(|d| d.id == definition.id)
        {
            state.node_types.push(NodeDefinition {
                id: definition.id.clone(),
                name: definition.id.clone(),
                inputs: definition.inputs.clone(),
                outputs: definition.outputs.clone(),
                properties: Default::default(),
                source: String::new(),
                window: definition.window.clone(),
            });
        }
    }
    evaluate_with(
        state,
        root,
        signal,
        cancel,
        Arc::new(Bindings { registry, scripts }),
    )
}

/// Project a portable definition into the evaluator's compatibility view. No native
/// window handles, desktop bindings or presentation are required to execute it.
pub fn graph_state(workflow: &Workflow) -> Result<State> {
    let mut state = State::empty(".".into());
    state.workspaces[0].id = workflow.id.0.clone();
    state.selected_workspace = workflow.id.0.clone();
    state.connections = workflow.graph.connections.clone();
    state.types = workflow.graph.types.clone();
    state.casts = workflow.graph.casts.clone();
    state.node_types = workflow.graph.node_types.clone();
    for spec in &workflow.graph.nodes {
        let kind = spec
            .type_id
            .strip_prefix("local.")
            .and_then(|id| serde_json::from_value(json!(id)).ok())
            .unwrap_or(crate::model::NodeKind::Extension);
        let mut node = Node::new(
            &workflow.id.0,
            kind,
            spec.flow
                .properties
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or(".")
                .into(),
            Default::default(),
        );
        node.id = spec.id.0.clone();
        node.flow = spec.flow.clone();
        if kind == crate::model::NodeKind::Extension {
            node.flow.node_type = spec.type_id.clone();
        }
        if let Some(command) = spec.flow.properties.get("command").and_then(Value::as_str) {
            node.command = command.into();
        }
        state.nodes.push(node);
    }
    Ok(state)
}
