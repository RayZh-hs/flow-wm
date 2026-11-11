use crate::{model::*, store::Store};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const AGENT_SCHEMA: &str = r#"{"type":"object","properties":{"outcome":{"type":"string","enum":["success","issue"]},"summary":{"type":"string"},"artifacts":{"type":"array","items":{"type":"string"}}},"required":["outcome","summary","artifacts"],"additionalProperties":false}"#;
const GRAPH_RUN: &str = "\0graph";

#[derive(Debug, Deserialize)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    pub summary: String,
    pub artifacts: Vec<String>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    Success,
    Issue,
}
pub fn agent_result(output: &str) -> Result<AgentResult> {
    serde_json::from_str(output).context("Agent did not return the required success/issue outcome")
}

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
    Output {
        id: String,
        generation: u64,
        kind: String,
        line: String,
    },
    Finished {
        id: String,
        generation: u64,
        success: bool,
        error: Option<String>,
    },
}
struct Run {
    generation: u64,
    cancel: Arc<AtomicBool>,
    metric_due: Option<Instant>,
    writer: bool,
}

struct WindowProcess {
    session: String,
    window: u64,
    pid: i32,
    token: u64,
    pending: bool,
}

#[derive(Clone)]
pub struct Engine {
    pub performance: crate::performance::Performance,
    remote: Option<PathBuf>,
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
                        flow_builtins::local::registry(limits.clone()).expect("Builtin registry"),
                    );
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

struct Runtime {
    remotes: std::collections::BTreeMap<String, crate::remote::Remote>,
    network_due: Instant,
    session_watches: HashSet<String>,
    desktop_rows: HashMap<u64, Option<u64>>,
    limits: flow_builtins::local::Limits,
    orchestrator: flow_core::orchestrator::Orchestrator,
    state: State,
    store: Store,
    runs: HashMap<String, Run>,
    graph_active: bool,
    graph_root: Option<String>,
    graph_exit: Option<String>,
    graph_pending: VecDeque<(String, Option<(String, Value)>)>,
    signal_pending: HashMap<String, VecDeque<crate::graph::Activation>>,
    signal_args: HashMap<String, crate::graph::Activation>,
    suppressed_windows: HashSet<u64>,
    pending_deletions: HashSet<String>,
    process_monitor: Option<crate::processes::ProcessMonitor>,
    window_processes: HashMap<String, WindowProcess>,
    exit_pending: HashSet<String>,
    generation: u64,
    revision: u64,
    error: Option<String>,
    dirty: bool,
    tx: mpsc::Sender<Message>,
    ui: mpsc::Sender<UiCommand>,
    last_save: Instant,
    needs_save: bool,
    notifier: Arc<Notifier>,
}
impl Runtime {
    fn work(mut self, rx: mpsc::Receiver<Message>, shared: Arc<Mutex<Snapshot>>) {
        loop {
            let network_pending = !self.orchestrator.state.forwarding.is_empty()
                || !self.orchestrator.state.pending_remote.is_empty()
                || self.orchestrator.state.transfers.values().any(|t| {
                    t.source == self.orchestrator.state.host_id
                        && !self.orchestrator.state.transfer_acks.contains(&t.id)
                        && matches!(
                            t.phase,
                            flow_core::domain::TransferPhase::Prepared
                                | flow_core::domain::TransferPhase::Committed
                        )
                        && self.orchestrator.state.workflows.contains_key(&t.workflow)
                });
            let next_metric = self
                .runs
                .values()
                .filter_map(|r| r.metric_due)
                .map(|t| t.saturating_duration_since(Instant::now()))
                .min()
                .unwrap_or(Duration::from_secs(60));
            let next_metric = if network_pending {
                next_metric.min(self.network_due.saturating_duration_since(Instant::now()))
            } else {
                next_metric
            };
            let timeout = if self.needs_save {
                next_metric.min(Duration::from_millis(400).saturating_sub(self.last_save.elapsed()))
            } else {
                next_metric
            };
            match rx.recv_timeout(timeout) {
                Ok(Message::Completion(completion)) => {
                    let invocation = completion.invocation.clone();
                    let accepted = self.orchestrator.state.complete(
                        &completion.invocation,
                        completion.generation,
                        completion.result,
                        completion.status,
                    );
                    if accepted.unwrap_or(false) {
                        if let Some(execution) = self.orchestrator.state.executions.get(&invocation)
                            && let Some(node) = self.state.node_mut(&execution.node.0)
                        {
                            node.status = match execution.status {
                                flow_core::domain::ExecutionStatus::Complete => Status::Done,
                                flow_core::domain::ExecutionStatus::Interrupted => Status::Stopped,
                                _ => Status::Failed,
                            };
                            if let Some(values) = execution.result.as_object() {
                                node.flow
                                    .values
                                    .extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
                            }
                            node.output = execution
                                .result
                                .get("stdout")
                                .and_then(Value::as_str)
                                .map(String::from)
                                .unwrap_or_else(|| execution.result.to_string());
                        }
                        if let Some(destination) = self
                            .orchestrator
                            .state
                            .forwarding_targets
                            .get(&invocation)
                            .cloned()
                            && let Some(execution) =
                                self.orchestrator.state.executions.get(&invocation).cloned()
                        {
                            self.orchestrator.state.forwarding.push_back(
                                flow_core::transfer::ForwardedCompletion {
                                    destination,
                                    execution,
                                },
                            );
                            let _ = self.checkpoint();
                        }
                    }
                    self.state.orchestration = Some(self.orchestrator.state.clone());
                    self.dirty = true;
                }
                Ok(Message::Request(Request {
                    control: Control::Shutdown,
                    reply,
                })) => {
                    let owned_invocations = self
                        .orchestrator
                        .state
                        .executions
                        .values()
                        .any(|e| e.active() && e.host == self.orchestrator.state.host_id);
                    for execution in self
                        .orchestrator
                        .state
                        .executions
                        .values()
                        .filter(|e| e.active())
                    {
                        if let Ok(provider) =
                            self.orchestrator.registry.provider(&execution.provider)
                        {
                            let _ = provider.interrupt(&execution.handle);
                        }
                    }
                    for run in self.runs.values() {
                        run.cancel.store(true, Ordering::Relaxed);
                    }
                    for node in &mut self.state.nodes {
                        if !node.desktop_window
                            && matches!(node.status, Status::Running | Status::Queued)
                        {
                            node.status = Status::Stopped;
                            node.log(
                                "system",
                                "FlowWM shut down; background processes were stopped.",
                            );
                        }
                    }
                    for execution in self
                        .orchestrator
                        .state
                        .executions
                        .values_mut()
                        .filter(|e| e.active())
                    {
                        if execution.host == self.orchestrator.state.host_id {
                            execution.status = flow_core::domain::ExecutionStatus::Interrupted;
                            execution.result = json!({"error":"Runtime host stopped"});
                        }
                    }
                    self.state.orchestration = Some(self.orchestrator.state.clone());
                    if let Err(error) = self.store.save(&self.state) {
                        eprintln!("Save state: {error:#}");
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(json!({"ok":true})));
                    }
                    // Process workers observe cancellation and reap their process group.
                    if !self.runs.is_empty() || owned_invocations {
                        thread::sleep(Duration::from_millis(
                            self.limits.interrupt_grace_ms + self.limits.terminate_grace_ms + 200,
                        ));
                    }
                    return;
                }
                Ok(Message::Request(Request {
                    control: Control::Status,
                    reply,
                })) => {
                    if let Some(reply) = reply {
                        let _ = reply
                            .send(serde_json::to_value(&self.state).map_err(|e| e.to_string()));
                    }
                }
                Ok(Message::Request(Request {
                    control: Control::Ui { command },
                    reply,
                })) => {
                    let result = self
                        .ui
                        .send(command)
                        .map(|_| json!({"ok":true}))
                        .map_err(|e| e.to_string());
                    self.notifier.signal();
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                Ok(Message::Request(Request {
                    control: Control::SyncDesktop { mut desktop },
                    reply,
                })) => {
                    if desktop.connected && !desktop.session.is_empty() {
                        self.watch_desktop_session(&desktop.session);
                        let _ = self.orchestrator.state.workspaces.activate(
                            &format!("{}:desktop", self.orchestrator.state.host_id.0).into(),
                            &"desktop".into(),
                            desktop.session.clone(),
                        );
                        self.state.orchestration = Some(self.orchestrator.state.clone());
                    }
                    self.suppressed_windows
                        .retain(|id| desktop.windows.iter().any(|w| w.id == *id));
                    desktop
                        .windows
                        .retain(|w| !self.suppressed_windows.contains(&w.id));
                    if desktop.connected && desktop.windows_ready && desktop.workspaces_ready {
                        self.prune_window_processes(Some(&desktop.session));
                    }
                    // A shared application can outlive a closed window. Only
                    // keep its card when an exit workflow still needs the source.
                    let consumers: HashSet<_> = self
                        .state
                        .connections
                        .iter()
                        .map(|e| e.source.node.as_str())
                        .chain(
                            self.state
                                .nodes
                                .iter()
                                .flat_map(|n| n.dependencies.iter().map(String::as_str)),
                        )
                        .chain(self.state.watchers.iter().map(|w| w.source.as_str()))
                        .collect();
                    let retained = self
                        .window_processes
                        .iter()
                        .filter(|(id, p)| p.pending && consumers.contains(id.as_str()))
                        .map(|(id, _)| id.clone())
                        .chain(self.exit_pending.iter().cloned())
                        .collect();
                    let rows = desktop
                        .windows
                        .iter()
                        .map(|w| (w.id, w.workspace_id))
                        .collect::<HashMap<_, _>>();
                    let moved = self.desktop_rows != rows;
                    self.desktop_rows = rows;
                    let mut changed = crate::groups::synchronize_with_policy(
                        &mut self.state,
                        &desktop,
                        &retained,
                        self.limits.desktop_branches_follow_rows,
                    );
                    changed |= moved;
                    changed |= self.observe_window_processes(&desktop);
                    if changed {
                        self.sync_domain();
                        self.revision += 1;
                        *shared.lock().unwrap() = Snapshot {
                            state: Arc::new(self.state.clone()),
                            revision: self.revision,
                            error: self.error.clone(),
                        };
                        self.needs_save = true;
                        self.notifier.signal();
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(json!({"changed": changed})));
                    }
                }
                Ok(Message::Request(Request {
                    control: Control::Orchestrate { request },
                    reply,
                })) => {
                    let previous = self.orchestrator.state.revision;
                    let result = self.apply(Control::Orchestrate { request });
                    if self.orchestrator.state.revision != previous {
                        self.dirty = true;
                        self.revision += 1;
                        *shared.lock().unwrap() = Snapshot {
                            state: Arc::new(self.state.clone()),
                            revision: self.revision,
                            error: self.error.clone(),
                        };
                        self.notifier.signal();
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result.map_err(|e| e.to_string()));
                    }
                }
                Ok(Message::Request(Request { control, reply })) => {
                    let result = self.apply(control);
                    if result.is_ok() {
                        self.sync_domain();
                    }
                    if let Err(error) = &result {
                        self.error = Some(format!("{error:#}"));
                    }
                    self.dirty = true;
                    self.revision += 1;
                    *shared.lock().unwrap() = Snapshot {
                        state: Arc::new(self.state.clone()),
                        revision: self.revision,
                        error: self.error.clone(),
                    };
                    self.notifier.signal();
                    if let Some(reply) = reply {
                        let _ = reply.send(result.map_err(|e| format!("{e:#}")));
                    }
                }
                Ok(Message::Output {
                    id,
                    generation,
                    kind,
                    line,
                }) => {
                    if self
                        .runs
                        .get(&id)
                        .is_some_and(|r| r.generation == generation)
                    {
                        self.output(&id, &kind, &line);
                        self.dirty = true;
                    }
                }
                Ok(Message::Finished {
                    id,
                    generation,
                    success,
                    error,
                }) => {
                    if self
                        .runs
                        .get(&id)
                        .is_some_and(|r| r.generation == generation)
                    {
                        self.finish(&id, success, error);
                        self.complete_signal(&id);
                        self.dirty = true;
                    }
                }
                Ok(Message::GraphFinished {
                    id,
                    generation,
                    result,
                }) => {
                    if let Some(id) = self.graph_exit.take() {
                        self.exit_pending.remove(&id);
                    }
                    self.graph_active = false;
                    self.graph_root = None;
                    if self
                        .runs
                        .get(GRAPH_RUN)
                        .is_some_and(|r| r.generation == generation)
                    {
                        let stopped = self
                            .runs
                            .remove(GRAPH_RUN)
                            .unwrap()
                            .cancel
                            .load(Ordering::Relaxed);
                        if !stopped {
                            self.finish_graph(&id, result);
                        }
                    }
                    self.dirty = true;
                }
                Ok(Message::WindowProcessExited { token, result }) => {
                    self.window_process_exited(token, result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if network_pending && Instant::now() >= self.network_due {
                        self.reconcile_network();
                        self.network_due = Instant::now() + Duration::from_secs(2);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            self.prune_window_processes(None);
            self.schedule();
            if self.dirty {
                self.revision += 1;
                *shared.lock().unwrap() = Snapshot {
                    state: Arc::new(self.state.clone()),
                    revision: self.revision,
                    error: self.error.clone(),
                };
                self.notifier.signal();
                self.needs_save = true;
                self.dirty = false;
            }
            if self.needs_save && self.last_save.elapsed() >= Duration::from_millis(400) {
                match self.store.save(&self.state) {
                    Ok(()) => self.needs_save = false,
                    Err(error) => {
                        self.error = Some(format!("Save state: {error:#}"));
                        self.revision += 1;
                        *shared.lock().unwrap() = Snapshot {
                            state: Arc::new(self.state.clone()),
                            revision: self.revision,
                            error: self.error.clone(),
                        };
                        self.notifier.signal();
                    }
                }
                self.last_save = Instant::now();
            }
        }
    }
    fn prune_window_processes(&mut self, session: Option<&str>) {
        self.window_processes.retain(|id, process| {
            let keep = self.state.node(id).is_some_and(|n| {
                n.kind == NodeKind::Window && crate::graph::type_id(n) == "window"
            }) && session.is_none_or(|s| s == process.session);
            if !keep
                && process.pending
                && let Some(monitor) = &mut self.process_monitor
            {
                monitor.unwatch(process.token);
            }
            keep
        });
        self.exit_pending.retain(|id| self.state.node(id).is_some());
    }

    fn observe_window_processes(&mut self, desktop: &crate::niri::Desktop) -> bool {
        if !desktop.connected || !desktop.windows_ready || !desktop.workspaces_ready {
            return false;
        }
        let Some(monitor) = &mut self.process_monitor else {
            return false;
        };
        let mut changed = false;
        for window in &desktop.windows {
            let Some(pid) = window.pid else {
                continue;
            };
            let Some(node) = self.state.nodes.iter_mut().find(|n| {
                n.kind == NodeKind::Window
                    && crate::graph::type_id(n) == "window"
                    && n.window_id == Some(window.id)
            }) else {
                continue;
            };
            if self.window_processes.get(&node.id).is_some_and(|p| {
                p.session == desktop.session && p.window == window.id && p.pid == pid
            }) {
                continue;
            }
            if let Some(old) = self.window_processes.remove(&node.id).filter(|p| p.pending) {
                monitor.unwatch(old.token);
            }
            self.generation += 1;
            let token = self.generation;
            let result = monitor.watch(token, pid);
            let pending = result.is_ok();
            if let Err(error) = result {
                node.log(
                    "error",
                    format!("Cannot observe application process {pid}: {error}"),
                );
            } else {
                node.status = Status::Running;
                node.started_at = Some(now());
                node.finished_at = None;
            }
            self.window_processes.insert(
                node.id.clone(),
                WindowProcess {
                    session: desktop.session.clone(),
                    window: window.id,
                    pid,
                    token,
                    pending,
                },
            );
            changed = true;
        }
        changed
    }

    fn window_process_exited(&mut self, token: u64, result: std::result::Result<u8, String>) {
        let Some((id, process)) = self
            .window_processes
            .iter_mut()
            .find(|(_, p)| p.token == token && p.pending)
        else {
            return;
        };
        process.pending = false;
        let id = id.clone();
        let Some(node) = self.state.node_mut(&id) else {
            return;
        };
        self.dirty = true;
        match result {
            Ok(exit_code) => {
                node.finished_at = Some(now());
                node.status = if exit_code == 0 {
                    Status::Done
                } else {
                    Status::Failed
                };
                node.log(
                    "system",
                    format!("Application process exited with code {exit_code}."),
                );
                self.exit_pending.insert(id.clone());
                self.graph_pending.push_back((
                    id,
                    Some(("on_exit".into(), json!({"exit_code": exit_code}))),
                ));
            }
            Err(error) => {
                node.finished_at = Some(now());
                node.status = Status::Stopped;
                node.log(
                    "error",
                    format!("Cannot read application exit status: {error}"),
                );
            }
        }
    }

    fn cancel_destroyed(&self) {
        for node in &self.state.nodes {
            if self
                .orchestrator
                .state
                .destroyed
                .contains(&node.workspace.clone().into())
                && let Some(run) = self.runs.get(&node.id)
            {
                run.cancel.store(true, Ordering::Release);
            }
        }
    }
    fn sync_domain(&mut self) {
        if let Ok((envelope, _)) = flow_core::migration::Envelope::from_v1(self.state.clone()) {
            self.orchestrator.state = envelope.runtime;
            self.orchestrator.state.revision += 1;
            self.state.orchestration = Some(self.orchestrator.state.clone());
        }
    }
    fn watch_desktop_session(&mut self, session: &str) {
        if !std::path::Path::new(session).is_absolute()
            || !self.session_watches.insert(session.into())
        {
            return;
        }
        let session = session.to_string();
        let tx = self.tx.clone();
        let host = self.orchestrator.state.host_id.0.clone();
        thread::spawn(move || {
            use std::os::unix::net::UnixStream;
            if let Ok(mut stream) = UnixStream::connect(&session)
                && writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&niri_ipc::Request::EventStream).unwrap()
                )
                .is_ok()
            {
                let _ = std::io::copy(&mut stream, &mut std::io::sink());
                let _ = tx.send(Message::Request(Request {
                    control: Control::EndSession {
                        workspace: format!("{host}:desktop").into(),
                        incarnation: session,
                    },
                    reply: None,
                }));
            }
        });
    }
    fn remote_request(
        &self,
        host: &str,
        command: flow_core::protocol::Command,
        caller: Option<flow_core::domain::WorkflowId>,
        request_id: String,
    ) -> Result<Value> {
        self.remotes
            .get(host)
            .context("Remote host is disconnected or not configured")?
            .request(&flow_core::protocol::Request {
                protocol: 2,
                id: request_id,
                expected_revision: None,
                caller,
                command,
            })
    }
    fn discover_host(&mut self, host: &str) -> Result<()> {
        let value = self.remote_request(
            host,
            flow_core::protocol::Command::Snapshot,
            None,
            crate::model::id(),
        )?;
        let remote: flow_core::domain::RuntimeState = serde_json::from_value(value)?;
        ensure!(
            remote.host_id.0 == host,
            "Configured host ID differs from peer identity"
        );
        for definition in remote.workspaces.definitions() {
            self.orchestrator
                .state
                .workspaces
                .register(definition.clone())?;
            if let Some(available) = remote
                .workspaces
                .availability(&definition.id)
                .filter(|a| a.active)
            {
                self.orchestrator.state.workspaces.activate(
                    &definition.id,
                    &definition.provider,
                    available.incarnation.clone(),
                )?;
            }
        }
        for (id, workflow) in remote.workflows {
            if workflow.authority.0 == host && !self.orchestrator.state.destroyed.contains(&id) {
                self.orchestrator
                    .state
                    .workflows
                    .entry(id)
                    .or_insert(workflow);
            }
        }
        self.state.orchestration = Some(self.orchestrator.state.clone());
        Ok(())
    }
    fn checkpoint(&mut self) -> Result<()> {
        self.state.orchestration = Some(self.orchestrator.state.clone());
        self.store.save(&self.state)
    }
    fn continue_transfer(&mut self, id: &flow_core::domain::TransferId) -> Result<()> {
        use flow_core::{domain::TransferPhase, protocol::Command};
        let record = self
            .orchestrator
            .state
            .transfers
            .get(id)
            .context("Transfer not found")?
            .clone();
        if self.orchestrator.state.transfer_acks.contains(id) {
            return Ok(());
        }
        let host = record.destination.host().to_string();
        if record.phase == TransferPhase::Prepared {
            let bundle = self.orchestrator.state.transfer_bundle(id)?;
            self.remote_request(&host, Command::StageTransfer { bundle }, None, id.0.clone())?;
            self.orchestrator.state.commit_outgoing(id)?;
            self.checkpoint()?;
        }
        let bundle = self.orchestrator.state.transfer_bundle(id)?;
        self.remote_request(
            &host,
            Command::CommitTransfer { bundle },
            None,
            id.0.clone(),
        )?;
        self.orchestrator.state.transfer_acks.insert(id.clone());
        self.checkpoint()?;
        Ok(())
    }
    fn reconcile_network(&mut self) {
        use flow_core::{domain::TransferPhase, protocol::Command};
        let transfers: Vec<_> = self
            .orchestrator
            .state
            .transfers
            .values()
            .filter(|t| {
                t.source == self.orchestrator.state.host_id
                    && matches!(t.phase, TransferPhase::Prepared | TransferPhase::Committed)
                    && !self.orchestrator.state.transfer_acks.contains(&t.id)
                    && self.orchestrator.state.workflows.contains_key(&t.workflow)
            })
            .map(|t| t.id.clone())
            .take(4)
            .collect();
        for transfer in transfers {
            let _ = self.continue_transfer(&transfer);
        }
        for _ in 0..4 {
            let Some(pending) = self.orchestrator.state.forwarding.front().cloned() else {
                break;
            };
            if self
                .remote_request(
                    &pending.destination.0,
                    Command::ForwardCompletion {
                        execution: pending.execution.clone(),
                    },
                    None,
                    pending.execution.id.0.clone(),
                )
                .is_err()
            {
                break;
            }
            self.orchestrator.state.forwarding.pop_front();
            let _ = self.checkpoint();
        }
        for _ in 0..4 {
            let Some(pending) = self.orchestrator.state.pending_remote.front().cloned() else {
                break;
            };
            if self
                .remote_request(
                    &pending.host.0,
                    pending.command,
                    pending.caller,
                    crate::model::id(),
                )
                .is_err()
            {
                break;
            }
            self.orchestrator.state.pending_remote.pop_front();
            let _ = self.checkpoint();
        }
        self.dirty = true;
    }
    fn apply(&mut self, control: Control) -> Result<Value> {
        self.error = None;
        if self.graph_active {
            ensure!(
                !matches!(
                    &control,
                    Control::Create { .. }
                        | Control::Update { .. }
                        | Control::MoveToWorkspace { .. }
                        | Control::Connect { .. }
                        | Control::ConnectPins { .. }
                        | Control::Disconnect { .. }
                        | Control::DisconnectPins { .. }
                        | Control::Delete { .. }
                        | Control::RegisterType { .. }
                        | Control::RegisterCast { .. }
                        | Control::RegisterNode { .. }
                        | Control::Instantiate { .. }
                        | Control::Paste { .. }
                        | Control::PublishProperties { .. }
                        | Control::ExposeOutput { .. }
                        | Control::ClearHistory
                ),
                "Stop the active graph evaluation before editing its nodes or connections"
            );
        }
        match control {
            Control::ConnectHost { host, remote } => {
                self.remotes.insert(host.clone(), remote);
                self.discover_host(&host)?;
            }
            Control::EndSession {
                workspace,
                incarnation,
            } => {
                let cleanup = self
                    .orchestrator
                    .state
                    .end_session(&workspace, &incarnation)?;
                for execution in cleanup {
                    if execution.host != self.orchestrator.state.host_id {
                        self.orchestrator.state.pending_remote.push_back(
                            flow_core::transfer::RemoteCommand {
                                host: execution.host,
                                caller: None,
                                command: flow_core::protocol::Command::OriginInterrupt {
                                    invocation: execution.id,
                                    generation: execution.generation,
                                },
                            },
                        );
                    } else if let Ok(provider) =
                        self.orchestrator.registry.provider(&execution.provider)
                    {
                        let _ = provider.interrupt(&execution.handle);
                    }
                }
                self.cancel_destroyed();
                crate::projection::apply(&mut self.state, &self.orchestrator.state);
                self.state.orchestration = Some(self.orchestrator.state.clone());
            }
            Control::Orchestrate { request } => {
                ensure!(
                    request.protocol == flow_core::protocol::ORCHESTRATOR_VERSION,
                    "Unsupported orchestrator protocol"
                );
                if let Some(revision) = request.expected_revision {
                    ensure!(
                        revision == self.orchestrator.state.revision,
                        "Revision conflict; reconnect with a snapshot"
                    );
                }
                // Refresh only compatibility projections; retain ownership and active executions.
                let (envelope, _) = flow_core::migration::Envelope::from_v1(self.state.clone())?;
                self.orchestrator.state = envelope.runtime;

                use flow_core::protocol::Command as CoreCommand;
                if let CoreCommand::Move {
                    workflow,
                    workspace,
                } = &request.command
                    && workspace.host() != self.orchestrator.state.host_id.0
                {
                    self.orchestrator
                        .state
                        .lookup(workflow, request.caller.as_ref())?;
                    if !self
                        .orchestrator
                        .state
                        .transfers
                        .contains_key(&request.id.clone().into())
                    {
                        self.discover_host(workspace.host())?;
                        self.orchestrator.state.prepare_transfer(
                            request.id.clone().into(),
                            workflow,
                            workspace.clone(),
                        )?;
                        self.checkpoint()?;
                    }
                    let result = self.continue_transfer(&request.id.clone().into());
                    self.state.orchestration = Some(self.orchestrator.state.clone());
                    return Ok(serde_json::to_value(flow_core::protocol::Reply {
                        protocol: 2,
                        id: request.id,
                        revision: self.orchestrator.state.revision,
                        result: result.as_ref().ok().map(|_| Value::Null),
                        error: result.err().map(|e| flow_core::protocol::Error {
                            code: "handoff_pending".into(),
                            message: e.to_string(),
                            retryable: true,
                        }),
                    })?);
                }
                let target = match &request.command {
                    CoreCommand::Run { workflow, .. }
                    | CoreCommand::ReplaceGraph { workflow, .. }
                    | CoreCommand::Receive { workflow, .. } => Some(workflow.clone()),
                    CoreCommand::Broadcast { delivery } => Some(delivery.target.clone()),
                    CoreCommand::Acknowledge { .. } => request.caller.clone(),
                    _ => None,
                };
                if let Some(target) = target
                    && let Some(workflow) = self.orchestrator.state.workflows.get(&target)
                    && workflow.authority != self.orchestrator.state.host_id
                {
                    let host = workflow.authority.0.clone();
                    let value = self.remote_request(
                        &host,
                        request.command,
                        request.caller,
                        request.id.clone(),
                    )?;
                    return Ok(serde_json::to_value(flow_core::protocol::Reply {
                        protocol: 2,
                        id: request.id,
                        revision: self.orchestrator.state.revision,
                        result: Some(value),
                        error: None,
                    })?);
                }
                self.orchestrator.storage = Some(Arc::new(crate::store::RuntimeCheckpoint {
                    store: self.store.clone(),
                    compatibility: self.state.clone(),
                }));
                let tx = self.tx.clone();
                let read_only = matches!(
                    &request.command,
                    flow_core::protocol::Command::Snapshot
                        | flow_core::protocol::Command::Workspaces
                        | flow_core::protocol::Command::Negotiate { .. }
                        | flow_core::protocol::Command::Subscribe { .. }
                );
                let result = self.orchestrator.execute(
                    request.command,
                    request.caller.as_ref(),
                    Arc::new(move |completion| {
                        let _ = tx.send(Message::Completion(completion));
                    }),
                );
                if !read_only {
                    self.cancel_destroyed();
                    crate::projection::apply(&mut self.state, &self.orchestrator.state);
                }
                self.state.orchestration = Some(self.orchestrator.state.clone());
                if !read_only {
                    self.store.save(&self.state)?;
                }
                return Ok(serde_json::to_value(flow_core::protocol::Reply {
                    protocol: 2,
                    id: request.id,
                    revision: self.orchestrator.state.revision,
                    result: result.as_ref().ok().cloned(),
                    error: result.err().map(|e| flow_core::protocol::Error {
                        code: "command_failed".into(),
                        message: e.to_string(),
                        retryable: false,
                    }),
                })?);
            }
            Control::Arrange { positions } => {
                for (id, p) in &positions {
                    let node = self.state.node(id).context("Node not found")?;
                    ensure!(
                        node.workspace == self.state.selected_workspace
                            && p.x.is_finite()
                            && p.y.is_finite(),
                        "Invalid arrangement"
                    );
                }
                for (id, p) in positions {
                    self.state.node_mut(&id).unwrap().position = p;
                }
                let workspace = self.state.selected_workspace.clone();
                self.state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .unwrap()
                    .graph_arranged = true;
            }
            Control::Paste { mut recipe, offset } => {
                ensure!(
                    self.state.nodes.len() + recipe.nodes.len() <= 2000,
                    "Node limit reached"
                );
                ensure!(
                    offset.x.is_finite() && offset.y.is_finite(),
                    "Invalid paste offset"
                );
                for node in &mut recipe.nodes {
                    node.position.x += offset.x;
                    node.position.y += offset.y;
                }
                let workspace = self.state.selected_workspace.clone();
                let cwd = self.state.workspace().cwd.clone();
                let ids = self.state.instantiate(&recipe, &workspace, cwd)?;
                return Ok(json!({"ids": ids}));
            }
            Control::Alert {
                id,
                message,
                critical,
                notify,
            } => self.raise_alert(&id, &message, critical, notify)?,
            Control::ClearAlert { id } => {
                self.state
                    .node_mut(&id)
                    .context("Node not found")?
                    .flow
                    .properties
                    .remove("_alert");
            }
            Control::Pins { id } => {
                let node = self.state.node(&id).context("Node not found")?;
                return Ok(
                    json!({"inputs":crate::graph::pins(&self.state, node, false), "outputs":crate::graph::pins(&self.state, node, true)}),
                );
            }
            Control::ExposeOutput { id, pin } => {
                let mut candidate = self.state.clone();
                let node = candidate.node_mut(&id).context("Node not found")?;
                node.flow.outputs.retain(|p| p.name != pin.name);
                node.flow.outputs.push(pin);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::PublishProperties { id, properties } => {
                let mut candidate = self.state.clone();
                candidate
                    .node_mut(&id)
                    .context("Node not found")?
                    .flow
                    .values
                    .extend(properties);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::ConnectPins { connection } => {
                crate::graph::connect(&mut self.state, connection)?
            }
            Control::DisconnectPins { connection } => {
                self.state.connections.retain(|e| *e != connection);
            }
            Control::Evaluate { id } => {
                self.state.node(&id).context("Node not found")?;
                self.graph_pending.push_back((id, None));
            }
            Control::Emit { source, args } => {
                let node = self.state.node(&source.node).context("Node not found")?;
                let pin = crate::graph::pins(&self.state, node, true)
                    .into_iter()
                    .find(|p| p.name == source.pin)
                    .context("Output pin not found")?;
                ensure!(pin.ty.is_signal(), "Expected a signal output");
                crate::graph::check_value(&self.state, &args, &pin.ty)?;
                crate::graph::signal_environment(&args)?;
                self.graph_pending
                    .push_back((source.node, Some((source.pin, args))));
            }
            Control::Activate { id, args } => {
                self.state.node(&id).context("Node not found")?;
                crate::graph::signal_environment(&args)?;
                self.graph_pending
                    .push_back((id, Some((String::new(), args))));
            }
            Control::RegisterType { definition } => {
                let mut candidate = self.state.clone();
                candidate.types.push(definition);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::RegisterCast { definition } => {
                let mut candidate = self.state.clone();
                candidate.casts.push(definition);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::RegisterNode { definition } => {
                let mut candidate = self.state.clone();
                candidate.node_types.push(definition);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::Status => return Ok(serde_json::to_value(&self.state)?),
            Control::Create { mut node } => {
                ensure!(self.state.nodes.len() < 2000, "Node limit reached");
                ensure!(self.state.node(&node.id).is_none(), "Duplicate node ID");
                node.cwd = node
                    .cwd
                    .canonicalize()
                    .context("Working directory does not exist")?;
                ensure!(node.cwd.is_dir(), "Working directory is not a directory");
                node.status = Status::Idle;
                node.logs.clear();
                if node.kind == NodeKind::Human {
                    node.output = node.command.clone();
                } else {
                    node.output.clear();
                }
                node.started_at = None;
                node.finished_at = None;
                let id = node.id.clone();
                let mut candidate = self.state.clone();
                candidate.nodes.push(*node);
                candidate.validate()?;
                self.state = candidate;
                return Ok(json!({"id":id}));
            }
            Control::Update { mut node } => {
                let original = self.state.node(&node.id).context("Node not found")?;
                ensure!(
                    (!original.status.active() || original.kind == NodeKind::Window)
                        && !self.runs.contains_key(&node.id),
                    "Stop the node before editing it"
                );
                node.cwd = node
                    .cwd
                    .canonicalize()
                    .context("Working directory does not exist")?;
                ensure!(node.cwd.is_dir(), "Working directory is not a directory");
                ensure!(
                    node.title.len() <= 200 && node.command.len() <= 64_000,
                    "Node text is too long"
                );
                node.status = original.status;
                node.output = original.output.clone();
                node.logs = original.logs.clone();
                node.window_id = original.window_id;
                node.window_match = original.window_match.clone();
                node.desktop_window = original.desktop_window;
                node.workspace = original.workspace.clone();
                node.column = original.column.clone();
                node.column_mode = original.column_mode;
                if original.desktop_window && node.title != original.title {
                    node.flow
                        .properties
                        .insert("display_title".into(), json!(node.title));
                }
                if let Some(alert) = original.flow.properties.get("_alert") {
                    node.flow.properties.insert("_alert".into(), alert.clone());
                } else {
                    node.flow.properties.remove("_alert");
                }
                let node_id = node.id.clone();
                let mut candidate = self.state.clone();
                *candidate.node_mut(&node_id).unwrap() = *node;
                candidate.validate()?;
                self.state = candidate;
            }
            Control::Move { id, position } => {
                ensure!(
                    position.x.is_finite() && position.y.is_finite(),
                    "Invalid position"
                );
                self.state.node_mut(&id).context("Node not found")?.position = position;
            }
            Control::MoveToWorkspace { id, workspace } => {
                ensure!(
                    self.state.workspaces.iter().any(|w| w.id == workspace),
                    "Workspace not found"
                );
                let node = self.state.node(&id).context("Node not found")?;
                ensure!(
                    !node.status.active() && !self.runs.contains_key(&id),
                    "Stop the node before moving it"
                );
                ensure!(
                    node.dependencies.is_empty()
                        && !self
                            .state
                            .connections
                            .iter()
                            .any(|e| e.source.node == id || e.target.node == id)
                        && !self
                            .state
                            .nodes
                            .iter()
                            .any(|n| n.dependencies.contains(&id)),
                    "Disconnect this node before moving it"
                );
                ensure!(
                    !self
                        .state
                        .watchers
                        .iter()
                        .any(|w| w.source == id || w.target == id),
                    "Remove its watcher before moving it"
                );
                self.state.node_mut(&id).unwrap().workspace = workspace;
            }
            Control::Connect { source, target } => {
                let a = self.state.node(&source).context("Source node not found")?;
                let b = self.state.node(&target).context("Target node not found")?;
                if a.kind.primitive() || b.kind.primitive() {
                    let outputs = crate::graph::pins(&self.state, a, true);
                    let inputs = crate::graph::pins(&self.state, b, false);
                    let (output, input) = outputs
                        .iter()
                        .find_map(|out| {
                            inputs
                                .iter()
                                .find(|input| {
                                    out.ty.is_signal() == input.ty.is_signal()
                                        && crate::graph::can_cast(&self.state, &out.ty, &input.ty)
                                })
                                .map(|input| (out, input))
                        })
                        .context("No compatible pins; choose pins in node parameters")?;
                    crate::graph::connect(
                        &mut self.state,
                        crate::graph::Connection {
                            source: crate::graph::Endpoint {
                                node: source,
                                pin: output.name.clone(),
                            },
                            target: crate::graph::Endpoint {
                                node: target,
                                pin: input.name.clone(),
                            },
                        },
                    )?;
                } else {
                    self.state.connect(&source, &target)?;
                }
            }
            Control::Disconnect { source, target } => {
                let node = self.state.node_mut(&target).context("Node not found")?;
                ensure!(
                    !node.status.active() || node.desktop_window,
                    "Stop the target before changing its inputs"
                );
                node.dependencies.retain(|d| d != &source);
                self.state
                    .connections
                    .retain(|e| e.source.node != source || e.target.node != target);
            }
            Control::Delete { id } => {
                let mut candidate = self.state.clone();
                candidate.node_mut(&id).context("Node not found")?.status = Status::Stopped;
                candidate.remove(&id)?;
                if let Some(run) = self.runs.get(&id) {
                    run.cancel.store(true, Ordering::Relaxed);
                    self.state.node_mut(&id).unwrap().status = Status::Stopped;
                    self.pending_deletions.insert(id);
                    return Ok(json!({"stopping":true}));
                }
                if let Some(node) = self
                    .state
                    .node(&id)
                    .filter(|n| n.desktop_window || n.kind == NodeKind::Window)
                    && let Some(window) = node.window_id
                {
                    crate::niri::request(niri_ipc::Request::Action(
                        niri_ipc::Action::CloseWindow { id: Some(window) },
                    ))?;
                    // Focus/title events can arrive before the asynchronous close acknowledgement.
                    self.suppressed_windows.insert(window);
                }
                self.state = candidate;
            }
            Control::ColumnMode { id, mode } => {
                let node = self.state.node(&id).context("Node not found")?;
                let column = node.column.clone().unwrap_or_else(|| node.id.clone());
                let workspace = node.workspace.clone();
                for node in &mut self.state.nodes {
                    if node.workspace == workspace
                        && node.column.as_ref().unwrap_or(&node.id) == &column
                    {
                        node.column_mode = mode;
                    }
                }
            }
            Control::RowView { workspace, view } => {
                self.state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .context("Workspace not found")?
                    .view = view;
                for node in &mut self.state.nodes {
                    if node.workspace == workspace {
                        node.column_mode = match view {
                            RowView::Nodes => ColumnMode::Node,
                            RowView::Windows => ColumnMode::Window,
                        };
                    }
                }
            }
            Control::ClearHistory => {
                let ids: Vec<_> = self
                    .state
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.workspace == self.state.selected_workspace
                            && (n.desktop_window
                                || (n.kind == NodeKind::Window && n.window_match.is_some()))
                            && n.window_id.is_none()
                    })
                    .map(|n| n.id.clone())
                    .collect();
                for id in ids {
                    // A closed input still used by an executing workflow is retained.
                    let _ = self.state.remove(&id);
                }
            }
            Control::Chat { id, text } => {
                ensure!(
                    !text.trim().is_empty() && text.len() <= 64_000,
                    "Enter a message (up to 64 KB)"
                );
                ensure!(!self.runs.contains_key(&id), "This agent is still working");
                let node = self.state.node_mut(&id).context("Agent node not found")?;
                ensure!(node.kind == NodeKind::Agent, "Select an agent node");
                ensure!(!node.status.active(), "This agent is still working");
                node.chat.push(ChatMessage {
                    role: "user".into(),
                    text: text.clone(),
                });
                if node.chat.len() > 100 {
                    node.chat.drain(..node.chat.len() - 100);
                }
                node.command = text;
                node.status = Status::Queued;
            }
            Control::Run { id, downstream } => {
                if self
                    .state
                    .node(&id)
                    .context("Node not found")?
                    .kind
                    .primitive()
                {
                    let node = self.state.node(&id).unwrap();
                    let signal = crate::graph::pins(&self.state, node, false)
                        .iter()
                        .any(|p| p.ty.is_signal())
                        .then(|| (String::new(), json!({})));
                    self.graph_pending.push_back((id, signal));
                    return Ok(json!({"ok":true}));
                }
                ensure!(self.state.node(&id).is_some(), "Node not found");
                let mut selected = HashSet::from([id.clone()]);
                if downstream {
                    loop {
                        let old = selected.len();
                        for n in &self.state.nodes {
                            if n.dependencies.iter().any(|d| selected.contains(d)) {
                                selected.insert(n.id.clone());
                            }
                        }
                        if selected.len() == old {
                            break;
                        }
                    }
                }
                for id in &selected {
                    ensure!(
                        (!self.state.node(id).unwrap().status.active()
                            || self.state.node(id).unwrap().status == Status::Queued)
                            && !self.runs.contains_key(id),
                        "A selected node is already active"
                    );
                }
                for id in selected {
                    let node = self.state.node_mut(&id).unwrap();
                    node.status = Status::Queued;
                    node.log("system", "Queued. Waiting for workflow inputs.");
                }
            }
            Control::RunWorkspace => {
                let workspace = self.state.selected_workspace.clone();
                let incoming: HashSet<_> = self
                    .state
                    .connections
                    .iter()
                    .filter(|e| {
                        self.state.node(&e.source.node).is_some_and(|n| {
                            crate::graph::pins(&self.state, n, true)
                                .iter()
                                .any(|p| p.name == e.source.pin && p.ty.is_signal())
                        })
                    })
                    .map(|e| e.target.node.clone())
                    .collect();
                let roots: Vec<_> = self
                    .state
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.workspace == workspace
                            && n.kind.primitive()
                            && !n.status.active()
                            && !incoming.contains(&n.id)
                            && crate::graph::pins(&self.state, n, false)
                                .iter()
                                .any(|p| p.ty.is_signal())
                    })
                    .map(|n| n.id.clone())
                    .collect();
                for id in roots {
                    self.graph_pending
                        .push_back((id, Some((String::new(), json!({})))));
                }
                for n in &mut self.state.nodes {
                    if n.workspace == workspace
                        && !n.desktop_window
                        && !n.status.active()
                        && !self.runs.contains_key(&n.id)
                        && !n.kind.primitive()
                        && !incoming.contains(&n.id)
                    {
                        n.status = Status::Queued;
                        n.log("system", "Workflow queued.");
                    }
                }
            }
            Control::Stop { id } => self.stop(&id)?,
            Control::Interrupt { id } => self.interrupt_node(&id)?,
            Control::Approve { id, output } => {
                let node = self.state.node_mut(&id).context("Node not found")?;
                ensure!(
                    node.status == Status::Review
                        || (matches!(node.kind, NodeKind::Human | NodeKind::Window)
                            && node.status == Status::Running),
                    "Node is not waiting for a human handoff"
                );
                if let Some(output) = output {
                    let output: String = output.chars().take(64_000).collect();
                    if node.kind == NodeKind::Human {
                        node.output.push_str(&format!("\n\nHandoff:\n{output}"));
                    } else {
                        node.output = output;
                    }
                }
                node.status = Status::Done;
                node.finished_at = Some(now());
                node.log("system", "Approved by you. The workflow can continue.");
                self.complete_signal(&id);
            }
            Control::Reject { id } => {
                let node = self.state.node_mut(&id).context("Node not found")?;
                ensure!(
                    node.status == Status::Review,
                    "Node is not waiting for review"
                );
                node.status = Status::Failed;
                node.finished_at = Some(now());
                node.log(
                    "error",
                    "Rejected by you. Success branches are blocked; failure branches may continue.",
                );
                self.complete_signal(&id);
            }
            Control::CreateWorkspace { name, cwd } => {
                ensure!(
                    !name.trim().is_empty() && name.len() <= 100,
                    "Enter a workspace name"
                );
                let cwd = cwd
                    .canonicalize()
                    .context("Working directory does not exist")?;
                ensure!(cwd.is_dir(), "Expected a directory");
                let workspace = Workspace {
                    id: id(),
                    name,
                    cwd,
                    notes: String::new(),
                    row: None,
                    window_array: false,
                    graph_arranged: false,
                    view: RowView::default(),
                };
                self.state.selected_workspace = workspace.id.clone();
                self.state.workspaces.push(workspace);
            }
            Control::SelectWorkspace { id } => {
                ensure!(
                    self.state.workspaces.iter().any(|w| w.id == id),
                    "Workspace not found"
                );
                self.state.selected_workspace = id;
            }
            Control::BindRow { workspace, row } => {
                ensure!(
                    self.state.workspaces.iter().any(|w| w.id == workspace),
                    "Group not found"
                );
                for other in &mut self.state.workspaces {
                    if other.id != workspace
                        && other
                            .row
                            .as_ref()
                            .is_some_and(|r| r.session == row.session && r.id == row.id)
                    {
                        ensure!(
                            !self.state.nodes.iter().any(|n| n.workspace == other.id),
                            "This row already contains a node group"
                        );
                        other.row = None;
                    }
                }
                self.state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .unwrap()
                    .row = Some(row);
            }
            Control::Notes { text } => {
                ensure!(text.len() <= 256_000, "Workspace notes are too long");
                let id = self.state.selected_workspace.clone();
                self.state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == id)
                    .unwrap()
                    .notes = text;
            }
            Control::HumanProgress { id, text } => {
                ensure!(text.len() <= 256_000, "Human work is too long");
                let node = self.state.node_mut(&id).context("Node not found")?;
                ensure!(
                    node.kind == NodeKind::Human,
                    "Only human nodes have editable work"
                );
                node.output = text;
            }
            Control::SaveRecipe { name, nodes } => {
                let recipe = self.state.recipe(name, &nodes)?;
                self.state.recipes.push(recipe);
            }
            Control::ImportRecipe { mut recipe } => {
                ensure!(
                    recipe.nodes.len() <= 500 && !recipe.nodes.is_empty(),
                    "Recipe must contain 1–500 nodes"
                );
                let mut validation = self.state.clone();
                let ws = validation.selected_workspace.clone();
                let cwd = validation.workspace().cwd.clone();
                validation.instantiate(&recipe, &ws, cwd)?;
                recipe.id = id();
                self.state.recipes.push(recipe);
            }
            Control::Instantiate { id } => {
                let recipe = self
                    .state
                    .recipes
                    .iter()
                    .find(|r| r.id == id)
                    .context("Recipe not found")?
                    .clone();
                let ws = self.state.selected_workspace.clone();
                let cwd = self.state.workspace().cwd.clone();
                let ids = self.state.instantiate(&recipe, &ws, cwd)?;
                return Ok(json!({"ids":ids}));
            }
            Control::Watch { watcher } => {
                let mut candidate = self.state.clone();
                candidate.watchers.retain(|w| w.id != watcher.id);
                candidate.watchers.push(watcher);
                candidate.validate()?;
                self.state = candidate;
            }
            Control::DeleteWatcher { id } => self.state.watchers.retain(|w| w.id != id),
            Control::Ui { command } => {
                self.ui.send(command)?;
            }
            Control::Shutdown | Control::SyncDesktop { .. } => unreachable!(),
        }
        Ok(json!({"ok":true}))
    }
    fn stop(&mut self, id: &str) -> Result<()> {
        // Closing a window is an explicit removal, including when its shared
        // application process remains alive or has already finished.
        if self
            .state
            .node(id)
            .is_some_and(|n| n.kind == NodeKind::Window)
        {
            self.apply(Control::Delete { id: id.into() })?;
            return Ok(());
        }
        if let Some(window) = self.state.node(id).and_then(|n| n.window_id) {
            crate::niri::request(niri_ipc::Request::Action(niri_ipc::Action::CloseWindow {
                id: Some(window),
            }))?;
        }
        self.interrupt_node(id)
    }
    fn interrupt_node(&mut self, id: &str) -> Result<()> {
        let node = self.state.node(id).context("Node not found")?;
        ensure!(
            node.kind != NodeKind::Window,
            "This application provider does not support process-safe interruption; close its window separately"
        );
        self.graph_pending.retain(|(root, _)| root != id);
        self.signal_pending.remove(id);
        self.signal_args.remove(id);
        let graph = self.graph_root.as_deref() == Some(id);
        let node = self.state.node_mut(id).unwrap();
        ensure!(
            node.status.active() || self.runs.contains_key(id) || graph,
            "Node is not active"
        );
        node.status = Status::Stopped;
        node.finished_at = Some(now());
        node.log("system", "Stopped by you.");
        if graph && let Some(run) = self.runs.get(GRAPH_RUN) {
            run.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(run) = self.runs.get(id) {
            run.cancel.store(true, Ordering::Relaxed);
            if run.metric_due.is_some() {
                self.runs.remove(id);
            }
        }
        Ok(())
    }
    fn raise_alert(&mut self, id: &str, message: &str, critical: bool, notify: bool) -> Result<()> {
        let node = self.state.node_mut(id).context("Node not found")?;
        let message: String = message.chars().take(4096).collect();
        node.flow.properties.insert(
            "_alert".into(),
            json!({"message": message, "critical": critical}),
        );
        if notify {
            let title = node.title.clone();
            thread::spawn(move || {
                let result = Command::new("notify-send")
                    .args([
                        "--app-name=FlowWM",
                        "--hint=string:desktop-entry:flowwm",
                        "--urgency",
                        if critical { "critical" } else { "normal" },
                        "--",
                        &title,
                        &message,
                    ])
                    .status();
                if let Err(error) = result {
                    eprintln!("FlowWM notification: {error}");
                }
            });
        }
        Ok(())
    }
    fn finish_graph(
        &mut self,
        root: &str,
        result: std::result::Result<crate::graph::Evaluation, String>,
    ) {
        match result {
            Ok(result) => {
                for alert in result.alerts {
                    if let Err(error) =
                        self.raise_alert(&alert.id, &alert.message, alert.critical, true)
                    {
                        self.error = Some(error.to_string());
                    }
                }
                for (id, values) in result.values {
                    if let Some(node) = self.state.node_mut(&id) {
                        node.flow.values.extend(values);
                        if node.kind.primitive() {
                            node.status = Status::Done;
                            node.finished_at = Some(now());
                        }
                        if let Some(value) = node.flow.values.get("value") {
                            node.output = value
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| value.to_string());
                        }
                    }
                }
                for id in result.executed {
                    if let Some(node) = self.state.node_mut(&id).filter(|n| n.kind.primitive()) {
                        node.status = Status::Done;
                        node.finished_at = Some(now());
                    }
                }
                for (id, error) in result.errors {
                    if let Some(node) = self.state.node_mut(&id) {
                        node.status = Status::Failed;
                        node.log("error", error);
                    }
                }
                for activation in result.activations {
                    self.signal_pending
                        .entry(activation.id.clone())
                        .or_default()
                        .push_back(activation);
                }
                if let Some(node) = self
                    .state
                    .node_mut(root)
                    .filter(|n| n.kind.primitive() && n.status == Status::Running)
                {
                    node.status = Status::Done;
                    node.finished_at = Some(now());
                }
            }
            Err(error) => {
                if let Some(node) = self.state.node_mut(root).filter(|n| n.kind.primitive()) {
                    node.status = Status::Failed;
                    node.finished_at = Some(now());
                    node.log("error", &error);
                }
                self.error = Some(error);
            }
        }
    }
    fn complete_signal(&mut self, id: &str) {
        let Some(node) = self.state.node(id) else {
            return;
        };
        let pin = match node.status {
            Status::Done => "out",
            Status::Failed => "failed",
            _ => return,
        };
        let args = self
            .signal_args
            .remove(id)
            .map(|a| a.args)
            .unwrap_or_else(|| json!({}));
        if self
            .state
            .connections
            .iter()
            .any(|e| e.source.node == id && e.source.pin == pin)
        {
            self.graph_pending
                .push_back((id.into(), Some((pin.into(), args))));
        }
    }
    fn schedule(&mut self) {
        if !self.graph_active
            && let Some((id, signal)) = self.graph_pending.pop_front()
            && self.state.node(&id).is_some()
        {
            self.generation += 1;
            let generation = self.generation;
            let cancel = Arc::new(AtomicBool::new(false));
            self.runs.insert(
                GRAPH_RUN.into(),
                Run {
                    generation,
                    cancel: cancel.clone(),
                    metric_due: None,
                    writer: true,
                },
            );
            if let Some(node) = self.state.node_mut(&id).filter(|n| n.kind.primitive()) {
                node.status = Status::Running;
                node.started_at = Some(now());
                node.finished_at = None;
            }
            self.graph_active = true;
            self.graph_exit = signal
                .as_ref()
                .filter(|(pin, _)| pin == "on_exit")
                .map(|_| id.clone());
            self.graph_root = Some(id.clone());
            self.dirty = true;
            let state = self.state.clone();
            let tx = self.tx.clone();
            thread::spawn(move || {
                let result = crate::graph::evaluate_isolated(state, &id, signal, cancel)
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Message::GraphFinished {
                    id,
                    generation,
                    result,
                });
            });
        }
        let pending: Vec<_> = self.signal_pending.keys().cloned().collect();
        for id in pending {
            if self.state.node(&id).is_none() {
                self.signal_pending.remove(&id);
                continue;
            }
            if !self.runs.contains_key(&id)
                && self.state.node(&id).is_some_and(|n| !n.status.active())
            {
                if let Some(activation) = self
                    .signal_pending
                    .get_mut(&id)
                    .and_then(VecDeque::pop_front)
                {
                    self.signal_args.insert(id.clone(), activation);
                    self.state.node_mut(&id).unwrap().status = Status::Queued;
                    self.dirty = true;
                }
                if self.signal_pending.get(&id).is_some_and(VecDeque::is_empty) {
                    self.signal_pending.remove(&id);
                }
            }
        }
        let due: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, r)| r.metric_due.is_some_and(|t| Instant::now() >= t))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.runs.remove(&id);
            if let Some(n) = self.state.node_mut(&id)
                && n.status == Status::Running
            {
                n.status = Status::Queued;
            }
        }
        let ids: Vec<_> = self
            .state
            .nodes
            .iter()
            .filter(|n| {
                n.status == Status::Queued
                    && (self.signal_args.contains_key(&n.id) || self.state.ready(n))
            })
            .map(|n| n.id.clone())
            .collect();
        for id in ids {
            let n = self.state.node(&id).unwrap();
            let process = matches!(n.kind, NodeKind::Agent | NodeKind::Shell | NodeKind::Metric);
            if process
                && self
                    .runs
                    .values()
                    .filter(|r| r.metric_due.is_none())
                    .count()
                    >= self.limits.max_concurrent
            {
                continue;
            }
            let writer = n.kind == NodeKind::Shell || (n.kind == NodeKind::Agent && n.writable);
            if process
                && self.runs.iter().any(|(other, run)| {
                    run.metric_due.is_none()
                        && self
                            .state
                            .node(other)
                            .is_some_and(|o| o.cwd == n.cwd && (writer || run.writer))
                })
            {
                continue;
            }
            if let Err(error) = self.launch(&id)
                && let Some(node) = self.state.node_mut(&id)
            {
                node.status = Status::Failed;
                node.log("error", format!("{error:#}"));
                self.complete_signal(&id);
            }
            self.dirty = true;
        }
    }
    fn launch(&mut self, id: &str) -> Result<()> {
        if self.state.node(id).is_some_and(|n| n.kind.primitive()) {
            self.graph_pending.push_back((id.into(), None));
            self.state.node_mut(id).unwrap().status = Status::Idle;
            return Ok(());
        }
        let context = self
            .state
            .node(id)
            .filter(|n| !n.chat.is_empty())
            .map(|n| crate::agent::graph_context(&self.state, &n.workspace));
        let schema_path = self.store.directory().join("agent-result.schema.json");
        let inputs: Vec<_> = self
            .state
            .node(id)
            .unwrap()
            .dependencies
            .iter()
            .filter_map(|id| self.state.node(id))
            .map(|n| json!({"id": n.id, "title":n.title, "status":n.status, "output":n.output}))
            .collect();
        let node = self.state.node_mut(id).unwrap();
        node.status = Status::Running;
        node.started_at = Some(now());
        node.finished_at = None;
        if node.kind != NodeKind::Human {
            node.output.clear();
        }
        if node.kind == NodeKind::Gate {
            node.status = Status::Review;
            node.log(
                "system",
                "Decision required. Approve to continue, reject to take the failure branch.",
            );
            return Ok(());
        }
        if node.kind == NodeKind::Human
            || (node.kind == NodeKind::Window && !node.flow.properties.contains_key("application"))
        {
            node.log("system", "Your work is active. Mark it done when ready.");
            return Ok(());
        }
        ensure!(node.cwd.is_dir(), "Working directory no longer exists");
        let agent = node.kind == NodeKind::Agent;
        if agent && context.is_none() {
            std::fs::write(&schema_path, AGENT_SCHEMA)?;
        }
        let mut command = if let Some(application) = node
            .flow
            .properties
            .get("application")
            .and_then(Value::as_str)
            .filter(|_| node.kind == NodeKind::Window)
        {
            let mut command = Command::new("gtk-launch");
            command.arg(application);
            for key in [
                "XDG_CONFIG_HOME",
                "XDG_STATE_HOME",
                "XDG_CACHE_HOME",
                "QT_PLUGIN_PATH",
                "NIXPKGS_QT6_QML_IMPORT_PATH",
            ] {
                if let Some(original) = std::env::var_os(format!("DMS_ORIG_{key}")) {
                    if original.is_empty() {
                        command.env_remove(key);
                    } else {
                        command.env(key, original);
                    }
                }
            }
            command
        } else if context.is_some() {
            crate::agent::Config::load()?.process(node)
        } else if agent {
            let mut c = Command::new("codex");
            c.args([
                "exec",
                "--json",
                "--color",
                "never",
                "--sandbox",
                if node.writable {
                    "workspace-write"
                } else {
                    "read-only"
                },
                "-C",
            ])
            .arg(&node.cwd)
            .arg("--output-schema")
            .arg(&schema_path)
            .arg("-");
            c
        } else {
            let mut c = Command::new("/bin/sh");
            c.args(["-lc", &node.command]);
            c
        };
        command
            .current_dir(&node.cwd)
            .env("FLOWWM_INPUTS", serde_json::to_string(&inputs)?)
            .env("FLOWWM_NODE_ID", id)
            .env("FLOWWM_SOCKET", crate::agent::socket())
            .env(
                "FLOWWM_CLI",
                std::env::current_exe()?.with_file_name("flowwm"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // Child processes never inherit arguments from an unrelated signal.
        for (key, _) in
            std::env::vars_os().filter(|(k, _)| k.as_encoded_bytes().starts_with(b"FLOWWM_ARG_"))
        {
            command.env_remove(key);
        }
        if let Some(activation) = self.signal_args.get(id) {
            command.envs(crate::graph::signal_environment(&activation.args)?);
            command.env("FLOWWM_DATA_INPUTS", activation.inputs.to_string());
        }
        let mut child = command.spawn().with_context(|| {
            if agent {
                "Start Codex (install and authenticate the codex CLI first)"
            } else {
                "Start shell command"
            }
        })?;
        let pid = child.id();
        let mut prompt = if inputs.is_empty() {
            node.command.clone()
        } else {
            format!(
                "{}\n\nWorkflow inputs:\n{}",
                node.command,
                serde_json::to_string_pretty(&inputs)?
            )
        };
        if let Some(context) = context {
            prompt = format!("{context}\n\nUser request:\n{}", node.command);
        } else if agent {
            prompt.push_str("\n\nFlowWM workflow contract: return the required JSON object. Set outcome to success only when this step's goal is achieved. Set outcome to issue if blocked, uncertain, or if an issue requires attention; this pauses success branches. Put a complete downstream handoff in summary, and file paths in artifacts. Do not conceal issues to keep the pipeline moving.");
        }
        if let Some(mut stdin) = child.stdin.take() {
            thread::spawn(move || {
                if agent {
                    let _ = stdin.write_all(prompt.as_bytes());
                }
            });
        }
        self.generation += 1;
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let writer = node.kind == NodeKind::Shell || (agent && node.writable);
        self.runs.insert(
            id.into(),
            Run {
                generation,
                cancel: cancel.clone(),
                metric_due: None,
                writer,
            },
        );
        node.log(
            "system",
            format!("Started {} in {}", node.kind.label(), node.cwd.display()),
        );
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = stream_output(
            stdout,
            self.tx.clone(),
            id.into(),
            generation,
            if agent { "codex" } else { "stdout" },
        );
        let err = stream_output(stderr, self.tx.clone(), id.into(), generation, "stderr");
        let sender = self.tx.clone();
        let id = id.to_string();
        let metric = node.kind == NodeKind::Metric;
        let limits = self.limits.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let mut cancelled_at = None;
            let result = loop {
                let timed_out = start.elapsed()
                    > if metric {
                        Duration::from_secs(limits.metric_timeout_secs)
                    } else {
                        Duration::from_secs(limits.execution_timeout_secs)
                    };
                if (cancel.load(Ordering::Relaxed) || timed_out) && cancelled_at.is_none() {
                    signal_group(pid, libc::SIGINT);
                    cancelled_at = Some(Instant::now());
                }
                if cancelled_at.is_some_and(|t: Instant| {
                    t.elapsed() > Duration::from_millis(limits.interrupt_grace_ms)
                }) {
                    signal_group(pid, libc::SIGTERM);
                }
                if cancelled_at.is_some_and(|t: Instant| {
                    t.elapsed()
                        > Duration::from_millis(
                            limits.interrupt_grace_ms + limits.terminate_grace_ms,
                        )
                }) {
                    signal_group(pid, libc::SIGKILL);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => break Err(error),
                    Ok(None) => thread::sleep(Duration::from_millis(30)),
                }
            };
            // A script may have left subprocesses holding a pipe. Terminate its owned group.
            signal_group(pid, libc::SIGKILL);
            let _ = out.join();
            let _ = err.join();
            let success = result.as_ref().is_ok_and(|s| s.success()) && cancelled_at.is_none();
            let error = if cancelled_at.is_some() {
                Some("Run was stopped or exceeded its time limit".into())
            } else {
                result.err().map(|e| e.to_string())
            };
            let _ = sender.send(Message::Finished {
                id,
                generation,
                success,
                error,
            });
        });
        Ok(())
    }
    fn output(&mut self, id: &str, kind: &str, line: &str) {
        let Some(node) = self.state.node_mut(id) else {
            return;
        };
        if kind == "codex"
            && let Ok(value) = serde_json::from_str::<Value>(line)
        {
            match value["type"].as_str().unwrap_or("") {
                "thread.started" => {
                    if !node.chat.is_empty() {
                        node.agent_session = value["thread_id"].as_str().map(str::to_string);
                    }
                }
                "item.completed" | "item.started" => {
                    let item = &value["item"];
                    match item["type"].as_str().unwrap_or("") {
                        "agent_message" => {
                            if let Some(text) = item["text"].as_str() {
                                node.output = text.chars().take(64_000).collect();
                                node.log("agent", text);
                            }
                        }
                        "command_execution" => node.log(
                            "command",
                            format!(
                                "{}\n{}",
                                item["command"].as_str().unwrap_or(""),
                                item["aggregated_output"].as_str().unwrap_or("")
                            ),
                        ),
                        "file_change" => node.log("files", item["changes"].to_string()),
                        _ => {}
                    }
                }
                "error" | "turn.failed" => {
                    node.status = Status::Failed;
                    node.log("error", value.to_string());
                }
                _ => {}
            }
            return;
        }
        node.log(kind, line);
        if kind == "stdout" {
            node.output.push_str(line);
            node.output.push('\n');
            if node.output.len() > 64_000 {
                node.output = node
                    .output
                    .chars()
                    .rev()
                    .take(32_000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
            }
        }
    }
    fn finish(&mut self, id: &str, success: bool, error: Option<String>) {
        self.runs.remove(id);
        if self.pending_deletions.remove(id) {
            if let Err(error) = self.state.remove(id) {
                self.error = Some(error.to_string());
            }
            return;
        }
        let Some(node) = self.state.node_mut(id) else {
            return;
        };
        // gtk-launch can finish while the actual application remains alive.
        // Once bound, the pidfd observer owns the window's completion status.
        if node.kind == NodeKind::Window && self.window_processes.get(id).is_some_and(|p| p.pending)
        {
            return;
        }
        node.finished_at = Some(now());
        if node.status != Status::Running {
            return;
        }
        if !success {
            node.status = Status::Failed;
            node.log(
                "error",
                error.unwrap_or_else(|| {
                    "Process exited unsuccessfully. Inspect the output for details.".into()
                }),
            );
            return;
        }
        if node.kind == NodeKind::Agent && !node.chat.is_empty() {
            node.chat.push(ChatMessage {
                role: "assistant".into(),
                text: node.output.clone(),
            });
        } else if node.kind == NodeKind::Agent {
            match agent_result(&node.output) {
                Ok(result) if result.outcome == AgentOutcome::Issue => {
                    node.status = Status::Failed;
                    node.log("issue", result.summary);
                    return;
                }
                Err(error) => {
                    node.status = Status::Failed;
                    node.log("error", error.to_string());
                    return;
                }
                _ => {}
            }
        }
        if node.kind != NodeKind::Metric {
            node.status = if node.review {
                Status::Review
            } else {
                Status::Done
            };
            node.log(
                "system",
                if node.review {
                    "Work is ready for your review."
                } else {
                    "Completed. Outputs are ready for downstream nodes."
                },
            );
            return;
        }
        let last = node.output.trim().lines().last().unwrap_or("");
        let value = last.parse::<f64>().ok().or_else(|| {
            serde_json::from_str::<Value>(last)
                .ok()
                .and_then(|v| v["value"].as_f64())
        });
        let Some(value) = value.filter(|v| v.is_finite()) else {
            node.status = Status::Failed;
            node.log(
                "error",
                "Metric command must end with a number or {\"value\": number}.",
            );
            return;
        };
        node.metric.samples.push(Sample { time: now(), value });
        if node.metric.samples.len() > 80 {
            node.metric.samples.remove(0);
        }
        node.status = Status::Running;
        self.runs.insert(
            id.into(),
            Run {
                generation: self.generation,
                cancel: Arc::new(AtomicBool::new(false)),
                metric_due: Some(Instant::now() + Duration::from_secs(node.metric.interval_secs)),
                writer: false,
            },
        );
        let mut targets = vec![];
        for watcher in &mut self.state.watchers {
            if !watcher.enabled || watcher.source != id {
                continue;
            }
            let crossed = if watcher.above {
                value > watcher.threshold
            } else {
                value < watcher.threshold
            };
            if !crossed {
                watcher.armed = true;
            }
            if crossed && watcher.armed {
                targets.push((watcher.id.clone(), watcher.target.clone()));
            }
        }
        for (watcher_id, target) in targets {
            if self.runs.contains_key(&target) {
                continue;
            }
            if self.state.node(&target).is_some_and(|n| !n.status.active())
                && self
                    .apply(Control::Run {
                        id: target.clone(),
                        downstream: true,
                    })
                    .is_ok()
            {
                self.state.node_mut(&target).unwrap().log(
                    "watcher",
                    format!("Threshold crossed: {value}. Workflow queued."),
                );
                if let Some(w) = self.state.watchers.iter_mut().find(|w| w.id == watcher_id) {
                    w.armed = false;
                    w.last_triggered = Some(now());
                }
            }
        }
    }
}
fn signal_group(pid: u32, signal: i32) {
    // Each worker starts a new process group. Only that owned group receives signals.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}
fn stream_output(
    pipe: impl Read + Send + 'static,
    tx: mpsc::Sender<Message>,
    id: String,
    generation: u64,
    kind: &'static str,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut total = 0usize;
        let mut lines = 0usize;
        loop {
            let mut bytes = vec![];
            let count = (&mut reader).take(256 * 1024).read_until(b'\n', &mut bytes);
            match count {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            total += bytes.len();
            lines += 1;
            if total > 4 * 1024 * 1024 || lines > 4096 {
                continue;
            }
            let line = String::from_utf8_lossy(&bytes).trim_end().to_string();
            if tx
                .send(Message::Output {
                    id: id.clone(),
                    generation,
                    kind: kind.into(),
                    line,
                })
                .is_err()
            {
                break;
            }
        }
    })
}
