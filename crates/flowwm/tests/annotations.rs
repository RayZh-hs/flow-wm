mod support;
use flowwm::{
    annotations::{Annotation, Content, Edit},
    client::Action,
    model::*,
};

#[test]
fn groups_and_resized_text_persist_and_undo_without_changing_the_runtime_graph() {
    let mut state = State::empty("/tmp".into());
    for x in [100., 480.] {
        state.nodes.push(Node::new(
            &state.selected_workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x, y: 80. },
        ));
    }
    let ids = state
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let (client, _) = support::fixture(state);
    let before = client.snapshot().state.graph();
    let grouped = client
        .request(Action::Canvas {
            edit: Edit::Group { nodes: ids.clone() },
        })
        .unwrap();
    let id = grouped["id"].as_str().unwrap().to_owned();
    assert_eq!(client.snapshot().state.workspace().canvas.len(), 1);
    client.request(Action::Undo).unwrap();
    assert!(client.snapshot().state.workspace().canvas.is_empty());
    client.request(Action::Redo).unwrap();
    assert_eq!(client.snapshot().state.workspace().canvas[0].id, id);
    let mut text = Annotation::text(Position { x: 500., y: 450. });
    text.content = Content::Text {
        text: "A note\nwith 中文 and <markup>".into(),
    };
    text.width = 620.;
    text.height = 320.;
    client
        .request(Action::Canvas {
            edit: Edit::Put { item: text.clone() },
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(client.snapshot().state.graph()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    let (client, _) = client.reconnect();
    let saved = client.snapshot().state;
    assert_eq!(saved.workspace().canvas.len(), 2);
    let note = saved
        .workspace()
        .canvas
        .iter()
        .find(|item| item.id == text.id)
        .unwrap();
    assert_eq!(note.width, 620.);
    assert_eq!(note.height, 320.);
    assert!(matches!(&note.content, Content::Text { text } if text.contains("中文")));
    client
        .request(Action::Canvas {
            edit: Edit::Remove { id },
        })
        .unwrap();
    assert_eq!(client.snapshot().state.nodes.len(), 2);
    assert_eq!(client.snapshot().state.connections.len(), 0);
}

#[test]
fn groups_reject_foreign_members_and_text_rejects_invalid_bounds_transactionally() {
    let state = State::empty("/tmp".into());
    let (client, _) = support::fixture(state);
    assert!(
        client
            .request(Action::Canvas {
                edit: Edit::Group {
                    nodes: vec!["missing".into()]
                }
            })
            .is_err()
    );
    let mut note = Annotation::text(Position::default());
    note.width = -1.;
    assert!(
        client
            .request(Action::Canvas {
                edit: Edit::Put { item: note }
            })
            .is_err()
    );
    assert!(client.snapshot().state.workspace().canvas.is_empty());
}

#[test]
fn copied_groups_remap_members_and_text_only_pastes_have_fresh_identities() {
    let mut state = State::empty("/tmp".into());
    state.nodes.push(Node::new(
        &state.selected_workspace,
        NodeKind::Vox,
        "/tmp".into(),
        Position::default(),
    ));
    let original = state.nodes[0].id.clone();
    flowwm::annotations::apply(
        &mut state,
        Edit::Group {
            nodes: vec![original.clone()],
        },
    )
    .unwrap();
    let recipe = state
        .recipe("Group copy".into(), std::slice::from_ref(&original))
        .unwrap();
    let target = state.selected_workspace.clone();
    let ids = state.instantiate(&recipe, &target, "/tmp".into()).unwrap();
    assert!(
        matches!(&state.workspace().canvas[1].content, Content::Group { nodes } if nodes == &ids)
    );
    assert_ne!(
        state.workspace().canvas[0].id,
        state.workspace().canvas[1].id
    );
    let note = Annotation::text(Position { x: 700., y: 500. });
    let note_id = note.id.clone();
    flowwm::annotations::apply(&mut state, Edit::Put { item: note }).unwrap();
    let recipe = state
        .recipe("Note copy".into(), std::slice::from_ref(&note_id))
        .unwrap();
    assert!(recipe.nodes.is_empty());
    let (client, _) = support::fixture(state);
    let pasted = client
        .request(Action::Paste {
            recipe,
            offset: Position { x: 40., y: 60. },
        })
        .unwrap();
    let copied_id = pasted["canvas_ids"][0].as_str().unwrap();
    assert_ne!(copied_id, note_id);
    assert_eq!(
        client
            .snapshot()
            .state
            .workspace()
            .canvas
            .last()
            .unwrap()
            .position
            .x,
        740.
    );
    client.request(Action::Undo).unwrap();
    assert_eq!(client.snapshot().state.workspace().canvas.len(), 3);
    client.request(Action::Redo).unwrap();
    assert_eq!(client.snapshot().state.workspace().canvas.len(), 4);
}
