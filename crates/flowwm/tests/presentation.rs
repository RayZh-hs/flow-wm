use flowwm::{model::*, presentation::Writer};
#[test]
fn presentation_and_runtime_have_independent_writer_locks_and_atomic_documents() {
    let dir = tempfile::tempdir().unwrap();
    let writer = Writer::new(dir.path().into()).unwrap();
    assert!(Writer::new(dir.path().into()).is_err());
    let runtime = flow_host::store::Store::new(dir.path().into()).unwrap();
    let mut state = State::empty("/tmp".into());
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        "/tmp".into(),
        Position { x: 427., y: 193. },
    );
    let id = node.id.clone();
    state.nodes.push(node);
    writer.save(&state).unwrap();
    let before = std::fs::read(dir.path().join("presentation.json")).unwrap();
    flow_core::providers::Storage::save(&runtime, &flow_core::domain::RuntimeState::local())
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("presentation.json")).unwrap(),
        before
    );
    state.node_mut(&id).unwrap().position = Default::default();
    writer.load(&mut state).unwrap();
    assert_eq!(state.node(&id).unwrap().position.x, 427.);
    assert!(!dir.path().join("presentation.json.tmp").exists());
}
