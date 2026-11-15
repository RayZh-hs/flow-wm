use flow_builtins::local::{Environment, Limits, registry, registry_with_environment};
use flow_core::{domain::*, graph::*, orchestrator::Orchestrator, protocol::Command, providers::*};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

fn node(id: &str, kind: &str, properties: Value) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        type_id: format!("local.{kind}"),
        flow: Flow {
            properties: serde_json::from_value(properties).unwrap(),
            ..Default::default()
        },
    }
}
fn edge(from: &str, output: &str, to: &str, input: &str) -> Connection {
    Connection {
        source: Endpoint {
            node: from.into(),
            pin: output.into(),
        },
        target: Endpoint {
            node: to.into(),
            pin: input.into(),
        },
    }
}
fn eval(graph: FlowNet, root: &str) -> Evaluation {
    flow_builtins::evaluate(
        graph,
        EvaluationContext::default(),
        root,
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn graph(nodes: Vec<NodeSpec>, connections: Vec<Connection>) -> FlowNet {
    let mut graph = FlowNet {
        nodes,
        connections,
        ..Default::default()
    };
    registry(Limits::default())
        .unwrap()
        .prepare_graph(&mut graph);
    graph
}
fn core(graph: FlowNet) -> Orchestrator {
    let registry = registry(Limits::default()).unwrap();
    let mut state = RuntimeState::local();
    state
        .workspaces
        .activate(&"local:user".into(), &"local".into(), "session".into())
        .unwrap();
    let mut core = Orchestrator::new(state, registry);
    core.execute(
        Command::Create {
            workflow: Workflow {
                id: "flow".into(),
                name: "Utilities".into(),
                workspace: Some("local:user".into()),
                row_settings: Default::default(),
                visibility: Visibility::Public,
                owner: None,
                references: Default::default(),
                authority: "local".into(),
                transfer: None,
                graph,
            },
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    core
}
fn drain(core: &mut Orchestrator) {
    while let Some((frame, graph, context)) = core.next_frame() {
        let result = flow_builtins::evaluate(
            graph,
            context,
            &frame.root.0,
            (!frame.data).then_some((frame.pin.as_str(), frame.args.clone())),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        core.accept_frame(&frame.id, result).unwrap();
    }
}

#[test]
fn list_templates_propagate_types_and_reject_incompatible_items() {
    let mut graph = graph(
        vec![
            node("source", "constant", json!({"value":"hello"})),
            node("make", "list_create", json!({"input_count":1})),
            node("append", "list_append", json!({"item":"world"})),
            node("get", "list_get", json!({"index":1})),
        ],
        vec![
            edge("source", "value", "make", "item_1"),
            edge("make", "value", "append", "list"),
            edge("append", "value", "get", "list"),
        ],
    );
    graph.validate().unwrap();
    assert_eq!(
        pins(&graph, graph.node("append").unwrap(), true)[0].ty,
        Type::List {
            item: Box::new(Type::String)
        }
    );
    assert_eq!(
        pins(&graph, graph.node("get").unwrap(), true)[0].ty,
        Type::String
    );
    assert_eq!(eval(graph.clone(), "get").answer["value"], "world");
    graph
        .nodes
        .push(node("bad", "constant", json!({"value":false})));
    assert!(connect(&mut graph, edge("bad", "value", "make", "item_1")).is_err());
    graph.connections.retain(|e| e.target.node != "make");
    connect(&mut graph, edge("bad", "value", "make", "item_1")).unwrap();
    assert_eq!(
        pins(&graph, graph.node("get").unwrap(), true)[0].ty,
        Type::Bool
    );
}

#[test]
fn all_list_operations_preserve_order_and_check_indices() {
    let make = node(
        "make",
        "list_create",
        json!({"input_count":12,"item_type":{"kind":"uint"},
        "item_1":1,"item_2":2,"item_3":3,"item_4":4,"item_5":5,"item_6":6,
        "item_7":7,"item_8":8,"item_9":9,"item_10":10,"item_11":11,"item_12":12}),
    );
    assert_eq!(
        eval(graph(vec![make], vec![]), "make").answer["value"],
        json!((1..=12).collect::<Vec<_>>())
    );
    assert_eq!(
        eval(
            graph(
                vec![node("empty", "list_create", json!({"input_count":0}))],
                vec![]
            ),
            "empty"
        )
        .answer["value"],
        json!([])
    );
    for (kind, properties, expected) in [
        ("list_append", json!({"item":4}), json!([1, 2, 3, 4])),
        ("list_concat", json!({}), json!([1, 2, 3, 4, 5])),
        ("list_get", json!({"index":1}), json!(2)),
        ("list_set", json!({"index":1,"item":8}), json!([1, 8, 3])),
        ("list_remove", json!({"index":1}), json!([1, 3])),
        ("list_length", json!({}), json!(3)),
        ("list_slice", json!({"start":1,"count":99}), json!([2, 3])),
        ("list_contains", json!({"item":2}), json!(true)),
        ("list_reverse", json!({}), json!([3, 2, 1])),
    ] {
        let mut edges = vec![edge("list", "value", "op", "list")];
        if kind == "list_concat" {
            edges.push(edge("other", "value", "op", "other"));
        }
        let graph = graph(
            vec![
                node("list", "constant", json!({"value":[1,2,3]})),
                node("other", "constant", json!({"value":[4,5]})),
                node("op", kind, properties),
            ],
            edges,
        );
        let result = eval(graph, "op");
        assert!(result.errors.is_empty(), "{kind}: {:?}", result.errors);
        assert_eq!(result.answer["value"], expected, "{kind}");
    }
    for kind in ["list_get", "list_set", "list_remove", "list_slice"] {
        let result = flow_builtins::evaluate(
            graph(
                vec![
                    node("list", "constant", json!({"value":[1]})),
                    node("op", kind, json!({"index":2,"start":2,"count":1,"item":0})),
                ],
                vec![edge("list", "value", "op", "list")],
            ),
            EvaluationContext::default(),
            "op",
            None,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            result.is_err() || !result.unwrap().errors.is_empty(),
            "{kind}"
        );
    }
}

fn poll(core: &mut Orchestrator, sequence: u64) {
    let source = &core.state.sources[&NodeId::from("watch")];
    core.source_event(SourceEvent {
        delivery: None,
        node: "watch".into(),
        generation: source.generation,
        sequence,
        pin: "__poll".into(),
        args: json!({}),
        values: Default::default(),
        state: json!({}),
    })
    .unwrap();
    drain(core);
}
fn install_watch(core: &mut Orchestrator, previous: Value) {
    let spec = core.state.workflows[&WorkflowId::from("flow")]
        .graph
        .node("watch")
        .unwrap();
    let mut properties = spec.flow.properties.clone();
    properties
        .entry("interval_secs".to_owned())
        .or_insert(json!(1));
    core.state.sources.insert(
        "watch".into(),
        SourceRegistration {
            environment: None,
            subscription: None,
            workflow: "flow".into(),
            node: "watch".into(),
            type_id: "local.on_all_true".into(),
            provider: "local".into(),
            generation: core.state.revision + 1,
            handle: "manual-test-clock".into(),
            properties,
            sequence: 0,
            state: previous,
        },
    );
}
#[test]
fn all_true_resamples_live_values_and_rearms_only_when_repeatable() {
    for repeatable in [false, true] {
        let mut flag = node("flag", "window", json!({}));
        flag.flow.outputs.push(Pin {
            property: Some("flag".into()),
            ..Pin::new("flag", Type::Bool)
        });
        let graph = graph(
            vec![
                flag,
                node(
                    "watch",
                    "on_all_true",
                    json!({"input_count":2,"condition_2":true,"repeatable":repeatable}),
                ),
                node("sink", "shell", json!({"command":"true"})),
            ],
            vec![
                edge("flag", "flag", "watch", "condition_1"),
                edge("watch", "out", "sink", "in"),
            ],
        );
        let mut core = core(graph);
        install_watch(&mut core, Value::Null);
        for (sequence, value, count) in [
            (1, false, 0),
            (2, true, 1),
            (3, true, 1),
            (4, false, 1),
            (5, true, if repeatable { 2 } else { 1 }),
        ] {
            core.state
                .queue
                .published
                .entry("flag".into())
                .or_default()
                .insert("flag".into(), json!(value));
            poll(&mut core, sequence);
            assert_eq!(
                core.state
                    .executions
                    .values()
                    .filter(|e| e.node.0 == "sink")
                    .count(),
                count
            );
        }
        let previous = core.state.sources[&NodeId::from("watch")].state.clone();
        let state = serde_json::from_value(serde_json::to_value(&core.state).unwrap()).unwrap();
        let mut core = Orchestrator::new(state, core.registry.clone());
        install_watch(&mut core, previous);
        poll(&mut core, 1);
        assert_eq!(
            core.state
                .executions
                .values()
                .filter(|e| e.node.0 == "sink")
                .count(),
            if repeatable { 2 } else { 1 }
        );
    }
}

#[test]
fn all_fired_waits_across_correlations_ignores_duplicates_and_repeats() {
    let mut core = core(graph(
        vec![node("barrier", "on_all_fired", json!({"input_count":2}))],
        vec![],
    ));
    for (id, pin, count) in [
        ("a1", "signal_1", 0),
        ("a2", "signal_1", 0),
        ("b1", "signal_2", 1),
        ("b2", "signal_2", 1),
        ("a3", "signal_1", 2),
    ] {
        let activation = Activation {
            pin: pin.into(),
            id: "barrier".into(),
            args: json!({"from":id}),
            inputs: json!({}),
        };
        core.invoke_activation("flow".into(), id.into(), activation, None, Arc::new(|_| {}))
            .unwrap();
        assert_eq!(core.state.executions.len(), count);
        let state = serde_json::from_value(serde_json::to_value(&core.state).unwrap()).unwrap();
        core = Orchestrator::new(state, core.registry.clone());
    }
    for context in core.state.queue.invocations.values() {
        assert_eq!(context.request.args, json!({}));
    }
}

#[test]
fn session_actions_use_configurable_literal_argv_and_never_run_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result");
    std::fs::write(dir.path().join("session.kdl"), format!("action \"power_off\" \"/usr/bin/printf\" \"literal; $(touch {})\"\naction \"hibernate\"\n", output.display())).unwrap();
    let registry = registry_with_environment(
        Limits::default(),
        Environment {
            config_directory: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut graph = FlowNet {
        nodes: vec![node("power", "power_off", json!({}))],
        ..Default::default()
    };
    registry.prepare_graph(&mut graph);
    let definition = registry.definition("local.power_off").unwrap();
    assert!(
        definition.properties["argv"].default.as_ref().unwrap()[1]
            .as_str()
            .unwrap()
            .contains("$(touch")
    );
    let process = flow_builtins::session_actions::process(
        &Invocation {
            environment: None,
            input: "in".into(),
            id: "power".into(),
            workflow: "flow".into(),
            node: "power".into(),
            type_id: "local.power_off".into(),
            properties: [(
                "argv".into(),
                definition.properties["argv"].default.clone().unwrap(),
            )]
            .into(),
            args: json!({}),
            generation: 1,
        },
        &Environment::default(),
        &Limits::default(),
    )
    .unwrap();
    let result = process
        .command
        .get_args()
        .map(|s| s.to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(result.len(), 1);
    let evaluation = flow_core::scheduler::evaluate(
        graph,
        EvaluationContext::default(),
        "power",
        None,
        Arc::new(AtomicBool::new(false)),
        registry,
        Arc::new(flow_builtins::Scripts(Limits::default())),
    )
    .unwrap();
    assert!(evaluation.activations.is_empty());
    assert!(!output.exists());
}

#[test]
fn idle_output_is_opt_in_and_managed_invocations_report_busy() {
    let mut agent = node("agent", "agent", json!({"command":"unused"}));
    let mut graph = graph(vec![agent.clone()], vec![]);
    assert!(
        !pins(&graph, &agent, true)
            .iter()
            .any(|p| p.name == "is_idle")
    );
    agent
        .flow
        .properties
        .insert("expose_is_idle".into(), json!(true));
    graph.nodes[0] = agent.clone();
    assert!(
        pins(&graph, &agent, true)
            .iter()
            .any(|p| p.name == "is_idle" && p.ty == Type::Bool)
    );
    let mut core = core(graph);
    core.execute(
        Command::Read {
            workflow: "flow".into(),
            node: "agent".into(),
            evaluation: "idle-read".into(),
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    let (_, _, context) = core.next_frame().unwrap();
    assert_eq!(context.published[&NodeId::from("agent")]["is_idle"], true);
    core.invoke_activation(
        "flow".into(),
        "busy".into(),
        Activation {
            pin: "in".into(),
            id: "agent".into(),
            args: json!({}),
            inputs: json!({}),
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    let (_, _, context) = core.next_frame().unwrap();
    assert_eq!(context.published[&NodeId::from("agent")]["is_idle"], false);
    core.complete_invocation(Completion {
        invocation: "busy".into(),
        generation: core.state.executions[&InvocationId::from("busy")].generation,
        status: ExecutionStatus::Interrupted,
        result: json!({}),
    })
    .unwrap();
    assert_eq!(
        core.next_frame().unwrap().2.published[&NodeId::from("agent")]["is_idle"],
        true
    );
}
