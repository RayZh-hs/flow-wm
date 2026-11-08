use crate::model::NodeKind;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

/// Declarative extension macros create ordinary nodes. They cannot run code on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Macro {
    #[serde(default)]
    pub flow: crate::graph::Flow,
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: NodeKind,
    pub command: String,
}
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub macros: Vec<Macro>,
    #[serde(default)]
    pub types: Vec<crate::graph::TypeDefinition>,
    #[serde(default)]
    pub casts: Vec<crate::graph::CastDefinition>,
    #[serde(default)]
    pub nodes: Vec<crate::graph::NodeDefinition>,
}
pub fn builtins() -> Vec<Macro> {
    let mut macros = vec![
        Macro {
            flow: Default::default(),
            id: "agent".into(),
            name: "Agent session".into(),
            description: "A Codex session in this workspace".into(),
            kind: NodeKind::Agent,
            command: "Inspect this project and suggest the next useful steps.".into(),
        },
        Macro {
            flow: Default::default(),
            id: "shell".into(),
            name: "Shell command".into(),
            description: "A shell script with structured workflow inputs".into(),
            kind: NodeKind::Shell,
            command: "pwd && git status --short".into(),
        },
        Macro {
            flow: Default::default(),
            id: "metric".into(),
            name: "Metric".into(),
            description: "Sample a command and watch its value".into(),
            kind: NodeKind::Metric,
            command: "printf '0\\n'".into(),
        },
        Macro {
            flow: Default::default(),
            id: "gate".into(),
            name: "Human decision".into(),
            description: "Pause the pipeline for a decision".into(),
            kind: NodeKind::Gate,
            command: "Review the upstream results before continuing.".into(),
        },
        Macro {
            flow: Default::default(),
            id: "human".into(),
            name: "My work".into(),
            description: "Your work, in parallel with the agents".into(),
            kind: NodeKind::Human,
            command: "What are you working on?".into(),
        },
        Macro {
            flow: Default::default(),
            id: "window".into(),
            name: "Application window".into(),
            description: "Represent a Niri window in the graph".into(),
            kind: NodeKind::Window,
            command: String::new(),
        },
    ];
    for (id, kind, description) in [
        (
            "constant",
            NodeKind::Constant,
            "A typed constant data value",
        ),
        (
            "display",
            NodeKind::Display,
            "Display and pass through a data input",
        ),
        (
            "break_signal",
            NodeKind::BreakSignal,
            "Split an event into a trigger and argument data",
        ),
        (
            "merge_signal",
            NodeKind::MergeSignal,
            "Overwrite event arguments with a data record",
        ),
        (
            "conditional",
            NodeKind::Conditional,
            "Route a signal using a Boolean data input",
        ),
        (
            "vox",
            NodeKind::Vox,
            "Evaluate Vox with typed input and output bindings",
        ),
        (
            "python",
            NodeKind::Python,
            "Evaluate Python with typed input and output bindings",
        ),
    ] {
        let node = crate::model::Node::new("", kind, ".".into(), Default::default());
        macros.push(Macro {
            id: id.into(),
            kind,
            name: kind.label().into(),
            description: description.into(),
            command: node.command,
            flow: node.flow,
        });
    }
    macros
}
#[derive(Default)]
pub struct Extensions {
    pub macros: Vec<Macro>,
    pub types: Vec<crate::graph::TypeDefinition>,
    pub casts: Vec<crate::graph::CastDefinition>,
    pub nodes: Vec<crate::graph::NodeDefinition>,
}
impl Extensions {
    pub fn apply(&self, state: &mut crate::model::State) -> Result<()> {
        let mut candidate = state.clone();
        for ty in &self.types {
            candidate.types.retain(|d| d.name != ty.name);
            candidate.types.push(ty.clone());
        }
        for node in &self.nodes {
            candidate.node_types.retain(|d| d.id != node.id);
            candidate.node_types.push(node.clone());
        }
        for cast in &self.casts {
            candidate
                .casts
                .retain(|c| c.from != cast.from || c.to != cast.to);
            candidate.casts.push(cast.clone());
        }
        candidate.validate()?;
        *state = candidate;
        Ok(())
    }
}
pub fn load(directory: &Path) -> Result<Vec<Macro>> {
    Ok(load_bundle(directory)?.macros)
}
pub fn load_bundle(directory: &Path) -> Result<Extensions> {
    let mut bundle = Extensions {
        macros: builtins(),
        ..Default::default()
    };
    let macros = &mut bundle.macros;
    let mut ids: HashSet<_> = macros.iter().map(|m| m.id.clone()).collect();
    if !directory.exists() {
        return Ok(bundle);
    }
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        if entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        ensure!(
            entry.metadata()?.len() <= 1024 * 1024,
            "Extension manifest is too large"
        );
        let manifest: Manifest = serde_json::from_slice(&fs::read(entry.path())?)
            .with_context(|| format!("Invalid extension {}", entry.path().display()))?;
        ensure!(
            matches!(manifest.version, 1 | 2),
            "Unsupported extension version"
        );
        for m in manifest.macros {
            ensure!(ids.insert(m.id.clone()), "Duplicate macro ID: {}", m.id);
            ensure!(
                !m.name.trim().is_empty() && m.name.len() < 200,
                "Invalid macro name"
            );
            macros.push(m);
        }
        for node in &manifest.nodes {
            ensure!(
                ids.insert(node.id.clone()),
                "Duplicate node or macro ID: {}",
                node.id
            );
            macros.push(Macro {
                id: node.id.clone(),
                name: node.name.clone(),
                description: "Extension node".into(),
                kind: NodeKind::Extension,
                command: node.source.clone(),
                flow: crate::graph::Flow {
                    node_type: node.id.clone(),
                    ..Default::default()
                },
            });
        }
        bundle.types.extend(manifest.types);
        bundle.casts.extend(manifest.casts);
        bundle.nodes.extend(manifest.nodes);
    }
    let mut validation = crate::model::State::empty(".".into());
    validation.types = bundle.types.clone();
    validation.casts = bundle.casts.clone();
    validation.node_types = bundle.nodes.clone();
    validation.validate()?;
    Ok(bundle)
}
