//! Host supervision. Scheduling decisions and all durable queues belong to core.
use crate::{config, remote::Remote, store::Store};
use anyhow::{Context, Result, ensure};
use flow_core::{
    domain::*, graph::Evaluation, orchestrator::Orchestrator, protocol::*, providers::*,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Host {
    worker: PathBuf,
    tx: mpsc::SyncSender<Message>,
    shared: Arc<Shared>,
    directory: PathBuf,
    remotes: BTreeMap<String, Remote>,
}
struct Shared {
    state: Mutex<RuntimeState>,
    changed: Condvar,
    closed: AtomicBool,
}
struct RequestMessage {
    request: Request,
    reply: mpsc::Sender<Reply>,
}
enum Message {
    Request(RequestMessage),
    Completion(Completion),
    Source(SourceEvent, mpsc::Sender<std::result::Result<bool, String>>),
    Evaluated(String, std::result::Result<Evaluation, String>),
    Capability(
        Command,
        Option<WorkflowId>,
        mpsc::Sender<std::result::Result<Value, String>>,
    ),
    Peer(String, RuntimeState),
    Network(NetworkResult),
    Shutdown,
}
struct NetworkResult {
    peers: BTreeMap<HostId, PeerHealth>,
    snapshots: Vec<(String, RuntimeState)>,
    staged: Vec<TransferId>,
    committed: Vec<TransferId>,
    forwarding: usize,
    pending: usize,
}
struct Supervisor {
    orchestrator: Orchestrator,
    limits: flow_builtins::local::Limits,
    remotes: BTreeMap<String, Remote>,
    peer_revisions: BTreeMap<String, u64>,
    network_busy: bool,
    network_due: Instant,
    peer_due: Instant,
    worker: PathBuf,
    evaluation: Option<(String, Arc<AtomicBool>)>,
    tx: mpsc::SyncSender<Message>,
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
impl Host {
    pub fn start(store: Store, worker: PathBuf, socket: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1024);
        let limits = config::runtime_limits()?;
        let mut state = store.load()?;
        state.bind_host(config::host_id()?)?;
        let desktop: Vec<_> = state
            .workspaces
            .definitions()
            .filter(|d| d.lifetime == Lifetime::Desktop)
            .filter_map(|d| {
                state
                    .workspaces
                    .availability(&d.id)
                    .map(|a| (d.id.clone(), a.incarnation.clone()))
            })
            .collect();
        for (workspace, incarnation) in desktop {
            state.workspaces.end(&workspace, &incarnation)?;
        }

        flow_builtins::sessions::LocalSessions::new(
            state.host_id.clone(),
            id(),
            limits
                .systemd_workspace
                .then(|| std::env::var("INVOCATION_ID").ok())
                .flatten(),
        )?
        .activate(&mut state.workspaces)?;
        let registry = flow_builtins::local::registry_with_environment(
            limits.clone(),
            flow_builtins::local::Environment {
                row_settings: config::load()?.row_settings,
                config_directory: Some(config::directory()),
                data_directory: Some(store.directory().into()),
                control_socket: Some(socket),
                cli: Some(worker.with_file_name("flowwm")),
                messaging: Some(Arc::new(Mailbox(tx.clone()))),
            },
        )?;
        let mut orchestrator = Orchestrator::new(state, registry);
        orchestrator.signal_capacity = limits.signal_queue_capacity;
        orchestrator.replay_capacity = limits.replay_capacity;
        orchestrator.storage = Some(Arc::new(store.clone()));
        orchestrator.recover();
        for source in orchestrator.state.sources.values() {
            orchestrator
                .state
                .queue
                .source_state
                .insert(source.node.clone(), (source.sequence, source.state.clone()));
        }
        orchestrator.state.sources.clear();
        orchestrator.checkpoint()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(orchestrator.state.clone()),
            changed: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let remotes = crate::remote::configured()?;
        let actor = Supervisor {
            orchestrator,
            limits,
            remotes: remotes.clone(),
            peer_revisions: Default::default(),
            network_busy: false,
            network_due: Instant::now(),
            peer_due: Instant::now(),
            worker: worker.clone(),
            evaluation: None,
            tx: tx.clone(),
        };
        let changes = shared.clone();
        thread::spawn(move || actor.work(rx, changes));
        Ok(Self {
            worker,
            tx,
            shared,
            directory: store.directory().into(),
            remotes,
        })
    }
    pub fn snapshot(&self) -> RuntimeState {
        self.shared.state.lock().unwrap().clone()
    }
    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }
    pub fn wake_subscribers(&self) {
        let _state = self.shared.state.lock().unwrap();
        self.shared.changed.notify_all();
    }
    pub fn wait_connected(&self, after: u64, disconnected: &AtomicBool) -> Option<RuntimeState> {
        let state = self.shared.state.lock().unwrap();
        let state = self
            .shared
            .changed
            .wait_while(state, |s| {
                s.revision <= after
                    && !self.shared.closed.load(Ordering::Acquire)
                    && !disconnected.load(Ordering::Acquire)
            })
            .unwrap();
        (!self.shared.closed.load(Ordering::Acquire) && !disconnected.load(Ordering::Acquire))
            .then(|| state.clone())
    }
    pub fn request(&self, request: Request) -> Result<Reply> {
        ensure!(
            request.protocol == ORCHESTRATOR_VERSION,
            "Unsupported orchestrator protocol {}; expected {}",
            request.protocol,
            ORCHESTRATOR_VERSION
        );
        if matches!(request.command, Command::Info) {
            return Ok(Reply {
                protocol: ORCHESTRATOR_VERSION,
                id: request.id,
                revision: self.snapshot().revision,
                result: Some(
                    json!({"protocol":ORCHESTRATOR_VERSION,"data_directory":self.directory,"config_directory":config::directory(),"worker":self.worker}),
                ),
                error: None,
            });
        }
        let state = self.snapshot();
        let target = match &request.command {
            Command::RowEnvironment { workflow }
            | Command::RowSettings { workflow }
            | Command::SetRowSettings { workflow, .. } => Some(workflow),
            Command::Read { workflow, .. }
            | Command::Run { workflow, .. }
            | Command::ReplaceGraph { workflow, .. }
            | Command::Publish { workflow, .. }
            | Command::Emit { workflow, .. }
            | Command::Move { workflow, .. }
            | Command::Destroy { workflow }
            | Command::Receive { workflow, .. } => Some(workflow),
            Command::Broadcast { delivery } => Some(&delivery.target),
            Command::Acknowledge { .. } => request.caller.as_ref(),
            _ => None,
        };
        if let Some(w) = target.and_then(|w| state.workflows.get(w))
            && w.authority != state.host_id
        {
            let remote = self
                .remotes
                .get(&w.authority.0)
                .context("Workflow host is disconnected or not configured")?;
            let result = remote.request(&request)?;
            if let Ok(value) = remote.request(&Request {
                protocol: ORCHESTRATOR_VERSION,
                id: id(),
                expected_revision: None,
                caller: Some(w.id.clone()),
                command: Command::Snapshot,
            }) && let Ok(peer) = serde_json::from_value(value)
            {
                let _ = self.tx.send(Message::Peer(w.authority.0.clone(), peer));
            }
            return Ok(Reply {
                protocol: ORCHESTRATOR_VERSION,
                id: request.id,
                revision: state.revision,
                result: Some(result),
                error: None,
            });
        }
        let destination = match &request.command {
            Command::Move { workspace, .. } => Some(workspace),
            Command::Create { workflow } => workflow.workspace.as_ref(),
            _ => None,
        };
        if let Some(workspace) = destination.filter(|w| w.host() != state.host_id.0) {
            let host = workspace.host();
            let remote = self
                .remotes
                .get(host)
                .context("Destination host is not configured")?;
            let peer = serde_json::from_value(remote.request(&Request {
                protocol: ORCHESTRATOR_VERSION,
                id: id(),
                expected_revision: None,
                caller: request.caller.clone(),
                command: Command::Snapshot,
            })?)?;
            self.tx.send(Message::Peer(host.into(), peer))?;
        }
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Message::Request(RequestMessage { request, reply }))?;
        rx.recv_timeout(Duration::from_secs(15))
            .context("Host command timed out")
    }
    pub fn shutdown(&self) {
        let _ = self.tx.send(Message::Shutdown);
        let state = self.shared.state.lock().unwrap();
        let _guard = self
            .shared
            .changed
            .wait_while(state, |_| !self.shared.closed.load(Ordering::Acquire))
            .unwrap();
    }
}
impl Supervisor {
    fn work(mut self, rx: mpsc::Receiver<Message>, shared: Arc<Shared>) {
        loop {
            let mut timeout = Duration::from_secs(3600);
            if self
                .orchestrator
                .state
                .executions
                .values()
                .any(|e| e.status == ExecutionStatus::Queued)
            {
                timeout = Duration::from_millis(50);
            }
            if !self.network_busy && !self.remotes.is_empty() {
                timeout = timeout.min(self.peer_due.saturating_duration_since(Instant::now()));
            }
            let network_pending = !self.orchestrator.state.forwarding.is_empty()
                || self
                    .orchestrator
                    .state
                    .peers
                    .values()
                    .any(|p| p.status != PeerStatus::Connected)
                || !self.orchestrator.state.pending_remote.is_empty()
                || self.orchestrator.state.executions.values().any(|e| {
                    e.active()
                        && self
                            .orchestrator
                            .state
                            .workflows
                            .get(&e.workflow)
                            .is_some_and(|w| w.authority != self.orchestrator.state.host_id)
                })
                || self.orchestrator.state.transfers.values().any(|t| {
                    !self.orchestrator.state.transfer_acks.contains(&t.id)
                        && matches!(t.phase, TransferPhase::Prepared | TransferPhase::Committed)
                });
            if network_pending && !self.network_busy {
                timeout = timeout.min(self.network_due.saturating_duration_since(Instant::now()));
            }
            let result = match rx.recv_timeout(timeout) {
                Ok(Message::Request(message)) => {
                    let request = message.request;
                    let result = if request
                        .expected_revision
                        .is_some_and(|r| r != self.orchestrator.state.revision)
                    {
                        Err(anyhow::anyhow!(
                            "Revision conflict; reconnect with a snapshot"
                        ))
                    } else {
                        let command = (|| -> Result<Command> {
                            match request.command {
                                Command::Move {
                                    workflow,
                                    workspace,
                                } if workspace.host() != self.orchestrator.state.host_id.0 => {
                                    Ok(Command::PrepareTransfer {
                                        transfer: request.id.clone().into(),
                                        workflow,
                                        destination: workspace,
                                    })
                                }
                                Command::Create { mut workflow }
                                    if workflow.workspace.as_ref().is_some_and(|w| {
                                        w.host() != self.orchestrator.state.host_id.0
                                    }) =>
                                {
                                    let destination = workflow.workspace.take().unwrap();
                                    let workflow_id = workflow.id.clone();
                                    workflow.authority = self.orchestrator.state.host_id.clone();
                                    if !self.orchestrator.state.workflows.contains_key(&workflow_id)
                                    {
                                        self.orchestrator.execute(
                                            Command::Create { workflow },
                                            request.caller.as_ref(),
                                            Arc::new(|_| {}),
                                        )?;
                                    }
                                    Ok(Command::PrepareTransfer {
                                        transfer: request.id.clone().into(),
                                        workflow: workflow_id,
                                        destination,
                                    })
                                }
                                command => Ok(command),
                            }
                        })();
                        let tx = self.tx.clone();
                        let read_only = command.as_ref().is_ok_and(|c| {
                            matches!(
                                c,
                                Command::Info
                                    | Command::Snapshot
                                    | Command::Workspaces
                                    | Command::RowSettings { .. }
                                    | Command::RowEnvironment { .. }
                                    | Command::Subscribe { .. }
                                    | Command::Negotiate { .. }
                                    | Command::Receive { .. }
                            )
                        });
                        command
                            .and_then(|command| {
                                self.orchestrator.execute(
                                    command,
                                    request.caller.as_ref(),
                                    Arc::new(move |c| {
                                        let _ = tx.send(Message::Completion(c));
                                    }),
                                )
                            })
                            .and_then(|value| {
                                if !read_only {
                                    self.orchestrator.checkpoint()?;
                                }
                                Ok(value)
                            })
                    };
                    let _ = message.reply.send(Reply {
                        protocol: ORCHESTRATOR_VERSION,
                        id: request.id,
                        revision: self.orchestrator.state.revision,
                        result: result.as_ref().ok().cloned(),
                        error: result.err().map(|e| Error {
                            code: "command_failed".into(),
                            message: format!("{e:#}"),
                            retryable: false,
                        }),
                    });
                    Ok(())
                }
                Ok(Message::Capability(command, caller, reply)) => {
                    if let Command::Broadcast { delivery } = &command
                        && let Some(workflow) =
                            self.orchestrator.state.workflows.get(&delivery.target)
                        && workflow.authority != self.orchestrator.state.host_id
                    {
                        let remote = self.remotes.get(&workflow.authority.0).cloned();
                        thread::spawn(move || {
                            let result = remote
                                .ok_or_else(|| anyhow::anyhow!("Destination host unavailable"))
                                .and_then(|remote| {
                                    remote.request(&Request {
                                        protocol: ORCHESTRATOR_VERSION,
                                        id: id(),
                                        expected_revision: None,
                                        caller,
                                        command,
                                    })
                                })
                                .map_err(|e| e.to_string());
                            let _ = reply.send(result);
                        });
                        continue;
                    }
                    let read_only = matches!(command, Command::Receive { .. });
                    let result = self
                        .orchestrator
                        .execute(command, caller.as_ref(), Arc::new(|_| {}))
                        .and_then(|v| {
                            if !read_only {
                                self.orchestrator.checkpoint()?;
                            }
                            Ok(v)
                        })
                        .map_err(|e| e.to_string());
                    let _ = reply.send(result);
                    Ok(())
                }
                Ok(Message::Source(event, reply)) => {
                    let result = (|| -> Result<bool> {
                        ensure!(
                            self.orchestrator
                                .state
                                .sources
                                .get(&event.node)
                                .is_some_and(|s| s.generation == event.generation),
                            "Source subscription was released or replaced"
                        );
                        self.orchestrator.source_event(event)
                    })()
                    .map_err(|e| e.to_string());
                    let _ = reply.send(result);
                    Ok(())
                }
                Ok(Message::Completion(completion)) => self
                    .orchestrator
                    .complete_invocation(completion)
                    .map(|_| ()),
                Ok(Message::Evaluated(id, result)) => {
                    self.evaluation = None;
                    match result {
                        Ok(result) => self.orchestrator.accept_frame(&id, result),
                        Err(error) => {
                            if let Some(frame) = self
                                .orchestrator
                                .state
                                .queue
                                .frames
                                .iter()
                                .find(|f| f.id == id)
                            {
                                let node = frame.root.0.clone();
                                self.orchestrator.accept_frame(
                                    &id,
                                    Evaluation {
                                        errors: BTreeMap::from([(node, error)]),
                                        ..Default::default()
                                    },
                                )
                            } else {
                                Ok(())
                            }
                        }
                    }
                }
                Ok(Message::Peer(host, state)) => self.merge_peer(&host, state),
                Ok(Message::Network(result)) => {
                    self.network_busy = false;
                    self.orchestrator.state.peers.extend(result.peers);
                    for (host, state) in result.snapshots {
                        let _ = self.merge_peer(&host, state);
                    }
                    for transfer in result.staged {
                        if self
                            .orchestrator
                            .state
                            .transfers
                            .get(&transfer)
                            .is_some_and(|t| t.phase == TransferPhase::Prepared)
                        {
                            let _ = self.orchestrator.state.commit_outgoing(&transfer);
                        }
                    }
                    self.orchestrator
                        .state
                        .transfer_acks
                        .extend(result.committed);
                    self.orchestrator
                        .state
                        .forwarding
                        .drain(..result.forwarding);
                    self.orchestrator
                        .state
                        .pending_remote
                        .drain(..result.pending);
                    self.orchestrator.state.revision += 1;
                    self.orchestrator.checkpoint()
                }
                Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some((_, cancel)) = &self.evaluation {
                        cancel.store(true, Ordering::Release);
                    }
                    for execution in self
                        .orchestrator
                        .state
                        .executions
                        .values()
                        .filter(|e| e.active() && e.host == self.orchestrator.state.host_id)
                    {
                        if let Ok(provider) =
                            self.orchestrator.registry.provider(&execution.provider)
                        {
                            let _ = provider.interrupt(&execution.handle);
                        }
                    }
                    for source in self.orchestrator.state.sources.values() {
                        if let Ok(provider) = self.orchestrator.registry.provider(&source.provider)
                        {
                            let _ = provider.unsubscribe(&source.handle);
                        }
                    }
                    let deadline = Instant::now()
                        + Duration::from_millis(
                            self.limits.interrupt_grace_ms + self.limits.terminate_grace_ms + 1000,
                        );
                    while Instant::now() < deadline
                        && self.orchestrator.state.executions.values().any(|e| {
                            e.host == self.orchestrator.state.host_id
                                && matches!(
                                    e.status,
                                    ExecutionStatus::Running
                                        | ExecutionStatus::Interrupting
                                        | ExecutionStatus::Waiting
                                )
                        })
                    {
                        if let Ok(Message::Completion(completion)) =
                            rx.recv_timeout(Duration::from_millis(20))
                        {
                            let _ = self.orchestrator.complete_invocation(completion);
                        }
                    }
                    let _ = self.orchestrator.checkpoint();
                    shared.closed.store(true, Ordering::Release);
                    shared.changed.notify_all();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("Runtime: {error:#}");
            }
            if let Some((id, cancel)) = &self.evaluation
                && !self
                    .orchestrator
                    .state
                    .queue
                    .frames
                    .iter()
                    .any(|f| &f.id == id)
            {
                cancel.store(true, Ordering::Release);
            }
            if let Err(error) = self.orchestrator.resume_completions() {
                eprintln!("Completion routing: {error:#}");
            }
            let tx = self.tx.clone();
            if let Err(error) = self.orchestrator.reconcile_sources(Arc::new(move |e| {
                let (reply, rx) = mpsc::channel();
                tx.send(Message::Source(e, reply))?;
                rx.recv_timeout(Duration::from_secs(10))?
                    .map_err(anyhow::Error::msg)?;
                Ok(())
            })) {
                eprintln!("Sources: {error:#}");
            }
            let tx = self.tx.clone();
            if let Err(error) = self.orchestrator.dispatch_ready(
                self.limits.max_concurrent,
                self.limits.serialize_working_directories,
                Arc::new(move |c| {
                    let _ = tx.send(Message::Completion(c));
                }),
            ) {
                eprintln!("Dispatch: {error:#}");
            }
            if self.evaluation.is_none()
                && let Some((frame, graph, context)) = self.orchestrator.next_frame()
            {
                let executable = self.worker.clone();
                let limits = self.limits.clone();
                let tx = self.tx.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                self.evaluation = Some((frame.id.clone(), cancel.clone()));
                thread::spawn(move || {
                    let result = flow_builtins::worker::evaluate_isolated(
                        &executable,
                        &limits,
                        graph,
                        context,
                        &frame.root.0,
                        if frame.data {
                            None
                        } else {
                            Some((frame.pin, frame.args))
                        },
                        cancel,
                    )
                    .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Message::Evaluated(frame.id, result));
                });
            }
            if Instant::now() >= self.network_due {
                self.reconcile_network();
                self.network_due =
                    Instant::now() + Duration::from_secs(self.limits.remote_retry_secs);
            }
            let mut state = shared.state.lock().unwrap();
            if state.revision != self.orchestrator.state.revision {
                *state = self.orchestrator.state.clone();
                shared.changed.notify_all();
            }
        }
    }
    fn merge_peer(&mut self, host: &str, remote: flow_core::domain::RuntimeState) -> Result<()> {
        ensure!(
            remote.host_id.0 == host,
            "Configured host ID differs from peer identity"
        );
        if self
            .peer_revisions
            .get(host)
            .is_some_and(|revision| *revision >= remote.revision)
        {
            return Ok(());
        }
        self.peer_revisions.insert(host.into(), remote.revision);

        for id in &remote.destroyed {
            if self
                .orchestrator
                .state
                .workflows
                .get(id)
                .is_some_and(|w| w.authority.0 == host)
            {
                let caller = self.orchestrator.state.workflows[id].owner.clone();
                self.orchestrator.execute(
                    Command::Destroy {
                        workflow: id.clone(),
                    },
                    caller.as_ref(),
                    Arc::new(|_| {}),
                )?;
            }
        }
        for definition in remote
            .workspaces
            .definitions()
            .filter(|d| d.id.host() == host)
        {
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
            } else if let Some(old) = self
                .orchestrator
                .state
                .workspaces
                .availability(&definition.id)
                .cloned()
            {
                self.orchestrator
                    .state
                    .workspaces
                    .end(&definition.id, &old.incarnation)?;
            }
        }
        let foreign: HashSet<_> = remote
            .workflows
            .values()
            .filter(|w| w.authority.0 == host)
            .map(|w| w.id.clone())
            .collect();
        for execution in remote
            .executions
            .into_values()
            .filter(|e| foreign.contains(&e.workflow))
        {
            let replace = self
                .orchestrator
                .state
                .executions
                .get(&execution.id)
                .is_none_or(|old| {
                    old.generation < execution.generation
                        || (old.generation == execution.generation && old.active())
                });
            if replace {
                self.orchestrator
                    .state
                    .executions
                    .insert(execution.id.clone(), execution);
            }
        }
        for (id, workflow) in remote.workflows {
            if workflow.authority.0 == host
                && !self.orchestrator.state.destroyed.contains(&id)
                && self
                    .orchestrator
                    .state
                    .workflows
                    .get(&id)
                    .is_none_or(|old| old.authority.0 == host)
            {
                self.orchestrator.state.workflows.insert(id, workflow);
            }
        }
        self.orchestrator.state.revision += 1;
        Ok(())
    }
    fn reconcile_network(&mut self) {
        if self.network_busy {
            return;
        }
        use flow_core::{domain::TransferPhase, protocol::Command};
        let mut hosts: HashSet<_> = self
            .orchestrator
            .state
            .executions
            .values()
            .filter(|e| e.active())
            .filter_map(|e| self.orchestrator.state.workflows.get(&e.workflow))
            .filter(|w| w.authority != self.orchestrator.state.host_id)
            .map(|w| (w.authority.0.clone(), Some(w.id.clone())))
            .collect();
        hosts.extend(
            self.orchestrator
                .state
                .peers
                .iter()
                .filter(|(_, p)| p.status != PeerStatus::Connected)
                .filter(|(id, _)| self.remotes.contains_key(&id.0))
                .map(|(id, _)| (id.0.clone(), None)),
        );
        if Instant::now() >= self.peer_due {
            hosts.extend(self.remotes.keys().cloned().map(|host| (host, None)));
            self.peer_due = Instant::now()
                + Duration::from_secs(self.limits.remote_retry_secs.saturating_mul(15));
        }
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
            .take(self.limits.remote_batch_size)
            .filter_map(|t| self.orchestrator.state.transfer_bundle(&t.id).ok())
            .collect();
        let forwarding: Vec<_> = self
            .orchestrator
            .state
            .forwarding
            .iter()
            .take(self.limits.remote_batch_size)
            .cloned()
            .collect();
        let pending: Vec<_> = self
            .orchestrator
            .state
            .pending_remote
            .iter()
            .take(self.limits.remote_batch_size)
            .cloned()
            .collect();
        if hosts.is_empty() && transfers.is_empty() && forwarding.is_empty() && pending.is_empty() {
            return;
        }
        self.network_busy = true;
        let remotes = self.remotes.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut health = BTreeMap::new();
            let mut request = |host: &str, command, caller, id| -> Result<Value> {
                let result = (|| {
                    remotes
                        .get(host)
                        .ok_or_else(|| crate::remote::PeerFailure {
                            status: PeerStatus::Unavailable,
                            detail: "Remote host is not configured".into(),
                        })?
                        .request(&flow_core::protocol::Request {
                            protocol: ORCHESTRATOR_VERSION,
                            id,
                            expected_revision: None,
                            caller,
                            command,
                        })
                })();
                let peer = match &result {
                    Ok(_) => PeerHealth {
                        status: PeerStatus::Connected,
                        detail: String::new(),
                    },
                    Err(error) => PeerHealth {
                        status: error
                            .downcast_ref::<crate::remote::PeerFailure>()
                            .map(|e| e.status.clone())
                            .unwrap_or(PeerStatus::Failed),
                        detail: format!("{error:#}"),
                    },
                };
                health.insert(host.to_owned().into(), peer);
                result
            };
            let mut result = NetworkResult {
                peers: BTreeMap::new(),
                snapshots: vec![],
                staged: vec![],
                committed: vec![],
                forwarding: 0,
                pending: 0,
            };
            // A slow or disconnected peer must not stall runtime commands or incoming events.
            for (host, workflow) in hosts {
                if let Ok(value) = request(&host, Command::Snapshot, workflow, id())
                    && let Ok(snapshot) = serde_json::from_value(value)
                {
                    result.snapshots.push((host, snapshot));
                }
            }
            for bundle in transfers {
                let transfer = bundle.transfer.clone();
                let staged = transfer.phase == TransferPhase::Prepared;
                let command = if staged {
                    Command::StageTransfer { bundle }
                } else {
                    Command::CommitTransfer { bundle }
                };
                if request(
                    transfer.destination.host(),
                    command,
                    None,
                    transfer.id.0.clone(),
                )
                .is_ok()
                {
                    if staged {
                        result.staged.push(transfer.id);
                    } else {
                        result.committed.push(transfer.id);
                    }
                }
            }
            for pending in forwarding {
                if request(
                    &pending.destination.0,
                    Command::ForwardCompletion {
                        execution: pending.execution.clone(),
                    },
                    None,
                    pending.execution.id.0,
                )
                .is_err()
                {
                    break;
                }
                result.forwarding += 1;
            }
            for pending in pending {
                if request(&pending.host.0, pending.command, pending.caller, id()).is_err() {
                    break;
                }
                result.pending += 1;
            }
            result.peers = health;
            let _ = tx.send(Message::Network(result));
        });
    }
}

struct Mailbox(mpsc::SyncSender<Message>);
impl Mailbox {
    fn call(&self, command: Command, caller: Option<WorkflowId>) -> Result<Value> {
        let (tx, rx) = mpsc::channel();
        self.0.send(Message::Capability(command, caller, tx))?;
        rx.recv_timeout(Duration::from_secs(10))?
            .map_err(anyhow::Error::msg)
    }
}
impl Messaging for Mailbox {
    fn broadcast(&self, delivery: Delivery) -> Result<bool> {
        let source = delivery.source.clone();
        Ok(
            self.call(Command::Broadcast { delivery }, Some(source))?["accepted"]
                .as_bool()
                .unwrap_or(false),
        )
    }
    fn receive(&self, workflow: WorkflowId, channel: String) -> Result<Option<Delivery>> {
        Ok(serde_json::from_value(self.call(
            Command::Receive {
                workflow: workflow.clone(),
                channel,
            },
            Some(workflow),
        )?)?)
    }
    fn acknowledge(&self, workflow: WorkflowId, delivery: DeliveryId) -> Result<()> {
        self.call(Command::Acknowledge { delivery }, Some(workflow))?;
        Ok(())
    }
}
