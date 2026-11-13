//! Provider-aware delivery frames using the existing Vox traversal and data cache.
//! Hosts execute returned activations through the registry and feed completion signals
//! into a fresh frame. Reading data cannot start an invoked node or emit a signal.
use crate::{domain::*, graph::*, providers::*};
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::AtomicBool};

struct Bindings {
    environment: crate::environment::Env,
    registry: Registry,
    workflow: WorkflowId,
    scripts: Arc<dyn ScriptEvaluator>,
}
#[derive(Debug)]
pub struct Backpressure;
impl std::fmt::Display for Backpressure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invocation queue is full")
    }
}
impl std::error::Error for Backpressure {}
impl Bindings {
    fn definition(&self, node: &NodeSpec) -> Option<&Definition> {
        self.registry.definition(&node.type_id).ok()
    }
}
impl ScriptEvaluator for Bindings {
    fn behavior(&self, node: &NodeSpec) -> Option<Behavior> {
        self.definition(node).map(|d| d.behavior.clone())
    }
    fn compute(&self, node: &NodeSpec, bindings: &Record, args: &Value) -> Option<Result<Value>> {
        let definition = self.definition(node)?;
        if definition.behavior != Behavior::Lazy {
            return None;
        }
        Some((|| {
            let mut properties: Record =
                serde_json::from_value(bindings.get("properties").cloned().unwrap_or(json!({})))?;
            properties.insert(
                "data_inputs".into(),
                bindings.get("inputs").cloned().unwrap_or(json!({})),
            );
            self.registry.provider(&definition.owner)?.read(Invocation {
                environment: Some(self.environment.clone()),
                input: String::new(),
                id: format!("data:{}", node.id).into(),
                workflow: self.workflow.clone(),
                node: node.id.clone(),
                type_id: definition.id.clone(),
                properties,
                args: args.clone(),
                generation: 0,
            })
        })())
    }
    fn evaluate(
        &self,
        node: &NodeSpec,
        bindings: &Record,
        args: &Value,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        self.scripts.evaluate(node, bindings, args, cancel)
    }
}

pub fn evaluate(
    mut state: FlowNet,
    context: EvaluationContext,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
    registry: Registry,
    scripts: Arc<dyn ScriptEvaluator>,
) -> Result<Evaluation> {
    // Registered native nodes carry schemas even when no Vox source is installed.
    for definition in registry.definitions() {
        if state.nodes.iter().any(|n| n.type_id == definition.id)
            && !state.node_types.iter().any(|d| d.id == definition.id)
        {
            state.node_types.push(definition.node_schema());
        }
    }
    context.apply(&mut state);
    let bindings = Arc::new(Bindings {
        environment: context.environment,
        registry,
        workflow: context.workflow,
        scripts,
    });
    if let Some(input) = context.input {
        evaluate_input_with(state, root, &input, cancel, bindings)
    } else {
        evaluate_with(state, root, signal, cancel, bindings)
    }
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A persisted delivery frame. It is removed only after its resolved activations
/// and published values have been committed to the runtime document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub data: bool,
    pub id: String,
    pub workflow: WorkflowId,
    pub root: NodeId,
    pub pin: String,
    pub args: Value,
    pub correlation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvocationContext {
    pub request: Invocation,
    pub correlation: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    #[serde(default)]
    pub routed: std::collections::BTreeSet<InvocationId>,
    pub frames: std::collections::VecDeque<Frame>,
    pub completions: BTreeMap<InvocationId, Completion>,
    pub invocations: BTreeMap<InvocationId, InvocationContext>,
    pub resources: BTreeMap<InvocationId, Vec<ResourceClaim>>,
    pub accepted: BTreeMap<String, WorkflowId>,
    pub published: BTreeMap<NodeId, Record>,
    pub computed: BTreeMap<NodeId, Record>,
    pub source_state: BTreeMap<NodeId, (u64, Value)>,
    /// Join arrivals are keyed by node and explicit correlation, never UI status.
    pub joins: BTreeMap<String, std::collections::BTreeSet<String>>,
    pub joins_done: std::collections::BTreeSet<String>,
    pub join_args: BTreeMap<String, BTreeMap<String, Value>>,
}
impl crate::orchestrator::Orchestrator {
    pub fn enqueue_frame(&mut self, frame: Frame, caller: Option<&WorkflowId>) -> Result<bool> {
        use anyhow::ensure;
        let workflow = self.state.lookup(&frame.workflow, caller)?;
        ensure!(
            workflow.authority == self.state.host_id,
            "Use the authoritative host"
        );
        ensure!(
            workflow.transfer.is_none(),
            "Workflow handoff is in progress"
        );
        let node = workflow
            .graph
            .node(&frame.root.0)
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;
        let definition = self.registry.definition(&node.type_id);
        let behavior = definition.as_ref().ok().map(|d| &d.behavior).or_else(|| {
            workflow
                .graph
                .node_types
                .iter()
                .find(|d| d.id == node.type_id)
                .map(|d| &d.behavior)
        });
        anyhow::ensure!(
            definition.is_ok()
                || frame.data
                || !frame.pin.is_empty()
                || matches!(
                    type_id(node),
                    "vox"
                        | "python"
                        | "display"
                        | "conditional"
                        | "merge_signal"
                        | "break_signal"
                        | "window"
                )
                || workflow
                    .graph
                    .node_types
                    .iter()
                    .any(|d| d.id == node.type_id && !d.source.is_empty()),
            "Node provider is unavailable at the destination"
        );
        ensure!(
            frame.data
                || !frame.pin.is_empty()
                || !behavior.is_some_and(|b| matches!(b, Behavior::Source)),
            "Sources cannot be manually invoked"
        );
        validate_signal_args(&frame.args)?;
        if self.state.queue.accepted.contains_key(&frame.id)
            || self.state.queue.frames.iter().any(|f| f.id == frame.id)
        {
            return Ok(false);
        }
        ensure!(
            self.state.queue.frames.len() < self.signal_capacity,
            "Activation queue is full"
        );
        ensure!(
            self.state.queue.accepted.len() < self.replay_capacity,
            "Activation replay ledger is full"
        );
        self.state.queue.frames.push_back(frame);
        self.state.revision += 1;
        Ok(true)
    }
    pub fn next_frame(&self) -> Option<(Frame, FlowNet, EvaluationContext)> {
        self.state.queue.frames.iter().find_map(|frame| {
            let workflow = self.state.workflows.get(&frame.workflow)?;
            if workflow.authority != self.state.host_id || workflow.transfer.is_some() {
                return None;
            }
            Some((
                frame.clone(),
                workflow.graph.clone(),
                EvaluationContext {
                    input: None,
                    environment: self.registry.row_environment(&workflow.row_settings).ok()?,
                    workflow: workflow.id.clone(),
                    published: self.state.queue.published.clone(),
                },
            ))
        })
    }
    pub fn accept_frame(&mut self, id: &str, mut result: Evaluation) -> Result<()> {
        let Some(index) = self.state.queue.frames.iter().position(|f| f.id == id) else {
            return Ok(());
        };
        let frame = self.state.queue.frames[index].clone();
        if self
            .state
            .workflows
            .get(&frame.workflow)
            .is_none_or(|w| w.authority != self.state.host_id || w.transfer.is_some())
        {
            return Ok(());
        }
        let before = self.state.clone();
        let accepted = (|| -> Result<()> {
            for (node, values) in result.values {
                self.state
                    .queue
                    .computed
                    .entry(node.into())
                    .or_default()
                    .extend(values);
            }
            anyhow::ensure!(
                !frame.data || result.activations.is_empty(),
                "Data reads cannot start invoked work"
            );
            let mut unavailable = std::collections::BTreeSet::new();
            for (index, activation) in std::mem::take(&mut result.activations)
                .into_iter()
                .enumerate()
            {
                let invocation =
                    if activation.id == frame.root.0 && frame.pin.is_empty() && index == 0 {
                        frame.id.clone()
                    } else {
                        format!("{}:{index}", frame.id)
                    };
                let node = activation.id.clone();
                if let Err(error) = self.reserve_activation(
                    frame.workflow.clone(),
                    invocation.into(),
                    activation,
                    frame.correlation.clone(),
                ) {
                    if error.is::<Backpressure>() {
                        return Err(error);
                    }
                    unavailable.insert(node.clone());
                    result.errors.insert(node, error.to_string());
                }
            }
            for (index, (node, error)) in result.errors.iter().enumerate() {
                self.state
                    .queue
                    .published
                    .entry(node.clone().into())
                    .or_default()
                    .insert("error".into(), json!(error));
                if frame.data {
                    continue;
                }
                let spec = self.state.workflows[&frame.workflow]
                    .graph
                    .node(node)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Evaluation error refers to a missing node"))?;
                if self
                    .registry
                    .definition(&spec.type_id)
                    .is_ok_and(|d| matches!(d.behavior, Behavior::Source))
                {
                    continue;
                }
                let invocation: InvocationId = if node == &frame.root.0 && frame.pin.is_empty() {
                    frame.id.clone().into()
                } else {
                    format!("{}:error:{index}", frame.id).into()
                };
                if self.state.executions.contains_key(&invocation) {
                    continue;
                }
                let generation = self.state.revision + 1;
                self.state.executions.insert(
                    invocation.clone(),
                    Execution {
                        id: invocation.clone(),
                        workflow: frame.workflow.clone(),
                        node: node.clone().into(),
                        generation,
                        host: self.state.host_id.clone(),
                        provider: "evaluation".into(),
                        handle: invocation.0.clone(),
                        status: ExecutionStatus::Running,
                        result: Value::Null,
                    },
                );
                self.state.queue.invocations.insert(
                    invocation.clone(),
                    InvocationContext {
                        request: Invocation {
                            environment: None,
                            input: String::new(),
                            id: invocation.clone(),
                            workflow: frame.workflow.clone(),
                            node: node.clone().into(),
                            type_id: spec.type_id,
                            properties: Default::default(),
                            args: frame.args.clone(),
                            generation,
                        },
                        correlation: frame.correlation.clone(),
                    },
                );
                self.complete_invocation(Completion {
                    invocation,
                    generation,
                    status: if unavailable.contains(node) {
                        ExecutionStatus::Unavailable
                    } else {
                        ExecutionStatus::Failed
                    },
                    result: json!({"error":error}),
                })?;
            }
            if !frame.data
                && frame.pin.is_empty()
                && !self.state.executions.contains_key(&frame.id.clone().into())
            {
                self.state.executions.insert(
                    frame.id.clone().into(),
                    Execution {
                        id: frame.id.clone().into(),
                        workflow: frame.workflow.clone(),
                        node: frame.root.clone(),
                        generation: self.state.revision + 1,
                        host: self.state.host_id.clone(),
                        provider: "evaluation".into(),
                        handle: frame.id.clone(),
                        status: if result.errors.is_empty() {
                            ExecutionStatus::Complete
                        } else {
                            ExecutionStatus::Failed
                        },
                        result: json!({"errors":result.errors,"answer":result.answer}),
                    },
                );
            }
            self.state.queue.frames.remove(index);
            self.state.queue.accepted.insert(frame.id, frame.workflow);
            self.state.revision += 1;
            self.checkpoint()
        })();
        if accepted.is_err() {
            self.state = before;
        }
        accepted
    }
    pub fn checkpoint(&self) -> Result<()> {
        if let Some(storage) = &self.storage {
            storage.save(&self.state)?;
        }
        Ok(())
    }
    /// Unknown outcomes remain unavailable. Only records checkpointed as queued
    /// may start after recovery; handles from a previous host process are never reused.
    pub fn recover(&mut self) {
        let host = self.state.host_id.clone();
        for execution in self.state.executions.values_mut() {
            if execution.host == host
                && !self.state.queue.completions.contains_key(&execution.id)
                && matches!(
                    execution.status,
                    ExecutionStatus::Waiting
                        | ExecutionStatus::Running
                        | ExecutionStatus::Interrupting
                )
            {
                execution.status = ExecutionStatus::Unavailable;
                execution.result = json!({"error":"Origin restarted; outcome unknown. Explicitly start a new invocation to retry."});
            }
        }
    }
    pub fn dispatch_ready(
        &mut self,
        limit: usize,
        serialize_directories: bool,
        events: EventSink,
    ) -> Result<()> {
        let active: Vec<_> = self
            .state
            .executions
            .values()
            .filter(|e| {
                e.host == self.state.host_id
                    && matches!(
                        e.status,
                        ExecutionStatus::Running | ExecutionStatus::Interrupting
                    )
            })
            .collect();
        let mut available = limit.saturating_sub(active.len());
        let initial_available = available;
        let mut claims: Vec<_> = active
            .iter()
            .flat_map(|e| {
                self.state
                    .queue
                    .resources
                    .get(&e.id)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        let queued: Vec<_> = self
            .state
            .executions
            .values()
            .filter(|e| e.status == ExecutionStatus::Queued)
            .map(|e| e.id.clone())
            .collect();
        for id in queued {
            if available == 0 {
                break;
            }
            let execution = &self.state.executions[&id];
            let Some(workflow) = self.state.workflows.get(&execution.workflow) else {
                continue;
            };
            if workflow.authority != self.state.host_id || workflow.transfer.is_some() {
                continue;
            }
            let Some(workspace) = &workflow.workspace else {
                continue;
            };
            if self.state.workspaces.require_active(workspace).is_err() {
                continue;
            }
            let context = self
                .state
                .queue
                .invocations
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("Queued invocation has no resolved context"))?
                .clone();
            let provider = match self.registry.provider(&execution.provider) {
                Ok(provider) => provider,
                Err(error) => {
                    let generation = execution.generation;
                    self.complete_invocation(Completion {
                        invocation: id,
                        generation,
                        status: ExecutionStatus::Unavailable,
                        result: json!({"error":error.to_string()}),
                    })?;
                    continue;
                }
            };
            let resources = provider.resources(&context.request)?;
            if serialize_directories
                && resources.iter().any(|r| {
                    claims
                        .iter()
                        .any(|a| a.key == r.key && (a.exclusive || r.exclusive))
                })
            {
                continue;
            }
            self.state
                .queue
                .resources
                .insert(id.clone(), resources.clone());
            self.state.executions.get_mut(&id).unwrap().host = self.state.host_id.clone();
            self.state.executions.get_mut(&id).unwrap().status = ExecutionStatus::Running;
            self.state.revision += 1;
            self.checkpoint()?;
            match provider.invoke_observed(context.request, events.clone(), self.output.clone()) {
                Ok(handle) => self.state.executions.get_mut(&id).unwrap().handle = handle,
                Err(error) if error.is::<ProviderBusy>() => {
                    self.state.executions.get_mut(&id).unwrap().status = ExecutionStatus::Queued;
                    self.state.revision += 1;
                    continue;
                }
                Err(error) => {
                    let generation = self.state.executions[&id].generation;
                    self.complete_invocation(Completion {
                        invocation: id,
                        generation,
                        status: ExecutionStatus::Failed,
                        result: json!({"error":error.to_string()}),
                    })?;
                }
            }
            claims.extend(resources);
            available -= 1;
        }
        if available != initial_available {
            self.checkpoint()?;
        }
        Ok(())
    }
    pub fn resume_completions(&mut self) -> Result<()> {
        let completions: Vec<_> = self.state.queue.completions.values().cloned().collect();
        for completion in completions {
            self.complete_invocation(completion)?;
        }
        Ok(())
    }
    pub fn complete_invocation(&mut self, completion: Completion) -> Result<bool> {
        if completion.status == ExecutionStatus::Waiting {
            let execution = self
                .state
                .executions
                .get_mut(&completion.invocation)
                .ok_or_else(|| anyhow::anyhow!("Unknown invocation"))?;
            if execution.generation != completion.generation || !execution.active() {
                return Ok(false);
            }
            execution.status = ExecutionStatus::Waiting;
            execution.result = completion.result;
            self.state.revision += 1;
            self.checkpoint()?;
            return Ok(true);
        }
        let execution = self
            .state
            .executions
            .get(&completion.invocation)
            .ok_or_else(|| anyhow::anyhow!("Unknown invocation"))?;
        if execution.generation != completion.generation
            || !execution.active()
                && (self.state.queue.routed.contains(&execution.id)
                    || execution.status != completion.status
                    || execution.result != completion.result)
        {
            self.state.queue.completions.remove(&completion.invocation);
            return Ok(false);
        }
        let invocation = completion.invocation.clone();
        self.state
            .queue
            .completions
            .entry(invocation.clone())
            .or_insert(completion.clone());
        self.checkpoint()?;
        let before = self.state.clone();
        let accepted = (|| -> Result<bool> {
            let id = &completion.invocation;
            if self.state.executions[id].active()
                && !self.state.complete(
                    id,
                    completion.generation,
                    completion.result,
                    completion.status,
                )?
            {
                return Ok(false);
            }
            let execution = self.state.executions[id].clone();
            if let Some(destination) = self.state.forwarding_targets.get(id).cloned() {
                self.state
                    .forwarding
                    .push_back(crate::transfer::ForwardedCompletion {
                        destination,
                        execution: execution.clone(),
                    });
            }
            let Some(workflow) = self.state.workflows.get(&execution.workflow) else {
                return Ok(true);
            };
            if workflow.authority != self.state.host_id
                || execution.status == ExecutionStatus::Interrupted
            {
                return Ok(true);
            }
            if let Some(values) = execution.result.as_object() {
                self.state
                    .queue
                    .published
                    .entry(execution.node.clone())
                    .or_default()
                    .extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
                if let Some(stdout) = execution.result.get("stdout") {
                    self.state
                        .queue
                        .published
                        .entry(execution.node.clone())
                        .or_default()
                        .insert("output".into(), stdout.clone());
                }
            }
            let Some(node) = workflow.graph.node(&execution.node.0) else {
                return Ok(true);
            };
            let pins = crate::graph::pins(&workflow.graph, node, true);
            let context = self.state.queue.invocations.get(id).cloned();
            let args = context
                .as_ref()
                .map(|c| c.request.args.clone())
                .unwrap_or(json!({}));
            let correlation = context
                .map(|c| c.correlation)
                .unwrap_or_else(|| id.0.clone());
            let branch = if execution.status == ExecutionStatus::Complete {
                "out"
            } else {
                "failed"
            };
            let mut outputs: Vec<_> = pins
                .into_iter()
                .filter(|p| p.ty.is_signal())
                .filter_map(|p| {
                    execution
                        .result
                        .get(&p.name)
                        .cloned()
                        .or_else(|| (p.name == branch).then(|| args.clone()))
                        .map(|args| (p.name, args))
                })
                .collect();
            outputs.sort_by(|a, b| a.0.cmp(&b.0));
            for (pin, args) in outputs {
                if !self.state.workflows[&execution.workflow]
                    .graph
                    .connections
                    .iter()
                    .any(|e| e.source.node == execution.node.0 && e.source.pin == pin)
                {
                    continue;
                }
                self.enqueue_frame(
                    Frame {
                        data: false,
                        id: format!("{}:complete:{pin}", id.0),
                        workflow: execution.workflow.clone(),
                        root: execution.node.clone(),
                        pin,
                        args,
                        correlation: correlation.clone(),
                    },
                    Some(&execution.workflow),
                )?;
            }
            self.state.queue.routed.insert(id.clone());
            self.checkpoint()?;
            Ok(true)
        })();
        if accepted.is_err() {
            self.state = before;
        } else {
            self.state.queue.completions.remove(&invocation);
            self.state.revision += 1;
            self.checkpoint()?;
        }
        accepted
    }
}

impl crate::orchestrator::Orchestrator {
    pub fn reconcile_sources(&mut self, events: SourceSink) -> Result<()> {
        let mut desired = BTreeMap::new();
        for workflow in self
            .state
            .workflows
            .values()
            .filter(|w| w.authority == self.state.host_id && w.transfer.is_none())
        {
            if workflow
                .workspace
                .as_ref()
                .is_none_or(|w| self.state.workspaces.require_active(w).is_err())
            {
                continue;
            }
            for spec in &workflow.graph.nodes {
                let mut node = spec.clone();
                if let Some(schema) = workflow
                    .graph
                    .node_types
                    .iter()
                    .find(|d| d.id == node.type_id)
                {
                    for (name, property) in &schema.properties {
                        if let Some(value) = &property.default {
                            node.flow
                                .properties
                                .entry(name.clone())
                                .or_insert(value.clone());
                        }
                    }
                }
                let Ok(definition) = self.registry.definition(&node.type_id) else {
                    continue;
                };
                if definition.behavior == Behavior::Source
                    && workflow
                        .workspace
                        .as_ref()
                        .and_then(|w| self.state.workspaces.definition(w).ok())
                        .is_some_and(|w| definition.capabilities.is_subset(&w.capabilities))
                {
                    desired.insert(
                        node.id.clone(),
                        (workflow.id.clone(), node.clone(), definition.owner.clone()),
                    );
                }
            }
        }
        let removed: Vec<_> = self
            .state
            .sources
            .values()
            .filter(|s| {
                desired.get(&s.node).is_none_or(|(_, n, owner)| {
                    n.type_id != s.type_id
                        || n.flow.properties != s.properties
                        || owner != &s.provider
                })
            })
            .cloned()
            .collect();
        for source in removed {
            self.registry
                .provider(&source.provider)?
                .unsubscribe(&source.handle)?;
            self.state
                .queue
                .source_state
                .insert(source.node.clone(), (source.sequence, source.state.clone()));
            self.state.sources.remove(&source.node);
            self.state.revision += 1;
        }
        for (id, (workflow, node, provider)) in desired {
            if self.state.sources.contains_key(&id) {
                continue;
            }
            let mut source = SourceRegistration {
                environment: Some(
                    self.registry
                        .row_environment(&self.state.workflows[&workflow].row_settings)?,
                ),
                subscription: None,
                workflow,
                node: id.clone(),
                type_id: node.type_id,
                provider: provider.clone(),
                generation: self.state.revision + 1,
                handle: format!("{}:{}", id, self.state.revision + 1),
                properties: node.flow.properties,
                sequence: self
                    .state
                    .queue
                    .source_state
                    .get(&id)
                    .map(|s| s.0)
                    .unwrap_or(0),
                state: self
                    .state
                    .queue
                    .source_state
                    .get(&id)
                    .map(|s| s.1.clone())
                    .unwrap_or(Value::Null),
            };
            source.subscription = self.registry.provider(&provider)?.subscription(&source)?;
            if let Some(subscription) = &source.subscription {
                anyhow::ensure!(
                    !self
                        .state
                        .sources
                        .values()
                        .any(|s| s.workflow == source.workflow
                            && s.subscription
                                .as_ref()
                                .is_some_and(|p| p.channel == subscription.channel)),
                    "Only one registered receiver may consume a workflow channel; fan out its signal output"
                );
                crate::graph::validate_type(
                    &self.state.workflows[&source.workflow].graph,
                    &subscription.ty,
                    0,
                )?;
            }
            self.state.sources.insert(id.clone(), source.clone());
            self.state.revision += 1;
            self.checkpoint()?;
            match self
                .registry
                .provider(&provider)?
                .subscribe(source, events.clone())
            {
                Ok(handle) => self.state.sources.get_mut(&id).unwrap().handle = handle,
                Err(error) => {
                    self.state.sources.remove(&id);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    pub fn source_event(&mut self, event: SourceEvent) -> Result<bool> {
        let Some(source) = self.state.sources.get(&event.node) else {
            return Ok(false);
        };
        if source.generation != event.generation || source.sequence >= event.sequence {
            return Ok(false);
        }
        let workflow = source.workflow.clone();
        let node = source.node.clone();
        let previous = self.state.clone();
        let result = (|| -> Result<()> {
            self.state
                .queue
                .published
                .entry(node.clone())
                .or_default()
                .extend(event.values);
            if !event.pin.is_empty()
                && self.state.workflows.get(&workflow).is_some_and(|w| {
                    w.graph
                        .connections
                        .iter()
                        .any(|e| e.source.node == node.0 && e.source.pin == event.pin)
                })
            {
                self.enqueue_frame(
                    Frame {
                        data: false,
                        id: event
                            .delivery
                            .as_ref()
                            .map(|id| format!("receive:{}:{}", node, id))
                            .unwrap_or_else(|| {
                                format!("source:{}:{}:{}", node, event.generation, event.sequence)
                            }),
                        workflow: workflow.clone(),
                        root: node.clone(),
                        pin: event.pin,
                        args: event.args,
                        correlation: format!(
                            "source:{}:{}:{}",
                            node, event.generation, event.sequence
                        ),
                    },
                    Some(&workflow),
                )?;
            }
            let source = self.state.sources.get_mut(&node).unwrap();
            source.sequence = event.sequence;
            source.state = event.state;
            self.state.revision += 1;
            self.checkpoint()
        })();
        if result.is_err() {
            self.state = previous;
        }
        result.map(|_| true)
    }
    pub fn unmount_provider(&mut self, owner: &ProviderId) -> Result<Vec<Definition>> {
        anyhow::ensure!(
            !self
                .state
                .executions
                .values()
                .any(|e| &e.provider == owner && e.active()),
            "Resolve active invocations before unmounting"
        );
        let sources: Vec<_> = self
            .state
            .sources
            .values()
            .filter(|s| &s.provider == owner)
            .cloned()
            .collect();
        let provider = self.registry.provider(owner)?;
        for source in &sources {
            provider.unsubscribe(&source.handle)?;
        }
        for source in sources {
            self.state.sources.remove(&source.node);
        }
        self.checkpoint()?;
        self.registry.unmount(owner, &self.state.executions)
    }
}
