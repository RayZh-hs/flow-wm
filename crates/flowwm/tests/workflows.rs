mod support;
use flowwm::{client::Action, model::*};
use serde_json::json;
#[test]
fn a_wm_client_authors_native_nodes_without_storing_its_layout_in_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::empty(dir.path().into());
    let workflow = state.selected_workspace.clone();
    let (client, _) = support::fixture(state);
    let mut node = Node::new(
        &workflow,
        NodeKind::Shell,
        dir.path().into(),
        Position { x: 90., y: 150. },
    );
    node.command = "printf native".into();
    let id = node.id.clone();
    client
        .request(Action::Create {
            node: Box::new(node),
        })
        .unwrap();
    client
        .request(Action::Run {
            id: id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| {
        client
            .snapshot()
            .state
            .node(&id)
            .is_some_and(|n| n.status == Status::Done)
    });
    let runtime = client.snapshot().state.orchestration.clone().unwrap();
    let spec = runtime.workflows[&workflow.into()].graph.node(&id).unwrap();
    let serialized = serde_json::to_value(spec).unwrap();
    assert!(serialized.get("position").is_none());
    assert_eq!(spec.flow.properties["command"], "printf native");
    assert!(
        client
            .snapshot()
            .state
            .node(&id)
            .unwrap()
            .output
            .contains("native")
    );
}
#[test]
fn typed_connections_and_recipes_preserve_identity_and_clear_transient_publications() {
    let mut state = State::empty("/tmp".into());
    let workflow = state.selected_workspace.clone();
    let mut a = Node::new(
        &workflow,
        NodeKind::Shell,
        "/tmp".into(),
        Default::default(),
    );
    a.flow.values.insert("output".into(), json!("old"));
    let b = Node::new(
        &workflow,
        NodeKind::Shell,
        "/tmp".into(),
        Default::default(),
    );
    let ids = vec![a.id.clone(), b.id.clone()];
    state.nodes.extend([a, b]);
    state.connect(&ids[0], &ids[1]).unwrap();
    let recipe = state.recipe("Steps".into(), &ids).unwrap();
    assert!(recipe.nodes.iter().all(|n| n.flow.values.is_empty()));
    let copied = state
        .instantiate(&recipe, &workflow, "/tmp".into())
        .unwrap();
    assert!(copied.iter().all(|id| !ids.contains(id)));
    assert!(
        state
            .connections
            .iter()
            .any(|e| e.source.node == copied[0] && e.target.node == copied[1])
    );
}
#[test]
fn explicit_decisions_publish_native_waiting_state_and_keep_the_interface_when_interrupted() {
    let mut state = State::empty("/tmp".into());
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Gate,
        "/tmp".into(),
        Default::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    let (client, _) = support::fixture(state);
    client
        .request(Action::Run {
            id: id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| client.snapshot().state.node(&id).unwrap().status == Status::Review);
    client
        .request(Action::Interrupt { id: id.clone() })
        .unwrap();
    support::wait(|| client.snapshot().state.node(&id).unwrap().status == Status::Stopped);
    assert!(client.snapshot().state.node(&id).is_some());
}
#[test]
fn application_interrupt_is_separate_from_closing_and_never_signals_a_shared_process() {
    use flow_core::providers::ApplicationProvider;
    let provider = flowwm::desktop_provider::DesktopApplications;
    let operations = provider.operations("17").unwrap();
    assert!(operations.close_window);
    assert!(!operations.interrupt);
    assert!(provider.interrupt("17").is_err());
}
#[test]
fn agent_handoff_requires_an_explicit_success_or_issue() {
    use flow_builtins::agent::{AgentOutcome, agent_result};
    assert!(agent_result("all done").is_err());
    assert_eq!(
        agent_result(r#"{"outcome":"issue","summary":"needs review","artifacts":[]}"#)
            .unwrap()
            .outcome,
        AgentOutcome::Issue
    );
}

#[test]
fn reconnecting_the_wm_preserves_an_independent_invocation_and_its_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = State::empty(directory.path().into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        directory.path().into(),
        Position { x: 123., y: 456. },
    );
    node.command = "sleep 0.4; printf finished".into();
    let id = node.id.clone();
    state.nodes.push(node);
    let (client, _) = support::fixture(state);
    client
        .request(Action::Run {
            id: id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| client.snapshot().state.node(&id).unwrap().status == Status::Running);
    let before = client
        .snapshot()
        .state
        .orchestration
        .clone()
        .unwrap()
        .executions
        .values()
        .next()
        .unwrap()
        .clone();
    let (reconnected, _) = client.reconnect();
    support::wait(|| reconnected.snapshot().state.node(&id).unwrap().status == Status::Done);
    let view = reconnected.snapshot();
    let runtime = view.state.orchestration.as_ref().unwrap();
    assert_eq!(runtime.executions.len(), 1);
    assert_eq!(runtime.executions[&before.id].generation, before.generation);
    assert_eq!(runtime.executions[&before.id].result["stdout"], "finished");
    assert_eq!(view.state.node(&id).unwrap().position.x, 123.);
}

#[test]
fn graph_assistant_uses_current_commands_and_resumes_native_chat_context() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("chat.sh");
    std::fs::write(&script,r#"cat > prompt.txt
printf '%s\n' "$@" > arguments.txt
printf '%s' "$FLOWWM_SOCKET" > wm-socket.txt
printf '%s' "$FLOWWM_HOST_SOCKET" > host-socket.txt
printf '%s\n' '{"type":"thread.started","thread_id":"chat-session"}' '{"type":"item.completed","item":{"type":"agent_message","text":"A useful reply"}}'
"#).unwrap();
    let mut state = State::empty(directory.path().into());
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Agent,
        directory.path().into(),
        Position::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    let (client, _) = support::fixture(state);
    client.configure_agent(json!(["/bin/sh", script]));
    for question in ["First question", "Follow up"] {
        client
            .request(Action::Chat {
                id: id.clone(),
                text: question.into(),
            })
            .unwrap();
        support::wait(|| client.snapshot().state.node(&id).unwrap().status == Status::Done);
    }
    let state = client.snapshot().state;
    let chat = &state.node(&id).unwrap().chat;
    assert_eq!(
        chat.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        vec![
            "First question",
            "A useful reply",
            "Follow up",
            "A useful reply"
        ]
    );
    assert_eq!(
        state.node(&id).unwrap().agent_session.as_deref(),
        Some("chat-session")
    );
    assert!(
        std::fs::read_to_string(directory.path().join("arguments.txt"))
            .unwrap()
            .contains("chat-session")
    );
    assert_ne!(
        std::fs::read_to_string(directory.path().join("wm-socket.txt")).unwrap(),
        std::fs::read_to_string(directory.path().join("host-socket.txt")).unwrap()
    );
    let prompt = std::fs::read_to_string(directory.path().join("prompt.txt")).unwrap();
    assert!(
        prompt.contains("Follow up")
            && prompt.contains("action")
            && !prompt.contains("required JSON object")
    );
    let runtime = state.orchestration.as_ref().unwrap();
    assert!(
        !runtime
            .workflows
            .values()
            .flat_map(|w| w.graph.nodes.iter())
            .any(|n| n.flow.properties.contains_key("graph_context"))
    );
}
