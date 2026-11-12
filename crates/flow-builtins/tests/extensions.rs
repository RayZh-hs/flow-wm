use flow_builtins::{
    agent::{self, harness::Harnesses},
    local::{self, Environment, Limits},
};
use flow_core::{domain::ExecutionStatus, providers::*};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::Duration,
};

fn invocation(directory: &std::path::Path, ty: &str, id: &str) -> Invocation {
    Invocation {
        environment: None,
        input: "in".into(),
        id: id.into(),
        node: "session-node".into(),
        workflow: "workflow".into(),
        type_id: ty.into(),
        properties: BTreeMap::from([
            ("cwd".into(), json!(directory)),
            ("command".into(), json!("Hello from a test")),
            ("conversational".into(), json!(true)),
        ]),
        args: json!({"original":7}),
        generation: 1,
    }
}
fn environment(directory: &std::path::Path) -> Environment {
    Environment {
        config_directory: Some(directory.into()),
        data_directory: Some(directory.into()),
        ..Default::default()
    }
}

#[test]
fn builtin_harnesses_include_codex_claude_and_custom_commands_without_a_shell() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("agent.kdl"),
        r#"
default-harness "custom"
harness "custom" {
    adapter "jsonl"
    command "custom-cli" "a literal argument"
    run "--json"
    resume "--json" "--resume" "{session}"
}
"#,
    )
    .unwrap();
    let config = Harnesses::load(Some(directory.path())).unwrap();
    assert!(config.harnesses.contains_key("codex") && config.harnesses.contains_key("claude"));
    let command = config
        .select(None)
        .unwrap()
        .process(Some("literal; $(touch /tmp/unwanted)"), false, None)
        .unwrap();
    assert_eq!(command.get_program(), "custom-cli");
    assert_eq!(
        command
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "a literal argument",
            "--json",
            "--resume",
            "literal; $(touch /tmp/unwanted)"
        ]
    );
    std::fs::write(
        directory.path().join("agent.kdl"),
        "default-harness \"missing\"\n",
    )
    .unwrap();
    assert!(Harnesses::load(Some(directory.path())).is_err());
}

#[test]
fn builtin_session_streams_a_readable_transcript_and_resumes_after_provider_restart() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("fake.py");
    std::fs::write(
        &script,
        r#"import json, pathlib, sys
pathlib.Path('arguments.json').write_text(json.dumps(sys.argv[1:]))
pathlib.Path('prompt.txt').write_text(sys.stdin.read())
print(json.dumps({'type':'session','id':'exact-session'}), flush=True)
print(json.dumps({'type':'message','text':'A live response'}), flush=True)
"#,
    )
    .unwrap();
    std::fs::write(
        directory.path().join("agent.kdl"),
        format!(
            r#"
default-harness "fake"
harness "fake" {{
    adapter "jsonl"
    command "python3" "{}"
    run "--json"
    resume "--json" "--resume" "{{session}}"
}}
"#,
            script.display()
        ),
    )
    .unwrap();
    for id in ["first-invocation", "second-invocation"] {
        let registry =
            local::registry_with_environment(Limits::default(), environment(directory.path()))
                .unwrap();
        let definition = registry.definition(agent::TYPE_ID).unwrap();
        assert_eq!(definition.owner.0, "agent_sessions");
        let provider = registry.provider(&definition.owner).unwrap();
        let (tx, rx) = mpsc::channel();
        provider
            .invoke(
                invocation(directory.path(), agent::TYPE_ID, id),
                Arc::new(move |e| tx.send(e).unwrap()),
            )
            .unwrap();
        let completion = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            completion.status,
            ExecutionStatus::Complete,
            "{:?}",
            completion.result
        );
        assert_eq!(completion.result["stdout"], "A live response");
        assert_eq!(completion.result["agent_session"], "exact-session");
        assert_eq!(completion.result["harness"], "fake");
    }
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(directory.path().join("arguments.json")).unwrap())
            .unwrap();
    assert_eq!(args, ["--json", "--resume", "exact-session"]);
    assert!(
        std::fs::read_to_string(directory.path().join("prompt.txt"))
            .unwrap()
            .contains("Hello from a test")
    );
}

#[test]
fn subprocess_manifests_are_inert_and_use_the_invoked_provider_contract() {
    let directory = tempfile::tempdir().unwrap();
    let extensions = directory.path().join("extensions");
    std::fs::create_dir(&extensions).unwrap();
    let script = extensions.join("handler.py");
    std::fs::write(&script, "import json, pathlib, sys\nr=json.load(sys.stdin)\npathlib.Path('started').touch()\nprint(json.dumps({'stdout':r['operation'], 'value':r['args']['original']}))\n").unwrap();
    let manifest = json!({"version":3,"providers":[{"id":"test.send","owner":"test","name":"Send test", "behavior":"invoked", "capabilities":[], "inputs":[], "outputs":[], "command":["python3", "./handler.py"], "settings":["python3","./handler.py"]}]});
    std::fs::write(extensions.join("test.json"), manifest.to_string()).unwrap();
    let registry =
        local::registry_with_environment(Limits::default(), environment(directory.path())).unwrap();
    assert!(!directory.path().join("started").exists());
    let provider = registry.provider(&"test".into()).unwrap();
    assert!(
        provider
            .read(invocation(directory.path(), "test.send", "no-side-effect"))
            .is_err()
    );
    assert!(!directory.path().join("started").exists());
    let (tx, rx) = mpsc::channel();
    provider
        .invoke(
            invocation(directory.path(), "test.send", "send"),
            Arc::new(move |e| tx.send(e).unwrap()),
        )
        .unwrap();
    let completion = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        completion.status,
        ExecutionStatus::Complete,
        "{:?}",
        completion.result
    );
    assert_eq!(completion.result["stdout"], "invoke");
    assert_eq!(completion.result["value"], 7);
    assert!(directory.path().join("started").exists());
}
