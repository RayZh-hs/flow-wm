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
    let previous: std::collections::HashMap<_, _> = state
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.clone()))
        .collect();
    for workflow in runtime.workflows.values() {
        if !state.workspaces.iter().any(|w| w.id == workflow.id.0) {
            state.workspaces.push(Workspace {
                canvas: vec![],
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
        for spec in &workflow.graph.nodes {
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
                .unwrap_or("");
            let existing = state
                .nodes
                .iter()
                .position(|n| n.id == spec.id.0 && n.workspace == workflow.id.0);
            let needs_position = existing.is_none()
                && previous
                    .get(&spec.id.0)
                    .is_none_or(|n| n.workspace != workflow.id.0);
            let mut node = existing
                .map(|i| state.nodes[i].clone())
                .or_else(|| previous.get(&spec.id.0).cloned())
                .unwrap_or_else(|| {
                    Node::new(&workflow.id.0, kind, cwd.into(), Position::default())
                });
            node.id = spec.id.0.clone();
            node.workspace = workflow.id.0.clone();
            node.kind = kind;
            node.renderer = workflow
                .graph
                .node_types
                .iter()
                .chain(state.node_types.iter())
                .find(|d| d.id == spec.type_id)
                .and_then(|d| d.renderer.clone());
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
            if node.renderer.is_some()
                && let Some(definition) = workflow
                    .graph
                    .node_types
                    .iter()
                    .chain(state.node_types.iter())
                    .find(|d| d.id == spec.type_id)
            {
                for (name, property) in &definition.properties {
                    if let Some(value) = &property.default {
                        node.flow
                            .properties
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                }
            }
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
            if kind == NodeKind::Window {
                if let Some(name) = spec
                    .flow
                    .properties
                    .get("application_name")
                    .or_else(|| spec.flow.properties.get("application_id"))
                    .or_else(|| spec.flow.properties.get("application"))
                    .and_then(serde_json::Value::as_str)
                {
                    node.title = name.into();
                }
            } else {
                node.title.clear();
            }
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
            if needs_position {
                node.position =
                    crate::placement::Space::new(state, &node.workspace, |n| n.position.clone())
                        .append(
                            crate::placement::node_width(&node),
                            crate::placement::node_height(&node),
                        );
            }
            if let Some(index) = existing {
                state.nodes[index] = node;
            } else if let Some(index) = state.nodes.iter().position(|n| n.id == node.id) {
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
            row.name = workflow.row_name().unwrap_or(&workflow.name).to_owned();
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
            if source.type_id == "local.timer" {
                node.flow
                    .values
                    .insert("value".into(), serde_json::json!(source.sequence));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{CLOSED_HEIGHT, HEIGHT, WIDTH, gap};
    use flow_core::domain::{Visibility, Workflow};

    fn runtime(state: &State) -> RuntimeState {
        let mut runtime = RuntimeState::local();
        let workflow = Workflow {
            id: state.selected_workspace.clone().into(),
            name: "Placement".into(),
            row_settings: Default::default(),
            workspace: Some("local:desktop".into()),
            visibility: Visibility::Private,
            owner: None,
            references: Default::default(),
            graph: Default::default(),
            authority: runtime.host_id.clone(),
            transfer: None,
        };
        runtime.workflows.insert(workflow.id.clone(), workflow);
        runtime
    }

    #[test]
    fn timer_surface_uses_the_restored_source_counter() {
        let mut state = State::empty("/tmp".into());
        let mut runtime = runtime(&state);
        let spec = flow_core::domain::NodeSpec {
            id: "clock".into(),
            type_id: "local.timer".into(),
            flow: Default::default(),
        };
        runtime
            .workflows
            .values_mut()
            .next()
            .unwrap()
            .graph
            .nodes
            .push(spec);
        runtime.sources.insert(
            "clock".into(),
            flow_core::providers::SourceRegistration {
                environment: None,
                subscription: None,
                workflow: state.selected_workspace.clone().into(),
                node: "clock".into(),
                type_id: "local.timer".into(),
                provider: "local".into(),
                generation: 1,
                handle: "clock-source".into(),
                properties: [("interval_secs".into(), serde_json::json!(5))].into(),
                sequence: 1234,
                state: serde_json::json!({}),
            },
        );
        apply(&mut state, &runtime);
        let timer = state.node("clock").unwrap();
        assert_eq!(timer.flow.values["value"], serde_json::json!(1234));
        assert_eq!(timer.metric.interval_secs, 5);
        assert_eq!(timer.status, Status::Watching);
        apply(&mut state, &runtime);
        assert_eq!(
            state.node("clock").unwrap().flow.values["value"],
            serde_json::json!(1234)
        );
    }

    #[test]
    fn row_names_override_labels_and_clearing_restores_the_workflow_name() {
        let mut state = State::empty("/tmp".into());
        let mut runtime = runtime(&state);
        let id = state.selected_workspace.clone().into();
        runtime
            .workflows
            .get_mut(&id)
            .unwrap()
            .row_settings
            .insert("name".into(), "  Research  ".into());
        apply(&mut state, &runtime);
        assert_eq!(state.workspace().name, "Research");
        runtime
            .workflows
            .get_mut(&id)
            .unwrap()
            .row_settings
            .remove("name");
        apply(&mut state, &runtime);
        assert_eq!(state.workspace().name, "Placement");
    }

    #[test]
    fn new_windows_fill_the_compact_graph_instead_of_using_their_list_indexes() {
        let mut state = State::empty("/tmp".into());
        state.workspaces[0].graph_arranged = true;
        let mut runtime = runtime(&state);
        let graph = &mut runtime
            .workflows
            .get_mut(&state.selected_workspace.clone().into())
            .unwrap()
            .graph;
        let live_height = HEIGHT + 120.;
        for (kind, x, y) in [
            (NodeKind::Alert, 270., 80. + live_height + gap()),
            (NodeKind::Window, 120., 80.),
            (NodeKind::Vox, 100., 80. + 2. * (live_height + gap())),
            (NodeKind::Display, 440., 80. + 2. * (live_height + gap())),
        ] {
            let mut node = Node::new(
                &state.selected_workspace,
                kind,
                "/tmp".into(),
                Position { x, y },
            );
            node.desktop_window = kind == NodeKind::Window;
            graph.nodes.push(node.spec());
            state.nodes.push(node);
        }
        assert_eq!(
            crate::placement::node_height(&state.nodes[1]),
            CLOSED_HEIGHT
        );
        let original: Vec<_> = state.nodes.iter().map(|n| n.position.clone()).collect();
        let mut added = Vec::new();
        for _ in 0..2 {
            let node = Node::new(
                &state.selected_workspace,
                NodeKind::Window,
                "/tmp".into(),
                Position::default(),
            );
            added.push(node.id.clone());
            graph.nodes.push(node.spec());
        }
        apply(&mut state, &runtime);
        for (index, id) in added.iter().enumerate() {
            let position = &state.node(id).unwrap().position;
            assert_eq!(position.x, 120. + (index + 1) as f64 * (WIDTH + gap()));
            assert_eq!(position.y, 80.);
        }
        for (node, position) in state.nodes.iter().zip(&original) {
            assert_eq!((node.position.x, node.position.y), (position.x, position.y));
        }
        // Runtime reordering must not turn an initial placement into an ongoing layout.
        runtime
            .workflows
            .values_mut()
            .next()
            .unwrap()
            .graph
            .nodes
            .reverse();
        state.node_mut(&added[0]).unwrap().position = Position { x: -600., y: 900. };
        apply(&mut state, &runtime);
        let position = &state.node(&added[0]).unwrap().position;
        assert_eq!((position.x, position.y), (-600., 900.));
    }
}
