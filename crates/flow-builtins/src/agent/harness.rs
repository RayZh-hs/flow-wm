//! Harness commands are argument vectors. Prompts and session IDs never enter a shell.
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::Path, process::Command};

#[derive(Clone, Debug, knuffel::Decode)]
pub struct Harness {
    #[knuffel(argument)]
    pub id: String,
    #[knuffel(child, unwrap(argument))]
    pub adapter: String,
    #[knuffel(child, unwrap(arguments))]
    pub command: Vec<String>,
    #[knuffel(child, unwrap(arguments))]
    pub run: Option<Vec<String>>,
    #[knuffel(child, unwrap(arguments))]
    pub resume: Option<Vec<String>>,
    #[knuffel(child, unwrap(arguments))]
    pub interactive: Option<Vec<String>>,
}
#[derive(knuffel::Decode)]
struct Document {
    #[knuffel(child, unwrap(argument))]
    default_harness: Option<String>,
    #[knuffel(child, unwrap(arguments))]
    terminal: Option<Vec<String>>,
    #[knuffel(children(name = "harness"))]
    harnesses: Vec<Harness>,
}
pub struct Harnesses {
    pub default_harness: String,
    pub terminal: Vec<String>,
    pub harnesses: BTreeMap<String, Harness>,
}
impl Harnesses {
    pub fn load(directory: Option<&Path>) -> Result<Self> {
        let mut result = Self {
            default_harness: "codex".into(),
            terminal: vec![
                "alacritty",
                "--class",
                "{app_id}",
                "--title",
                "{title}",
                "--working-directory",
                "{cwd}",
                "-e",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            harnesses: ["codex", "claude"]
                .into_iter()
                .map(|id| {
                    (
                        id.into(),
                        Harness {
                            id: id.into(),
                            adapter: id.into(),
                            command: vec![id.into()],
                            run: None,
                            resume: None,
                            interactive: None,
                        },
                    )
                })
                .collect(),
        };
        // Existing command-only configurations continue to work until migrated.
        if let Some(directory) = directory {
            let legacy = super::Config::from_directory(Some(directory))?;
            result.harnesses.get_mut("codex").unwrap().command = legacy.command;
            let path = directory.join("agent.kdl");
            if path.exists() {
                let document: Document =
                    knuffel::parse(&path.to_string_lossy(), &std::fs::read_to_string(&path)?)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if let Some(default) = document.default_harness {
                    result.default_harness = default;
                }
                if let Some(terminal) = document.terminal {
                    result.terminal = terminal;
                }
                let mut ids = std::collections::BTreeSet::new();
                for harness in document.harnesses {
                    ensure!(
                        ids.insert(harness.id.clone()),
                        "Duplicate agent harness {}",
                        harness.id
                    );
                    ensure!(
                        matches!(
                            harness.adapter.as_str(),
                            "codex" | "claude" | "jsonl" | "text"
                        ),
                        "Unknown harness adapter {}",
                        harness.adapter
                    );
                    ensure!(
                        !harness.command.is_empty() && !harness.command[0].is_empty(),
                        "Harness needs a command"
                    );
                    result.harnesses.insert(harness.id.clone(), harness);
                }
            }
        }
        ensure!(
            result.harnesses.contains_key(&result.default_harness),
            "Default agent harness is not registered"
        );
        ensure!(
            !result.terminal.is_empty() && !result.terminal[0].is_empty(),
            "Agent terminal command is empty"
        );
        Ok(result)
    }
    pub fn select(&self, id: Option<&str>) -> Result<&Harness> {
        self.harnesses
            .get(id.unwrap_or(&self.default_harness))
            .context("Agent harness is not registered in agent.kdl")
    }
}
impl Harness {
    pub fn process(
        &self,
        session: Option<&str>,
        writable: bool,
        schema: Option<&Path>,
    ) -> Result<Command> {
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]);
        match self.adapter.as_str() {
            "codex" => {
                command.args([
                    "-c",
                    if writable {
                        "sandbox_mode=\"workspace-write\""
                    } else {
                        "sandbox_mode=\"read-only\""
                    },
                    "exec",
                    "--json",
                    "--skip-git-repo-check",
                ]);
                if let Some(session) = session {
                    command.args(["resume", session]);
                } else if let Some(schema) = schema {
                    command.arg("--output-schema").arg(schema);
                }
                command.arg("-");
            }
            "claude" => {
                command.args([
                    "--print",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--permission-mode",
                    if writable { "acceptEdits" } else { "plan" },
                ]);
                if let Some(session) = session {
                    command.args(["--resume", session]);
                }
                if let Some(schema) = schema {
                    command
                        .arg("--json-schema")
                        .arg(std::fs::read_to_string(schema)?);
                }
            }
            _ => {
                let args = if session.is_some() {
                    self.resume.as_ref().or(self.run.as_ref())
                } else {
                    self.run.as_ref()
                };
                if let Some(args) = args {
                    for arg in args {
                        command.arg(
                            arg.replace("{session}", session.unwrap_or_default())
                                .replace("{writable}", if writable { "true" } else { "false" }),
                        );
                    }
                }
            }
        }
        Ok(command)
    }
}
