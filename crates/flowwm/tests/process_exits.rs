//! The production desktop-provider process owns compositor lifetime and exit events.
mod support;
use flow_core::{domain::*, graph::*, protocol::Command};
use flowwm::{
    client::Action,
    model::{Node, NodeKind, RowView, State},
};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::{Command as Process, Stdio},
    sync::{Arc, Mutex},
};
struct Compositor {
    streams: Arc<Mutex<Vec<UnixStream>>>,
    close_requests: Arc<Mutex<Vec<u64>>>,
    socket: std::path::PathBuf,
    _directory: tempfile::TempDir,
}
impl Compositor {
    fn new(pid: i32) -> Self {
        Self::with_close_error(pid, None)
    }
    fn with_close_error(pid: i32, close_error: Option<&'static str>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("niri.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let streams = Arc::new(Mutex::new(Vec::<UnixStream>::new()));
        let peers = streams.clone();
        let close_requests = Arc::new(Mutex::new(Vec::new()));
        let closes = close_requests.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: niri_ipc::Request = serde_json::from_str(&line).unwrap();
                if let niri_ipc::Request::Action(niri_ipc::Action::CloseWindow { id: Some(id) }) =
                    request
                {
                    closes.lock().unwrap().push(id);
                    if let Some(error) = close_error {
                        writeln!(stream, "{}", json!({"Err":error})).unwrap();
                    } else {
                        // Deliver the event before acknowledging the request to
                        // exercise a client that closes immediately.
                        for peer in peers.lock().unwrap().iter_mut() {
                            writeln!(peer, "{}", json!({"WindowClosed":{"id":id}})).unwrap();
                        }
                        writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap();
                    }
                    continue;
                }
                if !matches!(request, niri_ipc::Request::EventStream) {
                    continue;
                }
                writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap();
                let workspace = json!({"id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":1});
                let window = window(pid, 1, "Shared app");
                writeln!(
                    stream,
                    "{}",
                    json!({"WorkspacesChanged":{"workspaces":[workspace]}})
                )
                .unwrap();
                writeln!(stream, "{}", json!({"WindowsChanged":{"windows":[window]}})).unwrap();
                peers.lock().unwrap().push(stream);
            }
        });
        Self {
            streams,
            close_requests,
            socket,
            _directory: directory,
        }
    }
    fn publish(&self, event: serde_json::Value) {
        for stream in self.streams.lock().unwrap().iter_mut() {
            writeln!(stream, "{event}").unwrap();
        }
    }
    fn disconnect(&self) {
        for stream in self.streams.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn window(pid: i32, id: u64, title: &str) -> serde_json::Value {
    json!({"id":id,"workspace_id":1,"title":title,"app_id":"test.shared","pid":pid,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[800,600],"window_size":[800,600],"tile_pos_in_workspace_view":[0,0],"window_offset_in_tile":[0,0]},"focus_timestamp":null})
}

fn application_node() -> (State, String) {
    let mut state = State::empty("/tmp".into());
    state.workspaces[0].view = RowView::Nodes;
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Default::default(),
    );
    node.flow
        .properties
        .insert("application".into(), json!("test.shared"));
    let id = node.id.clone();
    state.nodes.push(node);
    (state, id)
}

#[test]
fn restored_window_selectors_prune_closed_imports_and_reconnect_live_windows() {
    let compositor = Compositor::new(std::process::id() as i32);
    let mut state = State::empty("/tmp".into());
    let workflow = state.selected_workspace.clone();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let node = Node::new(
            &workflow,
            NodeKind::Window,
            "/tmp".into(),
            Default::default(),
        );
        ids.push(node.id.clone());
        state.nodes.push(node);
    }
    let [closed, live, unlaunched] = <[String; 3]>::try_from(ids).unwrap();
    state.connections.push(Connection {
        source: Endpoint {
            node: unlaunched.clone(),
            pin: "on_exit".into(),
        },
        target: Endpoint {
            node: closed.clone(),
            pin: "in".into(),
        },
    });
    state.connections.push(Connection {
        source: Endpoint {
            node: live.clone(),
            pin: "on_exit".into(),
        },
        target: Endpoint {
            node: unlaunched.clone(),
            pin: "in".into(),
        },
    });
    let (desktop, _) = support::fixture_with_provider_setup(
        state,
        Some(&compositor.socket),
        |_| {},
        |_, data| {
            let path = data.join("desktop.json");
            let mut bindings: flowwm::desktop_provider::Bindings =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            // Only declarative selectors survive after the live bindings have
            // been cleared, including across compositor sessions.
            bindings.session = "previous-session".into();
            bindings
                .selectors
                .insert(closed.clone(), "test.shared::Closed window".into());
            bindings
                .selectors
                .insert(live.clone(), "test.shared::Shared app".into());
            std::fs::write(path, serde_json::to_vec(&bindings).unwrap()).unwrap();
        },
    );
    support::wait(|| desktop.bindings().applications.contains_key(&live));
    support::wait(|| desktop.snapshot().state.node(&closed).is_none());
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    let graph = &runtime["workflows"][&workflow]["graph"];
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["id"] != closed)
    );
    assert_eq!(graph["connections"].as_array().unwrap().len(), 1);
    assert_eq!(graph["connections"][0]["source"]["node"], live);
    assert_eq!(graph["connections"][0]["target"]["node"], unlaunched);
    assert!(!desktop.bindings().selectors.contains_key(&closed));
    let (desktop, _) = desktop.reconnect();
    for view in [RowView::Windows, RowView::Nodes] {
        desktop
            .request(Action::RowView {
                workspace: workflow.clone(),
                view,
            })
            .unwrap();
        let state = desktop.snapshot().state;
        assert!(state.node(&closed).is_none());
        assert_eq!(state.node(&live).unwrap().window_id, Some(1));
        assert!(state.node(&unlaunched).is_some());
    }
    desktop.shutdown();
}

#[test]
fn manually_closing_imported_windows_removes_them_from_the_runtime_and_both_views() {
    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let state = State::empty("/tmp".into());
    let workflow = state.selected_workspace.clone();
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.len() == 1);
    for id in 2..=6 {
        compositor
            .publish(json!({"WindowOpenedOrChanged":{"window":window(pid, id, "Shared app")}}));
    }
    support::wait(|| desktop.bindings().applications.len() == 6);
    let ids: Vec<_> = desktop.bindings().applications.keys().cloned().collect();
    for id in 1..=6 {
        compositor.publish(json!({"WindowClosed":{"id":id}}));
    }
    support::wait(|| desktop.bindings().applications.is_empty());
    support::wait(|| {
        ids.iter()
            .all(|id| desktop.snapshot().state.node(id).is_none())
    });
    assert!(compositor.close_requests.lock().unwrap().is_empty());
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    assert!(
        runtime["workflows"][&workflow]["graph"]["nodes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(runtime["applications"].as_object().unwrap().is_empty());
    assert!(desktop.bindings().selectors.is_empty());
    let (desktop, _) = desktop.reconnect();
    for view in [RowView::Windows, RowView::Nodes] {
        desktop
            .request(Action::RowView {
                workspace: workflow.clone(),
                view,
            })
            .unwrap();
        assert!(desktop.snapshot().state.nodes.is_empty());
    }
    desktop.shutdown();
}

#[test]
fn reopening_an_application_replaces_its_registration_and_rejects_old_exit_events() {
    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let (state, id) = application_node();
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.contains_key(&id));
    let original = desktop.bindings().applications[&id].clone();

    compositor.publish(json!({"WindowClosed":{"id":1}}));
    support::wait(|| !desktop.bindings().applications.contains_key(&id));
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 2, "Reopened")}}));
    support::wait(|| {
        desktop
            .bindings()
            .applications
            .get(&id)
            .is_some_and(|b| b.window == 2)
    });
    let replacement = desktop.bindings().applications[&id].clone();
    assert_ne!(replacement.token, original.token);
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    assert_eq!(runtime["applications"][&id]["token"], replacement.token);
    let error = desktop
        .orchestrate(
            Command::ApplicationEvent {
                node: id.clone().into(),
                token: original.token,
                incarnation: compositor.socket.to_string_lossy().into_owned(),
                pin: "on_exit".into(),
                delivery: "obsolete-exit".into(),
                args: json!({"exit_code":0}),
            },
            Some(replacement.workflow.into()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Stale application event"));

    // PID metadata can change without a window ID changing, too.
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(0, 2, "Updated PID")}}));
    support::wait(|| {
        desktop
            .bindings()
            .applications
            .get(&id)
            .is_some_and(|b| b.pid == Some(0) && b.token != replacement.token)
    });
    desktop.shutdown();
}

fn recover_registration_before_desktop_save(keep_binding: bool) {
    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let (mut state, id) = application_node();
    let workflow = state.selected_workspace.clone();
    state.node_mut(&id).unwrap().window_id = Some(1);
    let (desktop, _) = support::fixture_with_provider_setup(
        state,
        Some(&compositor.socket),
        |_| {},
        |host, data| {
            let document = data.join("desktop.json");
            let mut bindings: flowwm::desktop_provider::Bindings =
                serde_json::from_slice(&std::fs::read(&document).unwrap()).unwrap();
            if keep_binding {
                bindings.applications.get_mut(&id).unwrap().pid = Some(pid);
            } else {
                bindings.applications.clear();
            }
            std::fs::write(document, serde_json::to_vec(&bindings).unwrap()).unwrap();
            host.command(Command::ActivateSession {
                workspace: "local:desktop".into(),
                provider: "desktop".into(),
                incarnation: bindings.session.clone(),
            })
            .unwrap();
            host.command(Command::RegisterApplication {
                registration: flow_core::providers::ApplicationRegistration {
                    workflow: workflow.into(),
                    node: id.clone().into(),
                    workspace: "local:desktop".into(),
                    incarnation: bindings.session,
                    token: "committed-before-desktop-save".into(),
                    operations: flow_core::providers::ApplicationOperations {
                        open: true,
                        close_window: true,
                        interrupt: false,
                        disown: true,
                    },
                },
            })
            .unwrap();
        },
    );
    support::wait(|| {
        let bindings = desktop.bindings();
        let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
        bindings.applications.get(&id).is_some_and(|b| {
            b.window == 1 && b.pid == Some(pid) && runtime["applications"][&id]["token"] == b.token
        })
    });
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 1, "Recovered")}}));
    support::wait(|| desktop.bindings().applications[&id].title == "Recovered");
    desktop.shutdown();
}

#[test]
fn provider_recovers_when_registration_was_saved_before_its_binding() {
    recover_registration_before_desktop_save(false);
}

#[test]
fn provider_recovers_when_the_saved_binding_has_an_older_token() {
    recover_registration_before_desktop_save(true);
}

#[test]
fn animated_titles_keep_ui_commands_live_when_metadata_storage_fails() {
    use flowwm::client::UiCommand;
    use std::time::Duration;

    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let (state, id) = application_node();
    let (desktop, events) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.contains_key(&id));
    desktop
        .request(Action::SyncDesktop {
            desktop: Default::default(),
        })
        .unwrap();
    let data = desktop.data_directory().unwrap();
    let token = desktop.bindings().applications[&id].token.clone();

    // Fail any attempted write, without stopping the production provider or its
    // subscription. Title updates must remain live; UI commands need no save.
    for document in ["desktop", "presentation"] {
        std::fs::create_dir(data.join(format!("{document}.json.tmp"))).unwrap();
    }
    for i in 0..100 {
        compositor.publish(
            json!({"WindowOpenedOrChanged":{"window":window(pid, 1, &format!("spinner-{i}"))}}),
        );
    }
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .node(&id)
            .is_some_and(|n| n.title == "spinner-99")
    });
    for _ in 0..100 {
        desktop
            .request(Action::SyncDesktop {
                desktop: Default::default(),
            })
            .unwrap();
    }
    desktop
        .request(Action::Ui {
            command: UiCommand::Launcher,
        })
        .unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        UiCommand::Launcher
    ));
    assert_eq!(desktop.bindings().applications[&id].token, token);

    for document in ["desktop", "presentation"] {
        std::fs::remove_dir(data.join(format!("{document}.json.tmp"))).unwrap();
    }
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 1, "Latest title")}}));
    support::wait(|| {
        std::fs::read(data.join("desktop.json"))
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<flowwm::desktop_provider::Bindings>(&bytes).ok()
            })
            .is_some_and(|bindings| {
                bindings
                    .applications
                    .get(&id)
                    .is_some_and(|b| b.title == "Latest title" && b.token == token)
            })
    });
}

#[test]
fn deleting_an_application_in_nodes_closes_only_its_window_and_removes_both_views() {
    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let (state, id) = application_node();
    let workflow = state.selected_workspace.clone();
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| {
        desktop
            .bindings()
            .applications
            .get(&id)
            .is_some_and(|b| b.window == 1)
            && desktop
                .snapshot()
                .state
                .node(&id)
                .is_some_and(|n| n.window_id == Some(1))
    });
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 2, "Other window")}}));
    support::wait(|| desktop.bindings().applications.len() == 2);

    desktop.request(Action::Delete { id: id.clone() }).unwrap();
    assert_eq!(*compositor.close_requests.lock().unwrap(), vec![1]);
    support::wait(|| !desktop.bindings().applications.contains_key(&id));

    // A queued update for the closing window must not recreate its node.
    compositor
        .publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 1, "Closing window")}}));
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(pid, 2, "Still alive")}}));
    support::wait(|| {
        desktop
            .bindings()
            .applications
            .values()
            .any(|b| b.title == "Still alive")
    });
    for view in [RowView::Windows, RowView::Nodes] {
        desktop
            .request(Action::RowView {
                workspace: workflow.clone(),
                view,
            })
            .unwrap();
        let snapshot = desktop.snapshot();
        assert!(snapshot.state.node(&id).is_none());
        assert!(snapshot.state.nodes.iter().all(|n| n.window_id != Some(1)));
        assert!(snapshot.state.nodes.iter().any(|n| n.window_id == Some(2)));
    }
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    assert!(
        runtime["workflows"][&workflow]["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["id"] != id)
    );
    assert!(runtime["applications"].get(&id).is_none());
    assert_eq!(desktop.bindings().applications.len(), 1);
    desktop.shutdown();
}

#[test]
fn a_failed_window_close_keeps_the_application_node_and_binding() {
    let compositor = Compositor::with_close_error(std::process::id() as i32, Some("Close failed"));
    let (state, id) = application_node();
    let workflow = state.selected_workspace.clone();
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.contains_key(&id));
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .node(&id)
            .is_some_and(|n| n.window_id == Some(1))
    });

    let error = desktop
        .request(Action::Delete { id: id.clone() })
        .unwrap_err();
    assert!(error.to_string().contains("Close failed"));
    assert_eq!(*compositor.close_requests.lock().unwrap(), vec![1]);
    assert_eq!(
        desktop.snapshot().state.node(&id).unwrap().window_id,
        Some(1)
    );
    assert!(desktop.bindings().applications.contains_key(&id));
    assert!(!desktop.bindings().disowned.contains(&1));
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    assert!(
        runtime["workflows"][&workflow]["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == id)
    );
    assert!(runtime["applications"].get(&id).is_some());
    desktop.shutdown();
}

#[test]
fn deleting_an_unlaunched_application_does_not_need_a_live_window() {
    let (state, id) = application_node();
    let (desktop, _) = support::fixture(state);
    desktop.request(Action::Delete { id: id.clone() }).unwrap();
    assert!(desktop.snapshot().state.node(&id).is_none());
    desktop.shutdown();
}

#[test]
fn provider_observes_application_exit_after_the_gtk_client_closes() {
    let mut application = Process::new("python3")
        .args(["-c", "import sys,os;sys.stdin.buffer.read(1);os._exit(37)"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let compositor = Compositor::new(application.id() as i32);
    let directory = tempfile::tempdir().unwrap();
    let (desktop, _) = support::fixture_with_niri(
        State::empty(directory.path().into()),
        Some(&compositor.socket),
    );
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .orchestration
            .as_ref()
            .is_some_and(|r| !r.applications.is_empty())
    });
    let runtime = desktop.snapshot().state.orchestration.clone().unwrap();
    let app = runtime.applications.values().next().unwrap().clone();
    let mut graph = runtime.workflows[&app.workflow].graph.clone();
    graph.nodes.push(NodeSpec {
        id: "sink".into(),
        type_id: "local.shell".into(),
        flow: Flow {
            properties: std::collections::BTreeMap::from([
                (
                    "command".into(),
                    json!("printf \"$FLOWWM_ARG_EXIT_CODE\" > exit-code"),
                ),
                ("cwd".into(), json!(directory.path())),
            ]),
            ..Default::default()
        },
    });
    graph.nodes.push(NodeSpec {
        id: "bake".into(),
        type_id: "local.bake_args_into_env".into(),
        flow: Flow {
            properties: [(
                "mappings".into(),
                json!({"exit_code":"FLOWWM_ARG_EXIT_CODE"}),
            )]
            .into(),
            ..Default::default()
        },
    });
    graph.connections.extend([
        Connection {
            source: Endpoint {
                node: "bake".into(),
                pin: "out".into(),
            },
            target: Endpoint {
                node: "sink".into(),
                pin: "in".into(),
            },
        },
        Connection {
            source: Endpoint {
                node: "bake".into(),
                pin: "env".into(),
            },
            target: Endpoint {
                node: "sink".into(),
                pin: "env".into(),
            },
        },
    ]);
    graph.connections.push(Connection {
        source: Endpoint {
            node: app.node.0.clone(),
            pin: "on_exit".into(),
        },
        target: Endpoint {
            node: "bake".into(),
            pin: "in".into(),
        },
    });
    desktop
        .orchestrate(
            Command::ReplaceGraph {
                workflow: app.workflow,
                graph,
            },
            None,
        )
        .unwrap();
    // A manually closed window must keep its signal source until the shared
    // process exits. Opening another window must not steal that source node.
    compositor.publish(json!({"WindowClosed":{"id":1}}));
    support::wait(|| !desktop.bindings().applications.contains_key(&app.node.0));
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .node(&app.node.0)
            .is_some_and(|n| n.window_id.is_none())
    });
    assert!(application.try_wait().unwrap().is_none());
    assert!(!directory.path().join("exit-code").exists());
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":window(application.id() as i32, 2, "Shared app")}}));
    support::wait(|| {
        desktop
            .bindings()
            .applications
            .values()
            .any(|b| b.window == 2)
    });
    assert!(!desktop.bindings().applications.contains_key(&app.node.0));
    assert!(desktop.snapshot().state.node(&app.node.0).is_some());
    desktop.shutdown();
    application.stdin.take().unwrap().write_all(b"x").unwrap();
    application.wait().unwrap();
    support::wait(|| directory.path().join("exit-code").exists());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("exit-code")).unwrap(),
        "37"
    );
}
#[test]
fn event_stream_disconnect_preserves_workflows_and_reconnects_the_same_applications() {
    let mut application = Process::new("sleep").arg("20").spawn().unwrap();
    let compositor = Compositor::new(application.id() as i32);
    let (desktop, _) =
        support::fixture_with_niri(State::empty("/tmp".into()), Some(&compositor.socket));
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .orchestration
            .as_ref()
            .is_some_and(|r| !r.applications.is_empty())
    });
    let runtime = desktop.snapshot().state.orchestration.clone().unwrap();
    for (id, workspace, owner) in [
        ("session-parent", "local:desktop", None),
        ("private-child", "local:user", Some("session-parent")),
        ("unrelated", "local:user", None),
    ] {
        desktop
            .orchestrate(
                Command::Create {
                    workflow: Workflow {
                        row_settings: Default::default(),
                        id: id.into(),
                        name: id.into(),
                        workspace: Some(workspace.into()),
                        owner: owner.map(Into::into),
                        visibility: if owner.is_some() {
                            Visibility::Private
                        } else {
                            Visibility::Public
                        },
                        references: Default::default(),
                        graph: Default::default(),
                        authority: runtime.host_id.clone(),
                        transfer: None,
                    },
                },
                owner.map(Into::into),
            )
            .unwrap();
    }
    let bindings = desktop.bindings();
    desktop.shutdown();
    for _ in 0..2 {
        compositor.disconnect();
        // The observer retries after two seconds. Wait for the new stream,
        // rather than mistaking the old snapshot for a successful reconnect.
        support::wait(|| !compositor.streams.lock().unwrap().is_empty());
        support::wait(|| desktop.bindings() == bindings);
        let snapshot = desktop
            .orchestrate(Command::Snapshot, Some("session-parent".into()))
            .unwrap();
        assert!(snapshot["workflows"].get("session-parent").is_some());
        assert!(snapshot["workflows"].get("private-child").is_some());
        assert!(snapshot["destroyed"].as_array().unwrap().is_empty());
    }
    assert!(
        desktop.orchestrate(Command::Snapshot, None).unwrap()["workflows"]
            .get("unrelated")
            .is_some()
    );
    assert!(application.try_wait().unwrap().is_none());
    let _ = application.kill();
    let _ = application.wait();
}

#[test]
fn compositor_process_exit_cleans_desktop_workflows_without_killing_shared_applications() {
    let mut application = Process::new("sleep").arg("20").spawn().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("niri.sock");
    let initial = [
        json!({"Ok":"Handled"}),
        json!({"WorkspacesChanged":{"workspaces":[{"id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":1}]}}),
        json!({"WindowsChanged":{"windows":[window(application.id() as i32, 1, "Shared app")]}}),
    ]
    .map(|event| format!("{event}\n"))
    .concat();
    // The IPC peer must be a separate process: closing a stream in this test
    // process deliberately models a recoverable transport failure instead.
    let mut compositor = Process::new("python3")
        .args([
            "-c",
            r#"import socket, sys
s = socket.socket(socket.AF_UNIX)
s.bind(sys.argv[1])
s.listen()
peers = []
while True:
    client, _ = s.accept()
    if client.makefile('rb').readline().strip() == b'"EventStream"':
        client.sendall(sys.argv[2].encode())
        peers.append(client)
    else:
        client.close()
"#,
        ])
        .arg(&socket)
        .arg(initial)
        .spawn()
        .unwrap();
    support::wait(|| socket.exists());
    let (desktop, _) = support::fixture_with_niri(State::empty("/tmp".into()), Some(&socket));
    support::wait(|| !desktop.bindings().applications.is_empty());
    let workflow = desktop
        .bindings()
        .applications
        .values()
        .next()
        .unwrap()
        .workflow
        .clone();
    desktop
        .orchestrate(
            Command::Move {
                workflow: workflow.clone().into(),
                workspace: "local:desktop".into(),
            },
            None,
        )
        .unwrap();
    desktop.shutdown();
    compositor.kill().unwrap();
    compositor.wait().unwrap();
    // Leave the socket pathname behind, as a crashed compositor can do.
    assert!(socket.exists());
    support::wait(|| {
        desktop.orchestrate(Command::Snapshot, None).unwrap()["destroyed"]
            .as_array()
            .unwrap()
            .contains(&json!(workflow))
    });
    assert!(application.try_wait().unwrap().is_none());
    let _ = application.kill();
    let _ = application.wait();
}

#[test]
fn provider_repairs_saved_duplicate_owners_and_does_not_republish_them() {
    use flowwm::model::{Node, NodeKind, Position};

    let compositor = Compositor::new(std::process::id() as i32);
    let mut state = State::empty("/tmp".into());
    let workflow = state.selected_workspace.clone();
    for _ in 0..3 {
        let mut node = Node::new(
            &workflow,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        node.window_id = Some(1);
        node.desktop_window = true;
        // Saved launchers must remain in the graph even when detached.
        node.flow
            .properties
            .insert("application".into(), json!("test.shared"));
        state.nodes.push(node);
    }
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.len() == 1);
    let bindings = desktop.bindings();
    let owner = bindings.applications.keys().next().unwrap();
    for revision in 0..3 {
        let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
        let mut graph: FlowNet =
            serde_json::from_value(runtime["workflows"][&workflow]["graph"].clone()).unwrap();
        assert_eq!(graph.nodes.len(), 3);
        for node in &mut graph.nodes {
            node.flow
                .properties
                .insert("title".into(), json!(format!("Revision {revision}")));
        }
        desktop
            .orchestrate(
                Command::ReplaceGraph {
                    workflow: workflow.clone().into(),
                    graph,
                },
                None,
            )
            .unwrap();
        support::wait(|| {
            desktop.snapshot().state.nodes.iter().all(|n| {
                n.flow.properties.get("title") == Some(&json!(format!("Revision {revision}")))
            })
        });
        let snapshot = desktop.snapshot();
        let attached: Vec<_> = snapshot
            .state
            .nodes
            .iter()
            .filter(|n| n.window_id == Some(1))
            .collect();
        assert_eq!(attached.len(), 1);
        assert_eq!(&attached[0].id, owner);
        assert_eq!(desktop.bindings().applications.len(), 1);
    }
    desktop.shutdown();
}

#[test]
fn moving_a_window_relocates_its_definition_binding_and_preview_without_recreating_it() {
    let pid = std::process::id() as i32;
    let compositor = Compositor::new(pid);
    let (mut state, id) = application_node();
    let original = state.selected_workspace.clone();
    let dependent = Node::new(
        &original,
        NodeKind::Shell,
        "/tmp".into(),
        Default::default(),
    );
    let dependent_id = dependent.id.clone();
    state.connections.push(Connection {
        source: Endpoint {
            node: id.clone(),
            pin: "on_exit".into(),
        },
        target: Endpoint {
            node: dependent_id.clone(),
            pin: "in".into(),
        },
    });
    state.nodes.push(dependent);
    let (desktop, _) = support::fixture_with_niri(state, Some(&compositor.socket));
    support::wait(|| desktop.bindings().applications.contains_key(&id));
    let token = desktop.bindings().applications[&id].token.clone();
    compositor.publish(json!({"WorkspacesChanged":{"workspaces":[
        {"id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":1},
        {"id":2,"idx":2,"name":"Destination","output":"TEST","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null}
    ]}}));
    let mut moved = window(pid, 1, "Moved application");
    moved["workspace_id"] = json!(2);
    compositor.publish(json!({"WindowOpenedOrChanged":{"window":moved}}));
    support::wait(|| {
        desktop
            .snapshot()
            .state
            .node(&id)
            .is_some_and(|n| n.workspace != original && n.window_id == Some(1))
    });
    let snapshot = desktop.snapshot();
    let destination = snapshot.state.node(&id).unwrap().workspace.clone();
    assert_eq!(
        snapshot.state.nodes.iter().filter(|n| n.id == id).count(),
        1
    );
    assert_eq!(
        snapshot.state.node(&dependent_id).unwrap().workspace,
        original
    );
    assert!(
        !snapshot
            .state
            .connections
            .iter()
            .any(|e| e.source.node == id)
    );
    assert_eq!(desktop.bindings().applications[&id].workflow, destination);
    assert_eq!(desktop.bindings().applications[&id].token, token);
    let runtime = desktop.orchestrate(Command::Snapshot, None).unwrap();
    let runtime: RuntimeState = serde_json::from_value(runtime).unwrap();
    assert!(
        runtime.workflows[&original.clone().into()]
            .graph
            .node(&id)
            .is_none()
    );
    assert!(
        runtime.workflows[&destination.clone().into()]
            .graph
            .node(&id)
            .is_some()
    );
    assert_eq!(
        runtime.applications[&id.clone().into()].workflow.0,
        destination
    );
    let position = snapshot.state.node(&id).unwrap().position.clone();
    desktop
        .request(Action::SelectWorkspace {
            id: destination.clone(),
        })
        .unwrap();
    assert_eq!(
        desktop.snapshot().state.node(&id).unwrap().position.x,
        position.x
    );
    let (reconnected, _) = desktop.reconnect();
    assert_eq!(
        reconnected.snapshot().state.node(&id).unwrap().workspace,
        destination
    );
    assert_eq!(
        reconnected.snapshot().state.node(&id).unwrap().window_id,
        Some(1)
    );
    reconnected.shutdown();
}
