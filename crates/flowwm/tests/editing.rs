mod support;
use flowwm::{
    catalog::Catalog,
    client::{Action, Presentation},
    extensions,
    graph::*,
    model::*,
};
use serde_json::json;

#[test]
fn agent_macros_can_be_connected_before_their_first_instance_exists() {
    let mut initial = State::empty("/tmp".into());
    let human = Node::new(
        &initial.selected_workspace,
        NodeKind::Human,
        "/tmp".into(),
        Default::default(),
    );
    initial.nodes.push(human.clone());
    let (engine, _) = support::fixture(initial);
    for macro_id in ["agent", "agent_workflow"] {
        let m = extensions::builtins()
            .into_iter()
            .find(|m| m.id == macro_id)
            .unwrap();
        let mut node = Node::new(
            &engine.snapshot().state.selected_workspace,
            m.kind,
            "/tmp".into(),
            Default::default(),
        );
        node.title = m.name;
        node.command = m.command;
        node.flow = m.flow;
        let catalog = Catalog::new(
            &engine.snapshot(),
            Presentation {
                lookup_node: Some(human.id.clone()),
                lookup_input: macro_id == "agent",
                ..Default::default()
            },
            &extensions::builtins(),
        );
        assert!(catalog.lookup.unwrap().outputs.iter().any(|output| {
            output
                .targets
                .iter()
                .any(|target| target.create && target.id == macro_id)
        }));
        let edge = if macro_id == "agent" {
            Connection {
                source: Endpoint {
                    node: node.id.clone(),
                    pin: "on_session_finish".into(),
                },
                target: Endpoint {
                    node: human.id.clone(),
                    pin: "in".into(),
                },
            }
        } else {
            Connection {
                source: Endpoint {
                    node: human.id.clone(),
                    pin: "accepted".into(),
                },
                target: Endpoint {
                    node: node.id.clone(),
                    pin: "in".into(),
                },
            }
        };
        let mut candidate = (*engine.snapshot().state).clone();
        candidate.nodes.push(node.clone());
        candidate.connections.push(edge.clone());
        candidate.validate().unwrap();
        engine
            .request(Action::Create {
                node: Box::new(node.clone()),
            })
            .unwrap();
        engine
            .request(Action::ConnectPins { connection: edge })
            .unwrap();
        let state = engine.snapshot().state;
        let created = state.node(&node.id).unwrap();
        assert_eq!(created.spec().type_id, node.spec().type_id);
        assert_eq!(created.kind, m.kind);
        assert!(!pins(&state, created, true).is_empty());
    }
    assert!(
        engine
            .snapshot()
            .state
            .orchestration
            .as_ref()
            .unwrap()
            .executions
            .is_empty()
    );
    engine.shutdown();
}

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
    constant
        .flow
        .properties
        .insert("_alert".into(), json!({"message":"Pending review"}));
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
    let (engine, _) = support::fixture(state);
    let result = engine
        .request(Action::Paste {
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
    assert!(!copy.flow.properties.contains_key("_alert"));
    assert_eq!(state.connections.len(), 2);
    assert_eq!(state.connections[1].source.node, ids[0]);
    assert_eq!(state.connections[1].target.node, ids[1]);
    engine
        .request(Action::Evaluate { id: ids[1].clone() })
        .unwrap();
    support::wait(|| {
        engine
            .snapshot()
            .state
            .node(&ids[1])
            .unwrap()
            .flow
            .values
            .get("value")
            == Some(&json!(7))
    });
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
    let (engine, _) = support::fixture(state);
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
    let reverse = Catalog::new(
        &engine.snapshot(),
        Presentation {
            lookup_node: Some(sink_id),
            lookup_pin: Some("number".into()),
            lookup_input: true,
            ..Default::default()
        },
        &extensions::builtins(),
    )
    .lookup
    .unwrap();
    assert!(reverse.input);
    assert_eq!(reverse.outputs.len(), 1);
    assert_eq!(reverse.outputs[0].name, "number");
    assert!(
        reverse.outputs[0]
            .targets
            .iter()
            .any(|t| !t.create && t.input == "value")
    );
    assert!(!reverse.outputs[0].targets.iter().any(|t| t.input == "out"));
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
            state.graph(),
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

#[test]
fn live_window_parameters_can_be_saved_without_stopping_or_detaching_the_window() {
    let mut state = State::empty("/tmp".into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Default::default(),
    );
    node.window_id = Some(7);
    node.desktop_window = true;
    node.status = Status::Running;
    state.nodes.push(node.clone());
    let (engine, _) = support::fixture(state);
    node.title = "My editor".into();
    node.window_id = None;
    engine
        .request(Action::Update {
            node: Box::new(node.clone()),
        })
        .unwrap();
    let snapshot = engine.snapshot();
    let saved = snapshot.state.node(&node.id).unwrap();
    assert_eq!(saved.window_id, Some(7));
    assert_eq!(saved.status, Status::Running);
    assert_eq!(saved.flow.properties["display_title"], "My editor");
    engine.shutdown();
}

#[test]
fn polling_extension_intervals_survive_saving_and_reconnecting() {
    for type_id in ["local.timer", "local.receive"] {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Extension,
            "/tmp".into(),
            Default::default(),
        );
        node.flow.node_type = type_id.into();
        node.flow
            .properties
            .insert("interval_secs".into(), json!(30));
        let id = node.id.clone();
        state.nodes.push(node);
        let (engine, _) = support::fixture(state);

        for interval in [10, 20] {
            let mut edited = engine.snapshot().state.node(&id).unwrap().clone();
            assert_ne!(edited.metric.interval_secs, interval);
            edited
                .flow
                .properties
                .insert("interval_secs".into(), json!(interval));
            engine
                .request(Action::Update {
                    node: Box::new(edited),
                })
                .unwrap();

            let snapshot = engine.snapshot();
            let saved = snapshot.state.node(&id).unwrap();
            assert_eq!(
                saved.flow.properties["interval_secs"], interval,
                "{type_id}"
            );
            assert_eq!(saved.metric.interval_secs, interval);
            let runtime = snapshot.state.orchestration.as_ref().unwrap();
            let source = runtime.sources.values().find(|s| s.node.0 == id).unwrap();
            assert_eq!(source.properties["interval_secs"], interval);
        }

        let (reconnected, _) = engine.reconnect();
        let snapshot = reconnected.snapshot();
        let saved = snapshot.state.node(&id).unwrap();
        assert_eq!(saved.flow.properties["interval_secs"], 20);
        assert_eq!(saved.metric.interval_secs, 20);
        reconnected.shutdown();
    }
}

#[test]
fn dedicated_agent_and_metric_controls_still_update_runtime_properties() {
    let mut state = State::empty("/tmp".into());
    let agent = Node::new(
        &state.selected_workspace,
        NodeKind::Agent,
        "/tmp".into(),
        Default::default(),
    );
    let metric = Node::new(
        &state.selected_workspace,
        NodeKind::Metric,
        "/tmp".into(),
        Default::default(),
    );
    state.nodes.extend([agent.clone(), metric.clone()]);
    let (engine, _) = support::fixture(state);
    for mut edited in [agent, metric] {
        edited.writable = true;
        edited.metric.interval_secs = 10;
        edited.metric.unit = "%".into();
        let id = edited.id.clone();
        let kind = edited.kind;
        engine
            .request(Action::Update {
                node: Box::new(edited),
            })
            .unwrap();
        let snapshot = engine.snapshot();
        let properties = &snapshot.state.node(&id).unwrap().flow.properties;
        if kind == NodeKind::Agent {
            assert_eq!(properties["writable"], true);
            assert!(!properties.contains_key("interval_secs"));
            assert!(!properties.contains_key("unit"));
        } else {
            assert_eq!(properties["interval_secs"], 10);
            assert_eq!(properties["unit"], "%");
            assert!(!properties.contains_key("writable"));
        }
    }
    engine.shutdown();
}
