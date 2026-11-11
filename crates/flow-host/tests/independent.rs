use flow_core::{
    domain::*,
    protocol::{Command, Request},
};
use flow_host::{
    engine::{Control, Engine},
    ipc,
    model::*,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn call(engine: &Engine, command: Command, caller: Option<&str>) -> Value {
    let reply = engine
        .request(Control::Orchestrate {
            request: Box::new(Request {
                protocol: 2,
                id: id(),
                expected_revision: None,
                caller: caller.map(Into::into),
                command,
            }),
        })
        .unwrap();
    assert!(reply["error"].is_null(), "{reply}");
    reply["result"].clone()
}
fn until(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(start.elapsed() < Duration::from_secs(10), "Timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
fn fixture() -> (Engine, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = State::empty(dir.path().into());
    let (engine, _) = Engine::new(state, Store::new(dir.path().into()).unwrap());
    (engine, dir)
}
#[test]
fn client_detach_does_not_cancel_real_process_and_new_client_observes_completion() {
    let (engine, dir) = fixture();
    let socket = dir.path().join("host.sock");
    let listener = ipc::bind(&socket).unwrap();
    ipc::serve(listener, engine.clone(), vec![]);
    let state = engine.snapshot().state;
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        dir.path().into(),
        Default::default(),
    );
    node.command = "sleep 0.15; printf 'survived'".into();
    let node_id = node.id.clone();
    engine
        .request(Control::Create {
            node: Box::new(node),
        })
        .unwrap();
    let (client, _) = Engine::connect(socket.clone()).unwrap();
    client
        .request(Control::Run {
            id: node_id.clone(),
            downstream: false,
        })
        .unwrap();
    client.shutdown();
    until(|| engine.snapshot().state.node(&node_id).unwrap().status == Status::Done);
    let (reconnected, _) = Engine::connect(socket).unwrap();
    assert!(
        reconnected
            .snapshot()
            .state
            .node(&node_id)
            .unwrap()
            .output
            .contains("survived")
    );
    reconnected.shutdown();
    engine.shutdown();
}
#[test]
fn real_registered_execution_moves_without_restart_and_interrupt_reaches_original_group() {
    let (engine, dir) = fixture();
    let graph = flow_core::domain::FlowNet {
        nodes: vec![flow_core::domain::NodeSpec {
            id: "process".into(),
            type_id: "local.shell".into(),
            flow: flow_core::graph::Flow {
                properties: std::collections::BTreeMap::from([
                    (
                        "command".into(),
                        json!(
                            "trap 'printf interrupted; exit 0' INT; while :; do sleep 0.05; done"
                        ),
                    ),
                    ("cwd".into(), json!(dir.path())),
                ]),
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    let work = Workflow {
        id: "moving".into(),
        name: "Moving workflow".into(),
        workspace: Some("local:user".into()),
        visibility: Visibility::Public,
        owner: None,
        references: Default::default(),
        graph,
        authority: "local".into(),
        transfer: None,
    };
    call(&engine, Command::Create { workflow: work }, None);
    let first = call(
        &engine,
        Command::Run {
            workflow: "moving".into(),
            node: "process".into(),
            invocation: "invocation".into(),
            args: json!({}),
        },
        None,
    );
    let duplicate = call(
        &engine,
        Command::Run {
            workflow: "moving".into(),
            node: "process".into(),
            invocation: "invocation".into(),
            args: json!({}),
        },
        None,
    );
    assert_eq!(first["handle"], duplicate["handle"]);
    // A registered desktop provider announces its session through desktop synchronization.
    engine
        .request(Control::SyncDesktop {
            desktop: flow_host::niri::Desktop {
                session: "session".into(),
                connected: true,
                ..Default::default()
            },
        })
        .unwrap();
    call(
        &engine,
        Command::Move {
            workflow: "moving".into(),
            workspace: "local:desktop".into(),
        },
        None,
    );
    call(
        &engine,
        Command::Move {
            workflow: "moving".into(),
            workspace: "local:user".into(),
        },
        None,
    );
    engine
        .request(Control::EndSession {
            workspace: "local:desktop".into(),
            incarnation: "session".into(),
        })
        .unwrap();
    let snapshot = call(&engine, Command::Snapshot, None);
    assert_eq!(
        snapshot["executions"]["invocation"]["handle"],
        first["handle"]
    );
    assert_eq!(snapshot["executions"]["invocation"]["status"], "running");
    thread::sleep(Duration::from_millis(80));
    call(
        &engine,
        Command::Interrupt {
            invocation: "invocation".into(),
        },
        None,
    );
    until(|| {
        call(&engine, Command::Snapshot, None)["executions"]["invocation"]["status"]
            == "interrupted"
    });
    assert!(
        call(&engine, Command::Snapshot, None)["executions"]["invocation"]["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("interrupted")
    );
    engine.shutdown();
}
#[test]
fn migration_and_future_version_failures_keep_the_original_files() {
    let dir = tempfile::tempdir().unwrap();
    let original = serde_json::to_vec(&State::empty(PathBuf::from("/portable"))).unwrap();
    std::fs::write(dir.path().join("state.json"), &original).unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let migrated = store.load(".".into()).unwrap();
    assert!(migrated.orchestration.is_some());
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        original
    );
    let current = dir.path().join("runtime-v2.json");
    let mut envelope: Value = serde_json::from_slice(&std::fs::read(&current).unwrap()).unwrap();
    envelope["version"] = json!(999);
    let unknown = serde_json::to_vec(&envelope).unwrap();
    std::fs::write(&current, &unknown).unwrap();
    assert!(store.load(".".into()).is_err());
    assert_eq!(std::fs::read(&current).unwrap(), unknown);
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        original
    );
}

fn named_host(name: &str) -> (Engine, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = State::empty(dir.path().into());
    let mut runtime = RuntimeState::local();
    runtime.bind_host(name.into()).unwrap();
    state.orchestration = Some(runtime);
    let (engine, _) = Engine::new(state, Store::new(dir.path().into()).unwrap());
    let socket = dir.path().join("host.sock");
    ipc::serve(ipc::bind(&socket).unwrap(), engine.clone(), vec![]);
    (engine, dir, socket)
}
#[test]
fn cross_host_handoff_preserves_real_execution_and_forwards_interrupt_and_result() {
    let (a, ad, asock) = named_host("a");
    let ready = ad.path().join("ready");
    let (b, _bd, bsock) = named_host("b");
    a.request(Control::ConnectHost {
        host: "b".into(),
        remote: flow_host::remote::Remote::Unix { socket: bsock },
    })
    .unwrap();
    b.request(Control::ConnectHost {
        host: "a".into(),
        remote: flow_host::remote::Remote::Unix {
            socket: asock.clone(),
        },
    })
    .unwrap();
    let graph = FlowNet {
        nodes: vec![NodeSpec {
            id: "worker".into(),
            type_id: "local.shell".into(),
            flow: flow_core::graph::Flow {
                properties: std::collections::BTreeMap::from([(
                    "command".into(),
                    json!(format!(
                        "trap 'echo origin-interrupted; exit 0' INT; printf ready > '{}'; while :; do sleep 0.05; done",
                        ready.display()
                    )),
                )]),
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    call(
        &a,
        Command::Create {
            workflow: Workflow {
                id: "remote-work".into(),
                name: "Remote work".into(),
                workspace: Some("a:user".into()),
                visibility: Visibility::Public,
                owner: None,
                references: Default::default(),
                graph,
                authority: "a".into(),
                transfer: None,
            },
        },
        None,
    );
    b.request(Control::ConnectHost {
        host: "a".into(),
        remote: flow_host::remote::Remote::Unix { socket: asock },
    })
    .unwrap();
    assert_eq!(
        call(&b, Command::Snapshot, None)["workflows"]["remote-work"]["authority"],
        "a"
    );
    let first = call(
        &a,
        Command::Run {
            workflow: "remote-work".into(),
            node: "worker".into(),
            invocation: "stable-run".into(),
            args: json!({}),
        },
        None,
    );
    until(|| ready.is_file());
    call(
        &a,
        Command::Move {
            workflow: "remote-work".into(),
            workspace: "b:user".into(),
        },
        None,
    );
    let moved = call(&b, Command::Snapshot, None);
    assert_eq!(moved["workflows"]["remote-work"]["authority"], "b");
    assert_eq!(moved["executions"]["stable-run"]["host"], "a");
    assert_eq!(moved["executions"]["stable-run"]["handle"], first["handle"]);
    call(
        &b,
        Command::Interrupt {
            invocation: "stable-run".into(),
        },
        None,
    );
    until(|| {
        call(&b, Command::Snapshot, None)["executions"]["stable-run"]["status"] == "interrupted"
    });
    assert!(
        call(&b, Command::Snapshot, None)["executions"]["stable-run"]["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("origin-interrupted")
    );
    a.shutdown();
    b.shutdown();
}

#[test]
fn synchronous_client_commands_see_fresh_state_and_late_frames_cannot_roll_it_back() {
    use std::{
        io::{BufRead, BufReader, Write},
        sync::{Arc, Mutex, mpsc},
    };
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("snapshot.sock");
    let listener = ipc::bind(&socket).unwrap();
    let base = State::empty(dir.path().into());
    let current = Arc::new(Mutex::new(base.clone()));
    let (release, gate) = mpsc::channel();
    let gate = Arc::new(Mutex::new(Some(gate)));
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let state = current.clone();
            let old = base.clone();
            let gate = gate.clone();
            thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                match request["op"].as_str().unwrap(){
                    "status"=>writeln!(stream,"{}",json!({"ok":&*state.lock().unwrap()})).unwrap(),
                    "notes"=>{state.lock().unwrap().workspaces[0].notes="fresh".into();writeln!(stream,"{}",json!({"ok":null})).unwrap();},
                    "snapshot"=>writeln!(stream,"{}",json!({"ok":flow_host::engine::Snapshot{state:Arc::new(state.lock().unwrap().clone()),revision:2,error:None}})).unwrap(),
                    "attach"=>{gate.lock().unwrap().take().unwrap().recv().unwrap();writeln!(stream,"{}",serde_json::to_string(&ipc::ClientFrame{snapshot:Some(flow_host::engine::Snapshot{state:Arc::new(old),revision:1,error:None}),commands:vec![flow_host::engine::UiCommand::Help]}).unwrap()).unwrap();},
                    _=>panic!("Unexpected request")
                }
            });
        }
    });
    let (client, commands) = Engine::connect(socket).unwrap();
    client
        .request(Control::Notes {
            text: "fresh".into(),
        })
        .unwrap();
    assert_eq!(client.snapshot().state.workspace().notes, "fresh");
    release.send(()).unwrap();
    assert!(matches!(
        commands.recv_timeout(Duration::from_secs(2)).unwrap(),
        flow_host::engine::UiCommand::Help
    ));
    assert_eq!(client.snapshot().revision, 2);
    assert_eq!(client.snapshot().state.workspace().notes, "fresh");
    client.shutdown();
    server.join().unwrap();
}

#[test]
fn signal_protocol_negotiates_separately_and_retries_do_not_duplicate_delivery() {
    let (engine, dir) = fixture();
    let socket = dir.path().join("signals.sock");
    ipc::serve(ipc::bind(&socket).unwrap(), engine.clone(), vec![]);
    for name in ["sender", "receiver"] {
        call(
            &engine,
            Command::Create {
                workflow: Workflow {
                    id: name.into(),
                    name: name.into(),
                    workspace: Some("local:user".into()),
                    visibility: Visibility::Public,
                    owner: None,
                    references: Default::default(),
                    graph: Default::default(),
                    authority: "local".into(),
                    transfer: None,
                },
            },
            None,
        );
    }
    let frame = flow_core::protocol::SignalFrame {
        signal_protocol: 1,
        id: "message".into(),
        delivery: Delivery {
            id: "message".into(),
            source: "sender".into(),
            target: "receiver".into(),
            channel: "done".into(),
            ty: flow_core::graph::Type::trigger(),
            args: json!({}),
            sequence: 0,
        },
    };
    assert_eq!(ipc::send(&socket, &frame).unwrap()["accepted"], true);
    assert_eq!(ipc::send(&socket, &frame).unwrap()["accepted"], false);
    assert_eq!(
        call(
            &engine,
            Command::Receive {
                workflow: "receiver".into(),
                channel: "done".into()
            },
            Some("receiver")
        )["id"],
        "message"
    );
    engine.shutdown();
}

#[test]
fn interruption_retains_the_interface_and_does_not_signal_an_observed_application() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = State::empty(dir.path().into());
    let mut work = Node::new(
        &state.selected_workspace,
        NodeKind::Human,
        dir.path().into(),
        Default::default(),
    );
    work.status = Status::Running;
    work.window_id = Some(123);
    let work_id = work.id.clone();
    let mut app = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        dir.path().into(),
        Default::default(),
    );
    app.status = Status::Running;
    app.window_id = Some(456);
    let app_id = app.id.clone();
    state.nodes.extend([work, app]);
    let (engine, _) = Engine::new(state, Store::new(dir.path().into()).unwrap());
    engine
        .request(Control::Interrupt {
            id: work_id.clone(),
        })
        .unwrap();
    assert_eq!(
        engine.snapshot().state.node(&work_id).unwrap().window_id,
        Some(123)
    );
    assert_eq!(
        engine.snapshot().state.node(&work_id).unwrap().status,
        Status::Stopped
    );
    assert!(
        engine
            .request(Control::Interrupt { id: app_id.clone() })
            .is_err()
    );
    assert_eq!(
        engine.snapshot().state.node(&app_id).unwrap().window_id,
        Some(456)
    );
    engine.shutdown();
}
