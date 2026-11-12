use flowwm::{model::*, presentation::Writer};
use std::os::unix::fs::MetadataExt;
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

#[test]
fn animated_titles_and_runtime_updates_do_not_rewrite_presentation() {
    let dir = tempfile::tempdir().unwrap();
    let writer = Writer::new(dir.path().into()).unwrap();
    let mut state = State::empty("/tmp".into());
    state.nodes.push(Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Position { x: 427., y: 193. },
    ));
    writer.save(&state).unwrap();
    let path = dir.path().join("presentation.json");
    let inode = std::fs::metadata(&path).unwrap().ino();
    // An attempted write would fail. Unchanged presentation must need no disk
    // access even when saves are requested for hundreds of native updates.
    std::fs::create_dir(dir.path().join("presentation.json.tmp")).unwrap();
    for i in 0..1000 {
        state.nodes[0].title = format!("spinner-{i}");
        state.nodes[0].output = format!("output-{i}");
        state.nodes[0].window_id = Some(i);
        writer.save(&state).unwrap();
    }
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);

    // Real edits must still persist, and a failed write must not poison the
    // cache so that retrying the same edit remains possible.
    state.nodes[0].position.x = 678.;
    assert!(writer.save(&state).is_err());
    std::fs::remove_dir(dir.path().join("presentation.json.tmp")).unwrap();
    writer.save(&state).unwrap();
    let id = state.nodes[0].id.clone();
    state.nodes[0].position = Default::default();
    writer.load(&mut state).unwrap();
    assert_eq!(state.node(&id).unwrap().position.x, 678.);
}

#[test]
fn loading_presentation_initializes_the_unchanged_save_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = State::empty("/tmp".into());
    {
        let writer = Writer::new(dir.path().into()).unwrap();
        writer.save(&state).unwrap();
    }
    let writer = Writer::new(dir.path().into()).unwrap();
    writer.load(&mut state).unwrap();
    std::fs::create_dir(dir.path().join("presentation.json.tmp")).unwrap();
    writer.save(&state).unwrap();
}
