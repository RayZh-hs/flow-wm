use flowwm::{
    catalog::Catalog,
    engine::{Control, Engine, Presentation},
    extensions,
    graph::*,
    model::*,
    store::Store,
};
use serde_json::json;

#[test]
fn clipboard_copies_internal_edges_as_idle_nodes_with_fresh_ids() {
    let mut state = State::empty("/tmp".into());
    let mut constant = Node::new(
        &state.selected_workspace,
        NodeKind::Constant,
        "/tmp".into(),
        Position { x: 20., y: 40. },
    );
    constant.flow.properties.insert("value".into(), json!(7));
    constant.flow.values.insert("value".into(), json!(999));
    let mut display = Node::new(
        &state.selected_workspace,
        NodeKind::Display,
        "/tmp".into(),
        Position { x: 420., y: 400. },
    );
    display.status = Status::Done;
    state.nodes.extend([constant.clone(), display.clone()]);
    connect(
        &mut state,
        Connection {
            source: Endpoint {
                node: constant.id.clone(),
                pin: "value".into(),
            },
            target: Endpoint {
                node: display.id.clone(),
                pin: "value".into(),
            },
        },
    )
    .unwrap();
    let recipe = state
        .recipe("Copy".into(), &[constant.id.clone(), display.id.clone()])
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (engine, _) = Engine::new(state, Store::new(temp.path().into()).unwrap());
    let result = engine
        .request(Control::Paste {
            recipe,
            offset: Position { x: 60., y: 80. },
        })
        .unwrap();
    let ids: Vec<String> = serde_json::from_value(result["ids"].clone()).unwrap();
    assert_eq!(ids.len(), 2);
    let state = engine.snapshot().state;
    let copy = state.node(&ids[0]).unwrap();
    assert_ne!(copy.id, constant.id);
    assert_eq!((copy.position.x, copy.position.y), (80., 120.));
    assert_eq!(copy.status, Status::Idle);
    assert!(copy.flow.values.is_empty());
    assert_eq!(state.connections.len(), 2);
    assert_eq!(state.connections[1].source.node, ids[0]);
    assert_eq!(state.connections[1].target.node, ids[1]);
    assert_eq!(
        evaluate(
            (*state).clone(),
            &ids[1],
            None,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
        )
        .unwrap()
        .answer["value"],
        7
    );
    engine.shutdown();
}

#[test]
fn connection_catalog_uses_the_registered_casts_and_each_output_type() {
    let mut state = State::empty("/tmp".into());
    let mut source = Node::new(
        &state.selected_workspace,
        NodeKind::Constant,
        "/tmp".into(),
        Default::default(),
    );
    source.flow.properties.insert("value".into(), json!(4));
    let mut sink = Node::new(
        &state.selected_workspace,
        NodeKind::Python,
        "/tmp".into(),
        Default::default(),
    );
    sink.flow.inputs.push(Pin::new("number", Type::Float));
    sink.flow.inputs.push(Pin::new("text", Type::String));
    let id = source.id.clone();
    let sink_id = sink.id.clone();
    state.nodes.extend([source, sink]);
    let temp = tempfile::tempdir().unwrap();
    let (engine, _) = Engine::new(state, Store::new(temp.path().into()).unwrap());
    let catalog = Catalog::new(
        &engine.snapshot(),
        Presentation {
            lookup_node: Some(id),
            ..Default::default()
        },
        &extensions::builtins(),
    );
    let output = &catalog.lookup.unwrap().outputs[0];
    assert_eq!(output.ty, "Int");
    assert!(
        output
            .targets
            .iter()
            .any(|t| t.id == sink_id && t.input == "number")
    );
    assert!(
        !output
            .targets
            .iter()
            .any(|t| t.id == sink_id && t.input == "text")
    );
    assert!(output.targets.iter().any(|t| t.id == "display" && t.create));
    assert!(!output.targets.iter().any(|t| t.id == "shell"));
    engine.shutdown();
}

#[test]
fn session_error_extension_routes_success_and_failure_without_core_agent_logic() {
    let bundle = extensions::load_bundle(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/extensions"),
    )
    .unwrap();
    for code in [0, 2, -1] {
        let mut state = State::empty("/tmp".into());
        bundle.apply(&mut state).unwrap();
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Extension,
            "/tmp".into(),
            Default::default(),
        );
        node.flow.node_type = "agent_sessions.handle_error".into();
        let id = node.id.clone();
        state.nodes.push(node);
        let args = json!({"ERROR":{"code":code,"kind":"test","message":""}});
        let result = evaluate(
            state,
            &id,
            Some(("", args.clone())),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            result.values[&id][if code == 0 { "success" } else { "error" }],
            args
        );
        assert!(!result.values[&id].contains_key(if code == 0 { "error" } else { "success" }));
    }
}
