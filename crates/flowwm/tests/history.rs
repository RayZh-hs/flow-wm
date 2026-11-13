mod support;
use flowwm::{
    client::Action,
    graph::{Connection, Endpoint},
    model::*,
};

#[test]
fn graph_edits_and_group_placements_undo_and_redo_without_rewinding_execution() {
    let mut state = State::empty("/tmp".into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Shell,
        "/tmp".into(),
        Position { x: 100., y: 80. },
    );
    node.command = "printf done".into();
    let id = node.id.clone();
    state.nodes.push(node);
    let (client, _) = support::fixture(state);
    client
        .request(Action::MoveMany {
            positions: vec![(id.clone(), Position { x: 550., y: 320. })],
        })
        .unwrap();
    client
        .request(Action::Run {
            id: id.clone(),
            downstream: false,
        })
        .unwrap();
    support::wait(|| client.snapshot().state.node(&id).unwrap().status == Status::Done);
    client.request(Action::Undo).unwrap();
    let view = client.snapshot();
    assert_eq!(view.state.node(&id).unwrap().position.x, 100.);
    assert_eq!(view.state.node(&id).unwrap().status, Status::Done);
    assert!(!view.state.workspace().graph_arranged);
    client.request(Action::Redo).unwrap();
    assert_eq!(client.snapshot().state.node(&id).unwrap().position.x, 550.);
    assert!(client.snapshot().state.workspace().graph_arranged);
    let mut edited = client.snapshot().state.node(&id).unwrap().clone();
    edited.title = "Edited command".into();
    client
        .request(Action::Update {
            node: Box::new(edited),
        })
        .unwrap();
    client.request(Action::Undo).unwrap();
    assert_eq!(
        client.snapshot().state.node(&id).unwrap().title,
        "Shell command"
    );
    client.request(Action::Redo).unwrap();
    assert_eq!(
        client.snapshot().state.node(&id).unwrap().title,
        "Edited command"
    );
    assert_eq!(
        client.snapshot().state.node(&id).unwrap().status,
        Status::Done
    );
}

#[test]
fn deletion_restores_connections_and_positions_and_new_edits_clear_redo() {
    let mut state = State::empty("/tmp".into());
    for x in [100., 460.] {
        state.nodes.push(Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x, y: 80. },
        ));
    }
    let ids: Vec<_> = state.nodes.iter().map(|n| n.id.clone()).collect();
    let edge = Connection {
        source: Endpoint {
            node: ids[0].clone(),
            pin: "out".into(),
        },
        target: Endpoint {
            node: ids[1].clone(),
            pin: "in".into(),
        },
    };
    state.connections.push(edge.clone());
    let (client, _) = support::fixture(state);
    client
        .request(Action::DeleteMany { ids: ids.clone() })
        .unwrap();
    assert!(client.snapshot().state.nodes.is_empty());
    client.request(Action::Undo).unwrap();
    assert_eq!(client.snapshot().state.nodes.len(), 2);
    assert!(client.snapshot().state.connections.contains(&edge));
    assert_eq!(
        client.snapshot().state.node(&ids[1]).unwrap().position.x,
        460.
    );
    client.request(Action::Redo).unwrap();
    assert!(client.snapshot().state.nodes.is_empty());
    client.request(Action::Undo).unwrap();
    client
        .request(Action::Move {
            id: ids[0].clone(),
            position: Position { x: 900., y: 80. },
        })
        .unwrap();
    client.request(Action::Redo).unwrap();
    assert_eq!(client.snapshot().state.nodes.len(), 2);
    assert_eq!(
        client.snapshot().state.node(&ids[0]).unwrap().position.x,
        900.
    );
    client
        .request(Action::DisconnectPins {
            connection: edge.clone(),
        })
        .unwrap();
    client.request(Action::Undo).unwrap();
    assert!(client.snapshot().state.connections.contains(&edge));
}

#[test]
fn creation_and_paste_are_reversible_without_reusing_native_handles() {
    let state = State::empty("/tmp".into());
    let node = Node::new(
        &state.selected_workspace,
        NodeKind::Constant,
        "/tmp".into(),
        Position { x: 700., y: 180. },
    );
    let id = node.id.clone();
    let (client, _) = support::fixture(state);
    client
        .request(Action::Create {
            node: Box::new(node),
        })
        .unwrap();
    client.request(Action::Undo).unwrap();
    assert!(client.snapshot().state.node(&id).is_none());
    client.request(Action::Redo).unwrap();
    assert_eq!(client.snapshot().state.node(&id).unwrap().position.x, 700.);
    let recipe = client
        .snapshot()
        .state
        .recipe("copy".into(), &[id.clone()])
        .unwrap();
    let result = client
        .request(Action::Paste {
            recipe,
            offset: Position { x: 300., y: 200. },
        })
        .unwrap();
    let new_id = result["ids"][0].as_str().unwrap();
    assert_eq!(client.snapshot().state.nodes.len(), 2);
    client.request(Action::Undo).unwrap();
    assert!(client.snapshot().state.node(new_id).is_none());
    client.request(Action::Redo).unwrap();
    assert_eq!(
        client.snapshot().state.node(new_id).unwrap().position.x,
        1000.
    );
    assert_eq!(
        client.snapshot().state.node(new_id).unwrap().window_id,
        None
    );
}
