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
