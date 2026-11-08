//! Reconcile Niri rows and window presentations without executing workflow nodes.
use crate::{model::*, niri::Desktop};
use std::collections::HashSet;

fn assign<T: PartialEq>(place: &mut T, value: T) -> bool {
    if *place == value {
        return false;
    }
    *place = value;
    true
}

pub fn selector(window: &niri_ipc::Window) -> String {
    format!(
        "{}::{}",
        window.app_id.as_deref().unwrap_or_default(),
        window.title.as_deref().unwrap_or_default()
    )
}

pub fn synchronize(state: &mut State, desktop: &Desktop) -> bool {
    if !desktop.connected
        || !desktop.windows_ready
        || !desktop.workspaces_ready
        || desktop.workspaces.is_empty()
    {
        return false;
    }
    let mut changed = false;
    let previous_session = state.workspace().row.as_ref().map(|b| b.session.clone());
    let mut rows: Vec<_> = desktop
        .workspaces
        .iter()
        .filter(|row| {
            row.is_focused
                || row.name.is_some()
                || desktop
                    .windows
                    .iter()
                    .any(|w| w.workspace_id == Some(row.id))
        })
        .collect();
    rows.sort_by_key(|row| (!row.is_focused, row.output.clone(), row.idx));
    for row in rows {
        let binding = Row {
            session: desktop.session.clone(),
            id: row.id,
            index: row.idx,
            output: row.output.clone(),
            name: row.name.clone(),
        };
        // A live identity wins over names remembered from earlier sessions.
        // Naming a row must never give it a second graph/canvas owner.
        let existing = state
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                w.row
                    .as_ref()
                    .is_some_and(|b| b.session == desktop.session && b.id == row.id)
            })
            .max_by_key(|(_, w)| {
                (
                    state.nodes.iter().filter(|n| n.workspace == w.id).count(),
                    w.id == state.selected_workspace,
                )
            })
            .map(|(index, _)| index)
            .or_else(|| {
                state
                    .workspaces
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| {
                        w.row.as_ref().is_some_and(|b| {
                            b.session != desktop.session
                                && ((b.name.is_some() && b.name == row.name)
                                    || (b.output == row.output
                                        && b.index == row.idx
                                        && (b.name.is_none()
                                            || (row.name.is_none()
                                                && Some(&b.session) == previous_session.as_ref()))))
                        })
                    })
                    .max_by_key(|(_, w)| {
                        let b = w.row.as_ref().unwrap();
                        (
                            b.name.is_some() && b.name == row.name,
                            Some(&b.session) == previous_session.as_ref(),
                            state.nodes.iter().filter(|n| n.workspace == w.id).count(),
                            w.id == state.selected_workspace,
                        )
                    })
                    .map(|(index, _)| index)
            });
        let index = existing
            .or_else(|| {
                row.is_focused
                    .then(|| {
                        state
                            .workspaces
                            .iter()
                            .position(|w| w.id == state.selected_workspace && w.row.is_none())
                    })
                    .flatten()
            })
            .unwrap_or_else(|| {
                let cwd = state.workspace().cwd.clone();
                state.workspaces.push(Workspace {
                    id: id(),
                    name: row
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Row {}", row.idx)),
                    cwd,
                    notes: String::new(),
                    row: None,
                    window_array: false,
                    graph_arranged: false,
                    view: RowView::default(),
                });
                changed = true;
                state.workspaces.len() - 1
            });
        // Repair duplicate bindings saved by older versions without discarding
        // either group's workflows. Live connected windows reconcile below.
        for (other_index, other) in state.workspaces.iter_mut().enumerate() {
            if other_index != index
                && other
                    .row
                    .as_ref()
                    .is_some_and(|b| b.session == desktop.session && b.id == row.id)
            {
                other.row = None;
                changed = true;
            }
        }
        let workspace = &mut state.workspaces[index];
        // Generated row labels follow reordering; user-chosen group names stay intact.
        if workspace.name.starts_with("Row ") {
            changed |= assign(
                &mut workspace.name,
                row.name
                    .clone()
                    .unwrap_or_else(|| format!("Row {}", row.idx)),
            );
        }
        changed |= assign(&mut workspace.row, Some(binding));
        if row.is_focused {
            changed |= assign(&mut state.selected_workspace, workspace.id.clone());
        }
    }
    let mut seen = HashSet::new();
    let mut ordered: Vec<_> = desktop.windows.iter().collect();
    ordered.sort_by_key(|w| (w.workspace_id, w.layout.pos_in_scrolling_layout, w.id));
    for window in ordered {
        let Some(group) = state.workspaces.iter().find(|g| {
            g.row
                .as_ref()
                .is_some_and(|r| r.session == desktop.session && Some(r.id) == window.workspace_id)
        }) else {
            continue;
        };
        let group_id = group.id.clone();
        let cwd = group.cwd.clone();
        let view = group.view;
        let key = selector(window);
        let existing = state
            .nodes
            .iter()
            .position(|n| n.window_id == Some(window.id))
            .or_else(|| {
                state.nodes.iter().position(|n| {
                    n.kind == NodeKind::Window
                        && n.window_id.is_none()
                        && n.window_match.as_ref() == Some(&key)
                })
            })
            .or_else(|| {
                if window.app_id.as_deref() != Some("dev.flowwm.Shell") {
                    return None;
                }
                let matches: Vec<_> = state
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| {
                        n.kind != NodeKind::Window
                            && n.window_id.is_none()
                            && n.workspace == group_id
                            && Some(n.title.as_str()) == window.title.as_deref()
                    })
                    .map(|(i, _)| i)
                    .collect();
                (matches.len() == 1).then(|| matches[0])
            });
        let index = existing.unwrap_or_else(|| {
            let right = state
                .nodes
                .iter()
                .filter(|n| n.workspace == group_id)
                .map(|n| n.position.x + 360.0)
                .fold(100.0, f64::max);
            let mut node = Node::new(
                &group_id,
                NodeKind::Window,
                cwd,
                Position { x: right, y: 80.0 },
            );
            node.desktop_window = true;
            state.nodes.push(node);
            changed = true;
            state.nodes.len() - 1
        });
        let relocated = state.nodes[index].workspace != group_id;
        if relocated {
            let node_id = state.nodes[index].id.clone();
            relocate_component(state, &node_id, &group_id);
            changed = true;
        }
        let node = &mut state.nodes[index];
        let newly_bound = node.window_id != Some(window.id);
        if newly_bound || relocated {
            node.column_mode = match view {
                RowView::Nodes => ColumnMode::Node,
                RowView::Windows => ColumnMode::Window,
            };
        }
        changed |= assign(&mut node.window_id, Some(window.id));
        seen.insert(node.id.clone());
        if node.kind == NodeKind::Window {
            if (newly_bound && node.status != Status::Done)
                || (node.desktop_window && node.status == Status::Stopped)
            {
                changed |= assign(&mut node.status, Status::Running);
            }
            changed |= assign(&mut node.window_match, Some(key));
            if node.desktop_window {
                changed |= assign(
                    &mut node.title,
                    window
                        .title
                        .clone()
                        .or_else(|| window.app_id.clone())
                        .unwrap_or_else(|| "Application window".into()),
                );
                changed |= assign(&mut node.command, window.app_id.clone().unwrap_or_default());
            }
        }
    }
    let removed: HashSet<_> = state
        .nodes
        .iter()
        .filter(|n| {
            n.desktop_window
                && n.window_id.is_some()
                && !seen.contains(&n.id)
                && state
                    .workspaces
                    .iter()
                    .any(|w| w.id == n.workspace && w.view == RowView::Windows)
        })
        .map(|n| n.id.clone())
        .collect();
    if !removed.is_empty() {
        state.nodes.retain(|n| !removed.contains(&n.id));
        for node in &mut state.nodes {
            node.dependencies.retain(|id| !removed.contains(id));
        }
        state
            .connections
            .retain(|e| !removed.contains(&e.source.node) && !removed.contains(&e.target.node));
        state
            .watchers
            .retain(|w| !removed.contains(&w.source) && !removed.contains(&w.target));
        changed = true;
    }
    for node in &mut state.nodes {
        if node.window_id.is_some() && !seen.contains(&node.id) {
            node.window_id = None;
            if node.kind == NodeKind::Window {
                node.status = Status::Stopped;
            }
            changed = true;
        }
    }
    // Migrate the former two-column grid once. User positioning survives all later events.
    for group in &mut state.workspaces {
        if !group.window_array {
            let start = state
                .nodes
                .iter()
                .filter(|n| n.workspace == group.id && !n.desktop_window)
                .map(|n| n.position.x + 360.0)
                .fold(100.0, f64::max);
            let mut windows: Vec<_> = state
                .nodes
                .iter_mut()
                .filter(|n| n.workspace == group.id && n.desktop_window)
                .collect();
            windows.sort_by_key(|n| {
                desktop
                    .windows
                    .iter()
                    .find(|w| Some(w.id) == n.window_id)
                    .and_then(|w| w.layout.pos_in_scrolling_layout)
                    .unwrap_or((usize::MAX, usize::MAX))
            });
            for (index, node) in windows.into_iter().enumerate() {
                node.position = Position {
                    x: start + index as f64 * 360.0,
                    y: 80.0,
                };
            }
            group.window_array = true;
            changed = true;
        }
    }
    // Reconcile column membership without tying persistent identities to shifting indices.
    let mut columns = std::collections::HashMap::new();
    let mut used_columns = HashSet::new();
    for window in &desktop.windows {
        let Some((index, _)) = window.layout.pos_in_scrolling_layout else {
            continue;
        };
        let Some(node) = state.nodes.iter().find(|n| n.window_id == Some(window.id)) else {
            continue;
        };
        columns
            .entry((window.workspace_id, index))
            .or_insert_with(|| {
                let mut column = node.column.clone().unwrap_or_else(|| node.id.clone());
                if !used_columns.insert((node.workspace.clone(), column.clone())) {
                    column = if used_columns.insert((node.workspace.clone(), node.id.clone())) {
                        node.id.clone()
                    } else {
                        id()
                    };
                }
                (column, node.column_mode)
            });
    }
    for window in &desktop.windows {
        let Some((index, _)) = window.layout.pos_in_scrolling_layout else {
            continue;
        };
        let Some((column, mode)) = columns.get(&(window.workspace_id, index)) else {
            continue;
        };
        if let Some(node) = state
            .nodes
            .iter_mut()
            .find(|n| n.window_id == Some(window.id))
        {
            changed |= assign(&mut node.column, Some(column.clone()));
            changed |= assign(&mut node.column_mode, *mode);
        }
    }
    changed
}

fn relocate_component(state: &mut State, id: &str, workspace: &str) {
    let mut component = HashSet::from([id.to_string()]);
    loop {
        let count = component.len();
        for edge in &state.connections {
            if component.contains(&edge.source.node) || component.contains(&edge.target.node) {
                component.insert(edge.source.node.clone());
                component.insert(edge.target.node.clone());
            }
        }
        for node in &state.nodes {
            for parent in &node.dependencies {
                if component.contains(&node.id) || component.contains(parent) {
                    component.insert(node.id.clone());
                    component.insert(parent.clone());
                }
            }
        }
        for watcher in &state.watchers {
            if component.contains(&watcher.source) || component.contains(&watcher.target) {
                component.insert(watcher.source.clone());
                component.insert(watcher.target.clone());
            }
        }
        if component.len() == count {
            break;
        }
    }
    for node in &mut state.nodes {
        if component.contains(&node.id) {
            node.workspace = workspace.into();
        }
    }
}
