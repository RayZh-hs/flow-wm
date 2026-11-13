//! WM client and local view model. It has no evaluator, scheduler or execution provider.
use crate::{model::*, presentation::Writer};
use anyhow::{Context, Result, ensure};
use flow_core::{domain::*, protocol::Command};
use flow_host::client::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
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
    RowSettings,
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
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
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
        source: String,
        target: String,
        threshold: f64,
        above: bool,
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
pub struct ViewSnapshot {
    pub state: Arc<State>,
    pub revision: u64,
    pub error: Option<String>,
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
#[derive(Clone)]
pub struct DesktopClient {
    pub performance: crate::performance::Performance,
    inner: Arc<Inner>,
}
struct Inner {
    host: Client,
    config_directory: PathBuf,
    notification_command: Vec<String>,
    provider_socket: PathBuf,
    bindings: Mutex<crate::desktop_provider::Bindings>,
    view: Mutex<ViewSnapshot>,
    changed: Condvar,
    closed: AtomicBool,
    ui: mpsc::Sender<UiCommand>,
    presentation: Mutex<Presentation>,
    launcher: Mutex<Option<mpsc::Sender<Option<String>>>>,
    writer: Mutex<Option<Writer>>,
    subscriptions: Mutex<std::collections::BTreeMap<&'static str, std::os::unix::net::UnixStream>>,
}
impl DesktopClient {
    pub fn connect(socket: PathBuf) -> Result<(Self, mpsc::Receiver<UiCommand>)> {
        Self::connect_with_provider(socket, crate::desktop_provider::socket())
    }
    pub fn connect_with_provider(
        socket: PathBuf,
        provider_socket: PathBuf,
    ) -> Result<(Self, mpsc::Receiver<UiCommand>)> {
        let host = Client::new(socket);
        let info = host.command(Command::Info)?;
        let directory: PathBuf = serde_json::from_value(info["data_directory"].clone())?;
        let config_directory: PathBuf = serde_json::from_value(info["config_directory"].clone())?;
        let notification_config = config_directory.join("config.kdl");
        let notification_command = if notification_config.exists() {
            niri_config::Config::load(&notification_config)
                .config
                .map_err(|e| anyhow::anyhow!("{e:?}"))?
                .flow
                .notification_command
        } else {
            None
        }
        .unwrap_or_else(crate::notifications::default_command);
        let writer = Writer::new(directory)?;
        let mut state = State::empty(std::env::current_dir()?);
        // Older hosts omit schemas from Info. Keep manifest definitions available
        // before the first instance exists, with the mounted host schemas taking precedence.
        crate::extensions::load_bundle(&config_directory.join("extensions"))?.apply(&mut state)?;
        for definition in serde_json::from_value::<Vec<crate::graph::NodeDefinition>>(
            info.get("node_types").cloned().unwrap_or_else(|| json!([])),
        )? {
            state.node_types.retain(|d| d.id != definition.id);
            state.node_types.push(definition);
        }
        crate::projection::apply(&mut state, &host.snapshot()?);
        writer.load(&mut state)?;
        let bindings = crate::desktop_provider::bindings_at(&provider_socket)?;
        bindings.apply(&mut state);
        let (ui, events) = mpsc::channel();
        let client = Self {
            performance: Default::default(),
            inner: Arc::new(Inner {
                host,
                config_directory,
                notification_command,
                provider_socket,
                bindings: Mutex::new(bindings),
                view: Mutex::new(ViewSnapshot {
                    state: Arc::new(state),
                    revision: 1,
                    error: None,
                }),
                changed: Condvar::new(),
                closed: AtomicBool::new(false),
                ui,
                presentation: Mutex::new(Presentation::default()),
                launcher: Mutex::new(None),
                writer: Mutex::new(Some(writer)),
                subscriptions: Mutex::new(Default::default()),
            }),
        };
        let connection = client.clone();
        std::thread::spawn(move || {
            while !connection.inner.closed.load(Ordering::Acquire) {
                let result = connection.inner.host.subscribe_connected(
                    |stream| connection.track_subscription("host", stream),
                    |reply| {
                        if connection.inner.closed.load(Ordering::Acquire) {
                            anyhow::bail!("Desktop closed");
                        }
                        if let Some(error) = reply.error {
                            anyhow::bail!("{}", error.message);
                        }
                        if let Some(value) = reply.result {
                            let event: RuntimeState = serde_json::from_value(value)?;
                            let selected = connection.snapshot().state.selected_workspace.clone();
                            let runtime = connection
                                .inner
                                .host
                                .snapshot_as(Some(selected.into()))
                                .unwrap_or(event);
                            connection.accept_runtime(runtime)?;
                        }
                        Ok(())
                    },
                );
                if let Err(error) = result {
                    connection.fail(format!("Host disconnected: {error:#}"));
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        let connection = client.clone();
        std::thread::spawn(move || {
            use std::{
                io::{BufRead, BufReader, Write},
                os::unix::net::UnixStream,
            };
            while !connection.inner.closed.load(Ordering::Acquire) {
                let result = (|| -> Result<()> {
                    let mut stream = UnixStream::connect(&connection.inner.provider_socket)?;
                    connection.track_subscription("desktop", &stream)?;
                    writeln!(
                        stream,
                        "{}",
                        json!({"desktop_protocol":1,"command":"subscribe"})
                    )?;
                    for line in BufReader::new(stream).lines() {
                        if connection.inner.closed.load(Ordering::Acquire) {
                            break;
                        }
                        let frame: Value = serde_json::from_str(&line?)?;
                        ensure!(
                            frame["desktop_protocol"] == 1,
                            "Unsupported desktop provider stream"
                        );
                        let bindings: crate::desktop_provider::Bindings =
                            serde_json::from_value(frame["bindings"].clone())?;
                        *connection.inner.bindings.lock().unwrap() = bindings.clone();
                        let mut view = connection.inner.view.lock().unwrap();
                        let mut state = (*view.state).clone();
                        bindings.apply(&mut state);
                        view.state = Arc::new(state);
                        view.revision += 1;
                        connection.inner.changed.notify_all();
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    connection.fail(format!("Desktop provider disconnected: {error:#}"));
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        Ok((client, events))
    }
    fn fail(&self, error: String) {
        let mut view = self.inner.view.lock().unwrap();
        view.error = Some(error);
        view.revision += 1;
        self.inner.changed.notify_all();
    }
    fn accept_runtime(&self, runtime: RuntimeState) -> Result<()> {
        let bindings = self.inner.bindings.lock().unwrap().clone();
        let mut view = self.inner.view.lock().unwrap();
        if view
            .state
            .orchestration
            .as_ref()
            .is_some_and(|r| r.revision > runtime.revision)
        {
            return Ok(());
        }
        for execution in runtime.executions.values() {
            if let Some(attention) = execution.result.get("attention")
                && let Some(node) = view.state.node(&execution.node.0)
                && execution.generation > node.attention_generation
            {
                crate::notifications::present(
                    &self.inner.notification_command,
                    &node.title,
                    attention["message"]
                        .as_str()
                        .unwrap_or("Needs your attention"),
                    attention["critical"].as_bool().unwrap_or(false),
                );
            }
        }
        let mut state = (*view.state).clone();
        crate::projection::apply(&mut state, &runtime);
        bindings.apply_native(&mut state);
        view.state = Arc::new(state);
        view.revision += 1;
        view.error = None;
        self.inner.changed.notify_all();
        Ok(())
    }
    pub fn snapshot(&self) -> ViewSnapshot {
        self.inner.view.lock().unwrap().clone()
    }
    pub fn data_directory(&self) -> Result<PathBuf> {
        Ok(self
            .inner
            .writer
            .lock()
            .unwrap()
            .as_ref()
            .context("WM closed")?
            .directory()
            .into())
    }
    pub fn orchestrate(&self, command: Command, caller: Option<WorkflowId>) -> Result<Value> {
        let selected = caller
            .clone()
            .or_else(|| Some(self.snapshot().state.selected_workspace.clone().into()));
        let result = self.inner.host.command_as(command, caller)?;
        self.accept_runtime(self.inner.host.snapshot_as(selected)?)?;
        Ok(result)
    }
    pub fn dispatch(&self, action: Action) {
        let client = self.clone();
        std::thread::spawn(move || {
            if let Err(error) = client.request(action) {
                client.fail(format!("{error:#}"));
            }
        });
    }
    pub fn presentation(&self) -> Presentation {
        self.inner.presentation.lock().unwrap().clone()
    }
    pub fn set_presentation(&self, value: Presentation) {
        *self.inner.presentation.lock().unwrap() = value;
        self.wake_ui();
    }
    pub fn connect_launcher(&self) -> mpsc::Receiver<Option<String>> {
        let (tx, rx) = mpsc::channel();
        *self.inner.launcher.lock().unwrap() = Some(tx);
        rx
    }
    pub fn open_launcher(&self, query: Option<String>) -> bool {
        let sent = self
            .inner
            .launcher
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|tx| tx.send(query).is_ok());
        if sent {
            self.wake_ui();
        }
        sent
    }
    pub fn wake_ui(&self) {
        let mut view = self.inner.view.lock().unwrap();
        view.revision += 1;
        self.inner.changed.notify_all();
    }
    pub fn wait_for_change(&self, after: u64) -> Option<u64> {
        let view = self.inner.view.lock().unwrap();
        let view = self
            .inner
            .changed
            .wait_while(view, |v| {
                v.revision == after && !self.inner.closed.load(Ordering::Acquire)
            })
            .unwrap();
        (!self.inner.closed.load(Ordering::Acquire)).then_some(view.revision)
    }
    fn track_subscription(
        &self,
        key: &'static str,
        stream: &std::os::unix::net::UnixStream,
    ) -> Result<()> {
        let mut subscriptions = self.inner.subscriptions.lock().unwrap();
        ensure!(!self.inner.closed.load(Ordering::Acquire), "WM closed");
        subscriptions.insert(key, stream.try_clone()?);
        Ok(())
    }
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::Release);
        for stream in std::mem::take(&mut *self.inner.subscriptions.lock().unwrap()).into_values() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        self.inner.writer.lock().unwrap().take();
        self.inner.changed.notify_all();
    }
    fn edit_graph(
        &self,
        workflow: &str,
        edit: impl FnOnce(&mut FlowNet) -> Result<()>,
    ) -> Result<()> {
        let runtime = self.inner.host.snapshot_as(Some(workflow.into()))?;
        let mut graph = runtime
            .workflows
            .get(&workflow.into())
            .context("Workflow not found")?
            .graph
            .clone();
        let extensions =
            crate::extensions::load_bundle(&self.inner.config_directory.join("extensions"))?;
        for definition in extensions.types {
            if !graph.types.iter().any(|d| d.name == definition.name) {
                graph.types.push(definition);
            }
        }
        for definition in extensions.casts {
            if !graph.casts.contains(&definition) {
                graph.casts.push(definition);
            }
        }
        edit(&mut graph)?;
        for definition in extensions.nodes {
            if graph.nodes.iter().any(|n| n.type_id == definition.id)
                && !graph.node_types.iter().any(|d| d.id == definition.id)
            {
                graph.node_types.push(definition);
            }
        }
        self.inner.host.command(Command::ReplaceGraph {
            workflow: workflow.into(),
            graph,
        })?;
        Ok(())
    }
    fn invocation(&self, state: &State, node: &str) -> Result<InvocationId> {
        let runtime = state
            .orchestration
            .as_ref()
            .context("Runtime unavailable")?;
        if let Some(frame) = runtime
            .queue
            .frames
            .iter()
            .find(|f| f.root.0 == node && f.pin.is_empty())
        {
            return Ok(frame.id.clone().into());
        }
        runtime
            .executions
            .values()
            .filter(|e| e.node.0 == node && e.active())
            .max_by_key(|e| e.generation)
            .map(|e| e.id.clone())
            .context("No active invocation")
    }
    fn run(&self, state: &State, id: &str, args: Value) -> Result<Value> {
        let node = state.node(id).context("Node not found")?;
        if node.kind == NodeKind::Window {
            return self.application_operation(id, "open");
        }
        self.inner.host.command(Command::Run {
            workflow: node.workspace.clone().into(),
            node: id.into(),
            invocation: crate::model::id().into(),
            args,
        })
    }
    pub fn open_window(&self, node: &str) -> Result<Value> {
        self.application_operation(node, "open")
    }
    fn application_operation(&self, node: &str, command: &str) -> Result<Value> {
        let bindings = self.inner.bindings.lock().unwrap().clone();
        let token = bindings.applications.get(node).map(|b| b.token.as_str());
        flow_host::ipc::send(
            &self.inner.provider_socket,
            &json!({"desktop_protocol":1,"command":command,"node":node,"token":token,"incarnation":bindings.session}),
        )
    }
    pub fn request(&self, action: Action) -> Result<Value> {
        let refresh_runtime = !matches!(
            &action,
            Action::Ui { .. }
                | Action::Shutdown
                | Action::Move { .. }
                | Action::Arrange { .. }
                | Action::ColumnMode { .. }
                | Action::RowView { .. }
                | Action::ClearAlert { .. }
                | Action::Alert { .. }
                | Action::SaveRecipe { .. }
                | Action::ImportRecipe { .. }
                | Action::Notes { .. }
                | Action::HumanProgress { .. }
                | Action::SyncDesktop { .. }
        );
        // Presentation edits are serialized under the WM lock, never the host's lock.
        let mut view = self.inner.view.lock().unwrap();
        let mut state = (*view.state).clone();
        let mut result = json!({"ok":true});
        match action {
            Action::Ui { command } => {
                self.inner.ui.send(command)?;
            }
            Action::Shutdown => {
                self.shutdown();
            }
            Action::Create { node } => {
                let mut spec = node.spec();
                if node.desktop_window
                    && state.node(&node.id).is_some_and(|n| n.title != node.title)
                {
                    spec.flow
                        .properties
                        .insert("display_title".into(), json!(node.title));
                }
                spec.flow
                    .properties
                    .insert("title".into(), json!(node.title));
                self.edit_graph(&node.workspace, |g| {
                    ensure!(g.node(&node.id).is_none(), "Duplicate node");
                    g.nodes.push(spec);
                    Ok(())
                })?;
                result = json!({"id":node.id});
                state.nodes.push(*node);
            }
            Action::Update { node } => {
                let mut spec = node.spec();
                if node.desktop_window
                    && state.node(&node.id).is_some_and(|n| n.title != node.title)
                {
                    spec.flow
                        .properties
                        .insert("display_title".into(), json!(node.title));
                }
                spec.flow
                    .properties
                    .insert("title".into(), json!(node.title));
                if node.kind == NodeKind::Agent {
                    spec.flow
                        .properties
                        .insert("writable".into(), json!(node.writable));
                }
                if node.kind == NodeKind::Metric {
                    spec.flow
                        .properties
                        .insert("interval_secs".into(), json!(node.metric.interval_secs));
                    spec.flow
                        .properties
                        .insert("unit".into(), json!(node.metric.unit));
                }
                self.edit_graph(&node.workspace, |g| {
                    *g.node_mut(&node.id).context("Node not found")? = spec;
                    Ok(())
                })?;
            }
            Action::Evaluate { id } => {
                let node = state.node(&id).context("Node not found")?;
                result = self.inner.host.command_as(
                    Command::Read {
                        workflow: node.workspace.clone().into(),
                        node: id.into(),
                        evaluation: crate::model::id().into(),
                    },
                    Some(node.workspace.clone().into()),
                )?;
            }
            Action::Run { id, .. } => {
                result = self.run(&state, &id, json!({}))?;
            }
            Action::Activate { id, args } => {
                result = self.run(&state, &id, args)?;
            }
            Action::RunWorkspace => {
                let incoming: std::collections::HashSet<_> = state
                    .connections
                    .iter()
                    .filter(|e| {
                        state.node(&e.source.node).is_some_and(|n| {
                            crate::graph::pins(&state, n, true)
                                .iter()
                                .any(|p| p.name == e.source.pin && p.ty.is_signal())
                        })
                    })
                    .map(|e| e.target.node.as_str())
                    .collect();
                for node in state.nodes.iter().filter(|n| {
                    n.workspace == state.selected_workspace
                        && !incoming.contains(n.id.as_str())
                        && crate::graph::pins(&state, n, false)
                            .iter()
                            .any(|p| p.ty.is_signal())
                        && !state.node_types.iter().any(|d| {
                            d.id == n.spec().type_id
                                && matches!(
                                    d.behavior,
                                    flow_core::providers::Behavior::Source
                                        | flow_core::providers::Behavior::Application
                                        | flow_core::providers::Behavior::Join
                                )
                        })
                }) {
                    self.run(&state, &node.id, json!({}))?;
                }
            }
            Action::Interrupt { id } => {
                self.inner.host.command(Command::Interrupt {
                    invocation: self.invocation(&state, &id)?,
                })?;
            }
            Action::Stop { id } => {
                let node = state.node(&id).context("Node not found")?;
                if node.window_id.is_some() {
                    self.application_operation(&id, "close_window")?;
                } else {
                    self.inner.host.command(Command::Interrupt {
                        invocation: self.invocation(&state, &id)?,
                    })?;
                }
            }
            Action::Approve { id, output } => {
                self.inner.host.command(Command::Resolve {
                    invocation: self.invocation(&state, &id)?,
                    accepted: true,
                    result: json!({"output":output.unwrap_or_default()}),
                })?;
            }
            Action::Reject { id } => {
                self.inner.host.command(Command::Resolve {
                    invocation: self.invocation(&state, &id)?,
                    accepted: false,
                    result: json!({}),
                })?;
            }
            Action::Emit { source, args } => {
                let node = state.node(&source.node).context("Node not found")?;
                self.inner.host.command_as(
                    Command::Emit {
                        workflow: node.workspace.clone().into(),
                        node: source.node.into(),
                        pin: source.pin,
                        delivery: crate::model::id().into(),
                        args,
                    },
                    Some(node.workspace.clone().into()),
                )?;
            }
            Action::PublishProperties { id, properties } => {
                let node = state.node(&id).context("Node not found")?;
                self.inner.host.command_as(
                    Command::Publish {
                        workflow: node.workspace.clone().into(),
                        node: id.into(),
                        values: properties,
                    },
                    Some(node.workspace.clone().into()),
                )?;
            }
            Action::ExposeOutput { id, pin } => {
                let node = state.node(&id).context("Node not found")?;
                self.edit_graph(&node.workspace, |g| {
                    let spec = g.node_mut(&id).context("Node not found")?;
                    spec.flow.outputs.retain(|p| p.name != pin.name);
                    spec.flow.outputs.push(pin);
                    Ok(())
                })?;
            }
            Action::Pins { id } => {
                let node = state.node(&id).context("Node not found")?;
                return Ok(
                    json!({"inputs":crate::graph::pins(&state,node,false),"outputs":crate::graph::pins(&state,node,true)}),
                );
            }
            Action::ConnectPins { connection } => {
                let node = state
                    .node(&connection.source.node)
                    .context("Node not found")?;
                self.edit_graph(&node.workspace, |g| {
                    flow_core::graph::connect(g, connection)
                })?;
            }
            Action::DisconnectPins { connection } => {
                let node = state
                    .node(&connection.source.node)
                    .context("Node not found")?;
                self.edit_graph(&node.workspace, |g| {
                    g.connections.retain(|e| e != &connection);
                    Ok(())
                })?;
            }
            Action::Connect { source, target } => {
                let node = state.node(&source).context("Node not found")?;
                self.edit_graph(&node.workspace, |g| {
                    let spec = g.node(&source).context("Node not found")?;
                    let outputs = flow_core::graph::pins(g, spec, true);
                    let pin = outputs
                        .iter()
                        .find(|p| p.name == "out" && p.ty.is_signal())
                        .or_else(|| outputs.iter().find(|p| p.ty.is_signal()))
                        .context("Node has no signal output")?
                        .name
                        .clone();
                    flow_core::graph::connect(
                        g,
                        crate::graph::Connection {
                            source: crate::graph::Endpoint { node: source, pin },
                            target: crate::graph::Endpoint {
                                node: target,
                                pin: "in".into(),
                            },
                        },
                    )
                })?;
            }
            Action::Disconnect { source, target } => {
                let node = state.node(&source).context("Node not found")?;
                self.edit_graph(&node.workspace, |g| {
                    g.connections
                        .retain(|e| e.source.node != source || e.target.node != target);
                    Ok(())
                })?;
            }
            Action::Delete { id } => {
                let node = state.node(&id).context("Node not found")?;
                if node.desktop_window && node.window_id.is_some() {
                    self.application_operation(&id, "delete")?;
                }
                self.edit_graph(&node.workspace, |g| {
                    g.nodes.retain(|n| n.id.0 != id);
                    g.connections
                        .retain(|e| e.source.node != id && e.target.node != id);
                    Ok(())
                })?;
                state.remove(&id)?;
            }
            Action::Move { id, position } => {
                ensure!(
                    position.x.is_finite() && position.y.is_finite(),
                    "Invalid position"
                );
                state.node_mut(&id).context("Node not found")?.position = position;
            }
            Action::Arrange { positions } => {
                for (id, position) in positions {
                    state.node_mut(&id).context("Node not found")?.position = position;
                }
                let selected = state.selected_workspace.clone();
                state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == selected)
                    .unwrap()
                    .graph_arranged = true;
            }
            Action::SelectWorkspace { id } => {
                ensure!(
                    state.workspaces.iter().any(|w| w.id == id),
                    "Workflow not found"
                );
                state.selected_workspace = id;
            }
            Action::CreateWorkspace { name, cwd } => {
                let id = crate::model::id();
                let host = state
                    .orchestration
                    .as_ref()
                    .context("Runtime unavailable")?
                    .host_id
                    .clone();
                self.inner.host.command(Command::Create {
                    workflow: Workflow {
                        row_settings: Default::default(),
                        id: id.clone().into(),
                        name: name.clone(),
                        workspace: Some(WorkspaceId::new(&host.0, "user")?),
                        visibility: Visibility::Public,
                        owner: None,
                        references: Default::default(),
                        graph: Default::default(),
                        authority: host,
                        transfer: None,
                    },
                })?;
                state.workspaces.push(Workspace {
                    id: id.clone(),
                    name,
                    cwd,
                    notes: String::new(),
                    row: None,
                    window_array: false,
                    graph_arranged: false,
                    view: Default::default(),
                });
                state.selected_workspace = id;
            }
            Action::Notes { text } => {
                let id = state.selected_workspace.clone();
                state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == id)
                    .unwrap()
                    .notes = text;
            }
            Action::BindRow { workspace, row } => {
                flow_host::ipc::send(
                    &self.inner.provider_socket,
                    &json!({"desktop_protocol":1,"command":"bind_row","workflow":workspace,"row":row}),
                )?;
                state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .context("Workflow not found")?
                    .row = Some(row);
            }
            Action::ColumnMode { id, mode } => {
                let node = state.node(&id).context("Node not found")?;
                let workflow = node.workspace.clone();
                let column = node.column.clone().unwrap_or_else(|| node.id.clone());
                for node in &mut state.nodes {
                    if node.workspace == workflow
                        && node.column.as_ref().unwrap_or(&node.id) == &column
                    {
                        node.column_mode = mode;
                    }
                }
            }
            Action::RowView { workspace, view } => {
                state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .context("Workflow not found")?
                    .view = view;
                for node in state.nodes.iter_mut().filter(|n| n.workspace == workspace) {
                    node.column_mode = match view {
                        RowView::Nodes => ColumnMode::Node,
                        RowView::Windows => ColumnMode::Window,
                    };
                }
            }
            Action::ClearAlert { id } => {
                state
                    .node_mut(&id)
                    .context("Node not found")?
                    .flow
                    .properties
                    .remove("_alert");
            }
            Action::Alert {
                id,
                message,
                critical,
                notify,
            } => {
                if notify {
                    let node = state.node(&id).context("Node not found")?;
                    crate::notifications::present(
                        &self.inner.notification_command,
                        &node.title,
                        &message,
                        critical,
                    );
                }
                state
                    .node_mut(&id)
                    .context("Node not found")?
                    .flow
                    .properties
                    .insert(
                        "_alert".into(),
                        json!({"message":message,"critical":critical}),
                    );
            }
            Action::SaveRecipe { name, nodes } => {
                let recipe = state.recipe(name, &nodes)?;
                state.recipes.push(recipe);
            }
            Action::ImportRecipe { recipe } => {
                state.recipes.push(recipe);
            }
            Action::Instantiate { id } => {
                let recipe = state
                    .recipes
                    .iter()
                    .find(|r| r.id == id)
                    .context("Recipe not found")?
                    .clone();
                result = self.paste(&mut state, recipe, Position::default())?;
            }
            Action::Paste { recipe, offset } => {
                result = self.paste(&mut state, recipe, offset)?;
            }
            Action::Chat { id, text } => {
                let node = state.node(&id).context("Node not found")?;
                let context = crate::agent::graph_context(&state, &node.workspace);
                let session = node.agent_session.clone();
                self.edit_graph(&node.workspace, |g| {
                    let spec = g.node_mut(&id).context("Node not found")?;
                    spec.flow.properties.insert("command".into(), json!(text));
                    spec.flow
                        .properties
                        .insert("conversational".into(), json!(true));
                    Ok(())
                })?;
                result = self.run(
                    &state,
                    &id,
                    json!({"graph_context":context,"agent_session":session}),
                )?;
            }
            Action::HumanProgress { id, text } => {
                state.node_mut(&id).context("Node not found")?.output = text;
            }
            Action::Watch {
                source,
                target,
                threshold,
                above,
            } => {
                let source = state.node(&source).context("Source not found")?;
                let mut watcher = Node::new(
                    &source.workspace,
                    NodeKind::Extension,
                    source.cwd.clone(),
                    source.position.clone(),
                );
                watcher.flow.node_type = "local.watcher".into();
                watcher.command = source.command.clone();
                watcher
                    .flow
                    .properties
                    .insert("threshold".into(), json!(threshold));
                watcher.flow.properties.insert("above".into(), json!(above));
                watcher
                    .flow
                    .properties
                    .insert("interval_secs".into(), json!(source.metric.interval_secs));
                let spec = watcher.spec();
                let id = spec.id.0.clone();
                self.edit_graph(&source.workspace, |g| {
                    g.nodes.push(spec);
                    g.connections.push(crate::graph::Connection {
                        source: crate::graph::Endpoint {
                            node: id,
                            pin: "out".into(),
                        },
                        target: crate::graph::Endpoint {
                            node: target,
                            pin: "in".into(),
                        },
                    });
                    Ok(())
                })?;
            }
            Action::SyncDesktop { desktop } => {
                self.synchronize(&mut state, &desktop)?;
            }
            Action::ClearHistory => {
                let mut removed = std::collections::BTreeMap::<String, Vec<String>>::new();
                for node in state.nodes.iter().filter(|n| n.closed_window()) {
                    removed
                        .entry(node.workspace.clone())
                        .or_default()
                        .push(node.id.clone());
                }
                for (workflow, ids) in removed {
                    self.edit_graph(&workflow, |g| {
                        g.nodes.retain(|n| !ids.contains(&n.id.0));
                        g.connections.retain(|e| {
                            !ids.contains(&e.source.node) && !ids.contains(&e.target.node)
                        });
                        Ok(())
                    })?;
                }
                state.nodes.retain(|n| !n.closed_window());
            }
            Action::RegisterType { definition } => {
                self.edit_graph(&state.selected_workspace, |g| {
                    g.types.push(definition);
                    Ok(())
                })?;
            }
            Action::RegisterCast { definition } => {
                self.edit_graph(&state.selected_workspace, |g| {
                    g.casts.push(definition);
                    Ok(())
                })?;
            }
            Action::RegisterNode { definition } => {
                self.edit_graph(&state.selected_workspace, |g| {
                    g.node_types.push(definition);
                    Ok(())
                })?;
            }
            Action::MoveToWorkspace { .. } => anyhow::bail!(
                "Move the workflow or explicitly detach the node's connections before reassigning it"
            ),
        }
        if refresh_runtime {
            let runtime = self
                .inner
                .host
                .snapshot_as(Some(state.selected_workspace.clone().into()))?;
            crate::projection::apply(&mut state, &runtime);
            self.inner.bindings.lock().unwrap().apply_native(&mut state);
        }
        if let Some(writer) = self.inner.writer.lock().unwrap().as_ref() {
            writer.save(&state)?;
        }
        view.state = Arc::new(state);
        view.revision += 1;
        view.error = None;
        self.inner.changed.notify_all();
        Ok(result)
    }
    fn paste(&self, state: &mut State, mut recipe: Recipe, offset: Position) -> Result<Value> {
        for node in &mut recipe.nodes {
            node.position.x += offset.x;
            node.position.y += offset.y;
        }
        let workflow = state.selected_workspace.clone();
        let cwd = state.workspace().cwd.clone();
        let ids = state.instantiate(&recipe, &workflow, cwd)?;
        let nodes: Vec<_> = ids
            .iter()
            .filter_map(|id| state.node(id))
            .map(Node::spec)
            .collect();
        let edges: Vec<_> = state
            .connections
            .iter()
            .filter(|e| ids.contains(&e.source.node) && ids.contains(&e.target.node))
            .cloned()
            .collect();
        self.edit_graph(&workflow, |g| {
            g.nodes.extend(nodes);
            g.connections.extend(edges);
            Ok(())
        })?;
        Ok(json!({"ids":ids}))
    }
    fn synchronize(&self, state: &mut State, _desktop: &crate::niri::Desktop) -> Result<()> {
        crate::desktop_provider::bindings_at(&self.inner.provider_socket)?.apply(state);
        Ok(())
    }
}
