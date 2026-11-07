use crate::model::NodeKind;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

/// Declarative extension macros create ordinary nodes. They cannot run code on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Macro {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: NodeKind,
    pub command: String,
}
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub macros: Vec<Macro>,
}
pub fn builtins() -> Vec<Macro> {
    vec![
        Macro {
            id: "agent".into(),
            name: "Agent session".into(),
            description: "A Codex session in this workspace".into(),
            kind: NodeKind::Agent,
            command: "Inspect this project and suggest the next useful steps.".into(),
        },
        Macro {
            id: "shell".into(),
            name: "Shell command".into(),
            description: "A shell script with structured workflow inputs".into(),
            kind: NodeKind::Shell,
            command: "pwd && git status --short".into(),
        },
        Macro {
            id: "metric".into(),
            name: "Metric".into(),
            description: "Sample a command and watch its value".into(),
            kind: NodeKind::Metric,
            command: "printf '0\\n'".into(),
        },
        Macro {
            id: "gate".into(),
            name: "Human decision".into(),
            description: "Pause the pipeline for a decision".into(),
            kind: NodeKind::Gate,
            command: "Review the upstream results before continuing.".into(),
        },
        Macro {
            id: "human".into(),
            name: "My work".into(),
            description: "Your work, in parallel with the agents".into(),
            kind: NodeKind::Human,
            command: "What are you working on?".into(),
        },
        Macro {
            id: "window".into(),
            name: "Application window".into(),
            description: "Represent a Niri window in the graph".into(),
            kind: NodeKind::Window,
            command: String::new(),
        },
    ]
}
pub fn load(directory: &Path) -> Result<Vec<Macro>> {
    let mut macros = builtins();
    let mut ids: HashSet<_> = macros.iter().map(|m| m.id.clone()).collect();
    if !directory.exists() {
        return Ok(macros);
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
        ensure!(manifest.version == 1, "Unsupported extension version");
        for m in manifest.macros {
            ensure!(ids.insert(m.id.clone()), "Duplicate macro ID: {}", m.id);
            ensure!(
                !m.name.trim().is_empty() && m.name.len() < 200,
                "Invalid macro name"
            );
            macros.push(m);
        }
    }
    Ok(macros)
}
