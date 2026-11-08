use flowwm::{graph::*, model::*};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn node(state: &mut State, kind: NodeKind) -> String {
    let node = Node::new(
        &state.selected_workspace,
        kind,
        "/tmp".into(),
        Position::default(),
    );
    let id = node.id.clone();
    state.nodes.push(node);
    id
}
fn wire(state: &mut State, a: &str, out: &str, b: &str, input: &str) {
    connect(
        state,
        Connection {
            source: Endpoint {
                node: a.into(),
                pin: out.into(),
            },
            target: Endpoint {
                node: b.into(),
                pin: input.into(),
            },
        },
    )
    .unwrap();
}
fn run(state: State, root: &str, signal: Option<(&str, Value)>) -> Evaluation {
    evaluate(state, root, signal, Arc::new(AtomicBool::new(false))).unwrap()
}

#[test]
fn constants_and_displays_are_lazy() {
    let mut state = State::empty("/tmp".into());
    let a = node(&mut state, NodeKind::Constant);
    state
        .node_mut(&a)
        .unwrap()
        .flow
        .properties
        .insert("value".into(), json!(42));
    let b = node(&mut state, NodeKind::Display);
    let unrelated = node(&mut state, NodeKind::Python);
    state.node_mut(&unrelated).unwrap().command = "raise Exception('must stay lazy')".into();
    wire(&mut state, &a, "value", &b, "value");
    let result = run(state, &b, None);
    assert_eq!(result.answer["value"], 42);
    assert_eq!(result.values[&b]["value"], 42);
    assert!(!result.values.contains_key(&unrelated));
    assert!(result.executed.is_empty());
}

#[test]
fn vox_and_python_bindings_return_records() {
    for (kind, source) in [
        (NodeKind::Vox, "{ value = inputs.x + args.offset, }"),
        (
            NodeKind::Python,
            "outputs = {'value': inputs['x'] + args['offset']}",
        ),
    ] {
        let mut state = State::empty("/tmp".into());
        let source_id = node(&mut state, NodeKind::Shell);
        let id = node(&mut state, kind);
        let n = state.node_mut(&id).unwrap();
        n.command = source.into();
        n.flow.inputs.push(Pin {
            default: Some(json!(40)),
            ..Pin::new("x", Type::Int)
        });
        wire(&mut state, &source_id, "out", &id, "in");
        let result = run(state, &source_id, Some(("out", json!({"offset":2}))));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.values[&id]["value"], 42);
    }
}

#[test]
fn break_merge_and_conditional_preserve_and_overwrite_arguments() {
    let mut state = State::empty("/tmp".into());
    let source = node(&mut state, NodeKind::Shell);
    let split = node(&mut state, NodeKind::BreakSignal);
    let merge = node(&mut state, NodeKind::MergeSignal);
    state
        .node_mut(&merge)
        .unwrap()
        .flow
        .properties
        .insert("args".into(), json!({"x": 99}));
    let condition = node(&mut state, NodeKind::Conditional);
    state
        .node_mut(&condition)
        .unwrap()
        .flow
        .properties
        .insert("condition".into(), json!(true));
    let yes = node(&mut state, NodeKind::Python);
    state.node_mut(&yes).unwrap().command =
        "import os\noutputs = {'value': [args, os.environ['FLOWWM_ARG_X']]}".into();
    let no = node(&mut state, NodeKind::Python);
    state.node_mut(&no).unwrap().command = "raise Exception('wrong branch')".into();
    wire(&mut state, &source, "out", &split, "in");
    wire(&mut state, &split, "out", &merge, "in");
    wire(&mut state, &split, "args", &merge, "args");
    // A separate input edits one argument after merging the full argument record.
    state.node_mut(&merge).unwrap().flow.inputs.push(Pin {
        default: Some(json!(99)),
        ..Pin::new("x", Type::Int)
    });
    wire(&mut state, &merge, "out", &condition, "in");
    wire(&mut state, &condition, "true", &yes, "in");
    wire(&mut state, &condition, "false", &no, "in");
    let result = run(
        state,
        &source,
        Some(("out", json!({"x": 1, "keep": "hello"}))),
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.values[&split]["args"], json!({"x":1,"keep":"hello"}));
    assert_eq!(
        result.values[&yes]["value"],
        json!([{"x":99,"keep":"hello"},"99"])
    );
    assert!(!result.executed.contains(&no));
}

#[test]
fn signal_fanout_runs_every_recipient_even_if_a_binding_fails() {
    let mut state = State::empty("/tmp".into());
    let source = node(&mut state, NodeKind::Shell);
    let bad = node(&mut state, NodeKind::Python);
    state.node_mut(&bad).unwrap().command = "raise Exception('expected failure')".into();
    let good = node(&mut state, NodeKind::Vox);
    wire(&mut state, &source, "out", &bad, "in");
    wire(&mut state, &source, "out", &good, "in");
    let result = run(state, &source, Some(("out", json!({}))));
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.values[&good]["value"], 42);
    assert_eq!(result.executed, vec![bad, good]);
}

#[test]
fn rejects_mixed_connections_duplicate_data_inputs_and_data_cycles() {
    let mut state = State::empty("/tmp".into());
    let a = node(&mut state, NodeKind::Display);
    let b = node(&mut state, NodeKind::Display);
    let c = node(&mut state, NodeKind::Shell);
    wire(&mut state, &a, "value", &b, "value");
    let edge = |source: &str, pin: &str, target: &str| Connection {
        source: Endpoint {
            node: source.into(),
            pin: pin.into(),
        },
        target: Endpoint {
            node: target.into(),
            pin: "value".into(),
        },
    };
    assert!(connect(&mut state, edge(&b, "value", &a)).is_err());
    assert!(connect(&mut state, edge(&c, "out", &a)).is_err());
    assert!(connect(&mut state, edge(&c, "output", &b)).is_err());
    assert_eq!(state.connections.len(), 1);
}

#[test]
fn environment_names_and_values_are_unambiguous() {
    assert_eq!(
        signal_environment(&json!({"path":"hello world", "nested":{"x":1}, "count":2})).unwrap()["FLOWWM_ARG_PATH"],
        "hello world"
    );
    assert!(signal_environment(&json!({"x":1,"X":2})).is_err());
    assert!(signal_environment(&json!({"bad-name":1})).is_err());
    assert!(signal_environment(&json!({"x":"\u{0}"})).is_err());
}

#[test]
fn extensions_register_types_casts_nodes_and_exposed_properties() {
    let mut state = State::empty("/tmp".into());
    state.types.push(TypeDefinition {
        name: "Count".into(),
        representation: Type::Int,
    });
    state.casts.push(CastDefinition {
        from: Type::Named {
            name: "Count".into(),
        },
        to: Type::String,
        source: "value.toString()".into(),
    });
    state.node_types.push(NodeDefinition {
        id: "custom".into(),
        name: "Custom".into(),
        inputs: vec![Pin {
            property: Some("count".into()),
            default: Some(json!(40)),
            ..Pin::new("count", Type::Int)
        }],
        outputs: vec![Pin {
            property: Some("answer".into()),
            ..Pin::new("exposed", Type::Int)
        }],
        properties: Default::default(),
        source: "{ answer = properties.count + 2, }".into(),
    });
    let custom = node(&mut state, NodeKind::Extension);
    state.node_mut(&custom).unwrap().flow.node_type = "custom".into();
    let result = run(state.clone(), &custom, None);
    assert_eq!(result.answer["exposed"], 42);
    let constant = node(&mut state, NodeKind::Constant);
    let n = state.node_mut(&constant).unwrap();
    n.flow.properties.insert("value".into(), json!(12));
    n.flow.outputs.push(Pin::new(
        "value",
        Type::Named {
            name: "Count".into(),
        },
    ));
    let display = node(&mut state, NodeKind::Display);
    state
        .node_mut(&display)
        .unwrap()
        .flow
        .inputs
        .push(Pin::new("value", Type::String));
    wire(&mut state, &constant, "value", &display, "value");
    assert_eq!(run(state, &display, None).answer["value"], "12");
}

#[test]
fn recipes_keep_pins_and_connections_but_clear_published_values() {
    let mut state = State::empty("/tmp".into());
    let a = node(&mut state, NodeKind::Constant);
    state
        .node_mut(&a)
        .unwrap()
        .flow
        .properties
        .insert("value".into(), json!(7));
    let b = node(&mut state, NodeKind::Display);
    state
        .node_mut(&b)
        .unwrap()
        .flow
        .values
        .insert("value".into(), json!(999));
    wire(&mut state, &a, "value", &b, "value");
    let recipe = state.recipe("Test".into(), &[a, b]).unwrap();
    let mut restored = State::empty("/tmp".into());
    let ws = restored.selected_workspace.clone();
    let ids = restored.instantiate(&recipe, &ws, "/tmp".into()).unwrap();
    assert_eq!(restored.connections.len(), 1);
    assert!(restored.nodes.iter().all(|n| n.flow.values.is_empty()));
    assert_eq!(run(restored, &ids[1], None).answer["value"], 7);
}

#[test]
fn dashboard_properties_are_read_without_executing_the_dashboard() {
    let mut state = State::empty("/tmp".into());
    let dashboard = node(&mut state, NodeKind::Python);
    let n = state.node_mut(&dashboard).unwrap();
    n.command = "raise Exception('a property read must stay passive')".into();
    n.flow.outputs.push(Pin {
        property: Some("selection".into()),
        ..Pin::new("selected", Type::String)
    });
    n.flow
        .values
        .insert("selection".into(), json!("window-123"));
    let display = node(&mut state, NodeKind::Display);
    wire(&mut state, &dashboard, "selected", &display, "value");
    let result = run(state, &display, None);
    assert_eq!(result.answer["value"], "window-123");
    assert!(!result.values.contains_key(&dashboard));
}

#[test]
fn a_data_diamond_shares_computation_but_the_next_request_is_fresh() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("calls");
    let mut state = State::empty("/tmp".into());
    let leaf = node(&mut state, NodeKind::Python);
    state.node_mut(&leaf).unwrap().command = format!(
        "with open({:?}, 'a') as f: f.write('x')\noutputs = {{'value': 1}}",
        path.to_str().unwrap()
    );
    let a = node(&mut state, NodeKind::Display);
    let b = node(&mut state, NodeKind::Display);
    let root = node(&mut state, NodeKind::Vox);
    let n = state.node_mut(&root).unwrap();
    n.flow
        .inputs
        .extend([Pin::new("a", Type::Int), Pin::new("b", Type::Int)]);
    n.command = "{ value = inputs.a + inputs.b, }".into();
    wire(&mut state, &leaf, "value", &a, "value");
    wire(&mut state, &leaf, "value", &b, "value");
    wire(&mut state, &a, "value", &root, "a");
    wire(&mut state, &b, "value", &root, "b");
    assert_eq!(run(state.clone(), &root, None).answer["value"], 2);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
    assert_eq!(run(state, &root, None).answer["value"], 2);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "xx");
}

#[test]
fn extension_loading_registers_definitions_without_evaluating_code() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("custom.json"), serde_json::to_vec(&json!({"version":2,"types":[{"name":"Count","representation":{"kind":"int"}}],"nodes":[{"id":"custom","name":"Custom","source":"intentionally invalid; never evaluated on load"}]})).unwrap()).unwrap();
    let bundle = flowwm::extensions::load_bundle(directory.path()).unwrap();
    assert!(
        bundle
            .macros
            .iter()
            .any(|m| m.id == "custom" && m.kind == NodeKind::Extension)
    );
    let mut state = State::empty("/tmp".into());
    bundle.apply(&mut state).unwrap();
    assert_eq!(state.types.len(), 1);
    assert_eq!(state.node_types.len(), 1);
    assert!(state.nodes.is_empty());
}

#[test]
fn vox_bindings_preserve_strings_and_empty_records() {
    let mut state = State::empty("/tmp".into());
    let id = node(&mut state, NodeKind::Vox);
    let text = "${not_interpolation} \"quote\" \\ path\n\u{8}";
    let n = state.node_mut(&id).unwrap();
    n.flow.properties.insert("text".into(), json!(text));
    n.command = "{ value = properties.text, empty = {}, }".into();
    let result = run(state, &id, None);
    assert_eq!(result.answer["value"], text);
    assert_eq!(result.answer["empty"], json!({}));
}

fn wait_for(engine: &flowwm::engine::Engine, predicate: impl Fn(&State) -> bool) {
    let start = std::time::Instant::now();
    loop {
        if predicate(&engine.snapshot().state) {
            return;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(8),
            "Timed out: {:?}",
            engine.snapshot().error
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[test]
fn engine_delivers_each_signal_with_its_own_arguments_and_lazy_data() {
    use flowwm::{
        engine::{Control, Engine},
        store::Store,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut state = State::empty(directory.path().into());
    let source = node(&mut state, NodeKind::Shell);
    let value = node(&mut state, NodeKind::Constant);
    state
        .node_mut(&value)
        .unwrap()
        .flow
        .properties
        .insert("value".into(), json!(123));
    let sink = node(&mut state, NodeKind::Shell);
    let path = directory.path().join("events");
    let n = state.node_mut(&sink).unwrap();
    n.command = format!(
        "printf '%s:%s\\n' \"$FLOWWM_ARG_COUNT\" \"$FLOWWM_DATA_INPUTS\" >> '{}'",
        path.display()
    );
    n.flow.inputs.push(Pin::new("number", Type::Int));
    wire(&mut state, &source, "out", &sink, "in");
    wire(&mut state, &value, "value", &sink, "number");
    let (engine, _) = Engine::new(state, Store::new(directory.path().join("state")).unwrap());
    for count in [1, 2] {
        engine
            .request(Control::Emit {
                source: Endpoint {
                    node: source.clone(),
                    pin: "out".into(),
                },
                args: json!({"count":count}),
            })
            .unwrap();
    }
    wait_for(&engine, |_| {
        std::fs::read_to_string(&path).is_ok_and(|s| s.lines().count() == 2)
    });
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "1:{\"number\":123}\n2:{\"number\":123}\n"
    );
    engine.shutdown();
}

#[test]
fn stop_interrupts_vox_and_python_loops_without_blocking_control() {
    use flowwm::{
        engine::{Control, Engine},
        store::Store,
    };
    for (kind, source) in [
        (NodeKind::Vox, "{ for (true) { } { value = 1, } }"),
        (NodeKind::Python, "while True: pass"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut state = State::empty(directory.path().into());
        let id = node(&mut state, kind);
        state.node_mut(&id).unwrap().command = source.into();
        let (engine, _) = Engine::new(state, Store::new(directory.path().join("state")).unwrap());
        engine
            .request(Control::Activate {
                id: id.clone(),
                args: json!({}),
            })
            .unwrap();
        wait_for(&engine, |s| s.node(&id).unwrap().status == Status::Running);
        std::thread::sleep(std::time::Duration::from_millis(100));
        engine.request(Control::Stop { id: id.clone() }).unwrap();
        wait_for(&engine, |s| s.node(&id).unwrap().status == Status::Stopped);
        let mut updated = engine.snapshot().state.node(&id).unwrap().clone();
        updated.command = if kind == NodeKind::Vox {
            "{ value = 7, }".into()
        } else {
            "outputs = {'value': 7}".into()
        };
        let start = std::time::Instant::now();
        loop {
            if engine
                .request(Control::Update {
                    node: Box::new(updated.clone()),
                })
                .is_ok()
            {
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        engine
            .request(Control::Run {
                id: id.clone(),
                downstream: false,
            })
            .unwrap();
        wait_for(&engine, |s| s.node(&id).unwrap().status == Status::Done);
        assert_eq!(
            engine.snapshot().state.node(&id).unwrap().flow.values["value"],
            7
        );
        engine.shutdown();
    }
}
