//! Keep native rows alive for workflow content, independently of the GTK canvas.
use super::Bindings;
use crate::model::State;
use std::collections::BTreeSet;

fn occupied_rows(state: &State, bindings: &Bindings) -> Vec<u64> {
    let workflows: BTreeSet<_> = state.nodes.iter().map(|n| n.workspace.as_str()).collect();
    bindings
        .rows
        .iter()
        .filter(|(workflow, row)| {
            row.session == bindings.session && workflows.contains(workflow.as_str())
        })
        .map(|(_, row)| row.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Default)]
pub(super) struct Publisher {
    last: Option<(String, Vec<u64>)>,
}

impl Publisher {
    pub(super) fn publish(&mut self, state: &State, bindings: &Bindings) {
        if bindings.session.is_empty() {
            return;
        }
        let current = (bindings.session.clone(), occupied_rows(state, bindings));
        if self.last.as_ref() == Some(&current) {
            return;
        }
        if crate::niri::request(niri_ipc::Request::SetNodeRowContents {
            workspace_ids: current.1.clone(),
        })
        .is_ok()
        {
            self.last = Some(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind, Row, RowView};

    #[test]
    fn rows_follow_the_last_node_even_in_windows_view_and_ignore_stale_sessions() {
        let mut state = State::empty("/tmp".into());
        state.workspaces[0].view = RowView::Windows;
        let workflow = state.selected_workspace.clone();
        let mut bindings = Bindings {
            session: "current".into(),
            ..Default::default()
        };
        bindings.rows.insert(
            workflow.clone(),
            Row {
                session: "current".into(),
                id: 10,
                index: 1,
                output: None,
                name: None,
            },
        );
        for kind in [NodeKind::Shell, NodeKind::Window] {
            state.nodes.push(Node::new(
                &workflow,
                kind,
                "/tmp".into(),
                Default::default(),
            ));
        }
        assert_eq!(occupied_rows(&state, &bindings), vec![10]);
        state.nodes.remove(0);
        assert_eq!(occupied_rows(&state, &bindings), vec![10]);
        bindings.session = "restarted".into();
        assert!(occupied_rows(&state, &bindings).is_empty());
        bindings.rows.get_mut(&workflow).unwrap().session = "restarted".into();
        assert_eq!(occupied_rows(&state, &bindings), vec![10]);
        state.nodes.clear();
        assert!(occupied_rows(&state, &bindings).is_empty());
    }
}
