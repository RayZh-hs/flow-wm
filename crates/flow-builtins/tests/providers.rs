use flow_builtins::local::{Environment, Limits, Local};
use flow_core::{domain::ExecutionStatus, providers::*};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::Duration,
};

fn invocation(id: &str, kind: &str, directory: &std::path::Path, command: &str) -> Invocation {
    Invocation {
        id: id.into(),
        workflow: "workflow".into(),
        node: id.into(),
        type_id: format!("local.{kind}"),
        properties: BTreeMap::from([
            ("command".into(), json!(command)),
            ("cwd".into(), json!(directory)),
        ]),
        args: json!({}),
        generation: 7,
    }
}
#[test]
fn bounded_output_is_drained_without_breaking_a_successful_command() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Local::default();
    let (tx, rx) = mpsc::channel();
    provider
        .invoke(
            invocation(
                "large",
                "shell",
                dir.path(),
                "head -c 2000000 /dev/zero; printf finished",
            ),
            Arc::new(move |e| {
                tx.send(e).unwrap();
            }),
        )
        .unwrap();
    let completion = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(completion.status, ExecutionStatus::Complete);
    assert_eq!(
        completion.result["stdout"].as_str().unwrap().len(),
        1_048_576
    );
}
#[test]
fn registered_agent_uses_the_configured_cli_and_validates_its_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("agent.sh");
    std::fs::write(&script, r#"cat > prompt.txt
printf '%s\n' '{"type":"thread.started","thread_id":"test-session"}' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"outcome\":\"issue\",\"summary\":\"Needs a decision\",\"artifacts\":[]}"}}'
"#).unwrap();
    std::fs::write(
        dir.path().join("agent.json"),
        json!({"command":["/bin/sh",script]}).to_string(),
    )
    .unwrap();
    let provider = Local::with_environment(
        Limits::default(),
        Environment {
            config_directory: Some(dir.path().into()),
            data_directory: Some(dir.path().into()),
            ..Default::default()
        },
    );
    let (tx, rx) = mpsc::channel();
    provider
        .invoke(
            invocation("agent", "agent", dir.path(), "Inspect this project"),
            Arc::new(move |e| {
                tx.send(e).unwrap();
            }),
        )
        .unwrap();
    let completion = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(completion.status, ExecutionStatus::Failed);
    assert_eq!(completion.result["summary"], "Needs a decision");
    assert_eq!(completion.result["agent_session"], "test-session");
    let prompt = std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap();
    assert!(prompt.contains("Inspect this project") && prompt.contains("required JSON object"));
}
#[test]
fn registered_metric_reports_a_typed_sample_and_does_not_accept_invalid_output() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Local::default();
    for (id, command, status) in [
        (
            "valid",
            "printf '{\"value\":42}'",
            ExecutionStatus::Complete,
        ),
        ("invalid", "printf unknown", ExecutionStatus::Failed),
    ] {
        let (tx, rx) = mpsc::channel();
        provider
            .invoke(
                invocation(id, "metric", dir.path(), command),
                Arc::new(move |e| {
                    tx.send(e).unwrap();
                }),
            )
            .unwrap();
        let completion = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(completion.status, status);
        if id == "valid" {
            assert_eq!(completion.result["value"], 42.0);
        }
    }
}

#[test]
fn decisions_wait_for_resolution_and_interruption_releases_the_registration() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Local::default();
    let (tx, rx) = mpsc::channel();
    let sink: EventSink = Arc::new(move |e| {
        tx.send(e).unwrap();
    });
    let handle = provider
        .invoke(invocation("decision", "gate", dir.path(), ""), sink.clone())
        .unwrap();
    assert!(rx.try_recv().is_err());
    provider
        .resolve(&handle, true, json!({"output":"Approved"}))
        .unwrap();
    let completion = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.status, ExecutionStatus::Complete);
    assert_eq!(completion.result["output"], "Approved");
    assert!(provider.resolve(&handle, true, json!({})).is_err());
    provider
        .invoke(invocation("decision", "human", dir.path(), ""), sink)
        .unwrap();
    provider.interrupt(&handle).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().status,
        ExecutionStatus::Interrupted
    );
}
