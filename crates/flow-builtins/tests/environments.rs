use flow_core::{domain::*, environment::Env, graph::*};
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
fn edge(source: &str, output: &str, target: &str, input: &str) -> Connection {
    Connection {
        source: Endpoint {
            node: source.into(),
            pin: output.into(),
        },
        target: Endpoint {
            node: target.into(),
            pin: input.into(),
        },
    }
}
fn evaluate(
    nodes: Vec<NodeSpec>,
    connections: Vec<Connection>,
    root: &str,
    args: Option<Value>,
) -> Evaluation {
    flow_builtins::evaluate(
        FlowNet {
            nodes,
            connections,
            ..Default::default()
        },
        EvaluationContext {
            environment: Env::from([
                ("KEEP".into(), "inherited".into()),
                ("DROP".into(), "old".into()),
            ]),
            ..Default::default()
        },
        root,
        args.map(|v| ("", v)),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
#[test]
fn transforms_inherit_row_values_and_empty_env_replaces_the_entire_environment() {
    let nodes = vec![
        node("set", "set_env", json!({"variables":{"A":"one","B":"two"}})),
        node("unset", "unset_env", json!({"names":["DROP","A"]})),
    ];
    let result = evaluate(
        nodes.clone(),
        vec![edge("set", "env", "unset", "env")],
        "unset",
        None,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.answer["env"], json!({"KEEP":"inherited","B":"two"}));
    let mut nodes = nodes;
    nodes.push(node("empty", "empty_env", json!({})));
    let result = evaluate(nodes, vec![edge("empty", "env", "set", "env")], "set", None);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.answer["env"], json!({"A":"one","B":"two"}));
}
#[test]
fn bake_maps_multiple_args_and_forwards_the_original_record_to_python() {
    let nodes = vec![
        node(
            "bake",
            "bake_args_into_env",
            json!({"mappings":{"count":"CUSTOM_COUNT","payload":"FLOWWM_ARG_PAYLOAD"}}),
        ),
        node(
            "python",
            "python",
            json!({"command":"import os\noutputs = {'value': [args, os.environ['CUSTOM_COUNT'], os.environ['FLOWWM_ARG_PAYLOAD'], os.environ.get('FLOWWM_ARG_COUNT'), os.environ['KEEP']]}"}),
        ),
    ];
    let result = evaluate(
        nodes,
        vec![
            edge("bake", "out", "python", "in"),
            edge("bake", "env", "python", "env"),
        ],
        "bake",
        Some(json!({"count":7,"payload":{"hello":"world"}})),
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.values["python"]["value"],
        json!([{"count":7,"payload":{"hello":"world"}},"7","{\"hello\":\"world\"}",null,"inherited"])
    );
}
#[test]
fn invalid_env_names_values_and_ambiguous_argument_destinations_are_rejected() {
    for value in [
        json!({"BAD=NAME":"x"}),
        json!({"OK":1}),
        json!({"OK":"\u{0}"}),
    ] {
        assert!(check_value(&FlowNet::default(), &value, &Type::Env).is_err());
    }
    for mappings in [
        json!({"a":"SAME","b":"SAME"}),
        json!({"missing":"ENV"}),
        json!({"a":"BAD=NAME"}),
    ] {
        let result = evaluate(
            vec![node(
                "bake",
                "bake_args_into_env",
                json!({"mappings":mappings}),
            )],
            vec![],
            "bake",
            Some(json!({"a":1,"b":2})),
        );
        assert!(!result.errors.is_empty());
    }
    assert!(validate_signal_args(&json!({"a":1,"A":2,"text":"\u{0}"})).is_ok());
}
#[test]
fn pwd_extension_uses_home_and_terminal_hooks_are_configurable() {
    let mut registry = flow_core::providers::Registry::default();
    registry
        .session_environment
        .insert("HOME".into(), "/home/test user".into());
    let mut extension = flow_builtins::row_settings::Extension::default();
    extension.install(&mut registry);
    let env = registry.row_environment(&Env::new()).unwrap();
    assert_eq!(env["FLOWWM_ROWSET_PWD"], "/home/test user");
    assert_eq!(
        extension.terminal_arguments("/usr/bin/alacritty", &env),
        ["--working-directory", "/home/test user"]
    );
    extension.terminals.insert("alacritty".into(), vec![]);
    assert!(extension.terminal_arguments("alacritty", &env).is_empty());
    assert!(
        extension
            .terminal_arguments("kitty", &Env::new())
            .is_empty()
    );
}

#[test]
fn reading_an_application_env_input_only_evaluates_its_lazy_dependencies() {
    let result = flow_builtins::evaluate(
        FlowNet {
            nodes: vec![
                node("set", "set_env", json!({"variables":{"APP":"configured"}})),
                node("app", "window", json!({})),
            ],
            connections: vec![edge("set", "env", "app", "env")],
            ..Default::default()
        },
        EvaluationContext {
            input: Some("env".into()),
            environment: [("SESSION".into(), "inherited".into())].into(),
            ..Default::default()
        },
        "app",
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.activations.is_empty() && result.executed.is_empty());
    assert_eq!(
        result.answer,
        json!({"SESSION":"inherited","APP":"configured"})
    );
}
