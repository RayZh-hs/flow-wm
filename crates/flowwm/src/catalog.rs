use crate::{
    engine::{Presentation, Snapshot},
    extensions::Macro,
    model::{ColumnMode, NodeKind, Row, RowView, Status},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogNode {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub kind: NodeKind,
    pub status: Status,
    pub summary: String,
    pub window_id: Option<u64>,
    pub column: Option<String>,
    pub column_mode: ColumnMode,
    pub alert: bool,
    pub app_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogWorkspace {
    pub id: String,
    pub name: String,
    pub nodes: usize,
    pub attention: usize,
    pub row: Option<Row>,
    #[serde(default)]
    pub view: RowView,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogRecipe {
    pub id: String,
    pub name: String,
    pub nodes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub lookup: Option<Lookup>,
    pub version: u32,
    pub selected_workspace: String,
    pub presentation: Presentation,
    pub nodes: Vec<CatalogNode>,
    pub workspaces: Vec<CatalogWorkspace>,
    pub recipes: Vec<CatalogRecipe>,
    pub macros: Vec<Macro>,
    pub running: usize,
    pub attention: usize,
    pub open_windows: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LookupTarget {
    pub id: String,
    pub input: String,
    pub create: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LookupOutput {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub ty: String,
    pub targets: Vec<LookupTarget>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lookup {
    pub node: String,
    pub title: String,
    pub outputs: Vec<LookupOutput>,
    pub selected: Option<String>,
    pub input: bool,
}
impl Lookup {
    fn new(
        state: &crate::model::State,
        id: &str,
        macros: &[Macro],
        selected: Option<&str>,
        reverse: bool,
    ) -> Option<Self> {
        let source = state.node(id)?;
        let outputs = crate::graph::pins(state, source, !reverse)
            .into_iter()
            .filter(|pin| selected.is_none_or(|name| pin.name == name))
            .map(|output| {
                let mut targets = vec![];
                let accepts = |input: &crate::graph::Pin| {
                    output.ty.is_signal() == input.ty.is_signal()
                        && if reverse {
                            crate::graph::can_cast(state, &input.ty, &output.ty)
                        } else {
                            crate::graph::can_cast(state, &output.ty, &input.ty)
                        }
                };
                for node in state
                    .nodes
                    .iter()
                    .filter(|n| n.workspace == source.workspace && n.id != source.id)
                {
                    for pin in crate::graph::pins(state, node, reverse)
                        .into_iter()
                        .filter(&accepts)
                    {
                        if !reverse
                            && !pin.ty.is_signal()
                            && state
                                .connections
                                .iter()
                                .any(|e| e.target.node == node.id && e.target.pin == pin.name)
                        {
                            continue;
                        }
                        targets.push(LookupTarget {
                            id: node.id.clone(),
                            input: pin.name,
                            create: false,
                        });
                    }
                }
                for m in macros {
                    let mut node = crate::model::Node::new(
                        &source.workspace,
                        m.kind,
                        source.cwd.clone(),
                        Default::default(),
                    );
                    node.flow = m.flow.clone();
                    for pin in crate::graph::pins(state, &node, reverse)
                        .into_iter()
                        .filter(&accepts)
                    {
                        targets.push(LookupTarget {
                            id: m.id.clone(),
                            input: pin.name,
                            create: true,
                        });
                    }
                }
                LookupOutput {
                    label: crate::graph::pin_label(source, &output, !reverse),
                    description: crate::graph::pin_description(&output, !reverse).into(),
                    name: output.name,
                    ty: output.ty.to_string(),
                    targets,
                }
            })
            .collect();
        Some(Self {
            node: source.id.clone(),
            title: source.title.clone(),
            outputs,
            selected: selected.map(str::to_string),
            input: reverse,
        })
    }
}
impl Catalog {
    pub fn new(snapshot: &Snapshot, presentation: Presentation, macros: &[Macro]) -> Self {
        let state = &snapshot.state;
        Self {
            lookup: presentation.lookup_node.as_deref().and_then(|id| {
                Lookup::new(
                    state,
                    id,
                    macros,
                    presentation.lookup_pin.as_deref(),
                    presentation.lookup_input,
                )
            }),
            version: 1,
            selected_workspace: state.selected_workspace.clone(),
            presentation,
            nodes: state
                .nodes
                .iter()
                .map(|n| CatalogNode {
                    id: n.id.clone(),
                    title: n.title.clone(),
                    workspace: n.workspace.clone(),
                    kind: n.kind,
                    status: n.status,
                    window_id: n.window_id,
                    column: n.column.clone(),
                    column_mode: n.column_mode,
                    alert: n.flow.properties.contains_key("_alert"),
                    app_id: n
                        .window_match
                        .as_ref()
                        .map(|s| s.split("::").next().unwrap_or_default().to_string()),
                    summary: n
                        .command
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect(),
                })
                .collect(),
            workspaces: state
                .workspaces
                .iter()
                .map(|w| CatalogWorkspace {
                    id: w.id.clone(),
                    name: w.name.clone(),
                    nodes: state.nodes.iter().filter(|n| n.workspace == w.id).count(),
                    attention: state
                        .nodes
                        .iter()
                        .filter(|n| n.workspace == w.id && n.flow.properties.contains_key("_alert"))
                        .count(),
                    row: w.row.clone(),
                    view: w.view,
                })
                .collect(),
            recipes: state
                .recipes
                .iter()
                .map(|r| CatalogRecipe {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    nodes: r.nodes.len(),
                })
                .collect(),
            macros: macros.to_vec(),
            running: state
                .nodes
                .iter()
                .filter(|n| n.status == Status::Running && n.kind != NodeKind::Window)
                .count(),
            open_windows: state
                .nodes
                .iter()
                .filter(|n| n.workspace == state.selected_workspace && n.window_id.is_some())
                .count(),
            attention: state
                .nodes
                .iter()
                .filter(|n| {
                    matches!(n.status, Status::Failed | Status::Review)
                        || n.flow.properties.contains_key("_alert")
                })
                .count(),
        }
    }
}
