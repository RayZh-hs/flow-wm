//! Workspace canvas objects. They are presentation, never executable graph nodes.
use crate::model::{Position, State, id};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub title: String,
    pub position: Position,
    pub width: f64,
    pub height: f64,
    #[serde(flatten)]
    pub content: Content,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Text { text: String },
    Group { nodes: Vec<String> },
}
impl Annotation {
    pub fn text(position: Position) -> Self {
        Self {
            id: id(),
            title: "Text".into(),
            position,
            width: 320.,
            height: 180.,
            content: Content::Text {
                text: String::new(),
            },
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty() && self.id.len() <= 128,
            "Invalid canvas item identity"
        );
        ensure!(
            self.position.x.is_finite()
                && self.position.y.is_finite()
                && self.width.is_finite()
                && self.height.is_finite()
                && (160. ..=20000.).contains(&self.width)
                && (100. ..=20000.).contains(&self.height),
            "Invalid canvas item bounds"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Edit {
    Group { nodes: Vec<String> },
    Put { item: Annotation },
    Remove { id: String },
}
pub fn apply(state: &mut State, edit: Edit) -> Result<String> {
    let workspace = state.selected_workspace.clone();
    let (item, remove) = match edit {
        Edit::Group { mut nodes } => {
            nodes.sort();
            nodes.dedup();
            ensure!(!nodes.is_empty(), "Select nodes to group");
            ensure!(
                nodes.iter().all(|id| state
                    .node(id)
                    .is_some_and(|node| node.workspace == workspace)),
                "Group members must belong to this workspace"
            );
            (
                Some(Annotation {
                    id: id(),
                    title: "Group".into(),
                    position: Position::default(),
                    width: 320.,
                    height: 180.,
                    content: Content::Group { nodes },
                }),
                None,
            )
        }
        Edit::Put { item } => {
            item.validate()?;
            if let Content::Group { nodes } = &item.content {
                ensure!(
                    nodes.iter().all(|id| state
                        .node(id)
                        .is_some_and(|node| node.workspace == workspace)),
                    "Group members must belong to this workspace"
                );
            }
            (Some(item), None)
        }
        Edit::Remove { id } => (None, Some(id)),
    };
    let row = state
        .workspaces
        .iter_mut()
        .find(|row| row.id == workspace)
        .unwrap();
    if let Some(id) = remove {
        row.canvas.retain(|item| item.id != id);
        return Ok(id);
    }
    let item = item.unwrap();
    let id = item.id.clone();
    row.canvas.retain(|other| other.id != id);
    row.canvas.push(item);
    Ok(id)
}
