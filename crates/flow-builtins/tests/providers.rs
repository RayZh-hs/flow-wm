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
        input: String::new(),
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
                invocation(id, "metric_sample", dir.path(), command),
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
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().status,
        ExecutionStatus::Waiting
    );
    provider
        .resolve(&handle, true, json!({"output":"Approved"}))
        .unwrap();
    let mut completion = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    if completion.status == ExecutionStatus::Waiting {
        completion = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(completion.status, ExecutionStatus::Complete);
    assert_eq!(completion.result["output"], "Approved");
    assert!(provider.resolve(&handle, true, json!({})).is_err());
    provider
        .invoke(invocation("decision", "human", dir.path(), ""), sink)
        .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().status,
        ExecutionStatus::Waiting
    );
    provider.interrupt(&handle).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().status,
        ExecutionStatus::Interrupted
    );
}

#[test]
fn local_session_provider_activates_user_and_only_valid_service_incarnations() {
    use flow_builtins::sessions::LocalSessions;
    let mut runtime = flow_core::domain::RuntimeState::local();
    LocalSessions::new("local".into(), "user-1".into(), Some("invalid".into()))
        .unwrap()
        .activate(&mut runtime.workspaces)
        .unwrap();
    assert!(
        runtime
            .workspaces
            .require_active(&"local:user".into())
            .is_ok()
    );
    assert!(
        runtime
            .workspaces
            .require_active(&"local:systemd".into())
            .is_err()
    );
    let service = "0123456789abcdef0123456789abcdef";
    LocalSessions::new("local".into(), "user-2".into(), Some(service.into()))
        .unwrap()
        .activate(&mut runtime.workspaces)
        .unwrap();
    assert_eq!(
        runtime
            .workspaces
            .availability(&"local:systemd".into())
            .unwrap()
            .incarnation,
        service
    );
    assert!(
        runtime
            .workspaces
            .require_active(&"local:desktop".into())
            .is_err()
    );
    LocalSessions::new("local".into(), "user-3".into(), None)
        .unwrap()
        .activate(&mut runtime.workspaces)
        .unwrap();
    assert!(
        runtime
            .workspaces
            .require_active(&"local:systemd".into())
            .is_err()
    );
}
