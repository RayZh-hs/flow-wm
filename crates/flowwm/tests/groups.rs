use flowwm::{groups, model::*, niri::Desktop};
use niri_ipc::{Window, Workspace as NiriRow};

fn row(id: u64, index: u8, focused: bool) -> NiriRow {
    serde_json::from_value(serde_json::json!({"id":id,"idx":index,"name":null,"output":"DP-1","is_urgent":false,"is_active":focused,"is_focused":focused,"active_window_id":null})).unwrap()
}
fn window(id: u64, row: u64, title: &str) -> Window {
    serde_json::from_value(serde_json::json!({"id":id,"workspace_id":row,"title":title,"app_id":"test.editor","pid":100,"is_focused":false,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[800,600],"window_size":[800,600],"tile_pos_in_workspace_view":[16,60],"window_offset_in_tile":[0,0]},"focus_timestamp":null})).unwrap()
}
fn desktop() -> Desktop {
    Desktop {
        connected: true,
        session: "niri.session.1".into(),
        windows_ready: true,
        workspaces_ready: true,
        workspaces: vec![row(10, 1, true), row(20, 2, false), row(30, 3, false)],
        windows: vec![window(1, 10, "Editor"), window(2, 20, "Browser")],
        error: None,
    }
}

#[test]
fn active_niri_row_selects_its_group_and_empty_tail_does_not_create_groups() {
    let mut state = State::empty("/tmp".into());
    let original = state.selected_workspace.clone();
    let mut desktop = desktop();
    assert!(groups::synchronize(&mut state, &desktop));
    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.workspace().row.as_ref().unwrap().id, 10);
    assert_eq!(state.selected_workspace, original);
    assert_eq!(state.nodes.len(), 2);
    assert!(
        !groups::synchronize(&mut state, &desktop),
        "unchanged events must not save state or wake the UI"
    );
    desktop.activate_row(20, true);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.workspace().row.as_ref().unwrap().id, 20);
    assert_ne!(state.selected_workspace, original);
    state.validate().unwrap();
}

#[test]
fn row_views_survive_switching_reordering_and_new_windows() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    assert!(state.workspaces.iter().all(|w| w.view == RowView::Windows));
    state
        .workspaces
        .iter_mut()
        .find(|w| w.row.as_ref().unwrap().id == 10)
        .unwrap()
        .view = RowView::Nodes;
    desktop.activate_row(20, true);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.workspace().view, RowView::Windows);
    desktop.windows.push(window(3, 10, "New node"));
    desktop
        .windows
        .last_mut()
        .unwrap()
        .layout
        .pos_in_scrolling_layout = Some((2, 1));
    groups::synchronize(&mut state, &desktop);
    assert_eq!(
        state
            .nodes
            .iter()
            .find(|n| n.window_id == Some(3))
            .unwrap()
            .column_mode,
        ColumnMode::Node
    );
    desktop.workspaces[0].idx = 2;
    desktop.workspaces[1].idx = 1;
    desktop.activate_row(10, true);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.workspace().view, RowView::Nodes);
    assert_eq!(state.workspace().row.as_ref().unwrap().index, 2);
    desktop.windows.last_mut().unwrap().workspace_id = Some(20);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(
        state
            .nodes
            .iter()
            .find(|n| n.window_id == Some(3))
            .unwrap()
            .column_mode,
        ColumnMode::Window
    );
    // An unnamed trailing row has its own default; it never inherits Nodes.
    desktop.activate_row(30, true);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.workspace().view, RowView::Windows);
}

#[test]
fn row_reordering_and_title_changes_keep_node_ids_positions_and_group_identity() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let id = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    state.node_mut(&id).unwrap().position.x = 678.;
    desktop.windows[0].title = Some("Renamed editor".into());
    desktop.workspaces[0].idx = 2;
    desktop.workspaces[1].idx = 1;
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.nodes.len(), 2);
    assert_eq!(state.node(&id).unwrap().title, "Renamed editor");
    assert_eq!(state.node(&id).unwrap().position.x, 678.);
    assert_eq!(state.workspace().row.as_ref().unwrap().index, 2);
    desktop.windows.remove(0);
    groups::synchronize(&mut state, &desktop);
    assert!(state.node(&id).is_none());
}

#[test]
fn moving_a_window_moves_its_connected_branch_and_preserves_edges() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let id = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    let group = state.node(&id).unwrap().workspace.clone();
    let mut child = Node::new(&group, NodeKind::Gate, "/tmp".into(), Position::default());
    child.dependencies.push(id.clone());
    let child_id = child.id.clone();
    state.nodes.push(child);
    desktop.windows[0].workspace_id = Some(20);
    groups::synchronize(&mut state, &desktop);
    assert_ne!(state.node(&id).unwrap().workspace, group);
    assert_eq!(
        state.node(&id).unwrap().workspace,
        state.node(&child_id).unwrap().workspace
    );
    assert_eq!(state.node(&child_id).unwrap().dependencies, vec![id]);
    state.validate().unwrap();
}

#[test]
fn native_output_is_bound_to_its_existing_workflow_node() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    let mut node = Node::new(
        &state.selected_workspace,
        NodeKind::Human,
        "/tmp".into(),
        Position::default(),
    );
    node.title = "Draft plan".into();
    let id = node.id.clone();
    state.nodes.push(node);
    desktop.windows[0].title = Some("Draft plan".into());
    desktop.windows[0].app_id = Some("dev.flowwm.Shell".into());
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.node(&id).unwrap().window_id, Some(1));
    assert_eq!(
        state.nodes.len(),
        2,
        "native output must not create a second node"
    );
    assert_eq!(
        state.node(&id).unwrap().status,
        Status::Idle,
        "presentation must not start work"
    );
}

#[test]
fn a_compositor_restart_rebinds_rows_instead_of_trusting_reused_ids() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let group = state.selected_workspace.clone();
    for node in &mut state.nodes {
        node.window_id = None;
    }
    desktop.session = "niri.session.2".into();
    desktop.workspaces[0].id = 20;
    desktop.workspaces[1].id = 10;
    desktop.windows[0].workspace_id = Some(20);
    desktop.windows[1].workspace_id = Some(10);
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.selected_workspace, group);
    assert_eq!(state.workspace().row.as_ref().unwrap().id, 20);
    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.nodes.len(), 2);
}

#[test]
fn naming_a_live_row_cannot_rebind_a_stale_group_or_duplicate_its_canvas() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let current = state.selected_workspace.clone();
    let mut stale = state.workspace().clone();
    stale.id = id();
    stale.row.as_mut().unwrap().session = "previous-session".into();
    stale.row.as_mut().unwrap().name = Some("Row 1".into());
    state.workspaces.insert(0, stale);
    desktop.workspaces[0].name = Some("Row 1".into());
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.selected_workspace, current);
    assert_eq!(
        state
            .workspaces
            .iter()
            .filter(|w| w
                .row
                .as_ref()
                .is_some_and(|r| r.session == desktop.session && r.id == 10))
            .count(),
        1
    );
    state.validate().unwrap();
}

#[test]
fn duplicate_saved_row_bindings_recover_without_discarding_workflow_nodes() {
    let mut state = State::empty("/tmp".into());
    let desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let current = state.selected_workspace.clone();
    let mut duplicate = state.workspace().clone();
    duplicate.id = id();
    let duplicate_id = duplicate.id.clone();
    state.workspaces.insert(0, duplicate);
    state.selected_workspace = duplicate_id.clone();
    let nodes = state.nodes.len();
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.selected_workspace, current);
    assert_eq!(state.nodes.len(), nodes);
    assert!(
        state
            .workspaces
            .iter()
            .find(|w| w.id == duplicate_id)
            .unwrap()
            .row
            .is_none()
    );
    assert!(!groups::synchronize(&mut state, &desktop));
    state.validate().unwrap();
}

#[test]
fn a_named_node_row_rebinds_to_its_slot_before_the_new_session_restores_its_name() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let current = state.selected_workspace.clone();
    desktop.workspaces[0].name = Some("Row 1".into());
    groups::synchronize(&mut state, &desktop);
    desktop.session = "new-session".into();
    desktop.workspaces[0].name = None;
    for node in &mut state.nodes {
        node.window_id = None;
    }
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.selected_workspace, current);
    assert_eq!(state.workspaces.len(), 2);
    state.validate().unwrap();
}

#[test]
fn identical_window_titles_still_have_distinct_nodes() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    desktop.windows.push(window(3, 10, "Editor"));
    groups::synchronize(&mut state, &desktop);
    let first = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    let second = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(3))
        .unwrap()
        .id
        .clone();
    assert_ne!(first, second);
    desktop.windows[0].title = Some("Changed".into());
    groups::synchronize(&mut state, &desktop);
    assert_eq!(
        state
            .nodes
            .iter()
            .find(|n| n.window_id == Some(1))
            .unwrap()
            .id,
        first
    );
    assert_eq!(
        state
            .nodes
            .iter()
            .find(|n| n.window_id == Some(3))
            .unwrap()
            .id,
        second
    );
}

#[test]
fn partial_initial_events_cannot_clear_persisted_window_bindings() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let original: Vec<_> = state
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.window_id))
        .collect();
    let saved = desktop.clone();
    desktop.windows.clear();
    desktop.windows_ready = false;
    assert!(!groups::synchronize(&mut state, &desktop));
    assert_eq!(
        state
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.window_id))
            .collect::<Vec<_>>(),
        original
    );
    desktop = saved.clone();
    desktop.workspaces_ready = false;
    assert!(!groups::synchronize(&mut state, &desktop));
    desktop = saved;
    desktop.windows[0].title = Some("New title during restart".into());
    groups::synchronize(&mut state, &desktop);
    assert_eq!(
        state
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.window_id))
            .collect::<Vec<_>>(),
        original
    );
}

#[test]
fn desktop_cache_tracks_background_active_windows_before_switching_rows() {
    let mut desktop = desktop();
    assert!(
        !desktop.apply_event(niri_ipc::Event::WorkspaceActiveWindowChanged {
            workspace_id: 20,
            active_window_id: Some(2)
        })
    );
    desktop.apply_event(niri_ipc::Event::WorkspaceActivated {
        id: 20,
        focused: true,
    });
    assert!(
        desktop
            .workspaces
            .iter()
            .find(|w| w.id == 20)
            .unwrap()
            .is_focused
    );
    assert!(
        !desktop
            .workspaces
            .iter()
            .find(|w| w.id == 10)
            .unwrap()
            .is_active
    );
    assert!(
        desktop
            .windows
            .iter()
            .find(|w| w.id == 2)
            .unwrap()
            .is_focused
    );
    desktop.apply_event(niri_ipc::Event::WindowFocusChanged { id: None });
    assert!(!desktop.windows.iter().any(|w| w.is_focused));
    assert_eq!(
        desktop
            .workspaces
            .iter()
            .find(|w| w.id == 20)
            .unwrap()
            .active_window_id,
        Some(2)
    );
    desktop.activate_row(10, true);
    desktop.activate_row(20, true);
    assert!(
        desktop
            .windows
            .iter()
            .find(|w| w.id == 2)
            .unwrap()
            .is_focused
    );
}

#[test]
fn window_nodes_form_one_horizontal_array_and_keep_manual_positions() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    let mut b = window(3, 10, "Second");
    b.layout.pos_in_scrolling_layout = Some((2, 1));
    let mut c = window(4, 10, "Third");
    c.layout.pos_in_scrolling_layout = Some((3, 1));
    desktop.windows.extend([c, b]);
    groups::synchronize(&mut state, &desktop);
    let nodes: Vec<_> = [1, 3, 4]
        .map(|id| {
            state
                .nodes
                .iter()
                .find(|n| n.window_id == Some(id))
                .unwrap()
        })
        .into();
    assert!(
        nodes
            .windows(2)
            .all(|n| n[0].position.x < n[1].position.x && n[0].position.y == n[1].position.y)
    );
    let id = nodes[1].id.clone();
    state.node_mut(&id).unwrap().position.y += 40.0;
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.node(&id).unwrap().position.y, 120.0);
}

#[test]
fn native_column_membership_survives_reordering_and_separates_after_a_split() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    let mut second = window(3, 10, "Stacked");
    second.layout.pos_in_scrolling_layout = Some((1, 2));
    desktop.windows.push(second);
    groups::synchronize(&mut state, &desktop);
    let first = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    let second = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(3))
        .unwrap()
        .id
        .clone();
    assert_eq!(
        state.node(&first).unwrap().column,
        state.node(&second).unwrap().column
    );
    let original = state.node(&first).unwrap().column.clone();
    desktop.windows[0].layout.pos_in_scrolling_layout = Some((3, 1));
    desktop
        .windows
        .last_mut()
        .unwrap()
        .layout
        .pos_in_scrolling_layout = Some((3, 2));
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.node(&first).unwrap().column, original);
    desktop
        .windows
        .last_mut()
        .unwrap()
        .layout
        .pos_in_scrolling_layout = Some((4, 1));
    groups::synchronize(&mut state, &desktop);
    assert_ne!(
        state.node(&first).unwrap().column,
        state.node(&second).unwrap().column
    );
    assert!(!groups::synchronize(&mut state, &desktop));
}

#[test]
fn closing_in_windows_removes_dangling_edges_but_nodes_view_keeps_history() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    groups::synchronize(&mut state, &desktop);
    let source = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    let mut next = Node::new(
        &state.selected_workspace,
        NodeKind::Human,
        "/tmp".into(),
        Position::default(),
    );
    next.dependencies.push(source.clone());
    next.flow
        .inputs
        .push(flowwm::graph::Pin::new("text", flowwm::graph::Type::String));
    let target = next.id.clone();
    state.nodes.push(next);
    state.connections.push(flowwm::graph::Connection {
        source: flowwm::graph::Endpoint {
            node: source.clone(),
            pin: "output".into(),
        },
        target: flowwm::graph::Endpoint {
            node: target.clone(),
            pin: "text".into(),
        },
    });
    state.validate().unwrap();
    desktop.windows.retain(|w| w.id != 1);
    groups::synchronize(&mut state, &desktop);
    assert!(state.node(&source).is_none());
    assert!(state.node(&target).unwrap().dependencies.is_empty());
    assert!(state.connections.is_empty());
    let mut state = State::empty("/tmp".into());
    let mut desktop = crate::desktop();
    groups::synchronize(&mut state, &desktop);
    state
        .workspaces
        .iter_mut()
        .find(|w| w.id == state.selected_workspace)
        .unwrap()
        .view = RowView::Nodes;
    let source = state
        .nodes
        .iter()
        .find(|n| n.window_id == Some(1))
        .unwrap()
        .id
        .clone();
    desktop.windows.retain(|w| w.id != 1);
    groups::synchronize(&mut state, &desktop);
    assert!(state.node(&source).unwrap().closed_window());
}

#[test]
fn application_selection_binds_the_launched_window_to_the_configured_node() {
    let mut state = State::empty("/tmp".into());
    let mut desktop = desktop();
    desktop.windows.clear();
    groups::synchronize(&mut state, &desktop);
    let mut app = Node::new(
        &state.selected_workspace,
        NodeKind::Window,
        "/tmp".into(),
        Position { x: 500., y: 300. },
    );
    app.flow.properties.insert(
        "application".into(),
        serde_json::json!("org.test.Editor.desktop"),
    );
    app.flow
        .properties
        .insert("application_id".into(), serde_json::json!("TEST.EDITOR"));
    let id = app.id.clone();
    state.nodes.push(app);
    desktop.windows.push(window(7, 10, "A document"));
    groups::synchronize(&mut state, &desktop);
    assert_eq!(state.nodes.len(), 1);
    assert_eq!(state.node(&id).unwrap().window_id, Some(7));
    assert_eq!(state.node(&id).unwrap().position.x, 500.);
}
