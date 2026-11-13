mod support;

use flow_core::{domain::*, environment::Env, protocol::Command};
use flowwm::{client::Action, model::*};
use serde_json::json;
use std::path::Path;

fn settings(client: &support::TestClient, workflow: &str, directory: &Path) {
    client
        .orchestrate(
            Command::SetRowSettings {
                workflow: workflow.into(),
                settings: Env::from([("pwd".into(), directory.to_str().unwrap().into())]),
            },
            None,
        )
        .unwrap();
}

fn desktop(client: &support::TestClient, workflow: &str) {
    client
        .orchestrate(
            Command::ActivateSession {
                workspace: "local:desktop".into(),
                provider: "desktop".into(),
                incarnation: "fixture".into(),
            },
            None,
        )
        .unwrap();
    client
        .orchestrate(
            Command::Move {
                workflow: workflow.into(),
                workspace: "local:desktop".into(),
            },
            None,
        )
        .unwrap();
}

fn runtime(client: &support::TestClient) -> RuntimeState {
    serde_json::from_value(client.orchestrate(Command::Snapshot, None).unwrap()).unwrap()
}

fn completed(client: &support::TestClient, node: &str) {
    support::wait(|| {
        runtime(client)
            .executions
            .values()
            .any(|e| e.node.0 == node && !e.active())
    });
    let runtime = runtime(client);
    let execution = runtime
        .executions
        .values()
        .filter(|e| e.node.0 == node)
        .max_by_key(|e| e.generation)
        .unwrap();
    assert_eq!(
        execution.status,
        ExecutionStatus::Complete,
        "{:?}",
        execution.result
    );
}

#[test]
fn application_creation_and_recipes_stay_idle_until_manual_run_or_signal() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    for path in [&first, &second] {
        std::fs::create_dir(path).unwrap();
    }
    let state = State::empty(directory.path().into());
    let workflow = state.selected_workspace.clone();
    let (client, _) = support::fixture_with_config(state, None, |config| {
        let entries = config.parent().unwrap().join("data/applications");
        std::fs::create_dir_all(&entries).unwrap();
        std::fs::write(entries.join("flowwm-workflow-fixture.desktop"),
            "[Desktop Entry]\nType=Application\nName=Workflow fixture\nExec=/bin/sh -c \"pwd > launched.txt; /usr/bin/printenv APP_ENV > environment.txt\"\n").unwrap();
    });
    desktop(&client, &workflow);
    let mut app = Node::new(
        &workflow,
        NodeKind::Window,
        Default::default(),
        Default::default(),
    );
    app.flow
        .properties
        .insert("application".into(), json!("flowwm-workflow-fixture"));
    client
        .request(Action::Create {
            node: Box::new(app.clone()),
        })
        .unwrap();
    settings(&client, &workflow, &first);
    assert!(runtime(&client).executions.is_empty());
    assert!(!first.join("launched.txt").exists());
    client
        .request(Action::Evaluate { id: app.id.clone() })
        .unwrap();
    support::wait(|| runtime(&client).queue.frames.is_empty());
    assert!(
        !first.join("launched.txt").exists(),
        "Reading an application cannot launch it"
    );
    client
        .request(Action::Run {
            id: app.id.clone(),
            downstream: false,
        })
        .unwrap();
    completed(&client, &app.id);
    support::wait(|| first.join("launched.txt").exists());
    assert_eq!(
        std::fs::read_to_string(first.join("launched.txt"))
            .unwrap()
            .trim(),
        first.to_str().unwrap()
    );

    let recipe = client
        .snapshot()
        .state
        .recipe("Reusable app".into(), std::slice::from_ref(&app.id))
        .unwrap();
    assert!(recipe.nodes[0].cwd.as_os_str().is_empty());
    settings(&client, &workflow, &second);
    let value = client
        .request(Action::Paste {
            recipe,
            offset: Position::default(),
        })
        .unwrap();
    let copy = value["ids"][0].as_str().unwrap().to_owned();
    assert!(!second.join("launched.txt").exists());
    let mut env = Node::new(
        &workflow,
        NodeKind::Extension,
        Default::default(),
        Default::default(),
    );
    env.flow.node_type = "local.set_env".into();
    env.flow
        .properties
        .insert("variables".into(), json!({"APP_ENV":"from-signal"}));
    client
        .request(Action::Create {
            node: Box::new(env.clone()),
        })
        .unwrap();
    client
        .request(Action::ConnectPins {
            connection: flowwm::graph::Connection {
                source: flowwm::graph::Endpoint {
                    node: env.id,
                    pin: "env".into(),
                },
                target: flowwm::graph::Endpoint {
                    node: copy.clone(),
                    pin: "env".into(),
                },
            },
        })
        .unwrap();
    let mut source = Node::new(
        &workflow,
        NodeKind::Human,
        directory.path().into(),
        Default::default(),
    );
    source.id = "trigger".into();
    client
        .request(Action::Create {
            node: Box::new(source.clone()),
        })
        .unwrap();
    client
        .request(Action::ConnectPins {
            connection: flowwm::graph::Connection {
                source: flowwm::graph::Endpoint {
                    node: source.id.clone(),
                    pin: "accepted".into(),
                },
                target: flowwm::graph::Endpoint {
                    node: copy.clone(),
                    pin: "in".into(),
                },
            },
        })
        .unwrap();
    client
        .request(Action::Emit {
            source: flowwm::graph::Endpoint {
                node: source.id,
                pin: "accepted".into(),
            },
            args: json!({}),
        })
        .unwrap();
    completed(&client, &copy);
    support::wait(|| second.join("launched.txt").exists());
    assert_eq!(
        std::fs::read_to_string(second.join("launched.txt"))
            .unwrap()
            .trim(),
        second.to_str().unwrap()
    );
    support::wait(|| {
        std::fs::read_to_string(second.join("environment.txt"))
            .is_ok_and(|s| s.trim() == "from-signal")
    });

    let explicit = directory.path().join("explicit");
    std::fs::create_dir(&explicit).unwrap();
    app.cwd = explicit.clone();
    client
        .request(Action::Update {
            node: Box::new(app.clone()),
        })
        .unwrap();
    let recipe = client
        .snapshot()
        .state
        .recipe("Explicit app".into(), std::slice::from_ref(&app.id))
        .unwrap();
    assert_eq!(recipe.nodes[0].cwd, explicit);
    client
        .request(Action::Run {
            id: app.id,
            downstream: false,
        })
        .unwrap();
    support::wait(|| explicit.join("launched.txt").exists());
    assert_eq!(
        std::fs::read_to_string(explicit.join("launched.txt"))
            .unwrap()
            .trim(),
        explicit.to_str().unwrap()
    );
}

#[test]
fn unified_supervised_session_inherits_pwd_with_empty_env_and_keeps_failure_branches() {
    let directory = tempfile::tempdir().unwrap();
    let state = State::empty(directory.path().into());
    let workflow = state.selected_workspace.clone();
    let (client, _) = support::fixture_with_config(state, None, |config| {
        std::fs::write(config.join("agent.kdl"), format!(
            "default-harness \"fixture\"\nharness \"fixture\" {{\n adapter \"jsonl\"\n command \"/bin/sh\" \"{}\"\n}}\n", directory.path().join("agent.sh").display())).unwrap();
    });
    std::fs::write(
        directory.path().join("agent.sh"),
        r#"cat > prompt.txt
printf '%s\n' '{"stdout":"{\"outcome\":\"issue\",\"summary\":\"Needs review\",\"artifacts\":[]}"}'
"#,
    )
    .unwrap();
    settings(&client, &workflow, directory.path());
    let mut agent = Node::new(
        &workflow,
        NodeKind::Extension,
        Default::default(),
        Default::default(),
    );
    agent.command = "Review this workspace".into();
    agent.flow.node_type = flow_builtins::agent::TYPE_ID.into();
    let mut empty = Node::new(
        &workflow,
        NodeKind::Extension,
        Default::default(),
        Default::default(),
    );
    empty.flow.node_type = "local.empty_env".into();
    let gate = Node::new(
        &workflow,
        NodeKind::Gate,
        directory.path().into(),
        Default::default(),
    );
    for node in [&agent, &empty, &gate] {
        client
            .request(Action::Create {
                node: Box::new(node.clone()),
            })
            .unwrap();
    }
    for (source, output, target, input) in [
        (&empty.id, "env", &agent.id, "env"),
        (&agent.id, "failed", &gate.id, "in"),
    ] {
        client
            .request(Action::ConnectPins {
                connection: flowwm::graph::Connection {
                    source: flowwm::graph::Endpoint {
                        node: source.clone(),
                        pin: output.into(),
                    },
                    target: flowwm::graph::Endpoint {
                        node: target.clone(),
                        pin: input.into(),
                    },
                },
            })
            .unwrap();
    }
    assert!(!directory.path().join("prompt.txt").exists());
    client
        .request(Action::Run {
            id: agent.id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| {
        runtime(&client)
            .executions
            .values()
            .any(|e| e.node.0 == gate.id && e.status == ExecutionStatus::Waiting)
    });
    let runtime = runtime(&client);
    let run = runtime
        .executions
        .values()
        .find(|e| e.node.0 == agent.id)
        .unwrap();
    assert_eq!(run.status, ExecutionStatus::Failed);
    assert_eq!(run.result["summary"], "Needs review");
    let request = &runtime.queue.invocations[&run.id].request;
    assert_eq!(request.environment, Some(Env::new()));
    assert_eq!(request.properties["cwd"], json!(directory.path()));
    assert!(
        std::fs::read_to_string(directory.path().join("prompt.txt"))
            .unwrap()
            .contains("Review this workspace")
    );
}
