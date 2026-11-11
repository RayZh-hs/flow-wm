use crate::{model::*, store::Store};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const GRAPH_RUN: &str = "\0graph";
pub use flow_builtins::agent::{AgentOutcome, AgentResult, agent_result};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationSelection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub app_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UiCommand {
    Workflows,
    Toggle,
    Nodes,
    Windows,
    ZoomIn,
    ZoomOut,
    RunSelected,
    StopSelected,
    Navigate {
        direction: String,
    },
    MoveSelection {
        direction: String,
    },
    Arrange,
    ConnectSearch,
    CreateConnected {
        id: String,
        source: crate::graph::Endpoint,
        input: String,
        #[serde(default)]
        reverse: bool,
        #[serde(default)]
        application: Option<ApplicationSelection>,
    },
    Open {
        id: String,
    },
    Select {
        id: String,
    },
    Focus {
        id: String,
        expand: bool,
    },
    CreateApplication {
        id: String,
        name: String,
        #[serde(default)]
        app_id: String,
    },
    CreateMacro {
        id: String,
    },
    Recipe {
        id: String,
    },
    Workspace {
        id: String,
    },
    Search {
        mode: String,
    },
    Launcher,
    ResumeLookup,
    Edit {
        id: String,
    },
    Palette,
    Fit,
    Dismiss,
    Capture {
        path: PathBuf,
    },
    Quit,
    Notes,
    Recipes,
    RunGroup,
    Help,
    BarGeometry {
        output: String,
        insets: [i32; 4],
    },
    RefreshImages,
    AgentChat,
}

fn notify_default() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Control {
    ConnectHost {
        host: String,
        remote: crate::remote::Remote,
    },
    EndSession {
        workspace: flow_core::domain::WorkspaceId,
        incarnation: String,
    },
    Orchestrate {
        request: Box<flow_core::protocol::Request>,
    },
    Status,
    Alert {
        id: String,
        message: String,
        #[serde(default)]
        critical: bool,
        #[serde(default = "notify_default")]
        notify: bool,
    },
    ClearAlert {
        id: String,
    },
    Paste {
        recipe: Recipe,
        offset: Position,
    },
    Arrange {
        positions: Vec<(String, Position)>,
    },
    Pins {
        id: String,
    },
    ExposeOutput {
        id: String,
        pin: crate::graph::Pin,
    },
    PublishProperties {
        id: String,
        properties: crate::graph::Record,
    },
    Create {
        node: Box<Node>,
    },
    Update {
        node: Box<Node>,
    },
    Move {
        id: String,
        position: Position,
    },
    MoveToWorkspace {
        id: String,
        workspace: String,
    },
    Connect {
        source: String,
        target: String,
    },
    ConnectPins {
        connection: crate::graph::Connection,
    },
    DisconnectPins {
        connection: crate::graph::Connection,
    },
    Evaluate {
        id: String,
    },
    Emit {
        source: crate::graph::Endpoint,
        args: Value,
    },
    Activate {
        id: String,
        args: Value,
    },
    RegisterType {
        definition: crate::graph::TypeDefinition,
    },
    RegisterCast {
        definition: crate::graph::CastDefinition,
    },
    RegisterNode {
        definition: crate::graph::NodeDefinition,
    },
    Disconnect {
        source: String,
        target: String,
    },
    Delete {
        id: String,
    },
    Run {
        id: String,
        downstream: bool,
    },
    RunWorkspace,
    Stop {
        id: String,
    },
    Interrupt {
        id: String,
    },
    Approve {
        id: String,
        output: Option<String>,
    },
    Reject {
        id: String,
    },
    CreateWorkspace {
        name: String,
        cwd: PathBuf,
    },
    SelectWorkspace {
        id: String,
    },
    Notes {
        text: String,
    },
    HumanProgress {
        id: String,
        text: String,
    },
    SaveRecipe {
        name: String,
        nodes: Vec<String>,
    },
    ImportRecipe {
        recipe: Recipe,
    },
    Instantiate {
        id: String,
    },
    Watch {
        watcher: Watcher,
    },
    DeleteWatcher {
        id: String,
    },
    Ui {
        command: UiCommand,
    },
    Shutdown,
    SyncDesktop {
        desktop: crate::niri::Desktop,
    },
    BindRow {
        workspace: String,
        row: Row,
    },
    ColumnMode {
        id: String,
        mode: crate::model::ColumnMode,
    },
    RowView {
        workspace: String,
        view: crate::model::RowView,
    },
    ClearHistory,
    Chat {
        id: String,
        text: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: Arc<State>,
    pub revision: u64,
    pub error: Option<String>,
}
struct Request {
    control: Control,
    reply: Option<mpsc::Sender<std::result::Result<Value, String>>>,
}
enum Message {
    Completion(flow_core::providers::Completion),
    WindowProcessExited {
        token: u64,
        result: std::result::Result<u8, String>,
    },
    GraphFinished {
        id: String,
        generation: u64,
        result: std::result::Result<crate::graph::Evaluation, String>,
    },
    Request(Request),
    Output(flow_core::providers::Output),
}
struct Run {
    generation: u64,
    cancel: Arc<AtomicBool>,
    metric_due: Option<Instant>,
    writer: bool,
    process: bool,
    invocation: Option<flow_core::domain::InvocationId>,
}
impl Run {
    fn interrupt(&self, registry: &flow_core::providers::Registry) {
        self.cancel.store(true, Ordering::Release);
        if let Some(id) = &self.invocation
            && let Ok(provider) = registry.provider(&"local".into())
        {
            let _ = provider.interrupt(&id.0);
        }
    }
}

struct WindowProcess {
    session: String,
    window: u64,
    pid: i32,
    token: u64,
    pending: bool,
}

pub type SnapshotObserver = Arc<dyn Fn(&Snapshot) -> Result<()> + Send + Sync>;
#[derive(Clone)]
pub struct Engine {
    pub performance: crate::performance::Performance,
    remote: Option<PathBuf>,
    storage_directory: Option<PathBuf>,
    observer: Option<SnapshotObserver>,
    frontend: Arc<Mutex<Option<mpsc::Sender<UiCommand>>>>,
    remote_stream: Arc<Mutex<Option<std::os::unix::net::UnixStream>>>,
    tx: mpsc::Sender<Message>,
    snapshot: Arc<Mutex<Snapshot>>,
    join: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    notifier: Arc<Notifier>,
    presentation: Arc<Mutex<Presentation>>,
    launcher: Arc<Mutex<Option<mpsc::Sender<Option<String>>>>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Presentation {
    pub graph_visible: bool,
    pub lookup_return: bool,
    #[serde(default)]
    pub lookup_node: Option<String>,
    #[serde(default)]
    pub lookup_pin: Option<String>,
    #[serde(default)]
    pub lookup_input: bool,
}
struct Notifier {
    generation: Mutex<u64>,
    changed: Condvar,
    closed: AtomicBool,
}
impl Notifier {
    fn signal(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }
}
impl Engine {
    pub fn new(state: State, store: Store) -> (Self, mpsc::Receiver<UiCommand>) {
        Self::with_worker(state, store, crate::config::worker_executable().ok())
    }
    pub fn with_worker(
        mut state: State,
        store: Store,
        worker: Option<PathBuf>,
    ) -> (Self, mpsc::Receiver<UiCommand>) {
        if let Some(worker) = worker {
            crate::config::set_worker_executable(worker);
        }
        if let Ok((envelope, _)) = flow_core::migration::Envelope::from_v1(state.clone()) {
            state.orchestration = Some(envelope.runtime);
        }
        if let Some(runtime) = &mut state.orchestration {
            if let Ok(host) = crate::config::host_id()
                && host.0 != "local"
            {
                runtime.bind_host(host).expect("Configured host identity");
            }
            let _ = runtime.workspaces.activate(
                &format!("{}:user", runtime.host_id.0).into(),
                &"local".into(),
                crate::model::id(),
            );
        }
        let (tx, rx) = mpsc::channel();
        let (ui_tx, ui_messages) = mpsc::channel::<UiCommand>();
        let (local_ui, ui_rx) = mpsc::channel();
        let frontend: Arc<Mutex<Option<mpsc::Sender<UiCommand>>>> = Arc::new(Mutex::new(None));
        let snapshot = Arc::new(Mutex::new(Snapshot {
            state: Arc::new(state.clone()),
            revision: 0,
            error: None,
        }));
        let shared = snapshot.clone();
        let sender = tx.clone();
        let notifier = Arc::new(Notifier {
            generation: Mutex::new(1),
            changed: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let ui_frontend = frontend.clone();
        let ui_notifier = notifier.clone();
        thread::spawn(move || {
            for command in ui_messages {
                let _ = local_ui.send(command.clone());
                if let Some(frontend) = ui_frontend.lock().unwrap().as_ref() {
                    let _ = frontend.send(command);
                }
                ui_notifier.signal();
            }
        });
        let storage_directory = Some(store.directory().to_path_buf());
        let worker_notifier = notifier.clone();
        let join = thread::spawn(move || {
            let process_sender = sender.clone();
            let process_monitor = crate::processes::ProcessMonitor::new(move |token, result| {
                let _ = process_sender.send(Message::WindowProcessExited { token, result });
            })
            .inspect_err(|e| eprintln!("Start application process monitor: {e}"))
            .ok();
            Runtime {
                remotes: crate::remote::configured().unwrap_or_default(),
                network_due: Instant::now(),
                session_watches: HashSet::new(),
                desktop_rows: HashMap::new(),
                limits: crate::config::runtime_limits().unwrap_or_default(),
                orchestrator: {
                    let runtime = state
                        .orchestration
                        .clone()
                        .unwrap_or_else(flow_core::domain::RuntimeState::local);
                    let limits = crate::config::runtime_limits().unwrap_or_default();
                    let mut orchestrator = flow_core::orchestrator::Orchestrator::new(
                        runtime,
                        flow_builtins::local::registry_with_environment(
                            limits.clone(),
                            flow_builtins::local::Environment {
                                config_directory: Some(crate::config::directory()),
                                data_directory: Some(store.directory().into()),
                                control_socket: Some(crate::agent::socket()),
                                cli: std::env::current_exe()
                                    .ok()
                                    .map(|p| p.with_file_name("flowwm")),
                            },
                        )
                        .expect("Builtin registry"),
                    );
                    let output_sender = sender.clone();
                    orchestrator.output = Some(Arc::new(move |output| {
                        let _ = output_sender.send(Message::Output(output));
                    }));
                    orchestrator.signal_capacity = limits.signal_queue_capacity;
                    orchestrator.replay_capacity = limits.replay_capacity;
                    orchestrator
                },
                state,
                store,
                runs: HashMap::new(),
                graph_active: false,
                graph_root: None,
                graph_exit: None,
                graph_pending: VecDeque::new(),
                signal_pending: HashMap::new(),
                signal_args: HashMap::new(),
                suppressed_windows: HashSet::new(),
                pending_deletions: HashSet::new(),
                process_monitor,
                window_processes: HashMap::new(),
                exit_pending: HashSet::new(),
                generation: 0,
                revision: 0,
                error: None,
                dirty: true,
                tx: sender,
                ui: ui_tx,
                last_save: Instant::now(),
                needs_save: true,
                notifier: worker_notifier,
            }
            .work(rx, shared)
        });
        (
            Self {
                performance: crate::performance::Performance::default(),
                remote: None,
                storage_directory,
                observer: None,
                frontend,
                remote_stream: Arc::new(Mutex::new(None)),
                tx,
                snapshot,
                join: Arc::new(Mutex::new(Some(join))),
                notifier,
                presentation: Arc::new(Mutex::new(Presentation::default())),
                launcher: Arc::new(Mutex::new(None)),
            },
            ui_rx,
        )
    }
    pub fn connect_frontend(&self) -> mpsc::Receiver<UiCommand> {
        let (tx, rx) = mpsc::channel();
        *self.frontend.lock().unwrap() = Some(tx);
        rx
    }
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
    /// The compatibility client uses snapshots and commands; native frame traffic stays local.
    pub fn connect(socket: PathBuf) -> Result<(Self, mpsc::Receiver<UiCommand>)> {
        Self::connect_observed(socket, None)
    }
    pub fn data_directory(&self) -> Result<PathBuf> {
        if let Some(socket) = &self.remote {
            return Ok(serde_json::from_value(crate::ipc::send(
                socket,
                &json!({"op":"data_directory"}),
            )?)?);
        }
        self.storage_directory
            .clone()
            .context("Runtime data directory is unavailable")
    }
    pub fn connect_observed(
        socket: PathBuf,
        observer: Option<SnapshotObserver>,
    ) -> Result<(Self, mpsc::Receiver<UiCommand>)> {
        use std::os::unix::net::UnixStream;
        let state: State = serde_json::from_value(crate::ipc::send(&socket, &Control::Status)?)?;
        let snapshot = Arc::new(Mutex::new(Snapshot {
            state: Arc::new(state),
            revision: 0,
            error: None,
        }));
        let notifier = Arc::new(Notifier {
            generation: Mutex::new(1),
            changed: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let mut stream = UnixStream::connect(&socket)?;
        writeln!(stream, "{}", json!({"op":"attach", "protocol":2}))?;
        let remote_stream = Arc::new(Mutex::new(Some(stream.try_clone()?)));
        let (ui_tx, ui_rx) = mpsc::channel();
        let shared = snapshot.clone();
        let changed = notifier.clone();
        if let Some(observer) = &observer {
            observer(&snapshot.lock().unwrap())?;
        }
        let subscription_observer = observer.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                if changed.closed.load(Ordering::Acquire) {
                    break;
                }
                let result = (|| -> Result<()> {
                    let frame: crate::ipc::ClientFrame = serde_json::from_str(&line?)?;
                    if let Some(next) = frame.snapshot {
                        let mut current = shared.lock().unwrap();
                        if next.revision >= current.revision {
                            *current = next;
                            if let Some(observer) = &subscription_observer {
                                observer(&current)?;
                            }
                        }
                    }
                    for command in frame.commands {
                        let _ = ui_tx.send(command);
                    }
                    changed.signal();
                    Ok(())
                })();
                if let Err(error) = result {
                    shared.lock().unwrap().error = Some(format!("Runtime disconnected: {error}"));
                    changed.signal();
                    break;
                }
            }
            if !changed.closed.load(Ordering::Acquire) {
                shared.lock().unwrap().error =
                    Some("Runtime disconnected; reconnect the desktop to resume control".into());
                changed.signal();
            }
        });
        let (tx, rx) = mpsc::channel::<Message>();
        let worker_socket = socket.clone();
        let worker_notifier = notifier.clone();
        thread::spawn(move || {
            while let Ok(message) = rx.recv() {
                if worker_notifier.closed.load(Ordering::Acquire) {
                    break;
                }
                if let Message::Request(request) = message {
                    let result = crate::ipc::send(&worker_socket, &request.control)
                        .map_err(|e| e.to_string());
                    if let Some(reply) = request.reply {
                        let _ = reply.send(result);
                    }
                }
            }
        });
        Ok((
            Self {
                performance: Default::default(),
                remote: Some(socket),
                storage_directory: None,
                observer,
                frontend: Arc::new(Mutex::new(None)),
                remote_stream,
                tx,
                snapshot,
                join: Arc::new(Mutex::new(None)),
                notifier,
                presentation: Arc::new(Mutex::new(Presentation::default())),
                launcher: Arc::new(Mutex::new(None)),
            },
            ui_rx,
        ))
    }
    fn accept_snapshot(&self, next: Snapshot) {
        let mut current = self.snapshot.lock().unwrap();
        if next.revision >= current.revision {
            *current = next;
            if let Some(observer) = &self.observer
                && let Err(error) = observer(&current)
            {
                current.error = Some(format!("Save desktop presentation: {error:#}"));
            }
            drop(current);
            self.notifier.signal();
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
    pub fn presentation(&self) -> Presentation {
        self.presentation.lock().unwrap().clone()
    }
    pub fn connect_launcher(&self) -> mpsc::Receiver<Option<String>> {
        let (tx, rx) = mpsc::channel();
        *self.launcher.lock().unwrap() = Some(tx);
        rx
    }
    pub fn open_launcher(&self, query: Option<String>) -> bool {
        let mut launcher = self.launcher.lock().unwrap();
        if launcher.as_ref().is_some_and(|tx| tx.send(query).is_ok()) {
            self.notifier.signal();
            return true;
        }
        *launcher = None;
        false
    }
    pub fn set_presentation(&self, presentation: Presentation) {
        if let Some(socket) = &self.remote {
            let _ = crate::ipc::send(socket, &json!({"op":"presentation", "value":presentation}));
        }
        let mut current = self.presentation.lock().unwrap();
        if *current == presentation {
            return;
        }
        *current = presentation;
        drop(current);
        self.notifier.signal();
    }
    pub fn wait_for_change(&self, after: u64) -> Option<u64> {
        let generation = self.notifier.generation.lock().unwrap();
        let generation = self
            .notifier
            .changed
            .wait_while(generation, |n| {
                *n == after && !self.notifier.closed.load(Ordering::Acquire)
            })
            .unwrap();
        (!self.notifier.closed.load(Ordering::Acquire)).then_some(*generation)
    }
    pub fn dispatch(&self, control: Control) {
        let _ = self.tx.send(Message::Request(Request {
            control,
            reply: None,
        }));
    }
    pub fn wake_ui(&self) {
        self.notifier.signal();
    }
    pub fn request(&self, control: Control) -> Result<Value> {
        if let Some(socket) = &self.remote {
            let result = crate::ipc::send(socket, &control)?;
            let fresh: Snapshot =
                serde_json::from_value(crate::ipc::send(socket, &json!({"op":"snapshot"}))?)?;
            self.accept_snapshot(fresh);
            return Ok(result);
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Message::Request(Request {
                control,
                reply: Some(tx),
            }))
            .context("FlowWM has stopped")?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("FlowWM did not respond")?
            .map_err(anyhow::Error::msg)
    }
    pub fn shutdown(&self) {
        if self.remote.is_some() {
            self.notifier.closed.store(true, Ordering::Release);
            self.notifier.signal();
            if let Some(stream) = self.remote_stream.lock().unwrap().take() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            return;
        }
        self.notifier.closed.store(true, Ordering::Release);
        self.notifier.signal();
        self.dispatch(Control::Shutdown);
        if let Some(join) = self.join.lock().unwrap().take() {
            let _ = join.join();
        }
    }
}

mod compatibility;
use compatibility::Runtime;
