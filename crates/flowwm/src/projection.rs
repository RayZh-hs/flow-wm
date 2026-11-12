//! One-way WM view of the authoritative runtime. Never used for execution or runtime saves.
use crate::model::*;
use flow_core::domain::{RuntimeState, WorkflowId};
use std::collections::BTreeSet;
pub fn apply(state: &mut State, runtime: &RuntimeState) {
    let removed: BTreeSet<_> = state
        .workspaces
        .iter()
        .filter(|w| runtime.destroyed.contains(&WorkflowId(w.id.clone())))
        .map(|w| w.id.clone())
        .collect();
    state.workspaces.retain(|w| !removed.contains(&w.id));
    state.nodes.retain(|n| !removed.contains(&n.workspace));
    let ids: BTreeSet<_> = state.nodes.iter().map(|n| n.id.clone()).collect();
    state
        .connections
        .retain(|e| ids.contains(&e.source.node) && ids.contains(&e.target.node));
    for workflow in runtime.workflows.values() {
        if !state.workspaces.iter().any(|w| w.id == workflow.id.0) {
            state.workspaces.push(Workspace {
                id: workflow.id.0.clone(),
                name: workflow.name.clone(),
                cwd: std::path::PathBuf::from("."),
                notes: String::new(),
                row: None,
                window_array: true,
                graph_arranged: false,
                view: RowView::default(),
            });
        }
        let nodes: BTreeSet<_> = workflow
            .graph
            .nodes
            .iter()
            .map(|n| n.id.0.as_str())
            .collect();
        state
            .nodes
            .retain(|n| n.workspace != workflow.id.0 || nodes.contains(n.id.as_str()));
        for (index, spec) in workflow.graph.nodes.iter().enumerate() {
            let kind = match spec.type_id.as_str() {
                "local.shell" => NodeKind::Shell,
                "local.constant" => NodeKind::Constant,
                "local.python" => NodeKind::Python,
                "local.agent" => NodeKind::Agent,
                "local.metric" => NodeKind::Metric,
                "local.human" => NodeKind::Human,
                "local.gate" => NodeKind::Gate,
                "local.window" | "local.application" => NodeKind::Window,
                "local.display" => NodeKind::Display,
                "local.alert" => NodeKind::Alert,
                "local.vox" => NodeKind::Vox,
                "local.conditional" => NodeKind::Conditional,
                "local.break_signal" => NodeKind::BreakSignal,
                "local.merge_signal" => NodeKind::MergeSignal,
                _ => {
                    if workflow.graph.node_types.iter().any(|definition| {
                        definition.id == spec.type_id
                            && definition.behavior == flow_core::providers::Behavior::Application
                    }) {
                        NodeKind::Window
                    } else {
                        NodeKind::Extension
                    }
                }
            };
            let cwd = spec
                .flow
                .properties
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            let existing = state
                .nodes
                .iter()
                .position(|n| n.id == spec.id.0 && n.workspace == workflow.id.0);
            let mut node = existing.map(|i| state.nodes[i].clone()).unwrap_or_else(|| {
                Node::new(
                    &workflow.id.0,
                    kind,
                    cwd.into(),
                    Position {
                        x: 100. + index as f64 * 360.,
                        y: 80.,
                    },
                )
            });
            node.id = spec.id.0.clone();
            node.kind = kind;
            if workflow.graph.node_types.iter().any(|d| {
                d.id == spec.type_id && d.behavior == flow_core::providers::Behavior::Source
            }) {
                node.status = Status::Unavailable;
            }
            node.cwd = cwd.into();
            node.writable = spec
                .flow
                .properties
                .get("writable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let attention = node.flow.properties.get("_alert").cloned();
            node.flow = spec.flow.clone();
            if let Some(interval) = spec
                .flow
                .properties
                .get("interval_secs")
                .and_then(serde_json::Value::as_u64)
            {
                node.metric.interval_secs = interval;
            }
            if let Some(unit) = spec
                .flow
                .properties
                .get("unit")
                .and_then(serde_json::Value::as_str)
            {
                node.metric.unit = unit.into();
            }
            if let Some(attention) = attention {
                node.flow.properties.insert("_alert".into(), attention);
            }
            node.title = spec
                .flow
                .properties
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(node.kind.label())
                .into();
            node.flow.values = runtime
                .queue
                .computed
                .get(&spec.id)
                .cloned()
                .unwrap_or_default();
            if let Some(values) = runtime.queue.published.get(&spec.id) {
                node.flow.values.extend(values.clone());
            }
            if kind == NodeKind::Extension || !spec.type_id.starts_with("local.") {
                node.flow.node_type = spec.type_id.clone();
            }
            if let Some(command) = spec.flow.properties.get("command").and_then(|v| v.as_str()) {
                node.command = command.into();
            }
            if let Some(index) = existing {
                state.nodes[index] = node;
            } else {
                state.nodes.push(node);
            }
        }
        state.connections.retain(|edge| {
            !nodes.contains(edge.source.node.as_str()) && !nodes.contains(edge.target.node.as_str())
        });
        state.connections.extend(workflow.graph.connections.clone());
        for definition in &workflow.graph.node_types {
            if !state.node_types.iter().any(|d| d.id == definition.id) {
                state.node_types.push(definition.clone());
            }
        }
        for definition in &workflow.graph.types {
            if !state.types.iter().any(|d| d.name == definition.name) {
                state.types.push(definition.clone());
            }
        }
        for definition in &workflow.graph.casts {
            if !state
                .casts
                .iter()
                .any(|d| d.from == definition.from && d.to == definition.to)
            {
                state.casts.push(definition.clone());
            }
        }
        if let Some(row) = state.workspaces.iter_mut().find(|w| w.id == workflow.id.0) {
            row.name = workflow.name.clone();
            if workflow
                .workspace
                .as_ref()
                .is_some_and(|w| w.0 != format!("{}:desktop", runtime.host_id.0))
            {
                row.row = None;
            }
        }
    }
    let mut executions = runtime.executions.values().collect::<Vec<_>>();
    for node in &mut state.nodes {
        if node
            .flow
            .properties
            .get("conversational")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            node.chat.clear();
        }
    }
    executions.sort_by_key(|e| e.generation);
    for execution in executions {
        if let Some(node) = state.node_mut(&execution.node.0)
            && node.workspace == execution.workflow.0
        {
            if node
                .flow
                .properties
                .get("conversational")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                if let Some(prompt) = runtime
                    .queue
                    .invocations
                    .get(&execution.id)
                    .and_then(|c| c.request.properties.get("command"))
                    .and_then(serde_json::Value::as_str)
                {
                    node.chat.push(ChatMessage {
                        role: "user".into(),
                        text: prompt.into(),
                    });
                }
                if !execution.active()
                    && let Some(answer) = execution
                        .result
                        .get("stdout")
                        .or_else(|| execution.result.get("error"))
                        .and_then(serde_json::Value::as_str)
                {
                    node.chat.push(ChatMessage {
                        role: "assistant".into(),
                        text: answer.into(),
                    });
                }
            }
            if let Some(session) = execution
                .result
                .get("agent_session")
                .and_then(serde_json::Value::as_str)
                && node
                    .flow
                    .properties
                    .get("harness")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|harness| {
                        execution
                            .result
                            .get("harness")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|actual| actual == harness)
                    })
            {
                node.agent_session = Some(session.into());
            }
            if execution.generation > node.attention_generation
                && let Some(attention) = execution.result.get("attention")
            {
                node.flow
                    .properties
                    .insert("_alert".into(), attention.clone());
                node.attention_generation = execution.generation;
            }
            use flow_core::domain::ExecutionStatus;
            node.status = match execution.status {
                ExecutionStatus::Queued => Status::Queued,
                ExecutionStatus::Waiting => Status::Review,
                ExecutionStatus::Running | ExecutionStatus::Interrupting => Status::Running,
                ExecutionStatus::Complete => Status::Done,
                ExecutionStatus::Failed => Status::Failed,
                ExecutionStatus::Interrupted => Status::Stopped,
                ExecutionStatus::Unavailable => Status::Unavailable,
            };
            if !execution.active() {
                if let Some(values) = execution.result.as_object() {
                    node.flow.values.extend(
                        values
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone())),
                    );
                }
                node.output = execution
                    .result
                    .get("stdout")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| execution.result.to_string());
            }
        }
    }
    if state.workspaces.is_empty() {
        // An empty presentation row is not a new runtime workflow.
        let mut empty = State::empty(std::path::PathBuf::from("."));
        let mut row = empty.workspaces.remove(0);
        row.id = "__empty_presentation__".into();
        row.name = "No workflows".into();
        state.workspaces.push(row);
    }
    if !state
        .workspaces
        .iter()
        .any(|w| w.id == state.selected_workspace)
    {
        state.selected_workspace = state.workspaces[0].id.clone();
    }
    for source in runtime.sources.values() {
        if let Some(node) = state.node_mut(&source.node.0) {
            node.status = Status::Watching;
            if let Some(interval) = source
                .properties
                .get("interval_secs")
                .and_then(serde_json::Value::as_u64)
            {
                node.metric.interval_secs = interval;
            }
            if let Some(samples) = source.state.get("samples") {
                node.metric.samples = serde_json::from_value(samples.clone()).unwrap_or_default();
            }
        }
    }
    for frame in &runtime.queue.frames {
        if frame.pin.is_empty()
            && let Some(node) = state.node_mut(&frame.root.0)
        {
            node.status = Status::Queued;
        }
    }
    state.orchestration = Some(std::sync::Arc::new(runtime.clone()));
}
