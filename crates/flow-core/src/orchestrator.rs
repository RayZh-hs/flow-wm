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
        _events: EventSink,
    ) -> Result<Value> {
        match command {
            Command::UnregisterApplication {
                node,
                token,
                incarnation,
            } => {
                let application = self
                    .state
                    .applications
                    .get(&node)
                    .context("Application registration unavailable")?;
                ensure!(
                    application.token == token && application.incarnation == incarnation,
                    "Stale application registration"
                );
                self.state.lookup(&application.workflow, caller)?;
                self.state.applications.remove(&node);
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::RegisterApplication { registration } => {
                let workflow = self.state.lookup(&registration.workflow, caller)?;
                ensure!(
                    workflow.graph.node(&registration.node.0).is_some(),
                    "Application node not found"
                );
                self.state
                    .workspaces
                    .require_active(&registration.workspace)?;
                ensure!(
                    self.state
                        .workspaces
                        .availability(&registration.workspace)
                        .is_some_and(|a| a.incarnation == registration.incarnation),
                    "Stale application session"
                );
                if let Some(previous) = self.state.applications.get(&registration.node) {
                    ensure!(
                        previous.incarnation != registration.incarnation
                            || previous.token == registration.token,
                        "Application is already attached with another registration token"
                    );
                }
                self.state
                    .applications
                    .insert(registration.node.clone(), registration);
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::ApplicationEvent {
                node,
                token,
                incarnation,
                pin,
                delivery,
                args,
            } => {
                let application = self
                    .state
                    .applications
                    .get(&node)
                    .context("Application registration unavailable")?;
                ensure!(
                    application.token == token && application.incarnation == incarnation,
                    "Stale application event"
                );
                ensure!(
                    self.state
                        .workspaces
                        .availability(&application.workspace)
                        .is_some_and(|a| a.active && a.incarnation == incarnation),
                    "Application session ended"
                );
                self.execute(
                    Command::Emit {
                        workflow: application.workflow.clone(),
                        node,
                        pin,
                        delivery,
                        args,
                    },
                    caller,
                    _events,
                )
            }
            Command::Info => Ok(json!({"protocol": crate::protocol::ORCHESTRATOR_VERSION})),
            Command::ActivateSession {
                workspace,
                provider,
                incarnation,
            } => {
                self.state
                    .workspaces
                    .activate(&workspace, &provider, incarnation)?;
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::EndSession {
                workspace,
                incarnation,
            } => {
                let cleanup = self.state.end_session(&workspace, &incarnation)?;
                for execution in cleanup {
                    if execution.host != self.state.host_id {
                        self.state
                            .pending_remote
                            .push_back(crate::transfer::RemoteCommand {
                                host: execution.host,
                                caller: None,
                                command: Command::OriginInterrupt {
                                    invocation: execution.id,
                                    generation: execution.generation,
                                },
                            });
                    } else {
                        self.registry
                            .provider(&execution.provider)?
                            .interrupt(&execution.handle)?;
                    }
                }
                Ok(Value::Null)
            }
            Command::Publish {
                workflow,
                node,
                values,
            } => {
                let w = self.state.lookup(&workflow, caller)?;
                let spec = w.graph.node(&node.0).context("Node not found")?;
                for pin in crate::graph::pins(&w.graph, spec, true) {
                    if let Some(value) = values.get(pin.property.as_ref().unwrap_or(&pin.name)) {
                        crate::graph::check_value(&w.graph, value, &pin.ty)?;
                    }
                }
                self.state
                    .queue
                    .published
                    .entry(node)
                    .or_default()
                    .extend(values);
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::Emit {
                workflow,
                node,
                pin,
                delivery,
                args,
            } => {
                let w = self.state.lookup(&workflow, caller)?;
                let spec = w.graph.node(&node.0).context("Node not found")?;
                let output = crate::graph::pins(&w.graph, spec, true)
                    .into_iter()
                    .find(|p| p.name == pin && p.ty.is_signal())
                    .context("Signal output not found")?;
                crate::graph::check_value(&w.graph, &args, &output.ty)?;
                let correlation = delivery.0.clone();
                self.enqueue_frame(
                    crate::scheduler::Frame {
                        data: false,
                        id: delivery.0,
                        workflow,
                        root: node,
                        pin,
                        args,
                        correlation,
                    },
                    caller,
                )?;
                Ok(Value::Null)
            }

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
                self.complete_invocation(Completion {
                    invocation: execution.id,
                    generation: execution.generation,
                    result: execution.result,
                    status: execution.status,
                })?;
                Ok(Value::Null)
            }

            Command::Negotiate { versions } => {
                ensure!(
                    versions.contains(&crate::protocol::ORCHESTRATOR_VERSION),
                    "Unsupported protocol version"
                );
                Ok(
                    json!({"protocol":crate::protocol::ORCHESTRATOR_VERSION,"signal_protocol":1,"capabilities":["workflows","ownership","preserving_local_moves","typed_signals","invocations","reconnect_snapshot"]}),
                )
            }
            Command::Snapshot | Command::Subscribe { .. } => {
                let mut snapshot = self.state.clone();
                snapshot.incoming.clear();
                snapshot.forwarding.clear();
                snapshot.forwarding_targets.clear();
                snapshot.pending_remote.clear();
                snapshot.transfer_acks.clear();
                snapshot.destroyed.retain(|id| {
                    self.state.tombstones.get(id).is_some_and(|w| {
                        w.visibility == Visibility::Public
                            || caller == Some(id)
                            || w.owner.as_ref() == caller && caller.is_some()
                    })
                });
                snapshot
                    .tombstones
                    .retain(|id, _| snapshot.destroyed.contains(id));
                snapshot
                    .workflows
                    .retain(|id, _| self.state.visible(id, caller));
                snapshot
                    .executions
                    .retain(|_, e| snapshot.workflows.contains_key(&e.workflow));
                snapshot.deliveries.retain(|d| caller == Some(&d.target));
                snapshot
                    .sources
                    .retain(|_, s| snapshot.workflows.contains_key(&s.workflow));
                snapshot
                    .applications
                    .retain(|_, a| snapshot.workflows.contains_key(&a.workflow));
                let nodes: std::collections::BTreeSet<_> = snapshot
                    .workflows
                    .values()
                    .flat_map(|w| w.graph.nodes.iter().map(|n| n.id.clone()))
                    .collect();
                snapshot
                    .queue
                    .frames
                    .retain(|f| snapshot.workflows.contains_key(&f.workflow));
                snapshot
                    .queue
                    .invocations
                    .retain(|_, c| snapshot.workflows.contains_key(&c.request.workflow));
                snapshot
                    .queue
                    .completions
                    .retain(|id, _| snapshot.executions.contains_key(id));
                snapshot.queue.published.retain(|id, _| nodes.contains(id));
                snapshot.queue.computed.retain(|id, _| nodes.contains(id));
                snapshot
                    .queue
                    .source_state
                    .retain(|id, _| nodes.contains(id));
                snapshot
                    .queue
                    .resources
                    .retain(|id, _| snapshot.executions.contains_key(id));
                snapshot.queue.joins.retain(|key, _| {
                    snapshot
                        .workflows
                        .keys()
                        .any(|id| key.starts_with(&format!("{id}:")))
                });
                snapshot.queue.join_args.retain(|key, _| {
                    snapshot
                        .workflows
                        .keys()
                        .any(|id| key.starts_with(&format!("{id}:")))
                });
                snapshot.queue.joins_done.clear();
                snapshot.queue.routed.clear();
                snapshot.queue.accepted.clear();
                snapshot
                    .transfers
                    .retain(|_, t| snapshot.workflows.contains_key(&t.workflow));
                Ok(serde_json::to_value(snapshot)?)
            }
            Command::Workspaces => Ok(serde_json::to_value(&self.state.workspaces)?),
            Command::Create { mut workflow } => {
                ensure!(
                    workflow.authority == self.state.host_id,
                    "Create workflows at their authoritative host"
                );
                ensure!(
                    workflow
                        .workspace
                        .as_ref()
                        .is_none_or(|w| w.host() == self.state.host_id.0),
                    "Remote workspace assignment requires a host handoff"
                );
                if let Some(owner) = &workflow.owner {
                    ensure!(
                        caller == Some(owner),
                        "Private workflow creation requires its owner"
                    );
                }
                self.registry.prepare_graph(&mut workflow.graph);
                workflow.graph.validate()?;
                self.registry.validate_graph(&workflow.graph)?;
                let id = workflow.id.clone();
                self.state.create(workflow)?;
                Ok(json!({"workflow":id}))
            }
            Command::Destroy { workflow } => {
                if self.state.destroyed.contains(&workflow) {
                    return Ok(json!({"cleanup_pending":[]}));
                }
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
            Command::ReplaceGraph {
                workflow,
                mut graph,
            } => {
                self.state.lookup(&workflow, caller)?;
                let previous = &self.state.workflows[&workflow].graph;
                ensure!(
                    !self
                        .state
                        .executions
                        .values()
                        .any(|e| e.workflow == workflow
                            && e.active()
                            && serde_json::to_value(previous.node(&e.node.0)).ok()
                                != serde_json::to_value(graph.node(&e.node.0)).ok()),
                    "Interrupt an invocation before changing its node definition"
                );
                self.registry.prepare_graph(&mut graph);
                graph.validate()?;
                self.registry.validate_graph(&graph)?;
                self.state.validate_node_identity(&workflow, &graph)?;
                self.state
                    .queue
                    .frames
                    .retain(|f| f.workflow != workflow || graph.node(&f.root.0).is_some());
                self.state
                    .applications
                    .retain(|node, a| a.workflow != workflow || graph.node(&node.0).is_some());
                self.state.workflows.get_mut(&workflow).unwrap().graph = graph;
                self.state.revision += 1;
                Ok(Value::Null)
            }
            Command::Read {
                workflow,
                node,
                evaluation,
            } => {
                let correlation = evaluation.0.clone();
                self.enqueue_frame(
                    crate::scheduler::Frame {
                        data: true,
                        id: evaluation.0,
                        workflow,
                        root: node,
                        pin: String::new(),
                        args: json!({}),
                        correlation,
                    },
                    caller,
                )?;
                Ok(Value::Null)
            }
            Command::Run {
                workflow,
                node,
                invocation,
                args,
            } => {
                self.enqueue_frame(
                    crate::scheduler::Frame {
                        data: false,
                        id: invocation.0.clone(),
                        workflow,
                        root: node,
                        pin: String::new(),
                        args,
                        correlation: invocation.0.clone(),
                    },
                    caller,
                )?;
                Ok(json!({"invocation":invocation}))
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
                if !self.state.executions.contains_key(&invocation)
                    && let Some(index) = self
                        .state
                        .queue
                        .frames
                        .iter()
                        .position(|f| f.id == invocation.0)
                {
                    let frame = self.state.queue.frames[index].clone();
                    self.state.lookup(&frame.workflow, caller)?;
                    self.state.queue.frames.remove(index);
                    self.state
                        .queue
                        .accepted
                        .insert(frame.id, frame.workflow.clone());
                    self.state.executions.insert(
                        invocation.clone(),
                        Execution {
                            id: invocation.clone(),
                            workflow: frame.workflow,
                            node: frame.root,
                            generation: self.state.revision + 1,
                            host: self.state.host_id.clone(),
                            provider: "evaluation".into(),
                            handle: invocation.0,
                            status: ExecutionStatus::Interrupted,
                            result: json!({}),
                        },
                    );
                    self.state.revision += 1;
                    return Ok(Value::Null);
                }

                let execution = self
                    .state
                    .executions
                    .get(&invocation)
                    .context("Invocation not found")?;
                self.state.lookup(&execution.workflow, caller)?;
                if execution.status == ExecutionStatus::Queued {
                    self.state.executions.get_mut(&invocation).unwrap().status =
                        ExecutionStatus::Interrupted;
                    self.state.revision += 1;
                    return Ok(Value::Null);
                }
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
    pub fn invoke_activation(
        &mut self,
        workflow: WorkflowId,
        invocation: InvocationId,
        activation: crate::graph::Activation,
        caller: Option<&WorkflowId>,
        _events: EventSink,
    ) -> Result<Value> {
        self.state.lookup(&workflow, caller)?;
        let correlation = invocation.0.clone();
        self.reserve_activation(workflow, invocation, activation, correlation)
    }
    pub(crate) fn reserve_activation(
        &mut self,
        workflow: WorkflowId,
        invocation: InvocationId,
        mut activation: crate::graph::Activation,
        correlation: String,
    ) -> Result<Value> {
        if let Some(spec) = self
            .state
            .workflows
            .get(&workflow)
            .and_then(|w| w.graph.node(&activation.id))
            && self
                .registry
                .definition(&spec.type_id)
                .is_ok_and(|d| d.behavior == Behavior::Join)
        {
            let key = format!("{}:{}:{}", workflow, activation.id, correlation);
            if self.state.queue.joins_done.contains(&key) {
                return Ok(Value::Null);
            }
            let mut required: BTreeSet<_> =
                crate::graph::pins(&self.state.workflows[&workflow].graph, spec, false)
                    .into_iter()
                    .filter(|p| p.ty.is_signal() && p.name != "failed")
                    .map(|p| p.name)
                    .collect();
            required.remove("in");
            ensure!(
                !required.is_empty(),
                "Join must declare its required inputs"
            );
            let pin = activation.pin.clone();
            ensure!(
                required.contains(&pin) || pin == "failed",
                "Deliver join signals to a named input"
            );
            self.state
                .queue
                .joins
                .entry(key.clone())
                .or_default()
                .insert(pin.clone());
            self.state
                .queue
                .join_args
                .entry(key.clone())
                .or_default()
                .insert(pin.clone(), activation.args.clone());
            self.state.revision += 1;
            if pin != "failed" && !required.is_subset(&self.state.queue.joins[&key]) {
                return Ok(
                    json!({"waiting_for":required.difference(&self.state.queue.joins[&key]).collect::<Vec<_>>()}),
                );
            }
            self.state.queue.joins.remove(&key);
            self.state.queue.joins_done.insert(key.clone());
            let arrivals = self.state.queue.join_args.remove(&key).unwrap_or_default();
            if let Some(args) = activation.args.as_object_mut() {
                args.insert("arrivals".into(), serde_json::to_value(arrivals)?);
            }
        }
        if self
            .state
            .executions
            .values()
            .filter(|e| e.status == ExecutionStatus::Queued)
            .count()
            >= self.signal_capacity
        {
            return Err(crate::scheduler::Backpressure.into());
        }
        let node: NodeId = activation.id.into();
        let args = activation.args;
        let inputs = Some(activation.inputs);
        let caller = Some(&workflow);
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
        let generation = self.state.revision + 1;
        let mut properties = spec.flow.properties.clone();
        if let Some(inputs) = inputs {
            let schema = w.graph.node_types.iter().find(|d| d.id == spec.type_id);
            if let Some(schema) = schema {
                for (name, property) in &schema.properties {
                    if let Some(default) = &property.default {
                        properties.entry(name.clone()).or_insert(default.clone());
                    }
                }
            }
            for pin in definition
                .inputs
                .iter()
                .chain(spec.flow.inputs.iter())
                .chain(schema.into_iter().flat_map(|d| d.inputs.iter()))
            {
                if let Some(property) = &pin.property
                    && let Some(value) = inputs.get(&pin.name)
                {
                    properties.insert(property.clone(), value.clone());
                }
            }
            properties.insert("data_inputs".into(), inputs);
        }
        let request = Invocation {
            input: activation.pin,
            id: invocation.clone(),
            workflow: workflow.clone(),
            node: node.clone(),
            type_id: spec.type_id.clone(),
            properties,
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
            status: ExecutionStatus::Queued,
            result: Value::Null,
        };
        self.state.queue.invocations.insert(
            invocation.clone(),
            crate::scheduler::InvocationContext {
                request,
                correlation,
            },
        );
        self.state.executions.insert(invocation.clone(), execution);
        self.state.revision += 1;
        Ok(serde_json::to_value(&self.state.executions[&invocation])?)
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
        crate::graph::validate(self)
    }
}
