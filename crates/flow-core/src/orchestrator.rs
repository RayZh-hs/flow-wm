//! Runtime commands independent of UI commands and transport.
use crate::{domain::*, protocol::Command, providers::*};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub struct Orchestrator {
    pub state: RuntimeState,
    pub registry: Registry,
    pub storage: Option<std::sync::Arc<dyn Storage>>,
    pub output: Option<OutputSink>,
    pub signal_capacity: usize,
    pub replay_capacity: usize,
}
impl Orchestrator {
    pub fn new(state: RuntimeState, registry: Registry) -> Self {
        Self {
            state,
            registry,
            storage: None,
            output: None,
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
            Command::OriginInterrupt {
                invocation,
                generation,
            } => {
                let execution = self
                    .state
                    .executions
                    .get(&invocation)
                    .context("Unknown originating invocation")?;
                ensure!(
                    execution.host == self.state.host_id && execution.generation == generation,
                    "Wrong invocation origin or generation"
                );
                self.registry
                    .provider(&execution.provider)?
                    .interrupt(&execution.handle)?;
                Ok(Value::Null)
            }

            Command::StageTransfer { bundle } => {
                self.state.stage_transfer(bundle)?;
                Ok(Value::Null)
            }
            Command::CommitTransfer { bundle } => {
                self.state.commit_incoming(bundle)?;
                Ok(Value::Null)
            }
            Command::ForwardCompletion { execution } => {
                let existing = self
                    .state
                    .executions
                    .get(&execution.id)
                    .context("Unknown forwarded invocation")?;
                ensure!(
                    existing.host == execution.host && existing.generation == execution.generation,
                    "Invalid execution origin or generation"
                );
                self.state.complete(
                    &execution.id,
                    execution.generation,
                    execution.result,
                    execution.status,
                )?;
                Ok(Value::Null)
            }

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
                snapshot.incoming.clear();
                snapshot.forwarding.clear();
                snapshot.forwarding_targets.clear();
                snapshot.pending_remote.clear();
                snapshot.transfer_acks.clear();
                snapshot.destroyed.clear();
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
                if let Some(bundle) = self
                    .state
                    .incoming
                    .values()
                    .find(|b| b.workflow.id == workflow)
                {
                    ensure!(
                        bundle.workflow.visibility == Visibility::Public
                            || bundle.workflow.owner.as_ref() == caller,
                        "Incoming workflow is private"
                    );
                } else {
                    self.state.lookup(&workflow, caller)?;
                }
                let cleanup = self.state.destroy(&workflow)?;
                let mut pending = vec![];
                for execution in cleanup {
                    if execution.host != self.state.host_id {
                        self.state
                            .pending_remote
                            .push_back(crate::transfer::RemoteCommand {
                                host: execution.host.clone(),
                                caller: None,
                                command: Command::OriginInterrupt {
                                    invocation: execution.id.clone(),
                                    generation: execution.generation,
                                },
                            });
                        continue;
                    }
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
                let owner = caller.context("Make Public requires the owner")?;
                let authority = self
                    .state
                    .workflows
                    .get(&workflow)
                    .context("Workflow not found")?
                    .authority
                    .clone();
                self.state.make_public(&workflow, owner)?;
                if authority != self.state.host_id {
                    self.state
                        .pending_remote
                        .push_back(crate::transfer::RemoteCommand {
                            host: authority,
                            caller: Some(owner.clone()),
                            command: Command::MakePublic { workflow },
                        });
                }
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
                ensure!(
                    w.authority == self.state.host_id,
                    "Workflow authority moved to another host"
                );
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
                let previous_revision = self.state.revision;
                self.state.executions.insert(invocation.clone(), execution);
                self.state.revision += 1;
                if let Some(storage) = &self.storage
                    && let Err(error) = storage.save(&self.state)
                {
                    self.state.executions.remove(&invocation);
                    self.state.revision = previous_revision;
                    return Err(error);
                }
                match provider.invoke_observed(request, events, self.output.clone()) {
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
                Ok(serde_json::to_value(&self.state.executions[&invocation])?)
            }
            Command::Resolve {
                invocation,
                accepted,
                result,
            } => {
                let execution = self
                    .state
                    .executions
                    .get(&invocation)
                    .context("Invocation not found")?;
                self.state.lookup(&execution.workflow, caller)?;
                ensure!(
                    execution.active(),
                    "Invocation is no longer awaiting a resolution"
                );
                ensure!(
                    execution.host == self.state.host_id,
                    "Resolve the invocation at its originating host"
                );
                self.registry.provider(&execution.provider)?.resolve(
                    &execution.handle,
                    accepted,
                    result,
                )?;
                Ok(Value::Null)
            }
            Command::Interrupt { invocation } => {
                let execution = self
                    .state
                    .executions
                    .get(&invocation)
                    .context("Invocation not found")?;
                self.state.lookup(&execution.workflow, caller)?;
                if execution.active() {
                    if execution.host == self.state.host_id {
                        self.registry
                            .provider(&execution.provider)?
                            .interrupt(&execution.handle)?;
                    } else {
                        self.state
                            .pending_remote
                            .push_back(crate::transfer::RemoteCommand {
                                host: execution.host.clone(),
                                caller: None,
                                command: Command::OriginInterrupt {
                                    invocation: execution.id.clone(),
                                    generation: execution.generation,
                                },
                            });
                    }
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
            Command::PrepareTransfer {
                transfer,
                workflow,
                destination,
            } => {
                self.state.lookup(&workflow, caller)?;
                let record = self
                    .state
                    .prepare_transfer(transfer, &workflow, destination)?;
                Ok(serde_json::to_value(record)?)
            }
            Command::AbortTransfer { transfer } => {
                self.state.abort_transfer(&transfer)?;
                Ok(Value::Null)
            }
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
        let mut validation = crate::model::State::empty(".".into());
        validation.workspaces[0].id = "validation".into();
        validation.selected_workspace = "validation".into();
        validation.types = self.types.clone();
        validation.casts = self.casts.clone();
        validation.node_types = self.node_types.clone();
        validation.connections = self.connections.clone();
        for spec in &self.nodes {
            let type_id = spec.type_id.strip_prefix("local.").unwrap_or(&spec.type_id);
            let kind = if type_id == "constant" {
                crate::model::NodeKind::Constant
            } else {
                crate::model::NodeKind::Shell
            };
            let mut node =
                crate::model::Node::new("validation", kind, ".".into(), Default::default());
            node.id = spec.id.0.clone();
            node.flow = spec.flow.clone();
            node.flow.node_type = type_id.into();
            validation.nodes.push(node);
        }
        validation.validate()
    }
}
