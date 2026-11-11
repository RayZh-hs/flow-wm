use flowwm::{
    engine::{Control, Engine},
    ipc,
    model::*,
    presentation::Writer,
    store::Store,
};
use std::{
    thread,
    time::{Duration, Instant},
};

#[test]
fn desktop_writes_layout_and_headless_activity_preserves_it_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into())
        .unwrap()
        .with_client_presentation();
    let mut state = store.load(dir.path().into()).unwrap();
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        dir.path().into(),
        Default::default(),
    );
    let node_id = node.id.clone();
    state.nodes.push(node);
    let (host, _) = Engine::new(state, store.clone());
    let socket = dir.path().join("runtime.sock");
    ipc::serve(ipc::bind(&socket).unwrap(), host.clone(), vec![]);
    let writer = Writer::new(dir.path().into()).unwrap();
    assert!(Writer::new(dir.path().into()).is_err());
    let (desktop, _) = Engine::connect_observed(socket, Some(writer.observer())).unwrap();
    desktop
        .request(Control::Move {
            id: node_id.clone(),
            position: Position { x: 427., y: 193. },
        })
        .unwrap();
    let path = dir.path().join("presentation-v2.json");
    let start = Instant::now();
    loop {
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if document["nodes"][&node_id]["position"]["x"] == 427. {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    desktop.shutdown();
    let presentation = std::fs::read(&path).unwrap();
    host.request(Control::Move {
        id: node_id.clone(),
        position: Position { x: 800., y: 500. },
    })
    .unwrap();
    host.shutdown();
    assert_eq!(std::fs::read(&path).unwrap(), presentation);
    let restored = store.load(dir.path().into()).unwrap();
    assert_eq!(restored.node(&node_id).unwrap().position.x, 427.);
    assert_eq!(restored.node(&node_id).unwrap().position.y, 193.);
}
