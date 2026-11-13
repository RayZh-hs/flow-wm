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
pub mod harness;
pub mod terminal;

pub const TYPE_ID: &str = "agent_sessions.session";
pub const TERMINAL_TYPE: &str = "agent_sessions.terminal";
pub fn is_session(type_id: &str) -> bool {
    matches!(type_id, TYPE_ID | TERMINAL_TYPE | "local.agent")
}

pub fn is_interactive(type_id: &str) -> bool {
    type_id == TERMINAL_TYPE
}

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
    let config = harness::Harnesses::load(environment.config_directory.as_deref())?;
    let harness = config.select(text(properties, "harness"))?;
    let session = invocation
        .args
        .get("agent_session")
        .and_then(Value::as_str)
        .or_else(|| text(properties, "agent_session"));
    let schema_path = environment
        .data_directory
        .as_ref()
        .map(|p| p.join("agent-result.schema.json"));
    if let Some(schema) = &schema_path {
        use std::io::Write;
        // Publish the schema atomically; concurrent sessions must never read a
        // partially written schema. Every invocation uses the same contract.
        let temporary = schema.with_extension(format!("{}.tmp", invocation.generation));
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(SCHEMA.as_bytes())?;
        std::fs::rename(temporary, schema)?;
    }
    let mut command = harness.process(
        session,
        writable,
        if conversational {
            None
        } else {
            schema_path.as_deref()
        },
    )?;
    if let Some(socket) = text(properties, "wm_socket") {
        command.env("FLOWWM_SOCKET", socket);
    }
    let mut input = if let Some(context) = context {
        format!("{context}\n\nUser request:\n{prompt}")
    } else if let Some(inputs) = properties
        .get("data_inputs")
        .and_then(Value::as_object)
        .map(|v| {
            v.iter()
                .filter(|(k, _)| k.as_str() != "env")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .filter(|v| !v.is_empty())
    {
        format!(
            "{prompt}\n\nWorkflow inputs:\n{}",
            serde_json::to_string_pretty(&inputs)?
        )
    } else {
        prompt.into()
    };
    if !conversational {
        input.push_str("\n\nFlowWM workflow contract: return the required JSON object. Set outcome to success only when this step's goal is achieved. Set outcome to issue if blocked, uncertain, or if an issue requires attention; this pauses success branches. Put a complete downstream handoff in summary, and file paths in artifacts. Do not conceal issues to keep the pipeline moving.");
    }
    if let Some(config_directory) = &environment.config_directory {
        command.env("FLOWWM_CONFIG_DIR", config_directory);
    }
    let mut runner = Command::new("python3");
    runner.args(["-c", terminal::RUNNER]);
    // Keep harness-specific environment overrides when the shared supervisor
    // applies the row environment to the runner.
    let overrides: serde_json::Map<String, Value> = command
        .get_envs()
        .filter_map(|(key, value)| Some((key.to_str()?.into(), json!(value?.to_str()?))))
        .collect();
    let argv = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let input = json!({"argv":argv,"input":input,"node":invocation.node.0,"adapter":harness.adapter,"environment":overrides});
    Ok(Process {
        command: runner,
        input: Some(input.to_string()),
        stdout_stream: "agent",
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
                Some("system") if event["subtype"] == "init" => {
                    session = event["session_id"].as_str().map(String::from)
                }
                Some("assistant") => {
                    if let Some(parts) = event["message"]["content"].as_array() {
                        message = parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                }
                Some("result") => {
                    ensure!(event["is_error"] != true, "Agent failed: {event}");
                    if let Some(result) = event.get("structured_output") {
                        message = result.to_string();
                    } else if let Some(result) = event["result"].as_str() {
                        message = result.into();
                    }
                    session = event["session_id"].as_str().map(String::from).or(session);
                }
                Some("session") => session = event["id"].as_str().map(String::from),
                Some("message") => message = event["text"].as_str().unwrap_or_default().into(),
                _ if event.get("stdout").is_some() => {
                    message = event["stdout"].as_str().unwrap_or_default().into();
                    session = event["agent_session"]
                        .as_str()
                        .map(String::from)
                        .or(session);
                }
                _ if event.get("outcome").is_some() => message = event.to_string(),
                _ => {}
            }
        }
    }
    ensure!(!message.is_empty(), "Agent returned no final message");
    Ok(json!({"stdout":message,"agent_session":session}))
}

/// A builtin extension with its own provider and namespace. `local.agent` remains
/// available as a compatibility alias for saved workflows.
pub fn install(
    registry: &mut flow_core::providers::Registry,
    limits: Limits,
    environment: Environment,
) -> Result<()> {
    use flow_core::{
        graph::{Pin, Property, Type},
        providers::*,
    };
    use std::{collections::BTreeMap, sync::Arc};
    struct Sessions {
        limits: Limits,
        environment: Environment,
        processes: crate::process::Supervisor,
    }
    impl ExecutionProvider for Sessions {
        fn resources(&self, invocation: &Invocation) -> Result<Vec<ResourceClaim>> {
            if is_interactive(&invocation.type_id) {
                return Ok(vec![ResourceClaim {
                    key: format!("application:{}:{}", invocation.workflow, invocation.node),
                    exclusive: true,
                }]);
            }
            Ok(vec![
                ResourceClaim {
                    key: directory(invocation)
                        .canonicalize()?
                        .to_string_lossy()
                        .into(),
                    exclusive: invocation
                        .properties
                        .get("writable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                ResourceClaim {
                    key: format!(
                        "agent-session:{}:{}",
                        invocation.workflow.0, invocation.node.0
                    ),
                    exclusive: true,
                },
            ])
        }
        fn invoke(&self, invocation: Invocation, events: EventSink) -> Result<String> {
            self.invoke_observed(invocation, events, None)
        }
        fn invoke_observed(
            &self,
            mut invocation: Invocation,
            events: EventSink,
            output: Option<OutputSink>,
        ) -> Result<String> {
            if is_interactive(&invocation.type_id) {
                return crate::applications::invoke(invocation, &self.environment, events);
            }
            let settings = harness::Harnesses::load(self.environment.config_directory.as_deref())?;
            let harness = settings
                .select(text(&invocation.properties, "harness"))?
                .id
                .clone();
            let session_path = self.environment.data_directory.as_ref().map(|directory| {
                use std::hash::{Hash, Hasher};
                let mut hash = std::hash::DefaultHasher::new();
                (&invocation.workflow.0, &invocation.node.0).hash(&mut hash);
                directory
                    .join("agent-sessions")
                    .join(format!("{:016x}.json", hash.finish()))
            });
            if invocation
                .args
                .get("agent_session")
                .and_then(Value::as_str)
                .is_none()
                && invocation
                    .properties
                    .get("agent_session")
                    .and_then(Value::as_str)
                    .is_none()
                && let Some(path) = &session_path
                && let Ok(bytes) = std::fs::read(path)
                && let Ok(saved) = serde_json::from_slice::<Value>(&bytes)
                && saved["harness"] == harness
                && let Some(session) = saved["agent_session"].as_str()
            {
                invocation
                    .properties
                    .insert("agent_session".into(), json!(session));
            }
            let mut process = process(&invocation, &self.environment, &self.limits)?;
            if let Some(socket) = &self.environment.control_socket {
                process.command.env("FLOWWM_HOST_SOCKET", socket);
            }
            if let Some(cli) = &self.environment.cli {
                process.command.env("FLOWWM_CLI", cli);
            }
            let observed: EventSink = Arc::new(move |mut completion| {
                completion.result["harness"] = json!(harness);
                if completion.result["agent_session"].is_string()
                    && let Some(path) = &session_path
                {
                    let save = || -> Result<()> {
                        use std::{
                            io::Write,
                            os::unix::fs::{DirBuilderExt, OpenOptionsExt},
                        };
                        std::fs::DirBuilder::new()
                            .recursive(true)
                            .mode(0o700)
                            .create(path.parent().unwrap())?;
                        let temporary = path.with_extension("tmp");
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .truncate(true)
                            .write(true)
                            .mode(0o600)
                            .open(&temporary)?;
                        serde_json::to_writer(
                            &mut file,
                            &json!({"harness":harness,"agent_session":completion.result["agent_session"]}),
                        )?;
                        file.flush()?;
                        std::fs::rename(temporary, path)?;
                        Ok(())
                    };
                    if let Err(error) = save() {
                        completion.result["session_warning"] = json!(error.to_string());
                    }
                }
                events(completion);
            });
            self.processes
                .start(process, invocation, &self.limits, observed, output)
        }
        fn interrupt(&self, handle: &str) -> Result<()> {
            self.processes.interrupt(handle)
        }
    }
    let event = Type::Signal {
        args: BTreeMap::new(),
        open: true,
    };
    registry.mount(
        "agent_sessions".into(),
        vec![
            Definition {
                id: TERMINAL_TYPE.into(),
                owner: "agent_sessions".into(),
                behavior: Behavior::Application,
                capabilities: ["application".into()].into(),
                inputs: vec![
                    Pin::new("in", event.clone()),
                    Pin {
                        default: Some(json!({})),
                        ..Pin::new("env", Type::Env)
                    },
                ],
                outputs: vec![
                    Pin::new(
                        "on_session_finish",
                        Type::Signal {
                            args: BTreeMap::from([(
                                "ERROR".into(),
                                Type::Record {
                                    fields: BTreeMap::from([
                                        ("code".into(), Type::Int),
                                        ("kind".into(), Type::String),
                                        ("message".into(), Type::String),
                                    ]),
                                    open: false,
                                },
                            )]),
                            open: true,
                        },
                    ),
                    Pin::new(
                        "on_exit",
                        Type::Signal {
                            args: BTreeMap::from([("exit_code".into(), Type::UInt8)]),
                            open: false,
                        },
                    ),
                ],
                properties: BTreeMap::from([
                    (
                        "harness".into(),
                        Property {
                            ty: Type::String,
                            default: None,
                            output: false,
                        },
                    ),
                    (
                        "writable".into(),
                        Property {
                            ty: Type::Bool,
                            default: Some(json!(false)),
                            output: false,
                        },
                    ),
                ]),
                window: None,
                renderer: None,
            },
            Definition {
                id: TYPE_ID.into(),
                owner: "agent_sessions".into(),
                behavior: Behavior::Invoked,
                capabilities: ["shell".into()].into(),
                inputs: vec![
                    Pin::new("in", event.clone()),
                    Pin {
                        default: Some(json!({})),
                        ..Pin::new("env", Type::Env)
                    },
                ],
                outputs: vec![
                    Pin::new("out", event.clone()),
                    Pin::new("failed", event),
                    Pin::new("output", Type::String),
                ],
                properties: BTreeMap::from([
                    (
                        "harness".into(),
                        Property {
                            ty: Type::String,
                            default: None,
                            output: false,
                        },
                    ),
                    (
                        "writable".into(),
                        Property {
                            ty: Type::Bool,
                            default: Some(json!(false)),
                            output: false,
                        },
                    ),
                ]),
                window: None,
                renderer: None,
            },
        ],
        Arc::new(Sessions {
            limits,
            environment,
            processes: Default::default(),
        }),
    )
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
