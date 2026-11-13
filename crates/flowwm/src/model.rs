use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    #[default]
    Agent,
    Shell,
    Metric,
    Gate,
    Human,
    Window,
    Constant,
    Display,
    Alert,
    BreakSignal,
    MergeSignal,
    Conditional,
    Vox,
    Python,
    Extension,
}
impl NodeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Shell => "Shell",
            Self::Metric => "Metric",
            Self::Gate => "Decision",
            Self::Human => "You",
            Self::Window => "Window",
            Self::Constant => "Constant",
            Self::Display => "Display",
            Self::Alert => "Alert",
            Self::BreakSignal => "Break signal",
            Self::MergeSignal => "Merge signal",
            Self::Conditional => "Conditional",
            Self::Vox => "Vox",
            Self::Python => "Python",
            Self::Extension => "Extension",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Agent => "✳",
            Self::Shell => ">_",
            Self::Metric => "⌁",
            Self::Gate => "◇",
            Self::Human => "◉",
            Self::Window => "▤",
            Self::Constant => "=",
            Self::Display => "▣",
            Self::Alert => "!",
            Self::BreakSignal => "↙",
            Self::MergeSignal => "↗",
            Self::Conditional => "?",
            Self::Vox => "V",
            Self::Python => "Py",
            Self::Extension => "+",
        }
    }
    pub fn primitive(self) -> bool {
        matches!(
            self,
            Self::Constant
                | Self::Display
                | Self::Alert
                | Self::BreakSignal
                | Self::MergeSignal
                | Self::Conditional
                | Self::Vox
                | Self::Python
                | Self::Extension
        )
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
    Watching,
    Unavailable,
    Queued,
    Running,
    Review,
    Done,
    Failed,
    Stopped,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Watching => "Watching",
            Self::Unavailable => "Unavailable",
            Self::Queued => "Waiting",
            Self::Running => "Running",
            Self::Review => "Needs you",
            Self::Done => "Complete",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }
    pub fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Review)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnMode {
    #[default]
    Node,
    Window,
}

/// Row presentation is independent of both other rows and temporary UI popups.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowView {
    Nodes,
    #[default]
    Windows,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Log {
    pub time: u64,
    pub kind: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub time: u64,
    pub value: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub interval_secs: u64,
    pub unit: String,
    pub samples: Vec<Sample>,
}
impl Default for Metric {
    fn default() -> Self {
        Self {
            interval_secs: 30,
            unit: String::new(),
            samples: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub flow: crate::graph::Flow,
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub kind: NodeKind,
    pub command: String,
    pub cwd: PathBuf,
    pub position: Position,
    pub writable: bool,
    pub status: Status,
    pub output: String,
    pub logs: Vec<Log>,
    pub metric: Metric,
    /// Fallback selector when a new compositor session invalidates the live window ID.
    pub window_match: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
    #[serde(default)]
    pub desktop_window: bool,
    /// Stable graph column identity. Native windows in one Niri column share it.
    #[serde(default)]
    pub column: Option<String>,
    #[serde(default)]
    pub column_mode: ColumnMode,
    #[serde(default)]
    pub chat: Vec<ChatMessage>,
    #[serde(default)]
    pub agent_session: Option<String>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    #[serde(default)]
    pub attention_generation: u64,
}
impl Node {
    pub fn closed_window(&self) -> bool {
        self.window_id.is_none()
            && (self.desktop_window
                || (self.kind == NodeKind::Window && self.window_match.is_some()))
            && !matches!(
                self.spec().type_id.as_str(),
                flow_builtins::agent::TYPE_ID | "local.agent"
            )
    }

    pub fn new(workspace: &str, kind: NodeKind, cwd: PathBuf, position: Position) -> Self {
        let (title, command) = match kind {
            NodeKind::Agent => (
                "Agent session",
                "Inspect this project and summarize the next useful steps.",
            ),
            NodeKind::Shell => ("Shell command", "pwd && git status --short"),
            NodeKind::Metric => ("Watch a metric", "printf '0\\n'"),
            NodeKind::Gate => (
                "Review & continue",
                "Review the upstream results before continuing.",
            ),
            NodeKind::Human => ("My work", "A place for your work, alongside the agents."),
            NodeKind::Window => ("Application window", ""),
            NodeKind::Constant => ("Constant", ""),
            NodeKind::Display => ("Display", ""),
            NodeKind::Alert => ("Alert", ""),
            NodeKind::BreakSignal => ("Break signal", ""),
            NodeKind::MergeSignal => ("Merge signal", ""),
            NodeKind::Conditional => ("Conditional", ""),
            NodeKind::Vox => ("Vox", "{ value = 42, }"),
            NodeKind::Python => ("Python", "outputs = {'value': 42}"),
            NodeKind::Extension => ("Extension", ""),
        };
        Self {
            flow: crate::graph::Flow::default(),
            id: id(),
            workspace: workspace.into(),
            title: title.into(),
            kind,
            command: command.into(),
            cwd,
            position,
            writable: false,
            status: Status::Idle,
            output: String::new(),
            logs: vec![],
            metric: Metric::default(),
            window_match: None,
            window_id: None,
            desktop_window: false,
            column: None,
            column_mode: ColumnMode::Node,
            chat: vec![],
            agent_session: None,
            started_at: None,
            finished_at: None,
            attention_generation: 0,
        }
    }
    pub fn log(&mut self, kind: &str, text: impl Into<String>) {
        let text: String = text.into().chars().take(24_000).collect();
        self.logs.push(Log {
            time: now(),
            kind: kind.into(),
            text,
        });
        if self.logs.len() > 200 {
            self.logs.drain(..self.logs.len() - 200);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub notes: String,
    #[serde(default)]
    pub row: Option<Row>,
    #[serde(default)]
    pub window_array: bool,
    #[serde(default)]
    pub graph_arranged: bool,
    #[serde(default)]
    pub view: RowView,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Row {
    pub session: String,
    pub id: u64,
    pub index: u8,
    pub output: Option<String>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub connections: Vec<crate::graph::Connection>,
    #[serde(default)]
    pub types: Vec<crate::graph::TypeDefinition>,
    #[serde(default)]
    pub casts: Vec<crate::graph::CastDefinition>,
    #[serde(default)]
    pub node_types: Vec<crate::graph::NodeDefinition>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    #[serde(skip)]
    pub orchestration: Option<std::sync::Arc<flow_core::domain::RuntimeState>>,
    #[serde(default)]
    pub connections: Vec<crate::graph::Connection>,
    #[serde(default)]
    pub types: Vec<crate::graph::TypeDefinition>,
    #[serde(default)]
    pub casts: Vec<crate::graph::CastDefinition>,
    #[serde(default)]
    pub node_types: Vec<crate::graph::NodeDefinition>,
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    pub nodes: Vec<Node>,
    pub recipes: Vec<Recipe>,
    pub selected_workspace: String,
}
impl State {
    pub fn empty(cwd: PathBuf) -> Self {
        let workspace = Workspace {
            id: id(),
            name: "My workspace".into(),
            cwd,
            notes: String::new(),
            row: None,
            window_array: false,
            graph_arranged: false,
            view: RowView::default(),
        };
        Self {
            orchestration: None,
            connections: vec![],
            types: vec![],
            casts: vec![],
            node_types: vec![],
            version: 1,
            selected_workspace: workspace.id.clone(),
            workspaces: vec![workspace],
            nodes: vec![],
            recipes: vec![],
        }
    }
    pub fn behavior(&self, node: &Node) -> Option<flow_core::providers::Behavior> {
        let id = node.spec().type_id;
        self.node_types
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.behavior.clone())
    }
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    pub fn workspace(&self) -> &Workspace {
        self.workspaces
            .iter()
            .find(|w| w.id == self.selected_workspace)
            .unwrap_or(&self.workspaces[0])
    }
    pub fn connect(&mut self, source: &str, target: &str) -> Result<()> {
        crate::graph::connect(
            self,
            crate::graph::Connection {
                source: crate::graph::Endpoint {
                    node: source.into(),
                    pin: "out".into(),
                },
                target: crate::graph::Endpoint {
                    node: target.into(),
                    pin: "in".into(),
                },
            },
        )
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "Unsupported state version: {}",
            self.version
        );
        ensure!(
            !self.workspaces.is_empty(),
            "A state must contain a workspace"
        );
        let mut workspaces = HashSet::new();
        for w in &self.workspaces {
            ensure!(workspaces.insert(&w.id), "Duplicate workspace ID");
        }
        ensure!(
            workspaces.contains(&self.selected_workspace),
            "Selected workspace is missing"
        );
        let mut ids = HashSet::new();
        for n in &self.nodes {
            ensure!(ids.insert(&n.id), "Duplicate node ID");
            ensure!(
                workspaces.contains(&n.workspace),
                "Node references a missing workspace"
            );
            ensure!(
                n.position.x.is_finite() && n.position.y.is_finite(),
                "Invalid node position"
            );
            ensure!(
                n.metric.interval_secs >= 1,
                "Metric interval must be positive"
            );
        }
        crate::graph::validate(self)?;
        Ok(())
    }
    pub fn recipe(&self, name: String, selected: &[String]) -> Result<Recipe> {
        let mut nodes: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .cloned()
            .collect();
        ensure!(!nodes.is_empty(), "Select at least one node");
        for n in &mut nodes {
            n.status = Status::Idle;
            n.output.clear();
            n.logs.clear();
            n.metric.samples.clear();
            n.started_at = None;
            n.finished_at = None;
            n.window_match = None;
            n.window_id = None;
            n.desktop_window = false;
            n.column = None;
            n.column_mode = ColumnMode::Node;
            n.chat.clear();
            n.agent_session = None;
            n.flow.values.clear();
            n.flow.properties.remove("_alert");
        }
        Ok(Recipe {
            connections: self
                .connections
                .iter()
                .filter(|e| selected.contains(&e.source.node) && selected.contains(&e.target.node))
                .cloned()
                .collect(),
            types: self.types.clone(),
            casts: self.casts.clone(),
            node_types: self.node_types.clone(),
            id: id(),
            name,
            nodes,
        })
    }
    pub fn instantiate(
        &mut self,
        recipe: &Recipe,
        workspace: &str,
        cwd: PathBuf,
    ) -> Result<Vec<String>> {
        ensure!(
            self.workspaces.iter().any(|w| w.id == workspace),
            "Workspace not found"
        );
        ensure!(recipe.nodes.len() <= 500, "Recipe is too large");
        let mapping: std::collections::HashMap<_, _> =
            recipe.nodes.iter().map(|n| (n.id.clone(), id())).collect();
        ensure!(
            mapping.len() == recipe.nodes.len(),
            "Duplicate recipe node IDs"
        );
        let mut nodes = vec![];
        for template in &recipe.nodes {
            let mut n = Node::new(
                workspace,
                template.kind,
                cwd.clone(),
                template.position.clone(),
            );
            n.id = mapping[&template.id].clone();
            n.title = template.title.clone();
            n.command = template.command.clone();
            n.flow = template.flow.clone();
            n.cwd = if template.cwd == std::path::Path::new(".") {
                cwd.clone()
            } else {
                template.cwd.clone()
            };
            n.flow.values.clear();
            n.flow.properties.remove("_alert");
            n.writable = template.writable;
            n.metric.interval_secs = template.metric.interval_secs.max(1);
            n.metric.unit = template.metric.unit.clone();
            nodes.push(n);
        }
        let ids = nodes.iter().map(|n| n.id.clone()).collect();
        let mut candidate = self.clone();
        for ty in &recipe.types {
            if let Some(existing) = candidate.types.iter().find(|t| t.name == ty.name) {
                ensure!(existing == ty, "Conflicting recipe type: {}", ty.name);
            } else {
                candidate.types.push(ty.clone());
            }
        }
        for def in &recipe.node_types {
            if let Some(existing) = candidate.node_types.iter().find(|d| d.id == def.id) {
                ensure!(existing == def, "Conflicting recipe node: {}", def.id);
            } else {
                candidate.node_types.push(def.clone());
            }
        }
        for cast in &recipe.casts {
            if !candidate.casts.contains(cast) {
                candidate.casts.push(cast.clone());
            }
        }
        for edge in &recipe.connections {
            let mut edge = edge.clone();
            edge.source.node = mapping
                .get(&edge.source.node)
                .context("Missing recipe source")?
                .clone();
            edge.target.node = mapping
                .get(&edge.target.node)
                .context("Missing recipe target")?
                .clone();
            candidate.connections.push(edge);
        }
        candidate.nodes.extend(nodes);
        candidate.validate()?;
        *self = candidate;
        Ok(ids)
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let node = self
            .node(id)
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;
        if node.status.active() && !node.desktop_window {
            bail!("Stop or resolve the node before deleting it");
        }
        self.nodes.retain(|n| n.id != id);
        self.connections
            .retain(|e| e.source.node != id && e.target.node != id);
        Ok(())
    }
}
