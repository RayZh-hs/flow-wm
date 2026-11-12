use flow_core::{domain::*, graph::*, orchestrator::Orchestrator, protocol::Command, providers::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
fn workflow(id: &str, workspace: &str, owner: Option<&str>) -> Workflow {
    Workflow {
        id: id.into(),
        name: id.into(),
        workspace: Some(workspace.into()),
        visibility: if owner.is_some() {
            Visibility::Private
        } else {
            Visibility::Public
        },
        owner: owner.map(Into::into),
        references: Default::default(),
        graph: Default::default(),
        authority: workspace.split(':').next().unwrap().into(),
        transfer: None,
    }
}
fn runtime() -> RuntimeState {
    let mut state = RuntimeState::local();
    state
        .workspaces
        .activate(
            &"local:desktop".into(),
            &"desktop".into(),
            "session-A".into(),
        )
        .unwrap();
    state
        .workspaces
        .activate(&"local:user".into(), &"local".into(), "user-A".into())
        .unwrap();
    state
}
fn execution(id: &str, owner: &str) -> Execution {
    Execution {
        id: id.into(),
        workflow: owner.into(),
        node: "node".into(),
        generation: 1,
        host: "local".into(),
        provider: "test".into(),
        handle: "original-handle".into(),
        status: ExecutionStatus::Running,
        result: Value::Null,
    }
}
#[test]
fn session_incarnations_do_not_change_definitions_or_accept_stale_teardown() {
    let mut state = runtime();
    let definitions = serde_json::to_value(&state.workspaces).unwrap()["definitions"].clone();
    state
        .workspaces
        .activate(
            &"local:desktop".into(),
            &"desktop".into(),
            "session-B".into(),
        )
        .unwrap();
    assert!(
        state
            .end_session(&"local:desktop".into(), "session-A")
            .is_err()
    );
    assert!(
        state
            .workspaces
            .require_active(&"local:desktop".into())
            .is_ok()
    );
    state
        .end_session(&"local:desktop".into(), "session-B")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&state.workspaces).unwrap()["definitions"],
        definitions
    );
    assert!(
        state
            .workspaces
            .activate(&"local:desktop".into(), &"other".into(), "bad".into())
            .is_err()
    );
    let mut changed = state
        .workspaces
        .definition(&"local:desktop".into())
        .unwrap()
        .clone();
    changed.lifetime = Lifetime::Persistent;
    assert!(state.workspaces.register(changed).is_err());
}
#[test]
fn local_move_keeps_invocation_and_origin_after_old_session_ends() {
    let mut state = runtime();
    state
        .create(workflow("work", "local:desktop", None))
        .unwrap();
    state
        .executions
        .insert("run".into(), execution("run", "work"));
    state
        .reassign(&"work".into(), &"local:user".into())
        .unwrap();
    assert!(
        state
            .end_session(&"local:desktop".into(), "session-A")
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.executions[&"run".into()].handle, "original-handle");
    assert_eq!(state.executions[&"run".into()].host.0, "local");
    assert!(
        !state
            .complete(&"run".into(), 99, json!(0), ExecutionStatus::Complete)
            .unwrap()
    );
    assert!(
        state
            .complete(&"run".into(), 1, json!(42), ExecutionStatus::Complete)
            .unwrap()
    );
    assert!(state.workflows.contains_key(&"work".into()));
}
#[test]
fn ownership_cascades_across_workspaces_and_make_public_preserves_identity() {
    let mut state = runtime();
    state
        .create(workflow("parent", "local:desktop", None))
        .unwrap();
    state
        .create(workflow("child", "local:user", Some("parent")))
        .unwrap();
    state
        .create(workflow("grandchild", "local:user", Some("child")))
        .unwrap();
    assert!(state.lookup(&"child".into(), None).is_err());
    assert!(state.reference(&"parent".into(), &"child".into()).is_err());
    state
        .executions
        .insert("run".into(), execution("run", "child"));
    let original = serde_json::to_value(&state.executions).unwrap();
    state
        .make_public(&"child".into(), &"parent".into())
        .unwrap();
    assert_eq!(serde_json::to_value(&state.executions).unwrap(), original);
    assert_eq!(
        state.workflows[&"child".into()].workspace,
        Some("local:user".into())
    );
    assert!(
        state.workflows[&"parent".into()]
            .references
            .contains(&"child".into())
    );
    assert!(state.destroy(&"parent".into()).unwrap().is_empty());
    assert!(state.workflows.contains_key(&"child".into()));
    assert_eq!(state.destroy(&"child".into()).unwrap().len(), 1);
    assert!(state.workflows.is_empty());
}
#[test]
fn ownership_cycles_and_cross_workflow_connections_are_rejected() {
    let mut state = runtime();
    state.create(workflow("a", "local:user", None)).unwrap();
    state
        .create(workflow("b", "local:user", Some("a")))
        .unwrap();
    state.workflows.get_mut(&"a".into()).unwrap().owner = Some("b".into());
    state.workflows.get_mut(&"a".into()).unwrap().visibility = Visibility::Private;
    assert!(state.validate().is_err());
    let graph = FlowNet {
        nodes: vec![NodeSpec {
            id: "one".into(),
            type_id: "test.work".into(),
            flow: Default::default(),
        }],
        connections: vec![Connection {
            source: Endpoint {
                node: "one".into(),
                pin: "out".into(),
            },
            target: Endpoint {
                node: "other-workflow-node".into(),
                pin: "in".into(),
            },
        }],
        ..Default::default()
    };
    assert!(graph.validate().is_err());
}
#[test]
fn typed_delivery_is_bounded_private_checked_and_explicitly_acknowledged() {
    let mut state = runtime();
    for id in ["a", "b"] {
        state.create(workflow(id, "local:user", None)).unwrap();
    }
    state
        .create(workflow("private", "local:user", Some("b")))
        .unwrap();
    let delivery = Delivery {
        id: "event-1".into(),
        source: "a".into(),
        target: "b".into(),
        channel: "done".into(),
        ty: Type::Signal {
            args: BTreeMap::from([("code".into(), Type::UInt8)]),
            open: false,
        },
        args: json!({"code":42}),
        sequence: 1,
    };
    let mut bad = delivery.clone();
    bad.args = json!({"code":256});
    assert!(state.broadcast(bad, 1).is_err());
    let mut private = delivery.clone();
    private.target = "private".into();
    assert!(state.broadcast(private, 1).is_err());
    assert!(state.broadcast(delivery.clone(), 1).unwrap());
    assert!(!state.broadcast(delivery.clone(), 1).unwrap());
    let mut full = delivery.clone();
    full.id = "event-2".into();
    assert!(state.broadcast(full, 1).is_err());
    assert!(state.receive(&"b".into(), "done", &"a".into()).is_err());
    assert_eq!(
        state
            .receive(&"b".into(), "done", &"b".into())
            .unwrap()
            .unwrap()
            .args,
        json!({"code":42})
    );
    assert!(
        state
            .receive(&"b".into(), "done", &"b".into())
            .unwrap()
            .is_some()
    );
    state.acknowledge(&delivery.id, &"b".into(), 8).unwrap();
    assert!(!state.broadcast(delivery, 1).unwrap());
}
#[derive(Default)]
struct Fake {
    invocations: Mutex<Vec<Invocation>>,
}
impl ExecutionProvider for Fake {
    fn invoke(&self, invocation: Invocation, events: EventSink) -> anyhow::Result<String> {
        self.invocations.lock().unwrap().push(invocation.clone());
        events(Completion {
            invocation: invocation.id,
            generation: invocation.generation,
            status: ExecutionStatus::Complete,
            result: json!({"answer":42}),
        });
        Ok("fake-handle".into())
    }
    fn interrupt(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
#[test]
fn public_provider_api_runs_without_desktop_and_mount_is_transactional() {
    let provider = Arc::new(Fake::default());
    let mut registry = Registry::default();
    let definition = Definition {
        id: "test.work".into(),
        owner: "test".into(),
        behavior: Behavior::Invoked,
        inputs: vec![],
        outputs: vec![],
        capabilities: Default::default(),
        window: None,
        renderer: None,
        properties: Default::default(),
    };
    let mut bad = definition.clone();
    bad.id = "someone-else.work".into();
    assert!(
        registry
            .mount(
                "test".into(),
                vec![definition.clone(), bad],
                provider.clone()
            )
            .is_err()
    );
    assert!(registry.definition("test.work").is_err());
    registry
        .mount("test".into(), vec![definition], provider.clone())
        .unwrap();
    let mut state = runtime();
    let mut work = workflow("work", "local:user", None);
    work.graph.nodes.push(NodeSpec {
        id: "node".into(),
        type_id: "test.work".into(),
        flow: Default::default(),
    });
    state.create(work).unwrap();
    let mut core = Orchestrator::new(state, registry);
    let events = Arc::new(Mutex::new(vec![]));
    let sink = events.clone();
    let command = Command::Run {
        workflow: "work".into(),
        node: "node".into(),
        invocation: "run".into(),
        args: json!({}),
    };
    core.execute(
        command.clone(),
        None,
        Arc::new(move |event| sink.lock().unwrap().push(event)),
    )
    .unwrap();
    core.execute(command, None, Arc::new(|_| panic!("Duplicate invocation")))
        .unwrap();
    let (frame, _, _) = core.next_frame().unwrap();
    core.accept_frame(
        &frame.id,
        Evaluation {
            activations: vec![Activation {
                pin: String::new(),
                id: "node".into(),
                args: json!({}),
                inputs: json!({}),
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let sink = events.clone();
    core.dispatch_ready(
        8,
        false,
        Arc::new(move |event| sink.lock().unwrap().push(event)),
    )
    .unwrap();
    assert_eq!(provider.invocations.lock().unwrap().len(), 1);
    assert!(
        core.registry
            .unmount(&"test".into(), &core.state.executions)
            .is_err()
    );
    let event = events.lock().unwrap().pop().unwrap();
    core.state
        .complete(
            &event.invocation,
            event.generation,
            event.result,
            event.status,
        )
        .unwrap();
    assert_eq!(
        core.registry
            .unmount(&"test".into(), &core.state.executions)
            .unwrap()
            .len(),
        1
    );
}
struct Invoked;
impl ScriptEvaluator for Invoked {
    fn behavior(&self, node: &NodeSpec) -> Option<Behavior> {
        (node.type_id == "local.shell").then_some(Behavior::Invoked)
    }
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
#[test]
fn vox_constant_and_typed_signal_evaluate_headlessly_with_lazy_data() {
    let mut state = FlowNet::default();
    let mut constant = NodeSpec {
        id: "constant".into(),
        type_id: "local.constant".into(),
        flow: Default::default(),
    };
    constant.flow.properties.insert("value".into(), json!(42));
    let root = constant.id.0.clone();
    state.nodes.push(constant);
    assert_eq!(
        evaluate(state.clone(), &root, None, Arc::new(AtomicBool::new(false)))
            .unwrap()
            .answer["value"],
        42
    );
    let mut shell = NodeSpec {
        id: "shell".into(),
        type_id: "local.shell".into(),
        flow: Default::default(),
    };
    shell.flow.inputs.push(Pin::new("value", Type::Int));
    let target = shell.id.0.clone();
    state.nodes.push(shell);
    connect(
        &mut state,
        Connection {
            source: Endpoint {
                node: root,
                pin: "value".into(),
            },
            target: Endpoint {
                node: target.clone(),
                pin: "value".into(),
            },
        },
    )
    .unwrap();
    let result = evaluate_with(
        state,
        &target,
        Some(("", json!({"code":7}))),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Invoked),
    )
    .unwrap();
    assert_eq!(result.activations[0].inputs["value"], 42);
    assert_eq!(result.activations[0].args["code"], 7);
}

#[test]
fn interrupted_handoff_reloads_without_duplicating_authority_or_invocations() {
    let mut source = runtime();
    source.bind_host("a".into()).unwrap();
    let mut destination = runtime();
    destination.bind_host("b".into()).unwrap();
    let definition = destination
        .workspaces
        .definition(&"b:user".into())
        .unwrap()
        .clone();
    source.workspaces.register(definition.clone()).unwrap();
    source
        .workspaces
        .activate(&definition.id, &definition.provider, "b-session".into())
        .unwrap();
    source.create(workflow("moving", "a:user", None)).unwrap();
    source
        .executions
        .insert("run".into(), execution("run", "moving"));
    source.executions.get_mut(&"run".into()).unwrap().host = "a".into();
    source
        .prepare_transfer("handoff".into(), &"moving".into(), "b:user".into())
        .unwrap();
    destination
        .stage_transfer(source.transfer_bundle(&"handoff".into()).unwrap())
        .unwrap();
    assert!(!destination.workflows.contains_key(&"moving".into()));
    assert_eq!(source.workflows[&"moving".into()].authority.0, "a");
    let mut recovered: RuntimeState =
        serde_json::from_value(serde_json::to_value(&source).unwrap()).unwrap();
    let committed = recovered.commit_outgoing(&"handoff".into()).unwrap();
    assert_eq!(recovered.workflows[&"moving".into()].authority.0, "b");
    destination.commit_incoming(committed.clone()).unwrap();
    destination.commit_incoming(committed).unwrap();
    assert_eq!(destination.executions.len(), 1);
    assert_eq!(destination.executions[&"run".into()].host.0, "a");
    assert_eq!(
        destination.executions[&"run".into()].handle,
        "original-handle"
    );
    assert!(recovered.abort_transfer(&"handoff".into()).is_err());
}
#[test]
fn destruction_during_disconnected_handoff_prevents_late_activation() {
    let mut source = runtime();
    source.bind_host("a".into()).unwrap();
    let mut destination = runtime();
    destination.bind_host("b".into()).unwrap();
    let definition = destination
        .workspaces
        .definition(&"b:user".into())
        .unwrap()
        .clone();
    source.workspaces.register(definition.clone()).unwrap();
    source
        .workspaces
        .activate(&definition.id, &definition.provider, "b-session".into())
        .unwrap();
    source.create(workflow("moving", "a:user", None)).unwrap();
    source
        .prepare_transfer("handoff".into(), &"moving".into(), "b:user".into())
        .unwrap();
    destination
        .stage_transfer(source.transfer_bundle(&"handoff".into()).unwrap())
        .unwrap();
    let committed = source.commit_outgoing(&"handoff".into()).unwrap();
    source.destroy(&"moving".into()).unwrap();
    assert_eq!(source.pending_remote.len(), 1);
    destination.destroy(&"moving".into()).unwrap();
    assert!(destination.commit_incoming(committed).is_err());
    assert!(!destination.workflows.contains_key(&"moving".into()));
}

#[test]
fn portable_graphs_use_the_existing_type_checker_before_they_are_instantiated() {
    let mut graph = FlowNet {
        nodes: vec![
            NodeSpec {
                id: "constant".into(),
                type_id: "local.constant".into(),
                flow: Flow {
                    properties: BTreeMap::from([("value".into(), json!(42))]),
                    outputs: vec![Pin::new("value", Type::Int)],
                    ..Default::default()
                },
            },
            NodeSpec {
                id: "target".into(),
                type_id: "local.shell".into(),
                flow: Flow {
                    inputs: vec![Pin::new("count", Type::Bool)],
                    ..Default::default()
                },
            },
        ],
        connections: vec![Connection {
            source: Endpoint {
                node: "constant".into(),
                pin: "value".into(),
            },
            target: Endpoint {
                node: "target".into(),
                pin: "count".into(),
            },
        }],
        ..Default::default()
    };
    assert!(graph.validate().is_err());
    graph.nodes[1].flow.inputs[0].ty = Type::Int;
    graph.validate().unwrap();
    graph.nodes[0]
        .flow
        .properties
        .insert("value".into(), json!("wrong type"));
    assert!(graph.validate().is_err());
}
