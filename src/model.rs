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
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
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
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunWhen {
    #[default]
    Success,
    Failure,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
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
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub kind: NodeKind,
    pub command: String,
    pub cwd: PathBuf,
    pub position: Position,
    pub dependencies: Vec<String>,
    pub run_when: RunWhen,
    pub review: bool,
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
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}
impl Node {
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
        };
        Self {
            id: id(),
            workspace: workspace.into(),
            title: title.into(),
            kind,
            command: command.into(),
            cwd,
            position,
            dependencies: vec![],
            run_when: RunWhen::Success,
            review: false,
            writable: false,
            status: Status::Idle,
            output: String::new(),
            logs: vec![],
            metric: Metric::default(),
            window_match: None,
            window_id: None,
            desktop_window: false,
            started_at: None,
            finished_at: None,
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
pub struct Watcher {
    pub id: String,
    pub source: String,
    pub target: String,
    pub threshold: f64,
    pub above: bool,
    pub enabled: bool,
    pub armed: bool,
    pub last_triggered: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub watchers: Vec<Watcher>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    pub nodes: Vec<Node>,
    pub watchers: Vec<Watcher>,
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
        };
        Self {
            version: 1,
            selected_workspace: workspace.id.clone(),
            workspaces: vec![workspace],
            nodes: vec![],
            watchers: vec![],
            recipes: vec![],
        }
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
    pub fn ready(&self, node: &Node) -> bool {
        let parents: Vec<_> = node
            .dependencies
            .iter()
            .filter_map(|id| self.node(id))
            .collect();
        if parents.len() != node.dependencies.len() {
            return false;
        }
        match node.run_when {
            RunWhen::Success => parents.iter().all(|p| p.status == Status::Done),
            RunWhen::Failure => {
                !parents.is_empty()
                    && parents
                        .iter()
                        .all(|p| matches!(p.status, Status::Done | Status::Failed))
                    && parents.iter().any(|p| p.status == Status::Failed)
            }
        }
    }
    pub fn connect(&mut self, source: &str, target: &str) -> Result<()> {
        ensure!(source != target, "A node cannot depend on itself");
        let a = self
            .node(source)
            .ok_or_else(|| anyhow::anyhow!("Source node not found"))?;
        let b = self
            .node(target)
            .ok_or_else(|| anyhow::anyhow!("Target node not found"))?;
        ensure!(
            a.workspace == b.workspace,
            "Move both nodes into the same workspace first"
        );
        ensure!(
            !b.status.active(),
            "Stop the target node before changing its inputs"
        );
        let mut todo = vec![source.to_string()];
        let mut seen = HashSet::new();
        while let Some(next) = todo.pop() {
            ensure!(next != target, "This connection would create a cycle");
            if seen.insert(next.clone())
                && let Some(node) = self.node(&next)
            {
                todo.extend(node.dependencies.clone());
            }
        }
        let b = self.node_mut(target).unwrap();
        if !b.dependencies.iter().any(|id| id == source) {
            b.dependencies.push(source.into());
        }
        Ok(())
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
        fn visit<'a>(
            state: &'a State,
            node: &'a Node,
            active: &mut HashSet<&'a str>,
            done: &mut HashSet<&'a str>,
        ) -> Result<()> {
            if done.contains(node.id.as_str()) {
                return Ok(());
            }
            ensure!(active.insert(&node.id), "Workflow contains a cycle");
            for dependency in &node.dependencies {
                let parent = state
                    .node(dependency)
                    .ok_or_else(|| anyhow::anyhow!("Missing workflow input: {dependency}"))?;
                ensure!(parent.workspace == node.workspace, "Cross-workspace edge");
                visit(state, parent, active, done)?;
            }
            active.remove(node.id.as_str());
            done.insert(&node.id);
            Ok(())
        }
        let mut done = HashSet::new();
        for n in &self.nodes {
            visit(self, n, &mut HashSet::new(), &mut done)?;
        }
        for watcher in &self.watchers {
            let source = self
                .node(&watcher.source)
                .ok_or_else(|| anyhow::anyhow!("Missing watcher source"))?;
            let target = self
                .node(&watcher.target)
                .ok_or_else(|| anyhow::anyhow!("Missing watcher target"))?;
            ensure!(
                source.kind == NodeKind::Metric
                    && source.workspace == target.workspace
                    && source.id != target.id,
                "Invalid watcher endpoints"
            );
            ensure!(watcher.threshold.is_finite(), "Invalid watcher threshold");
        }
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
            n.dependencies.retain(|id| selected.contains(id));
            n.status = Status::Idle;
            n.output.clear();
            n.logs.clear();
            n.metric.samples.clear();
            n.started_at = None;
            n.finished_at = None;
            n.window_match = None;
            n.window_id = None;
            n.desktop_window = false;
            n.cwd = PathBuf::from(".");
        }
        Ok(Recipe {
            id: id(),
            name,
            nodes,
            watchers: self
                .watchers
                .iter()
                .filter(|w| selected.contains(&w.source) && selected.contains(&w.target))
                .cloned()
                .map(|mut w| {
                    w.armed = true;
                    w.last_triggered = None;
                    w
                })
                .collect(),
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
            n.dependencies = template
                .dependencies
                .iter()
                .map(|id| {
                    mapping
                        .get(id)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Recipe has a missing input"))
                })
                .collect::<Result<_>>()?;
            n.run_when = template.run_when;
            n.review = template.review;
            n.writable = template.writable;
            n.metric.interval_secs = template.metric.interval_secs.max(1);
            n.metric.unit = template.metric.unit.clone();
            nodes.push(n);
        }
        let ids = nodes.iter().map(|n| n.id.clone()).collect();
        let mut candidate = self.clone();
        candidate.nodes.extend(nodes);
        for watcher in &recipe.watchers {
            let mut watcher = watcher.clone();
            watcher.id = id();
            watcher.source = mapping
                .get(&watcher.source)
                .context("Recipe watcher source is missing")?
                .clone();
            watcher.target = mapping
                .get(&watcher.target)
                .context("Recipe watcher target is missing")?
                .clone();
            // Automation requires an explicit enable action after importing or instantiating.
            watcher.enabled = false;
            watcher.armed = true;
            watcher.last_triggered = None;
            candidate.watchers.push(watcher);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(ids)
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let node = self
            .node(id)
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;
        if node.status.active() {
            bail!("Stop or resolve the node before deleting it");
        }
        ensure!(
            !self
                .nodes
                .iter()
                .any(|n| n.dependencies.iter().any(|d| d == id) && n.status.active()),
            "An active node depends on this node"
        );
        self.nodes.retain(|n| n.id != id);
        for n in &mut self.nodes {
            n.dependencies.retain(|d| d != id);
        }
        self.watchers.retain(|w| w.source != id && w.target != id);
        Ok(())
    }
}
