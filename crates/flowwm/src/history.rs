//! Reversible graph edits, independent of execution state and native window handles.
use crate::{graph::Connection, model::*};
use anyhow::{Context, Result, ensure};
use flow_core::domain::{NodeSpec, RuntimeState, WorkflowId};
use std::collections::BTreeSet;

#[derive(Clone)]
struct Change<T> {
    before: T,
    after: T,
}
#[derive(Clone)]
struct GraphEdit {
    workflow: WorkflowId,
    nodes: Vec<(String, Change<Option<NodeSpec>>)>,
    removed: Vec<Connection>,
    added: Vec<Connection>,
}
#[derive(Clone, Default)]
pub struct Edit {
    graphs: Vec<GraphEdit>,
    positions: Vec<(String, Change<Option<Position>>)>,
    arranged: Vec<(String, Change<bool>)>,
}
#[derive(Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}
fn same<T: serde::Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
impl Edit {
    pub fn between(before: &State, after: &State) -> Self {
        let mut edit = Self::default();
        if let (Some(a), Some(b)) = (&before.orchestration, &after.orchestration)
            && !std::sync::Arc::ptr_eq(a, b)
        {
            for (id, old) in &a.workflows {
                let Some(new) = b.workflows.get(id) else {
                    continue;
                };
                let ids: BTreeSet<_> = old
                    .graph
                    .nodes
                    .iter()
                    .chain(&new.graph.nodes)
                    .map(|n| n.id.0.clone())
                    .collect();
                let nodes = ids
                    .into_iter()
                    .filter_map(|id| {
                        let before = old.graph.node(&id).cloned();
                        let after = new.graph.node(&id).cloned();
                        (!same(&before, &after)).then_some((id, Change { before, after }))
                    })
                    .collect::<Vec<_>>();
                let removed = old
                    .graph
                    .connections
                    .iter()
                    .filter(|e| !new.graph.connections.contains(e))
                    .cloned()
                    .collect::<Vec<_>>();
                let added = new
                    .graph
                    .connections
                    .iter()
                    .filter(|e| !old.graph.connections.contains(e))
                    .cloned()
                    .collect::<Vec<_>>();
                if !nodes.is_empty() || !removed.is_empty() || !added.is_empty() {
                    edit.graphs.push(GraphEdit {
                        workflow: id.clone(),
                        nodes,
                        removed,
                        added,
                    });
                }
            }
        }
        let ids: BTreeSet<_> = before
            .nodes
            .iter()
            .chain(&after.nodes)
            .map(|n| n.id.clone())
            .collect();
        for id in ids {
            let a = before.node(&id).map(|n| n.position.clone());
            let b = after.node(&id).map(|n| n.position.clone());
            if !same(&a, &b) {
                edit.positions.push((
                    id,
                    Change {
                        before: a,
                        after: b,
                    },
                ));
            }
        }
        for a in &before.workspaces {
            if let Some(b) = after.workspaces.iter().find(|w| w.id == a.id) {
                if a.graph_arranged != b.graph_arranged {
                    edit.arranged.push((
                        a.id.clone(),
                        Change {
                            before: a.graph_arranged,
                            after: b.graph_arranged,
                        },
                    ));
                }
            }
        }
        edit
    }
    pub fn scope(
        &mut self,
        nodes: &BTreeSet<String>,
        edge: Option<&(String, String)>,
        all_positions: bool,
    ) {
        if !all_positions {
            self.positions.retain(|(id, _)| nodes.contains(id));
        }
        for graph in &mut self.graphs {
            graph.nodes.retain(|(id, _)| nodes.contains(id));
            let keep = |e: &Connection| {
                nodes.contains(&e.source.node)
                    || nodes.contains(&e.target.node)
                    || edge.is_some_and(|(a, b)| e.source.node == *a && e.target.node == *b)
            };
            graph.removed.retain(keep);
            graph.added.retain(keep);
        }
        self.graphs
            .retain(|g| !g.nodes.is_empty() || !g.removed.is_empty() || !g.added.is_empty());
    }
    pub fn graphs(
        &self,
        runtime: &RuntimeState,
        redo: bool,
    ) -> Result<Vec<(WorkflowId, flow_core::domain::FlowNet)>> {
        let mut result = vec![];
        for edit in &self.graphs {
            let mut graph = runtime
                .workflows
                .get(&edit.workflow)
                .context("The edited workflow no longer exists")?
                .graph
                .clone();
            for (id, change) in &edit.nodes {
                let (expected, replacement) = if redo {
                    (&change.before, &change.after)
                } else {
                    (&change.after, &change.before)
                };
                ensure!(
                    same(&graph.node(id).cloned(), expected),
                    "Node was changed outside this edit; history cannot overwrite it"
                );
                graph.nodes.retain(|n| n.id.0 != *id);
                if let Some(spec) = replacement {
                    graph.nodes.push(spec.clone());
                }
            }
            let (remove, add) = if redo {
                (&edit.removed, &edit.added)
            } else {
                (&edit.added, &edit.removed)
            };
            graph.connections.retain(|e| !remove.contains(e));
            for edge in add {
                if !graph.connections.contains(edge) {
                    graph.connections.push(edge.clone());
                }
            }
            result.push((edit.workflow.clone(), graph));
        }
        Ok(result)
    }
    pub fn apply_view(&self, state: &mut State, redo: bool) {
        for (id, change) in &self.positions {
            if let (Some(node), Some(position)) = (
                state.node_mut(id),
                if redo { &change.after } else { &change.before },
            ) {
                node.position = position.clone();
            }
        }
        for (id, change) in &self.arranged {
            if let Some(w) = state.workspaces.iter_mut().find(|w| w.id == *id) {
                w.graph_arranged = if redo { change.after } else { change.before };
            }
        }
    }
}
impl History {
    pub fn record(&mut self, edit: Edit, limit: usize) {
        if limit == 0 {
            self.undo.clear();
            self.redo.clear();
            return;
        }
        if edit.graphs.is_empty() && edit.positions.is_empty() && edit.arranged.is_empty() {
            return;
        }
        self.undo.push(edit);
        if self.undo.len() > limit {
            self.undo.drain(..self.undo.len() - limit);
        }
        self.redo.clear();
    }
    pub fn peek(&self, redo: bool) -> Option<Edit> {
        if redo {
            self.redo.last()
        } else {
            self.undo.last()
        }
        .cloned()
    }
    pub fn applied(&mut self, redo: bool) {
        let (source, target) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some(edit) = source.pop() {
            target.push(edit);
        }
    }
}
