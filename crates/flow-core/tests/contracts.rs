use flow_core::{domain::*, graph::*, migration::Envelope, model::*, orchestrator::Orchestrator, protocol::Command, providers::*};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::AtomicBool}};
fn workflow(id: &str, workspace: &str, owner: Option<&str>) -> Workflow {
    Workflow { id:id.into(), name:id.into(), workspace:Some(workspace.into()), visibility:if owner.is_some(){Visibility::Private}else{Visibility::Public}, owner:owner.map(Into::into), references:Default::default(), graph:Default::default(), authority:workspace.split(':').next().unwrap().into(), transfer:None }
}
fn runtime() -> RuntimeState {
    let mut state = RuntimeState::local();
    state.workspaces.activate(&"local:desktop".into(), &"desktop".into(), "session-A".into()).unwrap();
    state.workspaces.activate(&"local:user".into(), &"local".into(), "user-A".into()).unwrap();
    state
}
fn execution(id: &str, owner: &str) -> Execution { Execution { id:id.into(), workflow:owner.into(), node:"node".into(), generation:1, host:"local".into(), provider:"test".into(), handle:"original-handle".into(), status:ExecutionStatus::Running, result:Value::Null } }
#[test]
fn session_incarnations_do_not_change_definitions_or_accept_stale_teardown() {
    let mut state = runtime();
    let definitions = serde_json::to_value(&state.workspaces).unwrap()["definitions"].clone();
    state.workspaces.activate(&"local:desktop".into(), &"desktop".into(), "session-B".into()).unwrap();
    assert!(state.end_session(&"local:desktop".into(), "session-A").is_err());
    assert!(state.workspaces.require_active(&"local:desktop".into()).is_ok());
    state.end_session(&"local:desktop".into(), "session-B").unwrap();
    assert_eq!(serde_json::to_value(&state.workspaces).unwrap()["definitions"], definitions);
    assert!(state.workspaces.activate(&"local:desktop".into(), &"other".into(), "bad".into()).is_err());
    let mut changed = state.workspaces.definition(&"local:desktop".into()).unwrap().clone(); changed.lifetime = Lifetime::Persistent;
    assert!(state.workspaces.register(changed).is_err());
}
#[test]
fn local_move_keeps_invocation_and_origin_after_old_session_ends() {
    let mut state = runtime(); state.create(workflow("work", "local:desktop", None)).unwrap();
    state.executions.insert("run".into(), execution("run", "work"));
    state.reassign(&"work".into(), &"local:user".into()).unwrap();
    assert!(state.end_session(&"local:desktop".into(), "session-A").unwrap().is_empty());
    assert_eq!(state.executions[&"run".into()].handle, "original-handle");
    assert_eq!(state.executions[&"run".into()].host.0, "local");
    assert!(state.complete(&"run".into(), 99, json!(0), ExecutionStatus::Complete).unwrap() == false);
    assert!(state.complete(&"run".into(), 1, json!(42), ExecutionStatus::Complete).unwrap());
    assert!(state.workflows.contains_key(&"work".into()));
}
#[test]
fn ownership_cascades_across_workspaces_and_make_public_preserves_identity() {
    let mut state = runtime();
    state.create(workflow("parent", "local:desktop", None)).unwrap();
    state.create(workflow("child", "local:user", Some("parent"))).unwrap();
    state.create(workflow("grandchild", "local:user", Some("child"))).unwrap();
    assert!(state.lookup(&"child".into(), None).is_err());
    assert!(state.reference(&"parent".into(), &"child".into()).is_err());
    state.executions.insert("run".into(), execution("run", "child"));
    let original = serde_json::to_value(&state.executions).unwrap();
    state.make_public(&"child".into(), &"parent".into()).unwrap();
    assert_eq!(serde_json::to_value(&state.executions).unwrap(), original);
    assert_eq!(state.workflows[&"child".into()].workspace, Some("local:user".into()));
    assert!(state.workflows[&"parent".into()].references.contains(&"child".into()));
    assert!(state.destroy(&"parent".into()).unwrap().is_empty());
    assert!(state.workflows.contains_key(&"child".into()));
    assert_eq!(state.destroy(&"child".into()).unwrap().len(), 1);
    assert!(state.workflows.is_empty());
}
#[test]
fn ownership_cycles_and_cross_workflow_connections_are_rejected() {
    let mut state = runtime(); state.create(workflow("a", "local:user", None)).unwrap(); state.create(workflow("b", "local:user", Some("a"))).unwrap();
    state.workflows.get_mut(&"a".into()).unwrap().owner = Some("b".into());
    state.workflows.get_mut(&"a".into()).unwrap().visibility = Visibility::Private;
    assert!(state.validate().is_err());
    let graph = FlowNet { nodes:vec![NodeSpec { id:"one".into(), type_id:"test.work".into(), flow:Default::default() }], connections:vec![Connection { source:Endpoint {node:"one".into(), pin:"out".into()}, target:Endpoint {node:"other-workflow-node".into(),pin:"in".into()} }], ..Default::default() };
    assert!(graph.validate().is_err());
}
#[test]
fn typed_delivery_is_bounded_private_checked_and_explicitly_acknowledged() {
    let mut state = runtime();
    for id in ["a", "b"] { state.create(workflow(id, "local:user", None)).unwrap(); }
    state.create(workflow("private", "local:user", Some("b"))).unwrap();
    let delivery = Delivery { id:"event-1".into(), source:"a".into(), target:"b".into(), channel:"done".into(), ty:Type::Signal {args:BTreeMap::from([("code".into(), Type::UInt8)]),open:false}, args:json!({"code":42}), sequence:1 };
    let mut bad = delivery.clone();bad.args=json!({"code":256});assert!(state.broadcast(bad, 1).is_err());
    let mut private = delivery.clone();private.target="private".into();assert!(state.broadcast(private, 1).is_err());
    assert!(state.broadcast(delivery.clone(), 1).unwrap());
    assert!(!state.broadcast(delivery.clone(), 1).unwrap());
    let mut full = delivery.clone(); full.id="event-2".into();assert!(state.broadcast(full, 1).is_err());
    assert!(state.receive(&"b".into(), "done", &"a".into()).is_err());
    assert_eq!(state.receive(&"b".into(), "done", &"b".into()).unwrap().unwrap().args,json!({"code":42}));
    assert!(state.receive(&"b".into(), "done", &"b".into()).unwrap().is_some());
    state.acknowledge(&delivery.id, &"b".into(), 8).unwrap();
    assert!(!state.broadcast(delivery, 1).unwrap());
}
#[test]
fn migration_preserves_ids_recipes_barriers_ports_and_unassigned_graphs() {
    let mut state = State::empty("/portable/project".into());state.workspaces[0].id="graph".into();state.selected_workspace="graph".into();
    let mut source = Node::new("graph",NodeKind::Shell,"/portable/project".into(),Position{x:12.,y:24.});source.id="source".into();
    let mut target = Node::new("graph",NodeKind::Shell,"/portable/project".into(),Position{x:42.,y:48.});target.id="target".into();target.dependencies.push("source".into());target.run_when=RunWhen::Failure;
    state.nodes.extend([source,target]);state.recipes.push(state.recipe("Recipe".into(), &["source".into(),"target".into()]).unwrap());
    let original=serde_json::to_value(&state).unwrap();let (envelope,presentation)=Envelope::from_v1(state).unwrap();
    assert!(envelope.runtime.workflows[&"graph".into()].workspace.is_none());
    assert_eq!(presentation.nodes[&"target".into()]["position"]["x"],42.);
    assert_eq!(serde_json::to_value(&envelope.compatibility).unwrap(),original);
    assert_eq!(envelope.compatibility.recipes[0].nodes[1].dependencies,vec!["source"]);
    let mut future=envelope;future.version=99;assert!(future.into_compatibility().is_err());
}
#[derive(Default)]
struct Fake { invocations:Mutex<Vec<Invocation>> }
impl ExecutionProvider for Fake {
    fn invoke(&self, invocation:Invocation, events:EventSink)->anyhow::Result<String> {
        self.invocations.lock().unwrap().push(invocation.clone());
        events(Completion {invocation:invocation.id,generation:invocation.generation,status:ExecutionStatus::Complete,result:json!({"answer":42})});Ok("fake-handle".into())
    }
    fn interrupt(&self,_:&str)->anyhow::Result<()> {Ok(())}
}
#[test]
fn public_provider_api_runs_without_desktop_and_mount_is_transactional() {
    let provider=Arc::new(Fake::default());let mut registry=Registry::default();
    let definition=Definition {id:"test.work".into(),owner:"test".into(),behavior:Behavior::Invoked,inputs:vec![],outputs:vec![],capabilities:Default::default(),window:None,renderer:None};
    let mut bad=definition.clone();bad.id="someone-else.work".into();
    assert!(registry.mount("test".into(),vec![definition.clone(),bad],provider.clone()).is_err());assert!(registry.definition("test.work").is_err());
    registry.mount("test".into(),vec![definition],provider.clone()).unwrap();
    let mut state=runtime();let mut work=workflow("work","local:user",None);work.graph.nodes.push(NodeSpec{id:"node".into(),type_id:"test.work".into(),flow:Default::default()});state.create(work).unwrap();
    let mut core=Orchestrator::new(state,registry);let events=Arc::new(Mutex::new(vec![]));let sink=events.clone();
    let command=Command::Run{workflow:"work".into(),node:"node".into(),invocation:"run".into(),args:json!({})};
    core.execute(command.clone(),None,Arc::new(move|event|sink.lock().unwrap().push(event))).unwrap();
    core.execute(command,None,Arc::new(|_|panic!("Duplicate invocation"))).unwrap();assert_eq!(provider.invocations.lock().unwrap().len(),1);
    assert!(core.registry.unmount(&"test".into(),&core.state.executions).is_err());
    let event=events.lock().unwrap().pop().unwrap();core.state.complete(&event.invocation,event.generation,event.result,event.status).unwrap();
    assert_eq!(core.registry.unmount(&"test".into(),&core.state.executions).unwrap().len(),1);
}
#[test]
fn vox_constant_and_typed_signal_evaluate_headlessly_with_lazy_data() {
    let mut state=State::empty(".".into());let ws=state.selected_workspace.clone();
    let mut constant=Node::new(&ws,NodeKind::Constant,".".into(),Default::default());constant.flow.properties.insert("value".into(),json!(42));
    let root=constant.id.clone();state.nodes.push(constant);
    assert_eq!(evaluate(state.clone(),&root,None,Arc::new(AtomicBool::new(false))).unwrap().answer["value"],42);
    let mut shell=Node::new(&ws,NodeKind::Shell,".".into(),Default::default());shell.flow.inputs.push(Pin::new("value",Type::Int));
    let target=shell.id.clone();state.nodes.push(shell);
    connect(&mut state,Connection{source:Endpoint{node:root,pin:"value".into()},target:Endpoint{node:target.clone(),pin:"value".into()}}).unwrap();
    let result=evaluate(state,&target,Some(("",json!({"code":7}))),Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!(result.activations[0].inputs["value"],42);assert_eq!(result.activations[0].args["code"],7);
}
