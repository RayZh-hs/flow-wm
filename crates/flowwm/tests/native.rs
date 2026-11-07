//! Explicit smoke test against a running Niri session; never runs in `cargo test`.
use flowwm::{
    engine::{Control, UiCommand},
    ipc,
    model::{State, Status},
    niri,
};
use niri_ipc::{Action, Request, Response, WorkspaceReferenceArg};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Native {
    process: Child,
    socket: PathBuf,
    workspace: Option<String>,
    original_workspace: Option<niri_ipc::Workspace>,
}
impl Native {
    fn send(&self, control: Control) {
        ipc::send(&self.socket, &control).unwrap();
    }
    fn ui(&self, command: UiCommand) {
        self.send(Control::Ui { command });
        thread::sleep(Duration::from_millis(450));
    }
    fn state(&self) -> State {
        serde_json::from_value(ipc::send(&self.socket, &Control::Status).unwrap()).unwrap()
    }
    fn capture(&self, directory: &Path, name: &str) {
        let path = directory.join(name);
        self.ui(UiCommand::Capture { path: path.clone() });
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(bytes.len() > 5000);
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = ipc::send(
            &self.socket,
            &Control::Ui {
                command: UiCommand::Quit,
            },
        );
        let start = Instant::now();
        loop {
            if self.process.try_wait().ok().flatten().is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                let _ = self.process.kill();
                let _ = self.process.wait();
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if let Some(name) = &self.workspace
            && let Ok(Response::Workspaces(workspaces)) = niri::request(Request::Workspaces)
            && workspaces
                .iter()
                .any(|w| w.name.as_ref() == Some(name) && w.active_window_id.is_none())
        {
            let _ = niri::request(Request::Action(Action::UnsetWorkspaceName {
                reference: Some(WorkspaceReferenceArg::Name(name.clone())),
            }));
        }
        if let Some(original) = &self.original_workspace
            && let Ok(Response::Workspaces(workspaces)) = niri::request(Request::Workspaces)
            && let Some(current) = workspaces.iter().find(|w| w.id == original.id)
        {
            let reference = current
                .name
                .clone()
                .map(WorkspaceReferenceArg::Name)
                .unwrap_or(WorkspaceReferenceArg::Index(current.idx));
            let _ = niri::request(Request::Action(Action::FocusWorkspace { reference }));
        }
    }
}

#[test]
#[ignore = "requires a GTK display; uses isolated state and simulated compositor events"]
fn windowed_row_modes_restore_during_switches_and_after_restart() {
    use flowwm::{catalog::Catalog, model::RowView};
    let dir = tempfile::tempdir().unwrap();
    let start = || {
        let socket = dir.path().join("rows.sock");
        let process = Command::new(env!("CARGO_BIN_EXE_flowwm"))
            .env_remove("NIRI_SOCKET")
            .arg("--socket")
            .arg(&socket)
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .args(["start", "--windowed", "--background"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let native = Native {
            process,
            socket,
            workspace: None,
            original_workspace: None,
        };
        let started = Instant::now();
        while ipc::send(&native.socket, &Control::Status).is_err() {
            assert!(started.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(20));
        }
        native
    };
    let wait = |native: &Native, row: u64, visible: bool| {
        let started = Instant::now();
        loop {
            let catalog: Catalog = serde_json::from_value(
                ipc::send(&native.socket, &serde_json::json!({"op":"catalog"})).unwrap(),
            )
            .unwrap();
            let group = catalog
                .workspaces
                .iter()
                .find(|w| w.id == catalog.selected_workspace)
                .unwrap();
            if group.row.as_ref().is_some_and(|r| r.id == row)
                && catalog.presentation.graph_visible == visible
            {
                assert_eq!(
                    group.view,
                    if visible {
                        RowView::Nodes
                    } else {
                        RowView::Windows
                    }
                );
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "row {row} did not restore visibility {visible}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(250));
    };
    let mut desktop: flowwm::niri::Desktop = serde_json::from_value(serde_json::json!({
        "session":"row-test", "connected":true, "windows_ready":true, "workspaces_ready":true,
        "windows":[], "error":null,
        "workspaces":[
            {"id":10,"idx":1,"name":"Row 1","output":"DP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":null},
            {"id":20,"idx":2,"name":"Row 2","output":"DP-1","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null}
        ]
    })).unwrap();
    let native = start();
    native.send(Control::SyncDesktop {
        desktop: desktop.clone(),
    });
    wait(&native, 10, false);
    let mut node = flowwm::model::Node::new(
        &native.state().selected_workspace,
        flowwm::model::NodeKind::Window,
        dir.path().into(),
        flowwm::model::Position { x: 80.0, y: 100.0 },
    );
    node.title = "Remember this selection".into();
    let selected = node.id.clone();
    native.send(Control::Create {
        node: Box::new(node),
    });
    native.ui(UiCommand::Nodes);
    wait(&native, 10, true);
    native.ui(UiCommand::Select { id: selected });
    let screenshots = dir.path().join("screenshots");
    fs::create_dir_all(&screenshots).unwrap();
    native.capture(&screenshots, "selected-before.png");
    desktop.activate_row(20, true);
    native.send(Control::SyncDesktop {
        desktop: desktop.clone(),
    });
    wait(&native, 20, false);
    native.ui(UiCommand::Toggle);
    wait(&native, 20, true);
    desktop.activate_row(10, true);
    native.send(Control::SyncDesktop {
        desktop: desktop.clone(),
    });
    wait(&native, 10, true);
    native.capture(&screenshots, "selected-after.png");
    assert_eq!(
        fs::read(screenshots.join("selected-before.png")).unwrap(),
        fs::read(screenshots.join("selected-after.png")).unwrap(),
        "returning to a row must restore both its camera and selection"
    );
    native.ui(UiCommand::Toggle);
    wait(&native, 10, false);
    desktop.activate_row(20, true);
    native.send(Control::SyncDesktop {
        desktop: desktop.clone(),
    });
    wait(&native, 20, true);
    // Interrupt several in-flight row slides, then check the final row again.
    for row in [10, 20, 10, 20, 10, 20] {
        desktop.activate_row(row, true);
        native.send(Control::SyncDesktop {
            desktop: desktop.clone(),
        });
        thread::sleep(Duration::from_millis(30));
    }
    wait(&native, 20, true);
    if let Some(directory) = std::env::var_os("FLOWWM_SCREENSHOT_DIR") {
        fs::create_dir_all(&directory).unwrap();
        native.capture(Path::new(&directory), "row-restored.png");
    }
    drop(native);
    let native = start();
    wait(&native, 20, true);
}

#[test]
#[ignore = "opens native Wayland surfaces; run explicitly inside Niri"]
fn graph_dialogs_and_output_windows_are_native_niri_surfaces() {
    assert!(std::env::var_os("NIRI_SOCKET").is_some());
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("control.sock");
    let original_workspace = match niri::request(Request::Workspaces).unwrap() {
        Response::Workspaces(w) => w.into_iter().find(|w| w.is_focused),
        _ => None,
    };
    let process = Command::new(env!("CARGO_BIN_EXE_flowwm"))
        .arg("--socket")
        .arg(&socket)
        .arg("--data-dir")
        .arg(dir.path().join("data"))
        .args(["start", "--example"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut native = Native {
        process,
        socket,
        workspace: None,
        original_workspace,
    };
    let start = Instant::now();
    while ipc::send(&native.socket, &Control::Status).is_err() {
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(100));
    }
    native.ui(UiCommand::Nodes);
    let Response::Layers(layers) = niri::request(Request::Layers).unwrap() else {
        panic!("No layers");
    };
    assert!(layers.iter().any(|l| l.namespace == "flowwm-graph"));
    let state = native.state();
    let screenshots = std::env::var_os("FLOWWM_SCREENSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.path().join("screenshots"));
    fs::create_dir_all(&screenshots).unwrap();
    let analyze = state
        .nodes
        .iter()
        .find(|n| n.title == "Analyze issue")
        .unwrap();
    native.ui(UiCommand::Select {
        id: analyze.id.clone(),
    });
    native.ui(UiCommand::Fit);
    native.capture(&screenshots, "native-graph.png");
    native.ui(UiCommand::Palette);
    let Response::Layers(layers) = niri::request(Request::Layers).unwrap() else {
        panic!();
    };
    assert!(layers.iter().any(|l| l.namespace == "flowwm-dialog"));
    native.capture(&screenshots, "native-palette.png");
    native.ui(UiCommand::Dismiss);
    for node in &state.nodes {
        native.ui(UiCommand::Edit {
            id: node.id.clone(),
        });
        let Response::Layers(layers) = niri::request(Request::Layers).unwrap() else {
            panic!();
        };
        assert!(
            layers.iter().any(|l| l.namespace == "flowwm-dialog"),
            "No native parameter dialog for {:?}",
            node.kind
        );
        native.ui(UiCommand::Dismiss);
    }
    let human = state
        .nodes
        .iter()
        .find(|n| n.kind == flowwm::model::NodeKind::Human)
        .unwrap();
    native.send(Control::Run {
        id: human.id.clone(),
        downstream: false,
    });
    native.workspace = Some(format!("flowwm-{}", &state.selected_workspace[..8]));
    native.ui(UiCommand::Open {
        id: human.id.clone(),
    });
    let Response::Windows(windows) = niri::request(Request::Windows).unwrap() else {
        panic!("No native windows");
    };
    let window = windows
        .iter()
        .find(|w| {
            w.title.as_deref() == Some(human.title.as_str())
                && w.app_id.as_deref() == Some("dev.flowwm.Shell")
        })
        .expect("Expanded node must be a real Niri window");
    native.capture(&screenshots, "native-node-window.png");
    niri::request(Request::Action(Action::CloseWindow {
        id: Some(window.id),
    }))
    .unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        native.state().node(&human.id).unwrap().status,
        Status::Running,
        "Closing a presentation must not stop work"
    );
    native.send(Control::Stop {
        id: human.id.clone(),
    });
    assert_eq!(
        native.state().node(&human.id).unwrap().status,
        Status::Stopped
    );
}
