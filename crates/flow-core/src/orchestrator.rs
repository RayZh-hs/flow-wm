//! Runtime commands independent of UI commands and transport.
use crate::{domain::*, protocol::Command, providers::*};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub struct Orchestrator {
    pub state: RuntimeState,
    pub registry: Registry,
    pub signal_capacity: usize,
    pub replay_capacity: usize,
}
impl Orchestrator {
    pub fn new(state: RuntimeState, registry: Registry) -> Self {
        Self {
            state,
            registry,
            signal_capacity: 4096,
            replay_capacity: 65536,
        }
    }
    pub fn execute(
        &mut self,
        command: Command,
        caller: Option<&WorkflowId>,
        events: EventSink,
    ) -> Result<Value> {
        match command {
            Command::Negotiate { versions } => {
                ensure!(
                    versions.contains(&crate::protocol::ORCHESTRATOR_VERSION),
                    "Unsupported protocol version"
                );
                Ok(
                    json!({"protocol":2,"signal_protocol":1,"capabilities":["workflows","ownership","preserving_local_moves","typed_signals","invocations","reconnect_snapshot"]}),
                )
            }
            Command::Snapshot | Command::Subscribe { .. } => {
                let mut snapshot = self.state.clone();
                snapshot
                    .workflows
                    .retain(|id, _| self.state.visible(id, caller));
                snapshot
                    .executions
                    .retain(|_, e| snapshot.workflows.contains_key(&e.workflow));
                snapshot.deliveries.retain(|d| caller == Some(&d.target));
                snapshot
                    .transfers
                    .retain(|_, t| snapshot.workflows.contains_key(&t.workflow));
                Ok(serde_json::to_value(snapshot)?)
            }
            Command::Workspaces => Ok(serde_json::to_value(&self.state.workspaces)?),
            Command::Create { workflow } => {
                if let Some(owner) = &workflow.owner {
                    ensure!(
                        caller == Some(owner),
                        "Private workflow creation requires its owner"
                    );
                }
                workflow.graph.validate()?;
                let id = workflow.id.clone();
                self.state.create(workflow)?;
                Ok(json!({"workflow":id}))
            }
            Command::Destroy { workflow } => {
                self.state.lookup(&workflow, caller)?;
                let cleanup = self.state.destroy(&workflow)?;
                let mut pending = vec![];
                for execution in cleanup {
                    match self
                        .registry
                        .provider(&execution.provider)
                        .and_then(|p| p.interrupt(&execution.handle))
                    {
                        Ok(()) => (),
                        Err(_) => pending.push(execution.id),
                    }
                }
                Ok(json!({"cleanup_pending":pending}))
            }
            Command::Move {
                workflow,
                workspace,
            } => {
                self.state.lookup(&workflow, caller)?;
                self.state.reassign(&workflow, &workspace)?;
                Ok(Value::Null)
            }
            Command::MakePublic { workflow } => {
                self.state
                    .make_public(&workflow, caller.context("Make Public requires the owner")?)?;
                Ok(Value::Null)
            }
            Command::Reference { workflow, target } => {
                self.state.lookup(&workflow, caller)?;
                self.state.reference(&workflow, &target)?;
                Ok(Value::Null)
            }
            Command::Unreference { workflow, target } => {
                self.state.lookup(&workflow, caller)?;
                self.state
                    .workflows
                    .get_mut(&workflow)
                    .unwrap()
                    .references
                    .remove(&target);
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::ReplaceGraph { workflow, graph } => {
                self.state.lookup(&workflow, caller)?;
                ensure!(
                    !self
                        .state
                        .executions
                        .values()
                        .any(|e| e.workflow == workflow && e.active()),
                    "Graph edits affecting running nodes require interruption first"
                );
                graph.validate()?;
                self.state.workflows.get_mut(&workflow).unwrap().graph = graph;
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::Run {
                workflow,
                node,
                invocation,
                args,
            } => {
                let w = self.state.lookup(&workflow, caller)?;
                if let Some(execution) = self.state.executions.get(&invocation) {
                    ensure!(
                        execution.workflow == workflow && execution.node == node,
                        "Invocation ID already belongs to another node"
                    );
                    return Ok(serde_json::to_value(execution)?);
                }
                ensure!(w.transfer.is_none(), "Workflow handoff is in progress");
                let workspace = w
                    .workspace
                    .as_ref()
                    .context("Assign the workflow to an available workspace first")?;
                self.state.workspaces.require_active(workspace)?;
                let spec = w
                    .graph
                    .nodes
                    .iter()
                    .find(|n| n.id == node)
                    .context("Node not found")?;
                let definition = self.registry.definition(&spec.type_id)?;
                ensure!(
                    !matches!(
                        definition.behavior,
                        Behavior::Source | Behavior::Application
                    ),
                    "This provider does not support manual invocation"
                );
                let workspace_def = self.state.workspaces.definition(workspace)?;
                ensure!(
                    definition
                        .capabilities
                        .is_subset(&workspace_def.capabilities),
                    "Destination workspace is missing required capabilities"
                );
                crate::graph::signal_environment(&args)?;
                let provider = self.registry.provider(&definition.owner)?;
                let generation = self.state.revision + 1;
                let request = Invocation {
                    id: invocation.clone(),
                    workflow: workflow.clone(),
                    node: node.clone(),
                    type_id: spec.type_id.clone(),
                    properties: spec.flow.properties.clone(),
                    args,
                    generation,
                };
                let execution = Execution {
                    id: invocation.clone(),
                    workflow,
                    node,
                    generation,
                    host: workspace.host().into(),
                    provider: definition.owner.clone(),
                    handle: invocation.0.clone(),
                    status: ExecutionStatus::Running,
                    result: Value::Null,
                };
                self.state.executions.insert(invocation.clone(), execution);
                match provider.invoke(request, events) {
                    Ok(handle) => {
                        self.state.executions.get_mut(&invocation).unwrap().handle = handle
                    }
                    Err(error) => {
                        self.state.executions.get_mut(&invocation).unwrap().status =
                            ExecutionStatus::Failed;
                        self.state.executions.get_mut(&invocation).unwrap().result =
                            json!({"error":error.to_string()});
                        self.state.revision += 1;
                        return Err(error);
                    }
                }
                self.state.revision += 1;
                Ok(serde_json::to_value(&self.state.executions[&invocation])?)
            }
            Command::Interrupt { invocation } => {
                let execution = self
                    .state
                    .executions
                    .get(&invocation)
                    .context("Invocation not found")?;
                self.state.lookup(&execution.workflow, caller)?;
                if execution.active() {
                    self.registry
                        .provider(&execution.provider)?
                        .interrupt(&execution.handle)?;
                    self.state.executions.get_mut(&invocation).unwrap().status =
                        ExecutionStatus::Interrupting;
                    self.state.revision += 1;
                }
                Ok(Value::Null)
            }
            Command::Broadcast { delivery } => {
                ensure!(
                    caller == Some(&delivery.source),
                    "Broadcast requires its source workflow"
                );
                Ok(json!({"accepted": self.state.broadcast(delivery, self.signal_capacity)?}))
            }
            Command::Receive { workflow, channel } => {
                Ok(serde_json::to_value(self.state.receive(
                    &workflow,
                    &channel,
                    caller.context("Receive requires its destination workflow")?,
                )?)?)
            }
            Command::Acknowledge { delivery } => {
                self.state.acknowledge(
                    &delivery,
                    caller.context("Acknowledgement requires its destination workflow")?,
                    self.replay_capacity,
                )?;
                Ok(Value::Null)
            }
            Command::PrepareTransfer { .. } | Command::AbortTransfer { .. } => anyhow::bail!(
                "Remote ownership handoff is unavailable until the peer advertises durable transfer support"
            ),
        }
    }
}
impl FlowNet {
    pub fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            ensure!(
                !node.id.0.is_empty() && ids.insert(&node.id.0),
                "Duplicate or empty node ID"
            );
            ensure!(!node.type_id.is_empty(), "Node needs a registered type ID");
        }
        for edge in &self.connections {
            ensure!(
                ids.contains(&edge.source.node) && ids.contains(&edge.target.node),
                "Connections must remain inside a workflow"
            );
        }
        Ok(())
    }
}
