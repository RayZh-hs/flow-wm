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
