//! Native window operations are separate from owned execution interruption.
use anyhow::{Result, ensure};
use flow_core::providers::{ApplicationOperations, ApplicationProvider};
pub struct DesktopApplications;
fn window(handle: &str) -> Result<u64> {
    Ok(handle.parse()?)
}
impl ApplicationProvider for DesktopApplications {
    fn operations(&self, handle: &str) -> Result<ApplicationOperations> {
        window(handle)?;
        Ok(ApplicationOperations {
            open: true,
            close_window: true,
            interrupt: false,
            disown: false,
        })
    }
    fn open(&self, handle: &str) -> Result<()> {
        crate::niri::focus(window(handle)?)
    }
    fn close_window(&self, handle: &str) -> Result<()> {
        crate::niri::request(niri_ipc::Request::Action(niri_ipc::Action::CloseWindow {
            id: Some(window(handle)?),
        }))?;
        Ok(())
    }
    fn interrupt(&self, _handle: &str) -> Result<()> {
        anyhow::bail!(
            "This application has not declared process-safe interruption; close its window separately"
        )
    }
    fn disown(&self, handle: &str) -> Result<()> {
        ensure!(
            self.operations(handle)?.disown,
            "Attachment cannot be disowned"
        );
        Ok(())
    }
}

use flow_core::{domain::*, protocol::Command, providers::ApplicationRegistration};
use flow_host::client::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, net::UnixListener},
    },
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool, mpsc},
};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bindings {
    pub session: String,
    pub selected: String,
    pub rows: BTreeMap<String, crate::model::Row>,
    pub applications: BTreeMap<String, Binding>,
    /// Declarative selectors reconnect attachments without persisting a live process handle.
    #[serde(default)]
    pub selectors: BTreeMap<String, String>,
    #[serde(default)]
    pub disowned: BTreeSet<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub window: u64,
    pub pid: Option<i32>,
    pub workflow: String,
    pub token: String,
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub title: String,
}
pub fn socket() -> PathBuf {
    std::env::var_os("FLOWWM_DESKTOP_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::ipc::default_socket().with_file_name("flowwm-desktop.sock"))
}
pub fn bindings() -> Result<Bindings> {
    bindings_at(&socket())
}
pub fn bindings_at(path: &std::path::Path) -> Result<Bindings> {
    Ok(serde_json::from_value(flow_host::ipc::send(
        path,
        &serde_json::json!({"desktop_protocol":1,"command":"bindings"}),
    )?)?)
}
impl Bindings {
    fn start_session(&mut self, session: &str) {
        if self.session != session {
            // Compositor IDs are only meaningful within their original session.
            // Keep declarative selectors and row hints for reconnection.
            self.applications.clear();
            self.disowned.clear();
            self.session = session.into();
        }
    }
    fn restore_row_hints(&self, state: &mut crate::model::State) {
        for row in &mut state.workspaces {
            if let Some(binding) = self.rows.get(&row.id) {
                row.row = Some(binding.clone());
            }
        }
    }
    pub fn apply(&self, state: &mut crate::model::State) {
        self.apply_native(state);
        if state.workspaces.iter().any(|w| w.id == self.selected) {
            state.selected_workspace = self.selected.clone();
        }
    }
    pub fn apply_native(&self, state: &mut crate::model::State) {
        for node in &mut state.nodes {
            if let Some(selector) = self.selectors.get(&node.id) {
                node.window_match = Some(selector.clone());
            }
        }
        for row in &mut state.workspaces {
            row.row = self
                .rows
                .get(&row.id)
                .filter(|r| !self.session.is_empty() && r.session == self.session)
                .cloned();
        }
        let mut windows = BTreeSet::new();
        for node in &mut state.nodes {
            if let Some(binding) = self.applications.get(&node.id).filter(|binding| {
                binding.workflow == node.workspace && windows.insert(binding.window)
            }) {
                node.window_id = Some(binding.window);
                node.desktop_window = true;
                node.column = binding.column.clone();
                if node.kind == crate::model::NodeKind::Window && !binding.title.is_empty() {
                    node.title = node
                        .flow
                        .properties
                        .get("display_title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&binding.title)
                        .into();
                }
                if node.kind == crate::model::NodeKind::Window {
                    node.status = crate::model::Status::Running;
                }
            } else if node.desktop_window {
                node.window_id = None;
                if node.kind == crate::model::NodeKind::Window {
                    node.status = crate::model::Status::Stopped;
                }
            }
        }
    }
}
enum Event {
    Desktop(crate::niri::Desktop),
    Runtime(Box<RuntimeState>),
    Bind {
        workflow: String,
        row: crate::model::Row,
        reply: mpsc::Sender<std::result::Result<(), String>>,
    },
    Disown {
        node: String,
        reply: mpsc::Sender<std::result::Result<(), String>>,
    },
    Exit(u64, std::result::Result<u8, String>),
}
fn save_bindings(directory: &std::path::Path, bindings: &Bindings) -> Result<()> {
    let temporary = directory.join("desktop.json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(bindings)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, directory.join("desktop.json"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

pub fn run() -> Result<()> {
    let host = Client::new(flow_host::ipc::host_socket());
    let directory: PathBuf =
        serde_json::from_value(host.command(Command::Info)?["data_directory"].clone())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("desktop.lock"))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Desktop provider already running"
    );
    let mut bindings = if directory.join("desktop.json").exists() {
        serde_json::from_slice::<Bindings>(&std::fs::read(directory.join("desktop.json"))?)?
    } else {
        Bindings::default()
    };
    // Do this before serving GTK: its first snapshot must not contain handles
    // restored from a previous login while the initial desktop events arrive.
    bindings.start_session(&std::env::var("NIRI_SOCKET").unwrap_or_default());
    let shared = Arc::new((Mutex::new((0u64, bindings.clone())), Condvar::new()));
    let (tx, rx) = mpsc::channel();
    let mutations = tx.clone();
    let listener: UnixListener = crate::ipc::bind(&socket())?;
    let service = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let shared = service.clone();
            let mutations = mutations.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut line = String::new();
                let response = (|| -> Result<serde_json::Value> {
                    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
                    ensure!(line.len() < 4096, "Provider request too large");
                    let request: serde_json::Value = serde_json::from_str(&line)?;
                    ensure!(
                        request["desktop_protocol"] == 1,
                        "Unsupported desktop provider protocol"
                    );
                    if request["command"] == "bind_row" {
                        let workflow = request["workflow"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("Workflow required"))?
                            .to_owned();
                        let row = serde_json::from_value(request["row"].clone())?;
                        let (reply, rx) = mpsc::channel();
                        mutations.send(Event::Bind {
                            workflow,
                            row,
                            reply,
                        })?;
                        rx.recv_timeout(std::time::Duration::from_secs(5))?
                            .map_err(anyhow::Error::msg)?;
                        return Ok(serde_json::Value::Null);
                    }
                    if request["command"] == "subscribe" {
                        let notifier = shared.clone();
                        let connection = flow_host::ipc::disconnect_signal(&stream, move || {
                            let _state = notifier.0.lock().unwrap();
                            notifier.1.notify_all();
                        })?;
                        let mut previous = None;
                        loop {
                            let current = shared.0.lock().unwrap();
                            let current = shared
                                .1
                                .wait_while(current, |v| {
                                    Some(v.0) == previous
                                        && !connection
                                            .closed
                                            .load(std::sync::atomic::Ordering::Acquire)
                                })
                                .unwrap();
                            if connection.closed.load(std::sync::atomic::Ordering::Acquire) {
                                return Ok(serde_json::Value::Null);
                            }
                            let (revision, bindings) = current.clone();
                            drop(current);
                            writeln!(
                                stream,
                                "{}",
                                serde_json::json!({"desktop_protocol":1,"revision":revision,"bindings":bindings})
                            )?;
                            previous = Some(revision);
                        }
                    }
                    if request["command"] != "bindings" {
                        let node = request["node"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("Application node is required"))?;
                        let bindings = shared.0.lock().unwrap().1.clone();
                        if let Some(binding) = bindings.applications.get(node) {
                            ensure!(
                                request["token"] == binding.token
                                    && request["incarnation"] == bindings.session,
                                "Stale application operation; refresh desktop bindings"
                            );
                            let applications = DesktopApplications;
                            match request["command"].as_str() {
                                Some("open") => applications.open(&binding.window.to_string())?,
                                Some("close_window") => {
                                    applications.close_window(&binding.window.to_string())?
                                }
                                Some("interrupt") => {
                                    applications.interrupt(&binding.window.to_string())?
                                }
                                Some("disown") => {
                                    let (reply, rx) = mpsc::channel();
                                    mutations.send(Event::Disown {
                                        node: node.into(),
                                        reply,
                                    })?;
                                    rx.recv_timeout(std::time::Duration::from_secs(5))?
                                        .map_err(anyhow::Error::msg)?;
                                }
                                _ => anyhow::bail!("Unsupported application operation"),
                            }
                        } else {
                            ensure!(
                                request["command"] == "open",
                                "Application has no live window"
                            );
                            let host = Client::new(flow_host::ipc::host_socket());
                            let runtime = host.snapshot()?;
                            let spec = runtime
                                .workflows
                                .values()
                                .find_map(|w| w.graph.node(node))
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Application node is unavailable")
                                })?;
                            let application = spec
                                .flow
                                .properties
                                .get("application")
                                .and_then(|v| v.as_str())
                                .ok_or_else(|| anyhow::anyhow!("Choose an application first"))?;
                            let mut child = std::process::Command::new("gtk-launch")
                                .arg(application)
                                .spawn()?;
                            std::thread::spawn(move || {
                                let _ = child.wait();
                            });
                        }
                        return Ok(serde_json::Value::Null);
                    }
                    Ok(serde_json::to_value(shared.0.lock().unwrap().1.clone())?)
                })();
                let reply = match response {
                    Ok(value) => serde_json::json!({"ok":value}),
                    Err(error) => serde_json::json!({"error":error.to_string()}),
                };
                let _ = writeln!(stream, "{reply}");
            });
        }
    });
    let niri_tx = tx.clone();
    crate::niri::listen_observed(
        Arc::new(AtomicBool::new(false)),
        Arc::new(move |d| {
            let _ = niri_tx.send(Event::Desktop(d));
        }),
    );
    let remote = host.clone();
    let runtime_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            let _ = remote.subscribe(|reply| {
                if let Some(value) = reply.result {
                    runtime_tx.send(Event::Runtime(Box::new(serde_json::from_value(value)?)))?;
                }
                Ok(())
            });
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
    let mut monitor = crate::processes::ProcessMonitor::new(move |token, result| {
        let _ = tx.send(Event::Exit(token, result));
    })?;
    let mut state = crate::model::State::empty(std::env::current_dir()?);
    crate::projection::apply(&mut state, &host.snapshot()?);
    bindings.apply(&mut state);
    let mut watches = BTreeMap::<u64, (String, String, String)>::new();
    let mut watched = BTreeSet::new();
    let mut token = 0u64;
    for event in rx {
        match event {
            Event::Bind {
                workflow,
                row,
                reply,
            } => {
                let result = (|| -> Result<()> {
                    ensure!(row.session == bindings.session, "Stale compositor session");
                    ensure!(
                        state.workspaces.iter().any(|w| w.id == workflow),
                        "Workflow not found"
                    );
                    for other in &mut state.workspaces {
                        if other
                            .row
                            .as_ref()
                            .is_some_and(|r| r.session == row.session && r.id == row.id)
                        {
                            other.row = None;
                        }
                    }
                    bindings
                        .rows
                        .retain(|_, r| r.session != row.session || r.id != row.id);
                    state
                        .workspaces
                        .iter_mut()
                        .find(|w| w.id == workflow)
                        .unwrap()
                        .row = Some(row.clone());
                    bindings.rows.insert(workflow.clone(), row);
                    bindings.selected = workflow;
                    save_bindings(&directory, &bindings)?;
                    Ok(())
                })()
                .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            Event::Disown { node, reply } => {
                let result = (|| -> Result<()> {
                    let binding = bindings
                        .applications
                        .get(&node)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Application is detached"))?;
                    host.command(Command::UnregisterApplication {
                        node: node.clone().into(),
                        token: binding.token.clone(),
                        incarnation: bindings.session.clone(),
                    })?;
                    let tokens: Vec<_> = watches
                        .iter()
                        .filter(|(_, w)| w.0 == node)
                        .map(|(t, _)| *t)
                        .collect();
                    for token in tokens {
                        monitor.unwatch(token);
                        watches.remove(&token);
                    }
                    bindings.applications.remove(&node);
                    bindings.disowned.insert(binding.window);
                    if let Some(node) = state.node_mut(&node) {
                        node.window_id = None;
                        node.desktop_window = false;
                    }
                    save_bindings(&directory, &bindings)?;
                    Ok(())
                })()
                .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            Event::Runtime(runtime) => {
                crate::projection::apply(&mut state, &runtime);
                bindings.apply(&mut state);
            }
            Event::Desktop(mut desktop) => {
                if !desktop.connected {
                    if !bindings.session.is_empty() {
                        let runtime = host.snapshot()?;
                        let _ = host.command(Command::EndSession {
                            workspace: WorkspaceId::new(&runtime.host_id.0, "desktop")?,
                            incarnation: bindings.session.clone(),
                        });
                    }
                    continue;
                }
                if !desktop.windows_ready || !desktop.workspaces_ready {
                    continue;
                }
                let runtime = host.snapshot()?;
                let workspace = WorkspaceId::new(&runtime.host_id.0, "desktop")?;
                if runtime
                    .workspaces
                    .availability(&workspace)
                    .is_none_or(|a| !a.active || a.incarnation != desktop.session)
                {
                    host.command(Command::ActivateSession {
                        workspace: workspace.clone(),
                        provider: "desktop".into(),
                        incarnation: desktop.session.clone(),
                    })?;
                }
                if bindings.session != desktop.session {
                    for token in watches.keys() {
                        monitor.unwatch(*token);
                    }
                    watches.clear();
                    watched.clear();
                    bindings.start_session(&desktop.session);
                    bindings.apply_native(&mut state);
                }
                desktop
                    .windows
                    .retain(|w| !bindings.disowned.contains(&w.id));
                bindings.restore_row_hints(&mut state);
                // A closed automatically observed card has no definition to keep.
                // Keep nodes with exit listeners until their process produces on_exit.
                let mut closed = BTreeMap::<WorkflowId, BTreeSet<String>>::new();
                for node in state
                    .nodes
                    .iter()
                    .filter(|n| n.desktop_window && n.kind == crate::model::NodeKind::Window)
                {
                    if node
                        .window_id
                        .is_some_and(|id| !desktop.windows.iter().any(|w| w.id == id))
                        && !node.flow.properties.contains_key("application")
                        && !state.connections.iter().any(|e| e.source.node == node.id)
                    {
                        closed
                            .entry(node.workspace.clone().into())
                            .or_default()
                            .insert(node.id.clone());
                    }
                }
                for (workflow, ids) in closed {
                    let mut graph = runtime.workflows[&workflow].graph.clone();
                    graph.nodes.retain(|n| !ids.contains(&n.id.0));
                    graph
                        .connections
                        .retain(|e| !ids.contains(&e.source.node) && !ids.contains(&e.target.node));
                    host.command(Command::ReplaceGraph { workflow, graph })?;
                    let tokens: Vec<_> = watches
                        .iter()
                        .filter(|(_, w)| ids.contains(&w.0))
                        .map(|(token, _)| *token)
                        .collect();
                    for token in tokens {
                        monitor.unwatch(token);
                        watches.remove(&token);
                    }
                }
                crate::groups::synchronize_with_processes(
                    &mut state,
                    &desktop,
                    &Default::default(),
                );
                bindings.rows.clear();
                for row in &state.workspaces {
                    if !runtime.workflows.contains_key(&row.id.clone().into()) {
                        host.command(Command::Create {
                            workflow: Workflow {
                                id: row.id.clone().into(),
                                name: row.name.clone(),
                                workspace: Some(workspace.clone()),
                                visibility: Visibility::Public,
                                owner: None,
                                references: Default::default(),
                                graph: Default::default(),
                                authority: runtime.host_id.clone(),
                                transfer: None,
                            },
                        })?;
                    }
                    if let Some(row_binding) = &row.row {
                        bindings.rows.insert(row.id.clone(), row_binding.clone());
                    }
                }
                bindings.selected = state.selected_workspace.clone();
                let mut applications = BTreeMap::new();
                for node in state.nodes.iter().filter(|n| n.window_id.is_some()) {
                    let window = node.window_id.unwrap();
                    let Some(native) = desktop.windows.iter().find(|w| w.id == window) else {
                        continue;
                    };
                    bindings
                        .selectors
                        .insert(node.id.clone(), crate::groups::selector(native));
                    let pid = native.pid;
                    if !runtime
                        .workflows
                        .values()
                        .any(|w| w.graph.node(&node.id).is_some())
                    {
                        let fresh = host.snapshot()?;
                        let mut graph = fresh.workflows[&node.workspace.clone().into()]
                            .graph
                            .clone();
                        graph.nodes.push(node.spec());
                        host.command(Command::ReplaceGraph {
                            workflow: node.workspace.clone().into(),
                            graph,
                        })?;
                    }
                    let registration_token = bindings
                        .applications
                        .get(&node.id)
                        .filter(|b| {
                            b.window == window && b.pid == pid && b.workflow == node.workspace
                        })
                        .map(|b| b.token.clone())
                        .unwrap_or_else(crate::model::id);
                    if runtime
                        .applications
                        .get(&node.id.clone().into())
                        .is_none_or(|r| {
                            r.token != registration_token || r.incarnation != desktop.session
                        })
                    {
                        host.command(Command::RegisterApplication {
                            registration: ApplicationRegistration {
                                workflow: node.workspace.clone().into(),
                                node: node.id.clone().into(),
                                workspace: workspace.clone(),
                                incarnation: desktop.session.clone(),
                                token: registration_token.clone(),
                                operations: ApplicationOperations {
                                    open: true,
                                    close_window: true,
                                    interrupt: false,
                                    disown: true,
                                },
                            },
                        })?;
                    }
                    applications.insert(
                        node.id.clone(),
                        Binding {
                            window,
                            pid,
                            workflow: node.workspace.clone(),
                            token: registration_token.clone(),
                            column: node.column.clone(),
                            title: node.title.clone(),
                        },
                    );
                    if let Some(pid) = pid
                        && watched.insert((node.id.clone(), pid, registration_token.clone()))
                    {
                        token += 1;
                        if monitor.watch(token, pid).is_ok() {
                            watches.insert(
                                token,
                                (node.id.clone(), registration_token, desktop.session.clone()),
                            );
                        }
                    }
                }
                // Reconciliation decides which node owns each window. Retaining
                // bindings by live window ID also kept detached owners, which
                // later runtime snapshots could reattach to the same preview.
                bindings.applications = applications;
            }
            Event::Exit(token, result) => {
                if let Some((node, registration_token, incarnation)) = watches.remove(&token)
                    && incarnation == bindings.session
                    && let Ok(exit_code) = result
                {
                    let runtime = host.snapshot()?;
                    if let Some(application) = runtime.applications.get(&node.clone().into()) {
                        let _ = host.command_as(
                            Command::ApplicationEvent {
                                node: node.into(),
                                token: registration_token.clone(),
                                incarnation,
                                pin: "on_exit".into(),
                                delivery: format!("application-exit:{registration_token}").into(),
                                args: serde_json::json!({"exit_code":exit_code}),
                            },
                            Some(application.workflow.clone()),
                        );
                    }
                }
            }
        }
        if serde_json::to_value(&shared.0.lock().unwrap().1)? != serde_json::to_value(&bindings)? {
            save_bindings(&directory, &bindings)?;
            let mut current = shared.0.lock().unwrap();
            current.0 += 1;
            current.1 = bindings.clone();
            shared.1.notify_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_login_drops_live_handles_but_keeps_reconnection_hints() {
        let mut bindings = Bindings {
            session: "previous-login".into(),
            ..Default::default()
        };
        bindings.applications.insert(
            "editor".into(),
            Binding {
                window: 85,
                pid: Some(100),
                workflow: "workflow".into(),
                token: "old-token".into(),
                column: None,
                title: "Editor".into(),
            },
        );
        bindings.disowned.insert(153);
        bindings
            .selectors
            .insert("editor".into(), "test.editor::Editor".into());
        bindings.start_session("previous-login");
        assert_eq!(bindings.applications.len(), 1);
        bindings.start_session("new-login");
        assert!(bindings.applications.is_empty());
        assert!(bindings.disowned.is_empty());
        assert_eq!(bindings.selectors["editor"], "test.editor::Editor");
        assert_eq!(bindings.session, "new-login");
    }

    #[test]
    fn stale_rows_are_private_reconnection_hints_until_reconciled() {
        let mut state = crate::model::State::empty("/tmp".into());
        let mut bindings = Bindings {
            session: "new-login".into(),
            ..Default::default()
        };
        bindings.rows.insert(
            state.selected_workspace.clone(),
            crate::model::Row {
                session: "previous-login".into(),
                id: 1,
                index: 1,
                output: Some("TEST".into()),
                name: None,
            },
        );
        bindings.apply_native(&mut state);
        assert!(state.workspace().row.is_none());
        bindings.restore_row_hints(&mut state);
        assert_eq!(
            state.workspace().row.as_ref().unwrap().session,
            "previous-login"
        );
        bindings.apply_native(&mut state);
        assert!(state.workspace().row.is_none());
    }
}
