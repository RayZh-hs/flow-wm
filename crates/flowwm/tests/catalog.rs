use flowwm::{
    catalog::Catalog,
    engine::{Control, Engine, Presentation},
    extensions, ipc,
    model::*,
    store::Store,
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    sync::Arc,
    time::Duration,
};

#[test]
fn window_connection_search_lists_only_exit_with_an_unsigned_exit_code() {
    let mut state = State::empty("/tmp".into());
    let mut window = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Position::default(),
    );
    window.desktop_window = true;
    window.window_id = Some(7);
    window.status = Status::Running;
    let id = window.id.clone();
    state.nodes.push(window);
    let snapshot = flowwm::engine::Snapshot {
        state: Arc::new(state),
        revision: 1,
        error: None,
    };
    let lookup = Catalog::new(
        &snapshot,
        Presentation {
            lookup_node: Some(id),
            ..Default::default()
        },
        &extensions::builtins(),
    )
    .lookup
    .unwrap();
    assert_eq!(lookup.outputs.len(), 1);
    let output = &lookup.outputs[0];
    assert_eq!(output.name, "on_exit");
    assert_eq!(output.label, "On exit");
    assert_eq!(output.ty, "Signal[{exit_code: UInt8}]");
    assert!(output.targets.iter().any(|t| t.create && t.id == "shell"));
}

#[test]
fn launcher_catalog_contains_metadata_without_agent_output_or_logs() {
    let mut state = State::empty("/tmp".into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Agent,
        "/tmp".into(),
        Position::default(),
    );
    node.output = "private-result-never-in-launcher".into();
    node.log("agent", "private-log-never-in-launcher");
    node.status = Status::Review;
    state.nodes.push(node);
    let snapshot = flowwm::engine::Snapshot {
        state: Arc::new(state),
        revision: 7,
        error: None,
    };
    let catalog = Catalog::new(&snapshot, Presentation::default(), &extensions::builtins());
    let json = serde_json::to_string(&catalog).unwrap();
    assert!(!json.contains("private-result") && !json.contains("private-log"));
    assert_eq!(catalog.attention, 1);
    assert_eq!(catalog.nodes.len(), 1);
}

#[test]
fn catalog_subscription_does_not_block_commands_and_updates_on_graph_changes() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("flowwm.sock");
    let store = Store::new(directory.path().join("data")).unwrap();
    let state = State::empty(directory.path().into());
    let workspace = state.selected_workspace.clone();
    let (engine, _commands) = Engine::new(state, store);
    ipc::serve(
        ipc::bind(&socket).unwrap(),
        engine.clone(),
        extensions::builtins(),
    );
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(stream, "{{\"op\":\"subscribe\"}}").unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(
        serde_json::from_str::<Catalog>(&line)
            .unwrap()
            .nodes
            .is_empty()
    );
    let node = Node::new(
        &workspace,
        NodeKind::Human,
        directory.path().into(),
        Position::default(),
    );
    ipc::send(
        &socket,
        &Control::Create {
            node: Box::new(node),
        },
    )
    .unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Catalog>(&line).unwrap().nodes.len(),
        1
    );
    engine.set_presentation(Presentation {
        graph_visible: true,
        lookup_return: false,
        lookup_node: None,
        ..Default::default()
    });
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert!(
        serde_json::from_str::<Catalog>(&line)
            .unwrap()
            .presentation
            .graph_visible
    );
    engine.shutdown();
}

#[test]
fn alerts_mark_the_owning_workspace_and_clear_without_changing_execution_status() {
    let mut state = State::empty("/tmp".into());
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Alert,
        "/tmp".into(),
        Position::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    let directory = tempfile::tempdir().unwrap();
    let (engine, _) = Engine::new(state, Store::new(directory.path().into()).unwrap());
    engine
        .request(Control::Alert {
            id: id.clone(),
            message: "Review this".into(),
            critical: true,
            notify: false,
        })
        .unwrap();
    let catalog = Catalog::new(
        &engine.snapshot(),
        Presentation::default(),
        &extensions::builtins(),
    );
    assert_eq!(catalog.attention, 1);
    assert_eq!(catalog.workspaces[0].attention, 1);
    assert!(catalog.nodes[0].alert);
    assert_eq!(catalog.nodes[0].status, Status::Idle);
    engine.request(Control::ClearAlert { id }).unwrap();
    assert_eq!(
        Catalog::new(&engine.snapshot(), Presentation::default(), &[]).attention,
        0
    );
    engine.shutdown();
}
