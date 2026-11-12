use flow_core::{
    domain::*, graph::*, orchestrator::Orchestrator, protocol::Command, providers::*, scheduler,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Provider {
    reads: AtomicUsize,
    invocations: Mutex<Vec<Invocation>>,
}
impl ExecutionProvider for Provider {
    fn read(&self, _: Invocation) -> anyhow::Result<Value> {
        Ok(json!({"value":self.reads.fetch_add(1,Ordering::SeqCst)+1}))
    }
    fn invoke(&self, invocation: Invocation, events: EventSink) -> anyhow::Result<String> {
        self.invocations.lock().unwrap().push(invocation.clone());
        events(Completion {
            invocation: invocation.id.clone(),
            generation: invocation.generation,
            status: ExecutionStatus::Complete,
            result: json!({"value":invocation.properties["data_inputs"]["left"]}),
        });
        Ok(invocation.id.0)
    }
    fn interrupt(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
struct NoScripts;
impl ScriptEvaluator for NoScripts {
    fn evaluate(
        &self,
        _: &NodeSpec,
        _: &Record,
        _: &Value,
        _: &AtomicBool,
    ) -> anyhow::Result<Value> {
        panic!("Registered provider must implement this computation")
    }
}
#[test]
fn headless_graph_uses_registered_computation_and_work_without_central_dispatch() {
    let provider = Arc::new(Provider {
        reads: AtomicUsize::new(0),
        invocations: Mutex::new(vec![]),
    });
    let mut registry = Registry::default();
    registry
        .mount(
            "test".into(),
            vec![
                Definition {
                    id: "test.lazy".into(),
                    owner: "test".into(),
                    behavior: Behavior::Lazy,
                    inputs: vec![],
                    outputs: vec![Pin::new("value", Type::Int)],
                    capabilities: Default::default(),
                    window: None,
                    renderer: None,
                    properties: Default::default(),
                },
                Definition {
                    id: "test.work".into(),
                    owner: "test".into(),
                    behavior: Behavior::Invoked,
                    inputs: vec![
                        Pin::new("in", Type::trigger()),
                        Pin::new("left", Type::Int),
                        Pin::new("right", Type::Int),
                    ],
                    outputs: vec![
                        Pin::new("out", Type::trigger()),
                        Pin::new("value", Type::Int),
                    ],
                    capabilities: Default::default(),
                    window: None,
                    renderer: None,
                    properties: Default::default(),
                },
            ],
            provider.clone(),
        )
        .unwrap();
    let mut runtime = RuntimeState::local();
    runtime
        .workspaces
        .activate(&"local:user".into(), &"local".into(), "user-session".into())
        .unwrap();
    let graph = FlowNet {
        nodes: vec![
            NodeSpec {
                id: "data".into(),
                type_id: "test.lazy".into(),
                flow: Flow {
                    node_type: "test.lazy".into(),
                    ..Default::default()
                },
            },
            NodeSpec {
                id: "work".into(),
                type_id: "test.work".into(),
                flow: Flow {
                    node_type: "test.work".into(),
                    ..Default::default()
                },
            },
        ],
        connections: ["left", "right"]
            .into_iter()
            .map(|pin| Connection {
                source: Endpoint {
                    node: "data".into(),
                    pin: "value".into(),
                },
                target: Endpoint {
                    node: "work".into(),
                    pin: pin.into(),
                },
            })
            .collect(),
        ..Default::default()
    };
    let workflow = Workflow {
        id: "workflow".into(),
        name: "Headless graph".into(),
        workspace: Some("local:user".into()),
        visibility: Visibility::Public,
        owner: None,
        references: Default::default(),
        graph,
        authority: "local".into(),
        transfer: None,
    };
    let mut core = Orchestrator::new(runtime, registry.clone());
    core.execute(Command::Create { workflow }, None, Arc::new(|_| {}))
        .unwrap();
    for delivery in 1..=2 {
        let state = core.state.workflows[&"workflow".into()].graph.clone();
        let frame = scheduler::evaluate(
            state,
            EvaluationContext {
                workflow: "workflow".into(),
                ..Default::default()
            },
            "work",
            Some(("", json!({}))),
            Arc::new(AtomicBool::new(false)),
            registry.clone(),
            Arc::new(NoScripts),
        )
        .unwrap();
        assert_eq!(provider.reads.load(Ordering::SeqCst), delivery);
        assert_eq!(frame.activations.len(), 1);
        assert_eq!(
            frame.activations[0].inputs,
            json!({"left":delivery,"right":delivery})
        );
        let completions = Arc::new(Mutex::new(vec![]));
        let sink = completions.clone();
        let activation = frame.activations[0].clone();
        core.invoke_activation(
            "workflow".into(),
            format!("delivery-{delivery}").into(),
            activation.clone(),
            None,
            Arc::new(move |e| sink.lock().unwrap().push(e)),
        )
        .unwrap();
        // Retrying the same delivery does not execute the work again.
        core.invoke_activation(
            "workflow".into(),
            format!("delivery-{delivery}").into(),
            activation,
            None,
            Arc::new(|_| panic!("Duplicate invocation")),
        )
        .unwrap();
        let output = completions.clone();
        core.dispatch_ready(8, false, Arc::new(move |e| output.lock().unwrap().push(e)))
            .unwrap();
        let completion = completions.lock().unwrap().pop().unwrap();
        core.state
            .complete(
                &completion.invocation,
                completion.generation,
                completion.result,
                completion.status,
            )
            .unwrap();
    }
    assert_eq!(provider.invocations.lock().unwrap().len(), 2);
    let state = core.state.workflows[&"workflow".into()].graph.clone();
    let frame = scheduler::evaluate(
        state,
        EvaluationContext {
            workflow: "workflow".into(),
            ..Default::default()
        },
        "work",
        None,
        Arc::new(AtomicBool::new(false)),
        registry,
        Arc::new(NoScripts),
    )
    .unwrap();
    assert!(frame.activations.is_empty());
    assert_eq!(
        provider.reads.load(Ordering::SeqCst),
        2,
        "Reading published application data must not invoke lazy dependencies or start work"
    );
}
