//! Deterministic v1 import. The host is responsible for atomic writes to a separate path.
use crate::{domain::*, model::State};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub const STATE_VERSION: u32 = 2;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PresentationDocument {
    pub selected_workflow: WorkflowId,
    pub workflows: BTreeMap<WorkflowId, Value>,
    pub nodes: BTreeMap<NodeId, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub runtime: RuntimeState,
    /// Lossless compatibility data, including recipes and completion barriers.
    pub compatibility: State,
}
impl Envelope {
    pub fn from_v1(mut state: State) -> Result<(Self, PresentationDocument)> {
        crate::graph::preserve_legacy_window_outputs(&mut state.nodes, &state.connections);
        for recipe in &mut state.recipes {
            crate::graph::preserve_legacy_window_outputs(&mut recipe.nodes, &recipe.connections);
        }
        state.validate()?;
        let mut runtime = state
            .orchestration
            .clone()
            .unwrap_or_else(RuntimeState::local);
        let mut presentation = PresentationDocument {
            selected_workflow: state.selected_workspace.clone().into(),
            ..Default::default()
        };
        for row in &state.workspaces {
            let id = WorkflowId(row.id.clone());
            let graph = FlowNet {
                nodes: state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == row.id)
                    .map(|n| NodeSpec {
                        id: n.id.clone().into(),
                        type_id: if n.flow.node_type.is_empty() { format!("local.{}", serde_json::to_value(n.kind).unwrap().as_str().unwrap()) } else { n.flow.node_type.clone() },
                        flow: {
                            let mut flow = n.flow.clone();
                            flow.properties.insert("command".into(), serde_json::json!(n.command));
                            flow.properties.insert("cwd".into(), serde_json::json!(n.cwd));
                            flow
                        },
                    })
                    .collect(),
                connections: state
                    .connections
                    .iter()
                    .filter(|e| {
                        state
                            .node(&e.source.node)
                            .is_some_and(|n| n.workspace == row.id)
                    })
                    .cloned()
                    .collect(),
                types: state.types.clone(),
                casts: state.casts.clone(),
                node_types: state.node_types.clone(),
            };
            if let Some(workflow) = runtime.workflows.get_mut(&id) {
                workflow.name = row.name.clone();
                workflow.graph = graph;
            } else {
                runtime.workflows.insert(
                    id.clone(),
                    Workflow {
                        id: id.clone(),
                        name: row.name.clone(),
                        workspace: row.row.as_ref().map(|_| WorkspaceId::from("local:desktop")),
                        visibility: Visibility::Public,
                        owner: None,
                        references: Default::default(),
                        graph,
                        authority: "local".into(),
                        transfer: None,
                    },
                );
            }
            presentation.workflows.insert(id, serde_json::json!({"row":row.row, "view":row.view, "window_array":row.window_array, "graph_arranged":row.graph_arranged}));
        }
        for node in &state.nodes {
            presentation.nodes.insert(node.id.clone().into(), serde_json::json!({"position":node.position, "column":node.column, "column_mode":node.column_mode}));
        }
        runtime.validate()?;
        state.orchestration = None;
        Ok((
            Self {
                version: STATE_VERSION,
                runtime,
                compatibility: state,
            },
            presentation,
        ))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == STATE_VERSION,
            "Unsupported state version {}",
            self.version
        );
        self.compatibility.validate()?;
        self.runtime.validate()
    }
    pub fn into_compatibility(mut self) -> Result<State> {
        self.validate()?;
        self.compatibility.orchestration = Some(self.runtime);
        Ok(self.compatibility)
    }
}
