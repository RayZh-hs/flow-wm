use crate::{model::*, niri::Desktop};

/// Walk workspace and graph order, keeping acknowledged nodes as cursor positions.
pub(crate) fn next<'a>(
    state: &'a State,
    desktop: &Desktop,
    current: Option<&str>,
) -> Option<&'a Node> {
    let mut nodes = Vec::new();
    for workspace in &state.workspaces {
        if state.orchestration.as_ref().is_some_and(|runtime| {
            !runtime.visible(
                &workspace.id.clone().into(),
                Some(&state.selected_workspace.clone().into()),
            )
        }) {
            continue;
        }
        let mut row: Vec<_> = state
            .nodes
            .iter()
            .filter(|n| n.workspace == workspace.id)
            .collect();
        row.sort_by(|a, b| {
            a.position
                .x
                .total_cmp(&b.position.x)
                .then(a.position.y.total_cmp(&b.position.y))
                .then(a.id.cmp(&b.id))
        });
        nodes.extend(row);
    }
    let start = current
        .and_then(|id| nodes.iter().position(|n| n.id == id))
        .map_or(0, |index| index + 1);
    nodes
        .iter()
        .cycle()
        .skip(start)
        .take(nodes.len())
        .copied()
        .find(|node| {
            node.alerted()
                || node.window_id.is_some_and(|id| {
                    desktop
                        .windows
                        .iter()
                        .any(|window| window.id == id && window.is_urgent)
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(state: &mut State, workspace: &str, x: f64, alert: bool) -> String {
        let mut node = Node::new(
            workspace,
            NodeKind::Shell,
            "/tmp".into(),
            Position { x, y: 0. },
        );
        if alert {
            node.flow
                .properties
                .insert("_alert".into(), serde_json::json!({"message":"Ready"}));
        }
        let id = node.id.clone();
        state.nodes.push(node);
        id
    }

    #[test]
    fn navigation_wraps_across_rows_after_acknowledging_the_current_node() {
        let mut state = State::empty("/tmp".into());
        let first = state.selected_workspace.clone();
        let mut second = state.workspace().clone();
        second.id = "second".into();
        state.workspaces.push(second);
        // Insertion order differs from canvas order.
        let right = node(&mut state, &first, 400., true);
        let left = node(&mut state, &first, 0., true);
        let other = node(&mut state, "second", 0., true);
        let desktop = Desktop::default();
        assert_eq!(next(&state, &desktop, None).unwrap().id, left);
        state
            .node_mut(&left)
            .unwrap()
            .flow
            .properties
            .remove("_alert");
        assert_eq!(next(&state, &desktop, Some(&left)).unwrap().id, right);
        assert_eq!(next(&state, &desktop, Some(&right)).unwrap().id, other);
        assert_eq!(next(&state, &desktop, Some(&other)).unwrap().id, right);
        assert_eq!(next(&state, &desktop, Some("deleted")).unwrap().id, right);
    }

    #[test]
    fn native_urgency_and_node_alerts_share_one_target_and_closed_windows_remain_inspectable() {
        let mut state = State::empty("/tmp".into());
        let workspace = state.selected_workspace.clone();
        let window = node(&mut state, &workspace, 0., true);
        state.node_mut(&window).unwrap().window_id = Some(7);
        let closed = node(&mut state, &workspace, 400., true);
        state.node_mut(&closed).unwrap().desktop_window = true;
        let mut desktop = Desktop::default();
        desktop.windows.push(serde_json::from_value(serde_json::json!({
            "id":7,"workspace_id":1,"title":"Editor","app_id":"test",
            "pid":null,"is_focused":false,"is_floating":false,"is_urgent":true,
            "layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[800,600],
                "window_size":[800,600],"tile_pos_in_workspace_view":[0,0],"window_offset_in_tile":[0,0]},
            "focus_timestamp":null
        })).unwrap());
        assert_eq!(next(&state, &desktop, None).unwrap().id, window);
        state
            .node_mut(&window)
            .unwrap()
            .flow
            .properties
            .remove("_alert");
        assert_eq!(next(&state, &desktop, Some(&closed)).unwrap().id, window);
        assert_eq!(next(&state, &desktop, Some(&window)).unwrap().id, closed);
        desktop.windows[0].is_urgent = false;
        state
            .node_mut(&closed)
            .unwrap()
            .flow
            .properties
            .remove("_alert");
        assert!(next(&state, &desktop, Some(&window)).is_none());
    }

    #[test]
    fn no_alerts_is_a_noop_including_empty_graphs_and_execution_status_alone() {
        let mut state = State::empty("/tmp".into());
        let desktop = Desktop::default();
        assert!(next(&state, &desktop, None).is_none());
        let workspace = state.selected_workspace.clone();
        let id = node(&mut state, &workspace, 0., false);
        for status in [Status::Failed, Status::Review, Status::Running] {
            state.node_mut(&id).unwrap().status = status;
            assert!(next(&state, &desktop, Some(&id)).is_none());
        }
    }
}
