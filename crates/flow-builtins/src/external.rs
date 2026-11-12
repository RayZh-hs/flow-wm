//! Opt-in subprocess extensions. Loading a manifest never starts its programs.
use crate::{
    local::{Environment, Limits},
    process::{Process, Supervisor},
};
use anyhow::{Context, Result, ensure};
use flow_core::{graph::Record, providers::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command, sync::Arc, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    #[serde(flatten)]
    pub definition: Definition,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub settings: Option<Vec<String>>,
}
#[derive(Deserialize)]
struct Manifest {
    version: u32,
    #[serde(default)]
    providers: Vec<Node>,
}
pub fn load(directory: &Path) -> Result<Vec<Node>> {
    if !directory.exists() {
        return Ok(vec![]);
    }
    let mut paths = std::fs::read_dir(directory)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut nodes = vec![];
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        ensure!(
            std::fs::metadata(&path)?.len() <= 1_048_576,
            "Extension manifest is too large"
        );
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("Read extension {}", path.display()))?;
        ensure!(manifest.version == 3, "Unsupported extension version");
        for mut node in manifest.providers {
            ensure!(
                node.definition.behavior == Behavior::Invoked,
                "Subprocess extensions must be invoked nodes"
            );
            ensure!(
                !node.command.is_empty() && !node.command[0].is_empty(),
                "Extension needs a command"
            );
            for command in std::iter::once(&mut node.command).chain(node.settings.iter_mut()) {
                ensure!(!command.is_empty(), "Settings needs a command");
                for argument in command {
                    if argument.starts_with("./") || argument.starts_with("../") {
                        *argument = path
                            .parent()
                            .unwrap()
                            .join(&argument)
                            .canonicalize()?
                            .to_string_lossy()
                            .into();
                    }
                }
            }
            nodes.push(node);
        }
    }
    Ok(nodes)
}
struct External {
    nodes: BTreeMap<String, Node>,
    processes: Supervisor,
    limits: Limits,
    environment: Environment,
}
impl ExecutionProvider for External {
    fn invoke(&self, invocation: Invocation, events: EventSink) -> Result<String> {
        self.invoke_observed(invocation, events, None)
    }
    fn invoke_observed(
        &self,
        invocation: Invocation,
        events: EventSink,
        output: Option<OutputSink>,
    ) -> Result<String> {
        let node = self
            .nodes
            .get(&invocation.type_id)
            .context("Unknown extension node")?;
        let mut command = Command::new(&node.command[0]);
        command.args(&node.command[1..]);
        if let Some(directory) = &self.environment.config_directory {
            command.env("FLOWWM_CONFIG_DIR", directory);
        }
        if let Some(socket) = &self.environment.control_socket {
            command.env("FLOWWM_HOST_SOCKET", socket);
        }
        let mut payload = serde_json::to_value(&invocation)?;
        payload["operation"] = json!("invoke");
        let process = Process {
            command,
            input: Some(payload.to_string()),
            stdout_stream: "extension",
            timeout: Duration::from_secs(self.limits.execution_timeout_secs),
            directory: crate::local::directory(&invocation),
            writer: false,
            decode: |text| {
                let value: Value = serde_json::from_str(text)
                    .context("Extension must return a JSON output record")?;
                ensure!(value.is_object(), "Extension output must be a record");
                ensure!(
                    value.get("error").is_none(),
                    "{}",
                    value["error"].as_str().unwrap_or("Extension failed")
                );
                Ok(value)
            },
        };
        self.processes
            .start(process, invocation, &self.limits, events, output)
    }
    fn interrupt(&self, handle: &str) -> Result<()> {
        self.processes.interrupt(handle)
    }
}
pub fn install(registry: &mut Registry, limits: Limits, environment: Environment) -> Result<()> {
    let Some(directory) = &environment.config_directory else {
        return Ok(());
    };
    let mut owners = BTreeMap::<_, Vec<Node>>::new();
    for node in load(&directory.join("extensions"))? {
        owners
            .entry(node.definition.owner.clone())
            .or_default()
            .push(node);
    }
    for (owner, nodes) in owners {
        let definitions = nodes.iter().map(|n| n.definition.clone()).collect();
        registry.mount(
            owner,
            definitions,
            Arc::new(External {
                nodes: nodes
                    .into_iter()
                    .map(|n| (n.definition.id.clone(), n))
                    .collect(),
                processes: Supervisor::default(),
                limits: limits.clone(),
                environment: environment.clone(),
            }),
        )?;
    }
    Ok(())
}

/// Settings protocol: one JSON request on stdin, one bounded JSON form on stdout.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Form {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub actions: Vec<FormAction>,
    #[serde(default)]
    pub properties: Record,
    #[serde(default)]
    pub ready: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub transient: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Choice {
    pub label: String,
    pub value: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FormAction {
    pub id: String,
    pub label: String,
}
