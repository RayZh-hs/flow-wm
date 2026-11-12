//! The production desktop-provider process owns compositor lifetime and exit events.
mod support;
use flow_core::{domain::*, graph::*, protocol::Command};
use flowwm::model::State;
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    process::{Command as Process, Stdio},
    sync::{Arc, Mutex},
};
struct Compositor {
    streams: Arc<Mutex<Vec<UnixStream>>>,
    socket: std::path::PathBuf,
    _directory: tempfile::TempDir,
}
impl Compositor {
    fn new(pid: i32) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("niri.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let streams = Arc::new(Mutex::new(Vec::new()));
        let peers = streams.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                if line.trim() != "\"EventStream\"" {
                    continue;
                }
                writeln!(stream, "{}", json!({"Ok":"Handled"})).unwrap();
                let workspace = json!({"id":1,"idx":1,"name":null,"output":"TEST","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":1});
                let window = json!({"id":1,"workspace_id":1,"title":"Shared app","app_id":"test.shared","pid":pid,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[800,600],"window_size":[800,600],"tile_pos_in_workspace_view":[0,0],"window_offset_in_tile":[0,0]},"focus_timestamp":null});
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
            socket,
            _directory: directory,
        }
    }
    fn disconnect(&self) {
        for stream in self.streams.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
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
    graph.connections.push(Connection {
        source: Endpoint {
            node: app.node.0,
            pin: "on_exit".into(),
        },
        target: Endpoint {
            node: "sink".into(),
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
fn compositor_teardown_cleans_current_desktop_assignments_without_killing_shared_applications() {
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
    desktop.shutdown();
    compositor.disconnect();
    support::wait(|| {
        let snapshot = desktop
            .orchestrate(Command::Snapshot, Some("session-parent".into()))
            .unwrap();
        snapshot["workflows"].get("session-parent").is_none()
            && snapshot["workflows"].get("private-child").is_none()
    });
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
