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
    unsupported_session: Option<String>,
}

impl Publisher {
    pub(super) fn publish(&mut self, state: &State, bindings: &Bindings) {
        self.publish_with(state, bindings, crate::niri::request);
    }

    fn publish_with(
        &mut self,
        state: &State,
        bindings: &Bindings,
        request: impl FnOnce(niri_ipc::Request) -> anyhow::Result<niri_ipc::Response>,
    ) {
        if bindings.session.is_empty()
            || self.unsupported_session.as_ref() == Some(&bindings.session)
        {
            return;
        }
        let current = (bindings.session.clone(), occupied_rows(state, bindings));
        if self.last.as_ref() == Some(&current) {
            return;
        }
        match request(niri_ipc::Request::SetNodeRowContents {
            workspace_ids: current.1.clone(),
        }) {
            Ok(_) => self.last = Some(current),
            Err(error) if error.to_string() == "error parsing request" => {
                // A compatible shell replacement can leave an older compositor
                // running until the next login. Probe this optional request once.
                self.unsupported_session = Some(bindings.session.clone());
            }
            Err(_) => (), // Retry transport errors on the next snapshot.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind, Row, RowView};

    #[test]
    fn unsupported_compositors_are_probed_once_but_transport_errors_are_retried() {
        let state = State::empty("/tmp".into());
        let mut bindings = Bindings {
            session: "old-compositor".into(),
            ..Default::default()
        };
        let mut publisher = Publisher::default();
        publisher.publish_with(&state, &bindings, |_| {
            anyhow::bail!("error parsing request")
        });
        for _ in 0..100 {
            publisher.publish_with(&state, &bindings, |_| {
                panic!("unsupported request was sent again")
            });
        }
        bindings.session = "new-compositor".into();
        publisher.publish_with(&state, &bindings, |_| anyhow::bail!("Connection refused"));
        let mut received = false;
        publisher.publish_with(&state, &bindings, |_| {
            received = true;
            Ok(niri_ipc::Response::Handled)
        });
        assert!(
            received,
            "a new compositor or a failed transport must retry"
        );
        publisher.publish_with(&state, &bindings, |_| {
            panic!("unchanged contents were resent")
        });
    }

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
