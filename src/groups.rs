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
        let existing = state.workspaces.iter().position(|w| {
            w.row.as_ref().is_some_and(|b| {
                if b.session == desktop.session {
                    b.id == row.id
                } else if let Some(name) = &b.name {
                    row.name.as_ref() == Some(name)
                } else {
                    b.output == row.output && b.index == row.idx
                }
            })
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
                });
                changed = true;
                state.workspaces.len() - 1
            });
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
    for window in &desktop.windows {
        let Some(group) = state.workspaces.iter().find(|g| {
            g.row
                .as_ref()
                .is_some_and(|r| r.session == desktop.session && Some(r.id) == window.workspace_id)
        }) else {
            continue;
        };
        let group_id = group.id.clone();
        let cwd = group.cwd.clone();
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
                .filter(|n| n.workspace == group_id && !n.desktop_window)
                .map(|n| n.position.x + 360.0)
                .fold(100.0, f64::max);
            let count = state
                .nodes
                .iter()
                .filter(|n| n.workspace == group_id && n.desktop_window)
                .count();
            let mut node = Node::new(
                &group_id,
                NodeKind::Window,
                cwd,
                Position {
                    x: right + (count % 2) as f64 * 360.0,
                    y: 80.0 + (count / 2) as f64 * 240.0,
                },
            );
            node.desktop_window = true;
            state.nodes.push(node);
            changed = true;
            state.nodes.len() - 1
        });
        if state.nodes[index].workspace != group_id {
            let node_id = state.nodes[index].id.clone();
            relocate_component(state, &node_id, &group_id);
            changed = true;
        }
        let node = &mut state.nodes[index];
        let newly_bound = node.window_id != Some(window.id);
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
    for node in &mut state.nodes {
        if node.window_id.is_some() && !seen.contains(&node.id) {
            node.window_id = None;
            if node.kind == NodeKind::Window {
                node.status = Status::Stopped;
            }
            changed = true;
        }
    }
    changed
}

fn relocate_component(state: &mut State, id: &str, workspace: &str) {
    let mut component = HashSet::from([id.to_string()]);
    loop {
        let count = component.len();
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
