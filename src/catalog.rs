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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogWorkspace {
    pub id: String,
    pub name: String,
    pub nodes: usize,
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
impl Catalog {
    pub fn new(snapshot: &Snapshot, presentation: Presentation, macros: &[Macro]) -> Self {
        let state = &snapshot.state;
        Self {
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
                .filter(|n| matches!(n.status, Status::Failed | Status::Review))
                .count(),
        }
    }
}
