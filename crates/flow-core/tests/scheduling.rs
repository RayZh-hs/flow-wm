//! Durable scheduling tests use only portable core and public provider contracts.
use flow_core::{
    domain::*, graph::*, orchestrator::Orchestrator, protocol::Command, providers::*, scheduler::*,
};
use serde_json::json;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
#[derive(Default)]
struct Provider {
    started: Mutex<Vec<Invocation>>,
    installed: Mutex<Vec<SourceRegistration>>,
    released: Mutex<Vec<String>>,
}
impl ExecutionProvider for Provider {
    fn invoke(&self, request: Invocation, _: EventSink) -> anyhow::Result<String> {
        self.started.lock().unwrap().push(request.clone());
        Ok(request.id.0)
    }
    fn interrupt(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn subscribe(&self, source: SourceRegistration, _: SourceSink) -> anyhow::Result<String> {
        self.installed.lock().unwrap().push(source.clone());
        Ok(source.handle)
    }
    fn unsubscribe(&self, handle: &str) -> anyhow::Result<()> {
        self.released.lock().unwrap().push(handle.into());
        Ok(())
    }
}
struct Scripts;
impl ScriptEvaluator for Scripts {
    fn evaluate(
        &self,
        _: &NodeSpec,
        _: &Record,
        _: &serde_json::Value,
        _: &AtomicBool,
    ) -> anyhow::Result<serde_json::Value> {
        unreachable!()
    }
}
fn setup() -> (Orchestrator, Arc<Provider>) {
    let provider = Arc::new(Provider::default());
    let mut registry = Registry::default();
    let signal = Type::Signal {
        args: Default::default(),
        open: true,
    };
    registry
        .mount(
            "test".into(),
            vec![
                Definition {
                    id: "test.work".into(),
                    owner: "test".into(),
                    behavior: Behavior::Invoked,
                    inputs: vec![Pin::new("in", signal.clone())],
                    outputs: vec![
                        Pin::new("out", signal.clone()),
                        Pin::new("failed", signal.clone()),
                    ],
                    capabilities: Default::default(),
                    window: None,
                    renderer: None,
                    properties: Default::default(),
                },
                Definition {
                    id: "test.source".into(),
                    owner: "test".into(),
                    behavior: Behavior::Source,
                    inputs: vec![],
                    outputs: vec![Pin::new("out", signal)],
                    capabilities: Default::default(),
                    window: None,
                    renderer: None,
                    properties: Default::default(),
                },
            ],
            provider.clone(),
        )
        .unwrap();
    let mut state = RuntimeState::local();
    state
        .workspaces
        .activate(&"local:user".into(), &"local".into(), "session".into())
        .unwrap();
    let mut core = Orchestrator::new(state, registry);
    core.execute(
        Command::Create {
            workflow: Workflow {
                row_settings: Default::default(),
                id: "flow".into(),
                name: "Flow".into(),
                workspace: Some("local:user".into()),
                visibility: Visibility::Public,
                owner: None,
                references: Default::default(),
                authority: "local".into(),
                transfer: None,
                graph: FlowNet {
                    nodes: vec![
                        NodeSpec {
                            id: "first".into(),
                            type_id: "test.work".into(),
                            flow: Flow::default(),
                        },
                        NodeSpec {
                            id: "last".into(),
                            type_id: "test.work".into(),
                            flow: Flow::default(),
                        },
                    ],
                    connections: vec![Connection {
                        source: Endpoint {
                            node: "first".into(),
                            pin: "out".into(),
                        },
                        target: Endpoint {
                            node: "last".into(),
                            pin: "in".into(),
                        },
                    }],
                    ..Default::default()
                },
            },
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    (core, provider)
}
fn evaluate_next(core: &mut Orchestrator) {
    let (frame, graph, context) = core.next_frame().unwrap();
    let result = flow_core::scheduler::evaluate(
        graph,
        context,
        &frame.root.0,
        Some((&frame.pin, frame.args.clone())),
        Arc::new(AtomicBool::new(false)),
        core.registry.clone(),
        Arc::new(Scripts),
    )
    .unwrap();
    core.accept_frame(&frame.id, result).unwrap();
}
fn frame(id: &str) -> Frame {
    Frame {
        data: false,
        id: id.into(),
        workflow: "flow".into(),
        root: "first".into(),
        pin: String::new(),
        args: json!({"payload":7}),
        correlation: id.into(),
    }
}
#[test]
fn resolved_queued_work_survives_serialization_and_unknown_running_work_is_not_replayed() {
    let (mut core, provider) = setup();
    core.enqueue_frame(frame("queued"), None).unwrap();
    evaluate_next(&mut core);
    assert_eq!(
        core.state.executions[&"queued".into()].status,
        ExecutionStatus::Queued
    );
    let saved = serde_json::to_vec(&core.state).unwrap();
    let mut resumed = Orchestrator::new(
        serde_json::from_slice(&saved).unwrap(),
        core.registry.clone(),
    );
    resumed.recover();
    resumed.dispatch_ready(1, true, Arc::new(|_| {})).unwrap();
    assert_eq!(provider.started.lock().unwrap().len(), 1);
    assert_eq!(provider.started.lock().unwrap()[0].args["payload"], 7);
    let saved = serde_json::to_vec(&resumed.state).unwrap();
    let mut crashed = Orchestrator::new(
        serde_json::from_slice(&saved).unwrap(),
        core.registry.clone(),
    );
    crashed.recover();
    crashed.dispatch_ready(1, true, Arc::new(|_| {})).unwrap();
    assert_eq!(provider.started.lock().unwrap().len(), 1);
    assert_eq!(
        crashed.state.executions[&"queued".into()].status,
        ExecutionStatus::Unavailable
    );
}
#[test]
fn full_delivery_queue_retains_a_completion_and_resumes_its_branch_once() {
    let (mut core, provider) = setup();
    core.signal_capacity = 1;
    core.enqueue_frame(frame("run"), None).unwrap();
    evaluate_next(&mut core);
    core.dispatch_ready(1, true, Arc::new(|_| {})).unwrap();
    core.enqueue_frame(frame("occupant"), None).unwrap();
    let request = provider.started.lock().unwrap()[0].clone();
    assert!(
        core.complete_invocation(Completion {
            invocation: request.id,
            generation: request.generation,
            status: ExecutionStatus::Complete,
            result: json!({"value":9})
        })
        .is_err()
    );
    assert_eq!(core.state.queue.completions.len(), 1);
    let mut recovered = Orchestrator::new(
        serde_json::from_slice(&serde_json::to_vec(&core.state).unwrap()).unwrap(),
        core.registry.clone(),
    );
    recovered.signal_capacity = 1;
    recovered.recover();
    assert_eq!(
        recovered.state.executions[&"run".into()].status,
        ExecutionStatus::Running
    );
    evaluate_next(&mut recovered);
    recovered.resume_completions().unwrap();
    assert!(recovered.state.queue.completions.is_empty());
    assert_eq!(recovered.state.queue.frames.len(), 1);
    recovered.resume_completions().unwrap();
    assert_eq!(recovered.state.queue.frames.len(), 1);
    assert_eq!(recovered.state.queue.frames[0].args["payload"], 7);
    recovered.dispatch_ready(1, true, Arc::new(|_| {})).unwrap();
    evaluate_next(&mut recovered);
    assert_eq!(
        recovered
            .state
            .executions
            .values()
            .filter(|e| e.node.0 == "last")
            .count(),
        1
    );
}
#[test]
fn changing_source_configuration_releases_the_old_generation_and_unmount_keeps_its_schema() {
    let (mut core, provider) = setup();
    let mut graph = core.state.workflows[&"flow".into()].graph.clone();
    graph.nodes.push(NodeSpec {
        id: "source".into(),
        type_id: "test.source".into(),
        flow: Flow::default(),
    });
    core.execute(
        Command::ReplaceGraph {
            workflow: "flow".into(),
            graph,
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    core.reconcile_sources(Arc::new(|_| Ok(()))).unwrap();
    let old = core.state.sources[&"source".into()].clone();
    let mut graph = core.state.workflows[&"flow".into()].graph.clone();
    graph
        .node_mut("source")
        .unwrap()
        .flow
        .properties
        .insert("setting".into(), json!(2));
    core.execute(
        Command::ReplaceGraph {
            workflow: "flow".into(),
            graph,
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    core.reconcile_sources(Arc::new(|_| Ok(()))).unwrap();
    assert_ne!(
        old.generation,
        core.state.sources[&"source".into()].generation
    );
    assert!(
        !core
            .source_event(SourceEvent {
                delivery: None,
                node: old.node,
                generation: old.generation,
                sequence: 1,
                pin: "out".into(),
                args: json!({}),
                values: Default::default(),
                state: json!({})
            })
            .unwrap()
    );
    core.unmount_provider(&"test".into()).unwrap();
    assert_eq!(provider.released.lock().unwrap().len(), 2);
    assert!(core.state.sources.is_empty());
    assert!(
        core.state.workflows[&"flow".into()]
            .graph
            .node_types
            .iter()
            .any(|d| d.id == "test.source")
    );
}

#[test]
fn a_delivered_signal_to_a_missing_provider_reports_unavailable_and_leaves_no_stuck_frame() {
    let (mut core, _) = setup();
    core.unmount_provider(&"test".into()).unwrap();
    core.execute(
        Command::Emit {
            workflow: "flow".into(),
            node: "first".into(),
            pin: "out".into(),
            delivery: "missing".into(),
            args: json!({"payload":7}),
        },
        None,
        Arc::new(|_| {}),
    )
    .unwrap();
    evaluate_next(&mut core);
    assert!(core.state.queue.frames.is_empty());
    let execution = core
        .state
        .executions
        .values()
        .find(|e| e.node.0 == "last")
        .unwrap();
    assert_eq!(execution.status, ExecutionStatus::Unavailable);
    assert!(
        execution.result["error"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
}
