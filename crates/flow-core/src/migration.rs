//! Deterministic v1 import. The host is responsible for atomic writes to a separate path.
use crate::{domain::*, model::State};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub const STATE_VERSION: u32 = 2;
pub const LEGACY_NODE_PROPERTY: &str = "flowwm.compatibility.v1";
/// Transitional completion-barrier and application state, without desktop handles or layout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyNode {
    pub title: String,
    pub dependencies: Vec<String>,
    pub run_when: crate::model::RunWhen,
    pub review: bool,
    pub writable: bool,
    pub status: crate::model::Status,
    pub output: String,
    pub metric: crate::model::Metric,
    pub chat: Vec<crate::model::ChatMessage>,
    pub agent_session: Option<String>,
}
impl LegacyNode {
    pub fn capture(node: &crate::model::Node) -> Self {
        Self {
            title: node.title.clone(),
            dependencies: node.dependencies.clone(),
            run_when: node.run_when,
            review: node.review,
            writable: node.writable,
            status: node.status,
            output: node.output.clone(),
            metric: node.metric.clone(),
            chat: node.chat.clone(),
            agent_session: node.agent_session.clone(),
        }
    }
    pub fn apply(self, node: &mut crate::model::Node) {
        node.title = self.title;
        node.dependencies = self.dependencies;
        node.run_when = self.run_when;
        node.review = self.review;
        node.writable = self.writable;
        node.status = self.status;
        node.output = self.output;
        node.metric = self.metric;
        node.chat = self.chat;
        node.agent_session = self.agent_session;
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresentationDocument {
    pub selected_workflow: WorkflowId,
    pub workflows: BTreeMap<WorkflowId, Value>,
    pub nodes: BTreeMap<NodeId, Value>,
}
impl PresentationDocument {
    pub fn capture(state: &State) -> Self {
        Self {
            selected_workflow: state.selected_workspace.clone().into(),
            workflows: state.workspaces.iter().map(|row| (row.id.clone().into(),
                serde_json::json!({"row":row.row,"view":row.view,"window_array":row.window_array,"graph_arranged":row.graph_arranged}))).collect(),
            nodes: state.nodes.iter().map(|node| (node.id.clone().into(),
                serde_json::json!({"position":node.position,"column":node.column,"column_mode":node.column_mode}))).collect(),
        }
    }
    pub fn apply(&self, state: &mut State) -> Result<()> {
        for row in &mut state.workspaces {
            if let Some(value) = self.workflows.get(&row.id.clone().into()) {
                row.row = serde_json::from_value(value["row"].clone())?;
                row.view = serde_json::from_value(value["view"].clone())?;
                row.window_array = serde_json::from_value(value["window_array"].clone())?;
                row.graph_arranged = serde_json::from_value(value["graph_arranged"].clone())?;
            }
        }
        for node in &mut state.nodes {
            if let Some(value) = self.nodes.get(&node.id.clone().into()) {
                node.position = serde_json::from_value(value["position"].clone())?;
                node.column = serde_json::from_value(value["column"].clone())?;
                node.column_mode = serde_json::from_value(value["column_mode"].clone())?;
            }
        }
        if state
            .workspaces
            .iter()
            .any(|w| w.id == self.selected_workflow.0)
        {
            state.selected_workspace = self.selected_workflow.0.clone();
        }
        Ok(())
    }
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
            if runtime.destroyed.contains(&id) || row.id == "__empty_presentation__" {
                continue;
            }
            let graph = FlowNet {
                nodes: state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == row.id)
                    .map(|n| NodeSpec {
                        id: n.id.clone().into(),
                        type_id: if n.flow.node_type.is_empty() {
                            format!(
                                "local.{}",
                                serde_json::to_value(n.kind).unwrap().as_str().unwrap()
                            )
                        } else {
                            n.flow.node_type.clone()
                        },
                        flow: {
                            let mut flow = n.flow.clone();
                            flow.properties
                                .insert("command".into(), serde_json::json!(n.command));
                            flow.properties
                                .insert("cwd".into(), serde_json::json!(n.cwd));
                            flow.properties
                                .insert("writable".into(), serde_json::json!(n.writable));
                            flow.properties
                                .insert("agent_session".into(), serde_json::json!(n.agent_session));
                            flow.properties.insert(
                                "conversational".into(),
                                serde_json::json!(!n.chat.is_empty()),
                            );
                            flow.properties.insert(
                                LEGACY_NODE_PROPERTY.into(),
                                serde_json::to_value(LegacyNode::capture(n)).unwrap(),
                            );
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
                if workflow.authority == runtime.host_id {
                    workflow.name = row.name.clone();
                    workflow.graph = graph;
                }
            } else {
                runtime.workflows.insert(
                    id.clone(),
                    Workflow {
                        id: id.clone(),
                        name: row.name.clone(),
                        workspace: row
                            .row
                            .as_ref()
                            .map(|_| WorkspaceId(format!("{}:desktop", runtime.host_id.0))),
                        visibility: Visibility::Public,
                        owner: None,
                        references: Default::default(),
                        graph,
                        authority: runtime.host_id.clone(),
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
