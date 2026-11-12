//! Fixtures start the production host and desktop provider as independent processes.
#![allow(dead_code)]
use flow_core::{domain::*, protocol::Command};
use flow_host::client::Client;
use flowwm::{
    client::{DesktopClient, UiCommand},
    model::*,
    presentation::Writer,
};
use std::{
    process::{Child, Command as Process, Stdio},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
pub fn wait(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
struct Processes {
    socket: std::path::PathBuf,
    provider_socket: std::path::PathBuf,
    host: Child,
    provider: Child,
    _directory: tempfile::TempDir,
}
impl Drop for Processes {
    fn drop(&mut self) {
        let _ = self.provider.kill();
        let _ = self.provider.wait();
        unsafe {
            libc::kill(self.host.id() as i32, libc::SIGTERM);
        }
        let _ = self.host.wait();
    }
}
#[derive(Clone)]
pub struct TestClient {
    client: DesktopClient,
    _processes: Arc<Processes>,
}
impl std::ops::Deref for TestClient {
    type Target = DesktopClient;
    fn deref(&self) -> &DesktopClient {
        &self.client
    }
}
impl TestClient {
    pub fn bindings(&self) -> flowwm::desktop_provider::Bindings {
        flowwm::desktop_provider::bindings_at(&self._processes.provider_socket).unwrap()
    }
    pub fn configure_agent(&self, command: serde_json::Value) {
        std::fs::write(
            self._processes._directory.path().join("config/agent.json"),
            serde_json::to_vec(&serde_json::json!({"command":command})).unwrap(),
        )
        .unwrap();
    }
    pub fn reconnect(&self) -> (Self, mpsc::Receiver<UiCommand>) {
        self.client.shutdown();
        let (client, events) = DesktopClient::connect_with_provider(
            self._processes.socket.clone(),
            self._processes.provider_socket.clone(),
        )
        .unwrap();
        (
            Self {
                client,
                _processes: self._processes.clone(),
            },
            events,
        )
    }
}
pub fn fixture(state: State) -> (TestClient, mpsc::Receiver<UiCommand>) {
    fixture_with_niri(state, None)
}
pub fn fixture_with_niri(
    state: State,
    niri: Option<&std::path::Path>,
) -> (TestClient, mpsc::Receiver<UiCommand>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let socket = path.join("host.sock");
    let provider_socket = path.join("desktop.sock");
    let data = path.join("data");
    std::fs::create_dir_all(path.join("config")).unwrap();
    std::fs::write(
        path.join("config/config.kdl"),
        "flow { notification-command; }\n",
    )
    .unwrap();
    let host_binary =
        std::path::Path::new(env!("CARGO_BIN_EXE_flowwm")).with_file_name("flow-core");
    let host = Process::new(host_binary)
        .arg("--socket")
        .arg(&socket)
        .arg("--data-dir")
        .arg(&data)
        .env("FLOWWM_CONFIG_DIR", path.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("Build the workspace binaries before running WM integration tests");
    let native = Client::new(socket.clone());
    wait(|| native.snapshot().is_ok());
    for row in &state.workspaces {
        let mut graph = state.graph();
        let ids: std::collections::BTreeSet<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == row.id)
            .map(|n| n.id.as_str())
            .collect();
        graph.nodes.retain(|n| ids.contains(n.id.0.as_str()));
        graph.connections.retain(|e| {
            ids.contains(e.source.node.as_str()) && ids.contains(e.target.node.as_str())
        });
        for node in &mut graph.nodes {
            if let Some(view) = state.node(&node.id.0) {
                node.flow
                    .properties
                    .insert("title".into(), serde_json::json!(view.title));
            }
        }
        native
            .command(Command::Create {
                workflow: Workflow {
                    row_settings: Default::default(),
                    id: row.id.clone().into(),
                    name: row.name.clone(),
                    workspace: Some("local:user".into()),
                    visibility: Visibility::Public,
                    owner: None,
                    references: Default::default(),
                    graph,
                    authority: "local".into(),
                    transfer: None,
                },
            })
            .unwrap();
    }
    let writer = Writer::new(data.clone()).unwrap();
    writer.save(&state).unwrap();
    drop(writer);
    let mut bindings = flowwm::desktop_provider::Bindings {
        session: niri
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        selected: state.selected_workspace.clone(),
        ..Default::default()
    };
    for row in &state.workspaces {
        if let Some(binding) = &row.row {
            bindings.rows.insert(row.id.clone(), binding.clone());
        }
    }
    for node in &state.nodes {
        if let Some(window) = node.window_id {
            bindings.applications.insert(
                node.id.clone(),
                flowwm::desktop_provider::Binding {
                    window,
                    pid: None,
                    workflow: node.workspace.clone(),
                    token: node.id.clone(),
                    column: node.column.clone(),
                    title: node.title.clone(),
                },
            );
        }
    }
    std::fs::write(
        data.join("desktop.json"),
        serde_json::to_vec(&bindings).unwrap(),
    )
    .unwrap();
    let mut provider = Process::new(env!("CARGO_BIN_EXE_flowwm-desktop"));
    provider
        .env("FLOWWM_HOST_SOCKET", &socket)
        .env("FLOWWM_DESKTOP_SOCKET", &provider_socket)
        .env("FLOWWM_CONFIG_DIR", path.join("config"))
        .env_remove("NIRI_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if let Some(niri) = niri {
        provider.env("NIRI_SOCKET", niri);
    }
    let provider = provider.spawn().unwrap();
    wait(|| provider_socket.exists());
    let (client, events) =
        DesktopClient::connect_with_provider(socket.clone(), provider_socket.clone()).unwrap();
    (
        TestClient {
            client,
            _processes: Arc::new(Processes {
                socket,
                provider_socket,
                host,
                provider,
                _directory: directory,
            }),
        },
        events,
    )
}
