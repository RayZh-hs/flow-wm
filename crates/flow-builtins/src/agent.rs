//! Agent CLI adapter. Configuration paths and control endpoints are supplied by the host.
use crate::{
    local::{Environment, Limits, directory, text},
    process::Process,
};
use anyhow::{Context, Result, ensure};
use flow_core::providers::Invocation;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, process::Command, time::Duration};

const SCHEMA: &str = r#"{"type":"object","properties":{"outcome":{"type":"string","enum":["success","issue"]},"summary":{"type":"string"},"artifacts":{"type":"array","items":{"type":"string"}}},"required":["outcome","summary","artifacts"],"additionalProperties":false}"#;
#[derive(Deserialize)]
pub struct Config {
    pub command: Vec<String>,
}
impl Config {
    pub fn from_directory(directory: Option<&Path>) -> Result<Self> {
        let path = directory.map(|d| d.join("agent.json"));
        let config = if let Some(path) = path.filter(|p| p.exists()) {
            serde_json::from_slice(&std::fs::read(path)?).context("Read agent.json")?
        } else {
            Self {
                command: vec!["codex".into()],
            }
        };
        ensure!(
            !config.command.is_empty() && !config.command[0].is_empty(),
            "Agent command is empty"
        );
        Ok(config)
    }
    pub fn process(&self, session: Option<&str>, writable: bool) -> Command {
        let mut command = self.command(session, true, writable);
        command.arg("-");
        command
    }
    fn command(&self, session: Option<&str>, conversational: bool, writable: bool) -> Command {
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]).args(["exec", "--json"]);
        if conversational {
            command.arg("--skip-git-repo-check");
            if let Some(session) = session {
                command.args(["resume", session]);
            }
        } else {
            command.args([
                "--color",
                "never",
                "--sandbox",
                if writable {
                    "workspace-write"
                } else {
                    "read-only"
                },
            ]);
        }
        command
    }
}
#[derive(Debug, Deserialize)]
pub struct AgentResult {
    pub outcome: AgentOutcome,
    pub summary: String,
    pub artifacts: Vec<String>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    Success,
    Issue,
}
pub fn agent_result(output: &str) -> Result<AgentResult> {
    serde_json::from_str(output).context("Agent did not return the required success/issue outcome")
}
pub(crate) fn process(
    invocation: &Invocation,
    environment: &Environment,
    limits: &Limits,
) -> Result<Process> {
    let properties = &invocation.properties;
    let prompt = text(properties, "command").context("Agent requires a prompt")?;
    let context = invocation
        .args
        .get("graph_context")
        .and_then(Value::as_str)
        .or_else(|| text(properties, "graph_context"));
    let conversational = context.is_some()
        || properties.get("conversational").and_then(Value::as_bool) == Some(true);
    let writable = properties
        .get("writable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let config = Config::from_directory(environment.config_directory.as_deref())?;
    let session = invocation
        .args
        .get("agent_session")
        .and_then(Value::as_str)
        .or_else(|| text(properties, "agent_session"));
    let mut command = config.command(session, conversational, writable);
    if let Some(socket) = invocation.args.get("wm_socket").and_then(Value::as_str) {
        command.env("FLOWWM_SOCKET", socket);
    }
    if !conversational {
        let schema_path = environment
            .data_directory
            .as_ref()
            .context("Agent provider requires a data directory")?
            .join("agent-result.schema.json");
        std::fs::write(&schema_path, SCHEMA)?;
        command
            .arg("-C")
            .arg(directory(invocation))
            .arg("--output-schema")
            .arg(schema_path);
    }
    command.arg("-");
    let mut input = if let Some(context) = context {
        format!("{context}\n\nUser request:\n{prompt}")
    } else if let Some(inputs) = properties
        .get("data_inputs")
        .filter(|v| v.as_object().is_some_and(|a| !a.is_empty()))
    {
        format!(
            "{prompt}\n\nWorkflow inputs:\n{}",
            serde_json::to_string_pretty(inputs)?
        )
    } else {
        prompt.into()
    };
    if !conversational {
        input.push_str("\n\nFlowWM workflow contract: return the required JSON object. Set outcome to success only when this step's goal is achieved. Set outcome to issue if blocked, uncertain, or if an issue requires attention; this pauses success branches. Put a complete downstream handoff in summary, and file paths in artifacts. Do not conceal issues to keep the pipeline moving.");
    }
    Ok(Process {
        command,
        input: Some(input),
        stdout_stream: "codex",
        timeout: Duration::from_secs(limits.execution_timeout_secs),
        directory: directory(invocation),
        writer: writable,
        decode: if conversational {
            decode_chat
        } else {
            decode_handoff
        },
    })
}
fn decode_chat(stdout: &str) -> Result<Value> {
    let mut message = String::new();
    let mut session = None;
    for line in stdout.lines() {
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            match event["type"].as_str() {
                Some("thread.started") => session = event["thread_id"].as_str().map(String::from),
                Some("item.completed" | "item.started")
                    if event["item"]["type"] == "agent_message" =>
                {
                    message = event["item"]["text"].as_str().unwrap_or("").into();
                }
                Some("error" | "turn.failed") => anyhow::bail!("Agent failed: {event}"),
                _ => {}
            }
        }
    }
    Ok(json!({"stdout":message,"agent_session":session}))
}
fn decode_handoff(stdout: &str) -> Result<Value> {
    let mut result = decode_chat(stdout)?;
    let output = result["stdout"].as_str().unwrap_or("");
    let handoff = agent_result(output)?;
    result["outcome"] = json!(if handoff.outcome == AgentOutcome::Success {
        "success"
    } else {
        "issue"
    });
    result["summary"] = json!(handoff.summary);
    result["artifacts"] = json!(handoff.artifacts);
    Ok(result)
}
