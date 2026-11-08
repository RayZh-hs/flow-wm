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
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UiCommand {
    Toggle,
    Nodes,
    Windows,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Control {
    Status,
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

#[derive(Clone)]
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

#[derive(Clone)]
pub struct Engine {
    pub performance: crate::performance::Performance,
    tx: mpsc::Sender<Message>,
    snapshot: Arc<Mutex<Snapshot>>,
    join: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    notifier: Arc<Notifier>,
    presentation: Arc<Mutex<Presentation>>,
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
        let (tx, rx) = mpsc::channel();
        let (ui_tx, ui_rx) = mpsc::channel();
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
        let worker_notifier = notifier.clone();
        let join = thread::spawn(move || {
            Runtime {
                state,
                store,
                runs: HashMap::new(),
                graph_active: false,
                graph_root: None,
                graph_pending: VecDeque::new(),
                signal_pending: HashMap::new(),
                signal_args: HashMap::new(),
                suppressed_windows: HashSet::new(),
                pending_deletions: HashSet::new(),
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
                tx,
                snapshot,
                join: Arc::new(Mutex::new(Some(join))),
                notifier,
                presentation: Arc::new(Mutex::new(Presentation::default())),
            },
            ui_rx,
        )
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
    pub fn presentation(&self) -> Presentation {
        self.presentation.lock().unwrap().clone()
    }
    pub fn set_presentation(&self, presentation: Presentation) {
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
        self.notifier.closed.store(true, Ordering::Release);
        self.notifier.signal();
        self.dispatch(Control::Shutdown);
        if let Some(join) = self.join.lock().unwrap().take() {
            let _ = join.join();
        }
    }
}

struct Runtime {
    state: State,
    store: Store,
    runs: HashMap<String, Run>,
    graph_active: bool,
    graph_root: Option<String>,
    graph_pending: VecDeque<(String, Option<(String, Value)>)>,
    signal_pending: HashMap<String, VecDeque<crate::graph::Activation>>,
    signal_args: HashMap<String, crate::graph::Activation>,
    suppressed_windows: HashSet<u64>,
    pending_deletions: HashSet<String>,
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
            let next_metric = self
                .runs
                .values()
                .filter_map(|r| r.metric_due)
                .map(|t| t.saturating_duration_since(Instant::now()))
                .min()
                .unwrap_or(Duration::from_secs(60));
            let timeout = if self.needs_save {
                next_metric.min(Duration::from_millis(400).saturating_sub(self.last_save.elapsed()))
            } else {
                next_metric
            };
            match rx.recv_timeout(timeout) {
                Ok(Message::Request(Request {
                    control: Control::Shutdown,
                    reply,
                })) => {
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
                    if let Err(error) = self.store.save(&self.state) {
                        eprintln!("Save state: {error:#}");
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(json!({"ok":true})));
                    }
                    // Process workers observe cancellation and reap their process group.
                    if !self.runs.is_empty() {
                        thread::sleep(Duration::from_millis(1200));
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
                    self.suppressed_windows
                        .retain(|id| desktop.windows.iter().any(|w| w.id == *id));
                    desktop
                        .windows
                        .retain(|w| !self.suppressed_windows.contains(&w.id));
                    let changed = crate::groups::synchronize(&mut self.state, &desktop);
                    if changed {
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
                Ok(Message::Request(Request { control, reply })) => {
                    let result = self.apply(control);
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
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
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
                    !original.status.active() && !self.runs.contains_key(&node.id),
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
                if let Some(node) = self.state.node(&id).filter(|n| n.desktop_window)
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
        self.graph_pending.retain(|(root, _)| root != id);
        self.signal_pending.remove(id);
        self.signal_args.remove(id);
        let graph = self.graph_root.as_deref() == Some(id);
        let node = self.state.node_mut(id).context("Node not found")?;
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
    fn finish_graph(
        &mut self,
        root: &str,
        result: std::result::Result<crate::graph::Evaluation, String>,
    ) {
        match result {
            Ok(result) => {
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
                    >= 8
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
        thread::spawn(move || {
            let start = Instant::now();
            let mut cancelled_at = None;
            let result = loop {
                let timed_out = start.elapsed()
                    > if metric {
                        Duration::from_secs(20)
                    } else {
                        Duration::from_secs(3600)
                    };
                if (cancel.load(Ordering::Relaxed) || timed_out) && cancelled_at.is_none() {
                    signal_group(pid, libc::SIGTERM);
                    cancelled_at = Some(Instant::now());
                }
                if cancelled_at.is_some_and(|t: Instant| t.elapsed() > Duration::from_secs(1)) {
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
