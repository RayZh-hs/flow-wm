//! Versioned domain model. Desktop rows and presentation never define runtime ownership.
use crate::graph::{CastDefinition, Connection, Flow, NodeDefinition, Pin, Type, TypeDefinition};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

macro_rules! identifier {
    ($($name:ident),+) => {$ (
        #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name { fn from(value: &str) -> Self { Self(value.into()) } }
        impl From<String> for $name { fn from(value: String) -> Self { Self(value) } }
        impl std::fmt::Display for $name { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) } }
    )+};
}
identifier!(
    WorkflowId,
    NodeId,
    InvocationId,
    ProviderId,
    HostId,
    TransferId,
    DeliveryId,
    WorkspaceId
);
impl WorkspaceId {
    pub fn new(host: &str, session: &str) -> Result<Self> {
        ensure!(
            !host.is_empty()
                && !session.is_empty()
                && !host.contains(':')
                && !session.contains(':'),
            "Workspace must be host:session"
        );
        Ok(Self(format!("{host}:{session}")))
    }
    pub fn host(&self) -> &str {
        self.0.split(':').next().unwrap_or_default()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    Desktop,
    User,
    Persistent,
    Provider,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDefinition {
    pub id: WorkspaceId,
    pub provider: ProviderId,
    pub lifetime: Lifetime,
    pub capabilities: BTreeSet<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub incarnation: String,
    pub active: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceRegistry {
    definitions: BTreeMap<WorkspaceId, WorkspaceDefinition>,
    availability: BTreeMap<WorkspaceId, Availability>,
}
impl WorkspaceRegistry {
    pub fn register(&mut self, definition: WorkspaceDefinition) -> Result<()> {
        let (host, session) = definition
            .id
            .0
            .split_once(':')
            .context("Workspace must be host:session")?;
        WorkspaceId::new(host, session)?;
        if let Some(existing) = self.definitions.get(&definition.id) {
            ensure!(
                existing == &definition,
                "Workspace definitions are immutable while installed"
            );
        } else {
            self.definitions.insert(definition.id.clone(), definition);
        }
        Ok(())
    }
    pub fn definitions(&self) -> impl Iterator<Item = &WorkspaceDefinition> {
        self.definitions.values()
    }
    pub fn definition(&self, id: &WorkspaceId) -> Result<&WorkspaceDefinition> {
        self.definitions
            .get(id)
            .context("Unknown predefined workspace")
    }
    pub fn availability(&self, id: &WorkspaceId) -> Option<&Availability> {
        self.availability.get(id)
    }
    pub fn activate(
        &mut self,
        id: &WorkspaceId,
        provider: &ProviderId,
        incarnation: String,
    ) -> Result<()> {
        ensure!(
            &self.definition(id)?.provider == provider,
            "Workspace provider does not own this session"
        );
        ensure!(
            !incarnation.is_empty(),
            "A session needs an incarnation token"
        );
        self.availability.insert(
            id.clone(),
            Availability {
                incarnation,
                active: true,
            },
        );
        Ok(())
    }
    pub fn require_active(&self, id: &WorkspaceId) -> Result<()> {
        self.definition(id)?;
        ensure!(
            self.availability(id).is_some_and(|a| a.active),
            "Workspace is unavailable"
        );
        Ok(())
    }
    pub fn end(&mut self, id: &WorkspaceId, incarnation: &str) -> Result<()> {
        let active = self
            .availability
            .get_mut(id)
            .context("Session is inactive")?;
        ensure!(
            active.incarnation == incarnation,
            "Stale session incarnation"
        );
        active.active = false;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeSpec {
    pub id: NodeId,
    pub type_id: String,
    pub flow: Flow,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FlowNet {
    pub nodes: Vec<NodeSpec>,
    pub connections: Vec<Connection>,
    pub types: Vec<TypeDefinition>,
    pub casts: Vec<CastDefinition>,
    pub node_types: Vec<NodeDefinition>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    #[default]
    Public,
    Private,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workflow {
    pub id: WorkflowId,
    pub name: String,
    pub workspace: Option<WorkspaceId>,
    pub visibility: Visibility,
    pub owner: Option<WorkflowId>,
    pub references: BTreeSet<WorkflowId>,
    pub graph: FlowNet,
    pub authority: HostId,
    #[serde(default)]
    pub transfer: Option<TransferId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Interrupting,
    Complete,
    Failed,
    Interrupted,
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Execution {
    pub id: InvocationId,
    pub workflow: WorkflowId,
    pub node: NodeId,
    pub generation: u64,
    pub host: HostId,
    pub provider: ProviderId,
    pub handle: String,
    pub status: ExecutionStatus,
    pub result: Value,
}
impl Execution {
    pub fn active(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Running | ExecutionStatus::Interrupting
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub id: DeliveryId,
    pub source: WorkflowId,
    pub target: WorkflowId,
    pub channel: String,
    pub ty: Type,
    pub args: Value,
    pub sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    Prepared,
    Accepted,
    Committed,
    Aborted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub id: TransferId,
    pub workflow: WorkflowId,
    pub source: HostId,
    pub destination: WorkspaceId,
    pub phase: TransferPhase,
    pub revision: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeState {
    pub revision: u64,
    pub workspaces: WorkspaceRegistry,
    pub workflows: BTreeMap<WorkflowId, Workflow>,
    pub executions: BTreeMap<InvocationId, Execution>,
    pub transfers: BTreeMap<TransferId, Transfer>,
    pub deliveries: VecDeque<Delivery>,
    #[serde(default)]
    pub acknowledged: BTreeSet<DeliveryId>,
}
impl RuntimeState {
    pub fn local() -> Self {
        let mut state = Self::default();
        for (session, provider, lifetime, caps) in [
            (
                "desktop",
                "desktop",
                Lifetime::Desktop,
                vec!["application", "shell", "python"],
            ),
            ("user", "local", Lifetime::User, vec!["shell", "python"]),
            (
                "systemd",
                "systemd",
                Lifetime::Persistent,
                vec!["shell", "python"],
            ),
        ] {
            state
                .workspaces
                .register(WorkspaceDefinition {
                    id: WorkspaceId::new("local", session).unwrap(),
                    provider: provider.into(),
                    lifetime,
                    capabilities: caps.into_iter().map(String::from).collect(),
                })
                .unwrap();
        }
        state
    }
    pub fn visible(&self, id: &WorkflowId, caller: Option<&WorkflowId>) -> bool {
        self.workflows.get(id).is_some_and(|w| {
            w.visibility == Visibility::Public
                || caller == Some(&w.id)
                || w.owner.as_ref() == caller && caller.is_some()
        })
    }
    pub fn lookup(&self, id: &WorkflowId, caller: Option<&WorkflowId>) -> Result<&Workflow> {
        ensure!(
            self.visible(id, caller),
            "Workflow is unavailable or private"
        );
        Ok(&self.workflows[id])
    }
    pub fn create(&mut self, workflow: Workflow) -> Result<()> {
        ensure!(
            !self.workflows.contains_key(&workflow.id),
            "Duplicate workflow ID"
        );
        if let Some(workspace) = &workflow.workspace {
            self.workspaces.require_active(workspace)?;
        }
        match (&workflow.visibility, &workflow.owner) {
            (Visibility::Private, Some(parent)) => {
                self.workflows
                    .get(parent)
                    .context("Owning workflow does not exist")?;
            }
            (Visibility::Public, None) => (),
            _ => anyhow::bail!(
                "Private workflows need one owner; public workflows cannot have an owner"
            ),
        }
        self.workflows.insert(workflow.id.clone(), workflow);
        self.revision += 1;
        Ok(())
    }
    pub fn reference(&mut self, parent: &WorkflowId, target: &WorkflowId) -> Result<()> {
        ensure!(
            self.lookup(target, None)?.visibility == Visibility::Public,
            "Only public workflows can be referenced"
        );
        ensure!(parent != target, "A workflow cannot reference itself");
        self.workflows
            .get_mut(parent)
            .context("Workflow not found")?
            .references
            .insert(target.clone());
        self.revision += 1;
        Ok(())
    }
    pub fn make_public(&mut self, id: &WorkflowId, caller: &WorkflowId) -> Result<()> {
        ensure!(
            self.workflows
                .get(id)
                .context("Workflow not found")?
                .owner
                .as_ref()
                == Some(caller),
            "Only the owning workflow can release ownership"
        );
        let workflow = self.workflows.get_mut(id).unwrap();
        workflow.owner = None;
        workflow.visibility = Visibility::Public;
        self.workflows
            .get_mut(caller)
            .context("Owner not found")?
            .references
            .insert(id.clone());
        self.revision += 1;
        Ok(())
    }
    pub fn reassign(&mut self, id: &WorkflowId, workspace: &WorkspaceId) -> Result<()> {
        self.workspaces.require_active(workspace)?;
        let workflow = self.workflows.get_mut(id).context("Workflow not found")?;
        ensure!(
            workflow.transfer.is_none(),
            "Workflow handoff is in progress"
        );
        ensure!(
            workflow.authority.0 == workspace.host(),
            "Cross-host moves require a recoverable handoff"
        );
        workflow.workspace = Some(workspace.clone());
        self.revision += 1;
        Ok(())
    }
    /// Returns active handles for provider teardown; completed history is retained.
    pub fn destroy(&mut self, id: &WorkflowId) -> Result<Vec<Execution>> {
        ensure!(self.workflows.contains_key(id), "Workflow not found");
        let mut removed = BTreeSet::from([id.clone()]);
        loop {
            let count = removed.len();
            for w in self.workflows.values() {
                if w.owner.as_ref().is_some_and(|p| removed.contains(p)) {
                    removed.insert(w.id.clone());
                }
            }
            if removed.len() == count {
                break;
            }
        }
        self.workflows.retain(|id, _| !removed.contains(id));
        for w in self.workflows.values_mut() {
            w.references.retain(|r| !removed.contains(r));
        }
        self.deliveries
            .retain(|d| !removed.contains(&d.target) && !removed.contains(&d.source));
        let mut cleanup = vec![];
        for execution in self
            .executions
            .values_mut()
            .filter(|e| removed.contains(&e.workflow) && e.active())
        {
            execution.status = ExecutionStatus::Interrupting;
            cleanup.push(execution.clone());
        }
        for transfer in self
            .transfers
            .values_mut()
            .filter(|t| removed.contains(&t.workflow) && t.phase != TransferPhase::Committed)
        {
            transfer.phase = TransferPhase::Aborted;
        }
        self.revision += 1;
        Ok(cleanup)
    }
    pub fn end_session(
        &mut self,
        workspace: &WorkspaceId,
        incarnation: &str,
    ) -> Result<Vec<Execution>> {
        self.workspaces.end(workspace, incarnation)?;
        let ids: Vec<_> = self
            .workflows
            .values()
            .filter(|w| w.workspace.as_ref() == Some(workspace))
            .map(|w| w.id.clone())
            .collect();
        let mut cleanup = vec![];
        for id in ids {
            if self.workflows.contains_key(&id) {
                cleanup.extend(self.destroy(&id)?);
            }
        }
        Ok(cleanup)
    }
    pub fn complete(
        &mut self,
        id: &InvocationId,
        generation: u64,
        result: Value,
        status: ExecutionStatus,
    ) -> Result<bool> {
        ensure!(
            !matches!(
                status,
                ExecutionStatus::Running | ExecutionStatus::Interrupting
            ),
            "Expected a terminal execution status"
        );
        let execution = self.executions.get_mut(id).context("Unknown invocation")?;
        if execution.generation != generation || !execution.active() {
            return Ok(false);
        }
        execution.result = result;
        execution.status = status;
        self.revision += 1;
        Ok(true)
    }
    pub fn broadcast(&mut self, delivery: Delivery, capacity: usize) -> Result<bool> {
        self.lookup(&delivery.source, Some(&delivery.source))?;
        self.lookup(&delivery.target, Some(&delivery.source))?;
        let workflow = &self.workflows[&delivery.target];
        ensure!(
            workflow.transfer.is_none(),
            "Destination is transferring; retry with the same delivery ID"
        );
        if self.acknowledged.contains(&delivery.id)
            || self.deliveries.iter().any(|d| d.id == delivery.id)
        {
            return Ok(false);
        }
        ensure!(
            self.deliveries.len() < capacity,
            "Signal queue is full; retry with the same delivery ID"
        );
        let mut validation = crate::model::State::empty(".".into());
        validation.types = workflow.graph.types.clone();
        validation.casts = workflow.graph.casts.clone();
        crate::graph::validate_type(&validation, &delivery.ty, 0)?;
        ensure!(delivery.ty.is_signal(), "Broadcast requires a signal type");
        crate::graph::check_value(&validation, &delivery.args, &delivery.ty)?;
        self.deliveries.push_back(delivery);
        self.revision += 1;
        Ok(true)
    }
    pub fn receive(
        &mut self,
        workflow: &WorkflowId,
        channel: &str,
        caller: &WorkflowId,
    ) -> Result<Option<Delivery>> {
        ensure!(
            caller == workflow,
            "Receive is restricted to the destination workflow"
        );
        self.lookup(workflow, Some(caller))?;
        let Some(index) = self
            .deliveries
            .iter()
            .position(|d| &d.target == workflow && d.channel == channel)
        else {
            return Ok(None);
        };
        // Acknowledgement is explicit so a disconnected receiver can retry.
        Ok(self.deliveries.get(index).cloned())
    }
    pub fn acknowledge(
        &mut self,
        id: &DeliveryId,
        caller: &WorkflowId,
        replay_limit: usize,
    ) -> Result<()> {
        let Some(index) = self.deliveries.iter().position(|d| &d.id == id) else {
            ensure!(self.acknowledged.contains(id), "Unknown delivery");
            return Ok(());
        };
        ensure!(
            &self.deliveries[index].target == caller,
            "Only the destination may acknowledge delivery"
        );
        ensure!(
            self.acknowledged.len() < replay_limit,
            "Replay ledger is full; checkpoint before accepting more signals"
        );
        self.deliveries.remove(index);
        self.acknowledged.insert(id.clone());
        self.revision += 1;
        Ok(())
    }
    pub fn prepare_transfer(
        &mut self,
        id: TransferId,
        workflow: &WorkflowId,
        destination: WorkspaceId,
    ) -> Result<Transfer> {
        self.workspaces.require_active(&destination)?;
        if let Some(transfer) = self.transfers.get(&id) {
            ensure!(
                &transfer.workflow == workflow && transfer.destination == destination,
                "Transfer ID reused for a different move"
            );
            return Ok(transfer.clone());
        }
        let w = self
            .workflows
            .get_mut(workflow)
            .context("Workflow not found")?;
        ensure!(w.transfer.is_none(), "Another handoff is in progress");
        ensure!(
            destination.host() != w.authority.0,
            "Use local reassignment for this workspace"
        );
        let transfer = Transfer {
            id: id.clone(),
            workflow: workflow.clone(),
            source: w.authority.clone(),
            destination,
            phase: TransferPhase::Prepared,
            revision: self.revision,
        };
        w.transfer = Some(id.clone());
        self.transfers.insert(id, transfer.clone());
        self.revision += 1;
        Ok(transfer)
    }
    pub fn validate(&self) -> Result<()> {
        for (id, w) in &self.workflows {
            ensure!(id == &w.id, "Workflow identity mismatch");
            if let Some(workspace) = &w.workspace {
                self.workspaces.definition(workspace)?;
            }
            ensure!(
                (w.visibility == Visibility::Private) == w.owner.is_some(),
                "Invalid workflow ownership"
            );
            let mut seen = BTreeSet::from([id]);
            let mut owner = w.owner.as_ref();
            while let Some(parent) = owner {
                ensure!(seen.insert(parent), "Workflow ownership cycle");
                owner = self
                    .workflows
                    .get(parent)
                    .context("Missing owning workflow")?
                    .owner
                    .as_ref();
            }
            for target in &w.references {
                ensure!(
                    self.lookup(target, None)?.visibility == Visibility::Public,
                    "Reference to private workflow"
                );
            }
        }
        Ok(())
    }
}

/// WM renderers receive semantic content; native IDs and pixels remain in the WM.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RendererContract {
    pub id: String,
    pub minimum_size: [f64; 2],
    pub pins: Vec<Pin>,
    pub anchors: BTreeMap<String, [f64; 2]>,
    pub hit_regions: Vec<[f64; 4]>,
    pub fallback_label: String,
}
