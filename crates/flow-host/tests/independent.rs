//! Production binaries, transport, workers and providers. No desktop model fixtures.
use flow_core::{domain::*, graph::*, protocol::Command};
use flow_host::{client::Client, ipc, store::Store};
use serde_json::{Value, json};
use std::{
    process::{Child, Command as Process, Stdio},
    thread,
    time::{Duration, Instant},
};
fn wait(mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < Duration::from_secs(12), "Timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
struct Fixture {
    child: Child,
    dir: tempfile::TempDir,
    client: Client,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(json!({}))
    }
    fn configured(config: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/runtime.kdl"),
            config
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| format!("{} {}\n", k.replace('_', "-"), v))
                .collect::<String>(),
        )
        .unwrap();
        let client = Client::new(dir.path().join("host.sock"));
        let child = Process::new(env!("CARGO_BIN_EXE_flow-core"))
            .args([
                "--socket",
                client.socket.to_str().unwrap(),
                "--data-dir",
                dir.path().join("data").to_str().unwrap(),
            ])
            .env("FLOWWM_CONFIG_DIR", dir.path().join("config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        wait(|| client.snapshot().is_ok());
        Self { child, dir, client }
    }
    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Process::new(env!("CARGO_BIN_EXE_flow-core"))
            .arg("--socket")
            .arg(&self.client.socket)
            .arg("--data-dir")
            .arg(self.dir.path().join("data"))
            .env("FLOWWM_CONFIG_DIR", self.dir.path().join("config"))
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        wait(|| self.client.snapshot().is_ok());
    }
    fn graph(&self, nodes: Vec<NodeSpec>, connections: Vec<Connection>) -> WorkflowId {
        let id: WorkflowId = uuid::Uuid::new_v4().to_string().into();
        self.client
            .command(Command::Create {
                workflow: Workflow {
                    row_settings: Default::default(),
                    id: id.clone(),
                    name: "Fixture".into(),
                    workspace: Some("local:user".into()),
                    visibility: Visibility::Public,
                    owner: None,
                    references: Default::default(),
                    graph: FlowNet {
                        nodes,
                        connections,
                        ..Default::default()
                    },
                    authority: "local".into(),
                    transfer: None,
                },
            })
            .unwrap();
        id
    }
    fn run(&self, workflow: &WorkflowId, node: &str, id: &str) {
        self.client
            .command(Command::Run {
                workflow: workflow.clone(),
                node: node.into(),
                invocation: id.into(),
                args: json!({"NUMBER":7}),
            })
            .unwrap();
    }
    fn until_terminal(&self, id: &str) -> Execution {
        wait(|| {
            self.client
                .snapshot()
                .unwrap()
                .executions
                .get(&id.into())
                .is_some_and(|e| !e.active())
        });
        self.client.snapshot().unwrap().executions[&id.into()].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn node(id: &str, kind: &str, command: &str, directory: &std::path::Path) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        type_id: format!("local.{kind}"),
        flow: Flow {
            properties: std::collections::BTreeMap::from([
                ("command".into(), json!(command)),
                ("cwd".into(), json!(directory)),
            ]),
            ..Default::default()
        },
    }
}
fn wire(a: &str, out: &str, b: &str, input: &str) -> Connection {
    Connection {
        source: Endpoint {
            node: a.into(),
            pin: out.into(),
        },
        target: Endpoint {
            node: b.into(),
            pin: input.into(),
        },
    }
}
#[test]
fn native_graph_executes_through_worker_and_survives_client_reconnection() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![
            node("first", "shell", "sleep 0.1; printf first", cwd),
            node(
                "second",
                "shell",
                "printf \"${FLOWWM_ARG_NUMBER-unset}\"",
                cwd,
            ),
        ],
        vec![wire("first", "out", "second", "in")],
    );
    f.run(&workflow, "first", "run");
    f.run(&workflow, "first", "run");
    let reconnected = Client::new(f.client.socket.clone());
    wait(|| {
        reconnected
            .snapshot()
            .unwrap()
            .executions
            .values()
            .any(|e| e.node.0 == "second" && e.status == ExecutionStatus::Complete)
    });
    let state = reconnected.snapshot().unwrap();
    assert_eq!(state.executions.len(), 2);
    assert_eq!(state.queue.published[&"second".into()]["stdout"], "unset");
    let saved = std::fs::read_to_string(cwd.join("data/runtime.json")).unwrap();
    for forbidden in [
        "compatibility",
        "window_id",
        "selected_workspace",
        "dependencies",
        "run_when",
    ] {
        assert!(!saved.contains(forbidden), "{forbidden}");
    }
    assert!(!cwd.join("data/presentation.json").exists());
}
#[test]
fn failures_follow_explicit_failure_pins_and_decisions_wait_for_resolution() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![
            node("bad", "shell", "exit 7", cwd),
            node("gate", "gate", "", cwd),
            node("after", "shell", "printf recovered", cwd),
        ],
        vec![
            wire("bad", "failed", "gate", "in"),
            wire("gate", "accepted", "after", "in"),
        ],
    );
    f.run(&workflow, "bad", "failure");
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .executions
            .values()
            .any(|e| e.node.0 == "gate" && e.status == ExecutionStatus::Waiting)
    });
    let state = f.client.snapshot().unwrap();
    assert!(!state.executions.values().any(|e| e.node.0 == "after"));
    let gate = state
        .executions
        .values()
        .find(|e| e.node.0 == "gate")
        .unwrap();
    f.client
        .command(Command::Resolve {
            invocation: gate.id.clone(),
            accepted: true,
            result: json!({"output":"reviewed"}),
        })
        .unwrap();
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .executions
            .values()
            .any(|e| e.node.0 == "after" && e.status == ExecutionStatus::Complete)
    });
}
#[test]
fn invocation_is_checkpointed_before_effects_and_directory_writers_are_serialized() {
    let f = Fixture::configured(json!({"max_concurrent":1}));
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![
            node("a", "shell", "sleep 0.15; printf A >> effects", cwd),
            node("b", "shell", "printf B >> effects", cwd),
        ],
        vec![],
    );
    f.run(&workflow, "a", "a-run");
    f.run(&workflow, "b", "b-run");
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(cwd.join("data/runtime.json")).unwrap()).unwrap();
    assert!(persisted["runtime"]["queue"]["frames"].as_array().is_some());
    f.until_terminal("a-run");
    f.until_terminal("b-run");
    assert_eq!(std::fs::read_to_string(cwd.join("effects")).unwrap(), "AB");
}
#[test]
fn interrupt_targets_the_origin_and_generation_checks_reject_late_completions() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![node(
            "work",
            "shell",
            "trap 'printf interrupted' INT; while :; do sleep 0.05; done",
            cwd,
        )],
        vec![],
    );
    f.run(&workflow, "work", "interrupt");
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .executions
            .get(&"interrupt".into())
            .is_some_and(|e| e.status == ExecutionStatus::Running)
    });
    f.client
        .command(Command::Move {
            workflow: workflow.clone(),
            workspace: "local:user".into(),
        })
        .unwrap();
    f.client
        .command(Command::Interrupt {
            invocation: "interrupt".into(),
        })
        .unwrap();
    let execution = f.until_terminal("interrupt");
    assert_eq!(execution.status, ExecutionStatus::Interrupted);
    let mut stale = execution.clone();
    stale.generation += 1;
    stale.status = ExecutionStatus::Complete;
    assert!(
        f.client
            .command(Command::ForwardCompletion { execution: stale })
            .is_err()
    );
}
#[test]
fn registered_sources_cannot_be_run_or_interrupted_and_teardown_owns_polling() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let mut metric = node("metric", "metric", "printf x >> samples; printf 2", cwd);
    metric
        .flow
        .properties
        .insert("interval_secs".into(), json!(1));
    let workflow = f.graph(vec![metric], vec![]);
    wait(|| cwd.join("samples").exists());
    assert!(
        f.client
            .command(Command::Run {
                workflow: workflow.clone(),
                node: "metric".into(),
                invocation: "bad".into(),
                args: json!({})
            })
            .is_err()
    );
    assert!(
        f.client
            .command(Command::Interrupt {
                invocation: "metric".into()
            })
            .is_err()
    );
    f.client.command(Command::Destroy { workflow }).unwrap();
    thread::sleep(Duration::from_millis(200));
    let before = std::fs::read(cwd.join("samples")).unwrap();
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(before, std::fs::read(cwd.join("samples")).unwrap());
}
#[test]
fn host_rejects_old_protocols_and_old_documents_without_modifying_them() {
    let f = Fixture::new();
    assert!(ipc::send(&f.client.socket, &json!({"op":"status"})).is_err());
    assert!(
        ipc::send(
            &f.client.socket,
            &json!({"protocol":2,"id":"old","command":{"command":"snapshot"}})
        )
        .is_err()
    );
    let dir = tempfile::tempdir().unwrap();
    let old = b"{\"version\":1}";
    std::fs::write(dir.path().join("state.json"), old).unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    assert!(store.load().is_err());
    assert_eq!(std::fs::read(dir.path().join("state.json")).unwrap(), old);
    assert!(!dir.path().join("runtime.json").exists());
}
#[test]
fn session_registration_rejects_stale_application_events() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(vec![node("app", "window", "", cwd)], vec![]);
    f.client
        .command(Command::ActivateSession {
            workspace: "local:desktop".into(),
            provider: "desktop".into(),
            incarnation: "new".into(),
        })
        .unwrap();
    f.client
        .command(Command::RegisterApplication {
            registration: flow_core::providers::ApplicationRegistration {
                workflow,
                node: "app".into(),
                workspace: "local:desktop".into(),
                incarnation: "new".into(),
                token: "app-token".into(),
                operations: flow_core::providers::ApplicationOperations {
                    open: true,
                    close_window: true,
                    interrupt: false,
                    disown: true,
                },
            },
        })
        .unwrap();
    assert!(
        f.client
            .command(Command::ApplicationEvent {
                node: "app".into(),
                token: "app-token".into(),
                incarnation: "old".into(),
                pin: "on_exit".into(),
                delivery: "exit".into(),
                args: json!({"exit_code":0})
            })
            .is_err()
    );
}

#[test]
fn explicit_join_waits_for_both_correlated_inputs_and_dispatches_once() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![
            node("root", "shell", "printf root", cwd),
            node("left", "shell", "printf left", cwd),
            node("right", "shell", "sleep 0.15; printf right", cwd),
            node("barrier", "join", "", cwd),
            node("last", "shell", "printf x >> joined", cwd),
        ],
        vec![
            wire("root", "out", "left", "in"),
            wire("root", "out", "right", "in"),
            wire("left", "out", "barrier", "left"),
            wire("right", "out", "barrier", "right"),
            wire("barrier", "out", "last", "in"),
        ],
    );
    f.run(&workflow, "root", "correlated");
    wait(|| cwd.join("joined").exists());
    thread::sleep(Duration::from_millis(100));
    assert_eq!(std::fs::read_to_string(cwd.join("joined")).unwrap(), "x");
    assert_eq!(
        f.client
            .snapshot()
            .unwrap()
            .executions
            .values()
            .filter(|e| e.node.0 == "barrier")
            .count(),
        1
    );
}
#[test]
fn moving_across_real_hosts_preserves_origin_and_forwards_interruption() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let a_socket = root.join("a.sock");
    let b_socket = root.join("b.sock");
    let mut children = Vec::new();
    for (host, peer, socket, peer_socket) in [
        ("a", "b", &a_socket, &b_socket),
        ("b", "a", &b_socket, &a_socket),
    ] {
        let config = root.join(format!("{host}-config"));
        std::fs::create_dir(&config).unwrap();
        std::fs::write(config.join("runtime.kdl"),format!("host-id {host:?}\nremote-retry-secs 1\nhost {peer:?} {{ transport \"unix\"; socket {:?}; }}\n",peer_socket.to_string_lossy())).unwrap();
        children.push(
            Process::new(env!("CARGO_BIN_EXE_flow-core"))
                .arg("--socket")
                .arg(socket)
                .arg("--data-dir")
                .arg(root.join(host))
                .env("FLOWWM_CONFIG_DIR", config)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    struct Guard(Vec<Child>);
    impl Drop for Guard {
        fn drop(&mut self) {
            for child in &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let _guard = Guard(children);
    let a = Client::new(a_socket);
    let b = Client::new(b_socket);
    wait(|| a.snapshot().is_ok() && b.snapshot().is_ok());
    a.command(Command::Create {
        workflow: Workflow {
            row_settings: Default::default(),
            id: "moving".into(),
            name: "Moved".into(),
            workspace: Some("a:user".into()),
            visibility: Visibility::Public,
            owner: None,
            references: Default::default(),
            graph: FlowNet {
                nodes: vec![node(
                    "remote-work",
                    "shell",
                    "while :; do sleep 0.05; done",
                    root,
                )],
                ..Default::default()
            },
            authority: "a".into(),
            transfer: None,
        },
    })
    .unwrap();
    a.command(Command::Run {
        workflow: "moving".into(),
        node: "remote-work".into(),
        invocation: "origin".into(),
        args: json!({}),
    })
    .unwrap();
    wait(|| {
        a.snapshot()
            .unwrap()
            .executions
            .get(&"origin".into())
            .is_some_and(|e| e.status == ExecutionStatus::Running)
    });
    let generation = a.snapshot().unwrap().executions[&"origin".into()].generation;
    a.command(Command::Move {
        workflow: "moving".into(),
        workspace: "b:user".into(),
    })
    .unwrap();
    wait(|| {
        b.snapshot()
            .unwrap()
            .workflows
            .get(&"moving".into())
            .is_some_and(|w| w.authority.0 == "b")
    });
    let moved = b.snapshot().unwrap();
    assert_eq!(
        moved.workflows[&"moving".into()].authority,
        HostId::from("b")
    );
    assert_eq!(moved.executions[&"origin".into()].host, HostId::from("a"));
    assert_eq!(moved.executions[&"origin".into()].generation, generation);
    b.command(Command::Interrupt {
        invocation: "origin".into(),
    })
    .unwrap();
    wait(|| {
        b.snapshot().unwrap().executions[&"origin".into()].status == ExecutionStatus::Interrupted
    });
}

#[test]
fn unknown_outcome_is_inspectable_after_restart_and_never_automatically_retried() {
    let mut f = Fixture::new();
    let workflow = f.graph(vec![node("approval", "human", "", f.dir.path())], vec![]);
    f.run(&workflow, "approval", "unknown");
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .executions
            .get(&"unknown".into())
            .is_some_and(|e| e.status == ExecutionStatus::Waiting)
    });
    let before = f.client.snapshot().unwrap().executions[&"unknown".into()].clone();
    f.restart();
    let state = f.client.snapshot().unwrap();
    let recovered = &state.executions[&"unknown".into()];
    assert_eq!(recovered.status, ExecutionStatus::Unavailable);
    assert_eq!(recovered.generation, before.generation);
    f.run(&workflow, "approval", "unknown");
    assert_eq!(f.client.snapshot().unwrap().executions.len(), 1);
    f.run(&workflow, "approval", "explicit-retry");
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .executions
            .get(&"explicit-retry".into())
            .is_some_and(|e| e.status == ExecutionStatus::Waiting)
    });
}
#[test]
fn failed_join_terminates_the_correlation_without_running_its_success_branch() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let workflow = f.graph(
        vec![
            node("root", "shell", "true", cwd),
            node("bad", "shell", "exit 1", cwd),
            node("late", "shell", "sleep 0.1", cwd),
            node("join", "join", "", cwd),
            node("success", "shell", "touch wrong", cwd),
            node("failure", "shell", "touch recovered", cwd),
        ],
        vec![
            wire("root", "out", "bad", "in"),
            wire("root", "out", "late", "in"),
            wire("bad", "failed", "join", "failed"),
            wire("late", "out", "join", "right"),
            wire("join", "out", "success", "in"),
            wire("join", "failed", "failure", "in"),
        ],
    );
    f.run(&workflow, "root", "failed-join");
    wait(|| cwd.join("recovered").exists());
    thread::sleep(Duration::from_millis(200));
    assert!(!cwd.join("wrong").exists());
    assert_eq!(
        f.client
            .snapshot()
            .unwrap()
            .executions
            .values()
            .filter(|e| e.node.0 == "join")
            .count(),
        1
    );
}
#[test]
fn receive_source_accepts_a_typed_delivery_once_across_reconnection() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let source = f.graph(vec![], vec![]);
    let mut receiver = node("receiver", "receive", "", cwd);
    receiver.flow.properties.extend([
        ("channel".into(), json!("updates")),
        (
            "signal_type".into(),
            json!(Type::Signal {
                args: Default::default(),
                open: true
            }),
        ),
        ("interval_secs".into(), json!(1)),
    ]);
    let target = f.graph(
        vec![
            receiver,
            node("record", "shell", "printf x >> deliveries", cwd),
        ],
        vec![wire("receiver", "out", "record", "in")],
    );
    wait(|| {
        f.client
            .snapshot()
            .unwrap()
            .sources
            .contains_key(&"receiver".into())
    });
    let delivery = Delivery {
        id: "one-delivery".into(),
        source: source.clone(),
        target: target.clone(),
        channel: "updates".into(),
        ty: Type::Signal {
            args: Default::default(),
            open: true,
        },
        args: json!({"value":7}),
        sequence: 0,
    };
    let send = || {
        f.client
            .command_as(
                Command::Broadcast {
                    delivery: delivery.clone(),
                },
                Some(source.clone()),
            )
            .unwrap()
    };
    assert_eq!(send()["accepted"], true);
    wait(|| cwd.join("deliveries").exists());
    assert_eq!(send()["accepted"], false);
    thread::sleep(Duration::from_millis(1100));
    assert_eq!(
        std::fs::read_to_string(cwd.join("deliveries")).unwrap(),
        "x"
    );
    assert_eq!(
        f.client
            .command_as(
                Command::Receive {
                    workflow: target.clone(),
                    channel: "updates".into()
                },
                Some(target)
            )
            .unwrap(),
        Value::Null
    );
}

struct Hosts {
    dir: tempfile::TempDir,
    children: Vec<Option<Child>>,
    clients: Vec<Client>,
}
impl Hosts {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clients: Vec<_> = ["a", "b"]
            .iter()
            .map(|h| Client::new(dir.path().join(format!("{h}.sock"))))
            .collect();
        for (i, host) in ["a", "b"].iter().enumerate() {
            let config = dir.path().join(format!("{host}-config"));
            std::fs::create_dir(&config).unwrap();
            std::fs::write(config.join("runtime.kdl"),format!("host-id {host:?}\nmax-concurrent 1\nremote-retry-secs 1\nhost {:?} {{ transport \"unix\"; socket {:?}; }}\n",["b","a"][i],clients[1-i].socket.to_string_lossy())).unwrap();
        }
        let mut f = Self {
            dir,
            children: vec![None, None],
            clients,
        };
        f.start(0);
        f.start(1);
        wait(|| {
            f.clients[0]
                .snapshot()
                .unwrap()
                .workspaces
                .definition(&"b:user".into())
                .is_ok()
        });
        f
    }
    fn start(&mut self, index: usize) {
        let host = ["a", "b"][index];
        self.children[index] = Some(
            Process::new(env!("CARGO_BIN_EXE_flow-core"))
                .arg("--socket")
                .arg(&self.clients[index].socket)
                .arg("--data-dir")
                .arg(self.dir.path().join(host))
                .env(
                    "FLOWWM_CONFIG_DIR",
                    self.dir.path().join(format!("{host}-config")),
                )
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait(|| self.clients[index].snapshot().is_ok());
    }
    fn stop(&mut self, index: usize) {
        if let Some(mut child) = self.children[index].take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }
    fn create(&self, id: &str, owner: Option<&str>, nodes: Vec<NodeSpec>) {
        self.clients[0]
            .command_as(
                Command::Create {
                    workflow: Workflow {
                        row_settings: Default::default(),
                        id: id.into(),
                        name: id.into(),
                        workspace: Some("a:user".into()),
                        visibility: if owner.is_some() {
                            Visibility::Private
                        } else {
                            Visibility::Public
                        },
                        owner: owner.map(Into::into),
                        references: Default::default(),
                        graph: FlowNet {
                            nodes,
                            ..Default::default()
                        },
                        authority: "a".into(),
                        transfer: None,
                    },
                },
                owner.map(Into::into),
            )
            .unwrap();
    }
}
impl Drop for Hosts {
    fn drop(&mut self) {
        self.stop(0);
        self.stop(1);
    }
}
#[test]
fn disconnected_handoff_recovers_and_owner_destruction_reconciles_private_cleanup() {
    let mut hosts = Hosts::new();
    hosts.create("owner", None, vec![]);
    hosts.create("private", Some("owner"), vec![]);
    hosts.stop(1);
    hosts.clients[0]
        .command_as(
            Command::PrepareTransfer {
                transfer: "recoverable".into(),
                workflow: "private".into(),
                destination: "b:user".into(),
            },
            Some("owner".into()),
        )
        .unwrap();
    wait(|| {
        hosts.clients[0]
            .snapshot()
            .unwrap()
            .peers
            .get(&"b".into())
            .is_some_and(|p| p.status == PeerStatus::Disconnected)
    });
    hosts.stop(0);
    hosts.start(0);
    hosts.start(1);
    wait(|| {
        hosts.clients[1]
            .snapshot_as(Some("owner".into()))
            .unwrap()
            .workflows
            .get(&"private".into())
            .is_some_and(|w| w.authority.0 == "b")
    });
    assert!(
        !hosts.clients[1]
            .snapshot()
            .unwrap()
            .workflows
            .contains_key(&"private".into())
    );
    hosts.stop(1);
    hosts.clients[0]
        .command(Command::Destroy {
            workflow: "owner".into(),
        })
        .unwrap();
    let saved: Value =
        serde_json::from_slice(&std::fs::read(hosts.dir.path().join("a/runtime.json")).unwrap())
            .unwrap();
    assert!(
        !saved["runtime"]["pending_remote"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    hosts.start(1);
    wait(|| {
        !hosts.clients[1]
            .snapshot_as(Some("owner".into()))
            .unwrap()
            .workflows
            .contains_key(&"private".into())
    });
}
#[test]
fn queued_invocation_uses_destination_while_late_origin_result_continues_the_graph() {
    let hosts = Hosts::new();
    let cwd = hosts.dir.path();
    let a = &hosts.clients[0];
    let b = &hosts.clients[1];
    hosts.create(
        "pipeline",
        None,
        vec![
            node("origin-node", "shell", "sleep 1; printf origin", cwd),
            node("queued-node", "shell", "printf destination", cwd),
            node("after-node", "shell", "printf after", cwd),
        ],
    );
    let mut graph = a.snapshot().unwrap().workflows[&"pipeline".into()]
        .graph
        .clone();
    graph
        .connections
        .push(wire("origin-node", "out", "after-node", "in"));
    a.command(Command::ReplaceGraph {
        workflow: "pipeline".into(),
        graph,
    })
    .unwrap();
    for (node, id) in [
        ("origin-node", "origin-late"),
        ("queued-node", "queued-future"),
    ] {
        a.command(Command::Run {
            workflow: "pipeline".into(),
            node: node.into(),
            invocation: id.into(),
            args: json!({"payload":17}),
        })
        .unwrap();
    }
    wait(|| {
        a.snapshot()
            .unwrap()
            .executions
            .get(&"queued-future".into())
            .is_some_and(|e| e.status == ExecutionStatus::Queued)
    });
    a.command(Command::Move {
        workflow: "pipeline".into(),
        workspace: "b:user".into(),
    })
    .unwrap();
    wait(|| {
        b.snapshot()
            .unwrap()
            .executions
            .values()
            .any(|e| e.node.0 == "after-node" && e.status == ExecutionStatus::Complete)
    });
    let result = b.snapshot().unwrap();
    assert_eq!(result.executions[&"origin-late".into()].host.0, "a");
    assert_eq!(result.executions[&"queued-future".into()].host.0, "b");
    assert_eq!(
        result
            .executions
            .values()
            .find(|e| e.node.0 == "after-node")
            .unwrap()
            .host
            .0,
        "b"
    );
    assert_eq!(
        result
            .executions
            .values()
            .filter(|e| e.node.0 == "after-node")
            .count(),
        1
    );
}

#[test]
fn scripts_and_alerts_run_headlessly_and_publish_semantic_attention() {
    let f = Fixture::new();
    let cwd = f.dir.path();
    let mut script = node(
        "script",
        "python",
        "outputs['value'] = 'Threshold reached'",
        cwd,
    );
    script.flow.outputs.push(Pin::new("value", Type::String));
    let workflow = f.graph(
        vec![
            script,
            node("alert", "alert", "", cwd),
            node("after-alert", "shell", "printf x > alerted", cwd),
        ],
        vec![
            wire("script", "value", "alert", "message"),
            wire("script", "out", "alert", "in"),
            wire("alert", "out", "after-alert", "in"),
        ],
    );
    f.run(&workflow, "script", "script-alert");
    wait(|| cwd.join("alerted").exists());
    let state = f.client.snapshot().unwrap();
    let alert = state
        .executions
        .values()
        .find(|e| e.node.0 == "alert")
        .unwrap();
    assert_eq!(alert.status, ExecutionStatus::Complete);
    assert_eq!(alert.result["attention"]["message"], "Threshold reached");
    assert_eq!(
        state
            .queue
            .published
            .get(&"script".into())
            .and_then(|v| v.get("value")),
        None,
        "Lazy values are evaluation results, never a persistent cache"
    );
}
#[test]
fn destroying_a_source_waits_for_its_owned_polling_process_to_exit() {
    let f = Fixture::configured(json!({"interrupt_grace_ms":50,"terminate_grace_ms":50}));
    let cwd = f.dir.path();
    let workflow=f.graph(vec![node("poll","metric","trap 'printf stopped > stopped; exit' INT TERM; printf started > started; while :; do sleep 0.05; done",cwd)],vec![]);
    wait(|| cwd.join("started").exists());
    f.client.command(Command::Destroy { workflow }).unwrap();
    wait(|| cwd.join("stopped").exists());
    wait(|| {
        !f.client
            .snapshot()
            .unwrap()
            .sources
            .contains_key(&"poll".into())
    });
}

#[test]
fn row_settings_are_snapshotted_per_run_and_env_inputs_can_replace_them() {
    let fixture = Fixture::new();
    let cwd = fixture.dir.path();
    let workflow = fixture.graph(
        vec![node(
            "print",
            "shell",
            "sleep 0.1; printf '%s:%s' \"$FLOWWM_ROWSET_PROJECT\" \"${FLOWWM_ARG_NUMBER-unset}\"",
            cwd,
        )],
        vec![],
    );
    let set = |value: &str| {
        fixture
            .client
            .command(Command::SetRowSettings {
                workflow: workflow.clone(),
                settings: [("project".into(), value.into())].into(),
            })
            .unwrap()
    };
    set("before");
    fixture.run(&workflow, "print", "before");
    wait(|| {
        fixture
            .client
            .snapshot()
            .unwrap()
            .executions
            .get(&"before".into())
            .is_some_and(|e| e.status == ExecutionStatus::Running)
    });
    set("after");
    assert_eq!(
        fixture.until_terminal("before").result["stdout"],
        "before:unset"
    );
    fixture.run(&workflow, "print", "after");
    assert_eq!(
        fixture.until_terminal("after").result["stdout"],
        "after:unset"
    );
    let mut graph = fixture.client.snapshot().unwrap().workflows[&workflow]
        .graph
        .clone();
    graph.nodes.push(node("empty", "empty_env", "", cwd));
    graph.connections.push(wire("empty", "env", "print", "env"));
    fixture
        .client
        .command(Command::ReplaceGraph {
            workflow: workflow.clone(),
            graph,
        })
        .unwrap();
    fixture.run(&workflow, "print", "empty");
    assert_eq!(fixture.until_terminal("empty").result["stdout"], ":unset");
    let settings = fixture
        .client
        .command(Command::RowSettings {
            workflow: workflow.clone(),
        })
        .unwrap();
    assert_eq!(settings["settings"]["project"], "after");
    assert!(
        settings["effective"]["pwd"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        fixture
            .client
            .command(Command::SetRowSettings {
                workflow,
                settings: [("x".into(), "a".into()), ("X".into(), "b".into())].into()
            })
            .is_err()
    );
}
