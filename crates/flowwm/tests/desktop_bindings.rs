use flowwm::{
    desktop_provider::{Binding, Bindings},
    model::*,
};

#[test]
fn saved_duplicate_window_bindings_have_one_owner_even_after_runtime_updates() {
    let mut state = State::empty("/tmp".into());
    let mut bindings = Bindings::default();
    for index in 0..3 {
        let node = Node::new(
            &state.selected_workspace,
            NodeKind::Window,
            "/tmp".into(),
            Position::default(),
        );
        bindings.applications.insert(
            node.id.clone(),
            Binding {
                window: 85,
                pid: None,
                workflow: node.workspace.clone(),
                token: format!("attachment-{index}"),
                column: None,
                title: "Editor".into(),
            },
        );
        state.nodes.push(node);
    }
    let bindings: Bindings =
        serde_json::from_slice(&serde_json::to_vec(&bindings).unwrap()).unwrap();
    for _ in 0..3 {
        bindings.apply_native(&mut state);
        assert_eq!(
            state
                .nodes
                .iter()
                .filter(|n| n.window_id == Some(85))
                .count(),
            1
        );
        assert_eq!(state.nodes.len(), 3, "keep every workflow definition");
    }
}

#[test]
fn an_attachment_cannot_follow_a_node_into_another_workflow() {
    let mut state = State::empty("/tmp".into());
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Position::default(),
    );
    node.desktop_window = true;
    node.window_id = Some(7);
    let mut bindings = Bindings::default();
    bindings.applications.insert(
        node.id.clone(),
        Binding {
            window: 7,
            pid: None,
            workflow: "previous-workflow".into(),
            token: "old-attachment".into(),
            column: None,
            title: "Editor".into(),
        },
    );
    state.nodes.push(node);
    bindings.apply_native(&mut state);
    assert_eq!(state.nodes[0].window_id, None);
    assert_eq!(state.nodes[0].status, Status::Stopped);
}
