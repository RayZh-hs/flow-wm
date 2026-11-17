mod support;
use flowwm::{
    catalog::Catalog,
    client::{Action, Presentation},
    extensions, ipc,
    model::*,
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    sync::Arc,
    time::Duration,
};

#[test]
fn sleep_and_logout_are_named_created_and_offered_as_signal_targets() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = State::empty(directory.path().into());
    let source = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        directory.path().into(),
        Position::default(),
    );
    let source_id = source.id.clone();
    state.nodes.push(source);
    let (engine, _) = support::fixture(state);
    let macros = extensions::builtins();
    for (type_id, label, search_terms) in [
        ("local.suspend", "Sleep", &["sleep", "suspend"][..]),
        (
            "local.log_out",
            "Log out",
            &["log out", "logout", "sign out"][..],
        ),
    ] {
        let entry = macros
            .iter()
            .find(|m| m.id == type_id)
            .expect("session action is in the creation catalog");
        assert_eq!(entry.name, label);
        let searchable = format!("{} {}", entry.name, entry.description).to_lowercase();
        for term in search_terms {
            assert!(
                searchable.contains(term),
                "{label} must be discoverable as {term}"
            );
        }
        let state = engine.snapshot().state;
        let mut node = Node::new(
            &state.selected_workspace,
            entry.kind,
            directory.path().into(),
            Position::default(),
        );
        node.flow = entry.flow.clone();
        let id = node.id.clone();
        engine
            .request(Action::Create {
                node: Box::new(node),
            })
            .unwrap();
        let snapshot = engine.snapshot();
        let node = snapshot.state.node(&id).unwrap();
        assert_eq!(node.label(), label);
        assert_eq!(node.spec().type_id, type_id);
        assert!(
            !node.flow.properties.contains_key("argv"),
            "New actions use their configured command"
        );
        let catalog = Catalog::new(
            &snapshot,
            Presentation {
                lookup_node: Some(source_id.clone()),
                lookup_pin: Some("on_exit".into()),
                ..Default::default()
            },
            &macros,
        );
        assert_eq!(
            catalog.nodes.iter().find(|n| n.id == id).unwrap().label,
            label
        );
        let outputs = &catalog.lookup.unwrap().outputs;
        assert!(
            outputs
                .iter()
                .flat_map(|p| &p.targets)
                .any(|target| target.create && target.id == type_id && target.input == "in")
        );
        assert!(
            outputs
                .iter()
                .flat_map(|p| &p.targets)
                .any(|target| !target.create && target.id == id && target.input == "in")
        );
    }
    assert!(
        engine
            .snapshot()
            .state
            .orchestration
            .as_ref()
            .unwrap()
            .executions
            .is_empty(),
        "Creating and discovering session actions must not run them"
    );
    engine.shutdown();
}

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
    let snapshot = flowwm::client::ViewSnapshot {
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
    let snapshot = flowwm::client::ViewSnapshot {
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
    let state = State::empty(directory.path().into());
    let workspace = state.selected_workspace.clone();
    let (engine, _commands) = support::fixture(state);
    ipc::serve(
        ipc::bind(&socket).unwrap(),
        (*engine).clone(),
        extensions::builtins(),
    );
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::json!({"wm_protocol":1,"command":{"action":"subscribe"}})
    )
    .unwrap();
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
        &Action::Create {
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
fn launcher_requests_share_the_subscription_without_catalog_changes_or_replay() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("flowwm.sock");
    let (engine, _) = support::fixture(State::empty(directory.path().into()));
    assert!(!engine.open_launcher(None));
    ipc::serve(
        ipc::bind(&socket).unwrap(),
        (*engine).clone(),
        extensions::builtins(),
    );
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::json!({"wm_protocol":1,"command":{"action":"subscribe","launcher":true}})
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str::<Catalog>(&line).unwrap();

    for query in [None, Some("@ + ".to_string()), Some("@ ".to_string())] {
        assert!(engine.open_launcher(query.clone()));
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            serde_json::json!({"launcher": {"query": query}})
        );
    }

    let reconnected = engine.connect_launcher();
    assert!(
        reconnected.try_recv().is_err(),
        "opening is an event, never replayed on reconnect"
    );
    assert!(engine.open_launcher(None));
    assert_eq!(reconnected.recv().unwrap(), None);
    drop(reconnected);
    assert!(
        !engine.open_launcher(None),
        "a disconnected shell must use the CLI fallback"
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
    let (engine, _) = support::fixture(state);
    engine
        .request(Action::Alert {
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
    engine.request(Action::ClearAlert { id }).unwrap();
    assert_eq!(
        Catalog::new(&engine.snapshot(), Presentation::default(), &[]).attention,
        0
    );
    engine.shutdown();
}
