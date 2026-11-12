//! Registered Linux builtin providers. Process supervision is shared across node types.
use crate::process::{Process, Supervisor};
use anyhow::{Context, Result, ensure};
use flow_core::{graph::Record, providers::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub source_interval_secs: u64,
    pub systemd_workspace: bool,
    pub serialize_working_directories: bool,
    pub max_concurrent: usize,
    pub execution_timeout_secs: u64,
    pub metric_timeout_secs: u64,
    pub evaluator_timeout_secs: u64,
    pub interrupt_grace_ms: u64,
    pub terminate_grace_ms: u64,
    pub signal_queue_capacity: usize,
    pub replay_capacity: usize,
    pub remote_retry_secs: u64,
    pub remote_timeout_secs: u64,
    pub remote_batch_size: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            source_interval_secs: 30,
            systemd_workspace: true,
            serialize_working_directories: true,
            max_concurrent: 8,
            execution_timeout_secs: 3600,
            metric_timeout_secs: 20,
            evaluator_timeout_secs: 60,
            interrupt_grace_ms: 1000,
            terminate_grace_ms: 1000,
            signal_queue_capacity: 4096,
            replay_capacity: 65536,
            remote_retry_secs: 2,
            remote_timeout_secs: 10,
            remote_batch_size: 4,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.source_interval_secs > 0
                && self.max_concurrent > 0
                && self.execution_timeout_secs > 0
                && self.metric_timeout_secs > 0
                && self.evaluator_timeout_secs > 0
                && self.signal_queue_capacity > 0
                && self.replay_capacity > 0,
            "Runtime limits must be positive"
        );
        ensure!(
            self.remote_retry_secs > 0
                && self.remote_timeout_secs > 0
                && self.remote_batch_size > 0,
            "Remote runtime limits must be positive"
        );
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct Environment {
    pub row_settings: crate::row_settings::Extension,
    pub config_directory: Option<PathBuf>,
    pub data_directory: Option<PathBuf>,
    pub control_socket: Option<PathBuf>,
    pub cli: Option<PathBuf>,
    pub messaging: Option<Arc<dyn Messaging>>,
}
type Factory = fn(&Invocation, &Environment, &Limits) -> Result<Process>;
pub struct Local {
    limits: Limits,
    environment: Environment,
    processes: Arc<Supervisor>,
    factories: BTreeMap<String, Factory>,
    sources: crate::sources::Sources,
    decisions: Mutex<BTreeMap<String, (Invocation, EventSink)>>,
}
impl Local {
    pub fn new(limits: Limits) -> Self {
        Self::with_environment(limits, Environment::default())
    }
    pub fn with_environment(limits: Limits, environment: Environment) -> Self {
        Self {
            sources: Default::default(),
            decisions: Mutex::new(BTreeMap::new()),
            limits,
            environment,
            processes: Arc::new(Supervisor::default()),
            factories: BTreeMap::from([
                ("local.shell".into(), shell as Factory),
                ("local.metric_sample".into(), metric as Factory),
                ("local.agent".into(), crate::agent::process as Factory),
            ]),
        }
    }
}
impl Default for Local {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}
impl ExecutionProvider for Local {
    fn validate(&self, type_id: &str, properties: &Record) -> Result<()> {
        crate::environment::validate_properties(type_id, properties)?;
        if matches!(
            type_id,
            "local.metric" | "local.timer" | "local.watcher" | "local.receive"
        ) {
            ensure!(
                properties
                    .get("interval_secs")
                    .and_then(Value::as_u64)
                    .is_some_and(|v| v > 0),
                "Source interval must be a positive integer"
            );
        }
        if matches!(type_id, "local.receive" | "local.broadcast") {
            let ty: flow_core::graph::Type = serde_json::from_value(
                properties
                    .get("signal_type")
                    .cloned()
                    .context("Messaging needs a signal type")?,
            )?;
            ensure!(ty.is_signal(), "Messaging requires a signal type");
        }
        Ok(())
    }

    fn subscription(&self, source: &SourceRegistration) -> Result<Option<SignalSubscription>> {
        if source.type_id != "local.receive" {
            return Ok(None);
        }
        let channel = text(&source.properties, "channel")
            .context("Receive needs a channel")?
            .to_owned();
        let ty = serde_json::from_value(
            source
                .properties
                .get("signal_type")
                .cloned()
                .unwrap_or_else(|| json!({"kind":"signal","args":{},"open":true})),
        )?;
        Ok(Some(SignalSubscription { channel, ty }))
    }

    fn resources(&self, invocation: &Invocation) -> Result<Vec<ResourceClaim>> {
        if matches!(
            invocation.type_id.as_str(),
            "local.shell" | "local.agent" | "local.metric_sample"
        ) {
            let directory = directory(invocation)
                .canonicalize()
                .context("Working directory no longer exists")?;
            let exclusive = invocation.type_id == "local.shell"
                || invocation
                    .properties
                    .get("writable")
                    .and_then(Value::as_bool)
                    == Some(true);
            Ok(vec![ResourceClaim {
                key: directory.to_string_lossy().into_owned(),
                exclusive,
            }])
        } else {
            Ok(Vec::new())
        }
    }

    fn subscribe(&self, source: SourceRegistration, events: SourceSink) -> Result<String> {
        self.sources.subscribe(
            source,
            events,
            self.processes.clone(),
            self.limits.clone(),
            self.environment.clone(),
        )
    }
    fn unsubscribe(&self, handle: &str) -> Result<()> {
        self.sources.unsubscribe(handle)
    }

    fn read(&self, invocation: Invocation) -> Result<Value> {
        if matches!(
            invocation.type_id.as_str(),
            "local.set_env" | "local.unset_env" | "local.empty_env" | "local.bake_args_into_env"
        ) {
            return crate::environment::read(&invocation);
        }
        ensure!(
            invocation.type_id == "local.constant",
            "Unsupported lazy computation"
        );
        Ok(json!({"value":invocation.properties.get("value")}))
    }
    fn invoke(&self, invocation: Invocation, events: EventSink) -> Result<String> {
        self.invoke_observed(invocation, events, None)
    }
    fn invoke_observed(
        &self,
        invocation: Invocation,
        events: EventSink,
        output: Option<OutputSink>,
    ) -> Result<String> {
        if matches!(invocation.type_id.as_str(), "local.human" | "local.gate") {
            let handle = invocation.id.0.clone();
            let mut decisions = self.decisions.lock().unwrap();
            ensure!(
                !decisions.contains_key(&handle),
                "Invocation is already awaiting a decision"
            );
            events(Completion {
                invocation: invocation.id.clone(),
                generation: invocation.generation,
                status: flow_core::domain::ExecutionStatus::Waiting,
                result: json!({}),
            });
            decisions.insert(handle.clone(), (invocation, events));
            return Ok(handle);
        }
        if invocation.type_id == "local.broadcast" {
            let messaging = self
                .environment
                .messaging
                .clone()
                .context("Messaging provider unavailable")?;
            let handle = invocation.id.0.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<Value> {
                    let target = text(&invocation.properties, "target")
                        .context("Broadcast needs a target workflow")?;
                    let channel = text(&invocation.properties, "channel")
                        .context("Broadcast needs a channel")?;
                    let ty = serde_json::from_value(
                        invocation
                            .properties
                            .get("signal_type")
                            .cloned()
                            .unwrap_or_else(|| json!({"kind":"signal","args":{},"open":true})),
                    )?;
                    let accepted = messaging.broadcast(flow_core::domain::Delivery {
                        id: invocation.id.0.clone().into(),
                        source: invocation.workflow.clone(),
                        target: target.into(),
                        channel: channel.into(),
                        ty,
                        args: invocation.args.clone(),
                        sequence: invocation.generation,
                    })?;
                    Ok(json!({"accepted":accepted,"out":invocation.args}))
                })();
                events(Completion {
                    invocation: invocation.id,
                    generation: invocation.generation,
                    status: if result.is_ok() {
                        flow_core::domain::ExecutionStatus::Complete
                    } else {
                        flow_core::domain::ExecutionStatus::Failed
                    },
                    result: result.unwrap_or_else(|e| json!({"error":e.to_string()})),
                });
            });
            return Ok(handle);
        }
        if matches!(invocation.type_id.as_str(), "local.alert" | "local.join") {
            let result = if invocation.type_id == "local.alert" {
                json!({"attention":{"message":invocation.properties.get("message"),"critical":invocation.properties.get("critical")},"out":invocation.args})
            } else if invocation.input == "failed" {
                json!({"failed":invocation.args})
            } else {
                json!({"out":invocation.args})
            };
            let handle = invocation.id.0.clone();
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status: if invocation.type_id == "local.join" && invocation.input == "failed" {
                    flow_core::domain::ExecutionStatus::Failed
                } else {
                    flow_core::domain::ExecutionStatus::Complete
                },
                result,
            });
            return Ok(handle);
        }
        if invocation.type_id == "local.constant" {
            let handle = invocation.id.0.clone();
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status: flow_core::domain::ExecutionStatus::Complete,
                result: json!({"value":invocation.properties.get("value")}),
            });
            return Ok(handle);
        }
        let factory = self
            .factories
            .get(&invocation.type_id)
            .context("Unsupported local provider node")?;
        let mut process = factory(&invocation, &self.environment, &self.limits)?;
        if let Some(socket) = &self.environment.control_socket {
            process.command.env("FLOWWM_HOST_SOCKET", socket);
        }
        if let Some(cli) = &self.environment.cli {
            process.command.env("FLOWWM_CLI", cli);
        }
        if let Some(inputs) = invocation.properties.get("data_inputs") {
            let mut inputs = inputs.clone();
            if let Some(inputs) = inputs.as_object_mut() {
                inputs.remove("env");
            }
            process
                .command
                .env("FLOWWM_DATA_INPUTS", inputs.to_string());
        }
        self.processes
            .start(process, invocation, &self.limits, events, output)
    }
    fn interrupt(&self, handle: &str) -> Result<()> {
        if let Some((invocation, events)) = self.decisions.lock().unwrap().remove(handle) {
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status: flow_core::domain::ExecutionStatus::Interrupted,
                result: json!({}),
            });
        }
        self.processes.interrupt(handle)
    }
    fn resolve(&self, handle: &str, accepted: bool, result: Value) -> Result<()> {
        let (invocation, events) = self
            .decisions
            .lock()
            .unwrap()
            .remove(handle)
            .context("Decision is no longer active")?;
        let mut result = result;
        if let Some(values) = result.as_object_mut() {
            values.insert(
                if accepted { "accepted" } else { "rejected" }.into(),
                invocation.args.clone(),
            );
        }
        events(Completion {
            invocation: invocation.id,
            generation: invocation.generation,
            status: if accepted {
                flow_core::domain::ExecutionStatus::Complete
            } else {
                flow_core::domain::ExecutionStatus::Failed
            },
            result,
        });
        Ok(())
    }
}
pub(crate) fn text<'a>(properties: &'a Record, name: &str) -> Option<&'a str> {
    properties.get(name).and_then(Value::as_str)
}
pub(crate) fn directory(invocation: &Invocation) -> PathBuf {
    text(&invocation.properties, "cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn shell(invocation: &Invocation, _: &Environment, limits: &Limits) -> Result<Process> {
    let source = text(&invocation.properties, "command").context("Shell requires a command")?;
    let mut command = Command::new("/bin/sh");
    command.args(["-c", source]);
    Ok(Process {
        command,
        input: None,
        stdout_stream: "stdout",
        timeout: Duration::from_secs(limits.execution_timeout_secs),
        directory: directory(invocation),
        writer: true,
        decode: |stdout| Ok(json!({"stdout":stdout})),
    })
}
pub(crate) fn metric(
    invocation: &Invocation,
    environment: &Environment,
    limits: &Limits,
) -> Result<Process> {
    let mut process = shell(invocation, environment, limits)?;
    process.timeout = Duration::from_secs(limits.metric_timeout_secs);
    process.writer = false;
    process.decode = |stdout| {
        let last = stdout.trim().lines().last().unwrap_or("");
        let value = last.parse::<f64>().ok().or_else(|| {
            serde_json::from_str::<Value>(last)
                .ok()
                .and_then(|v| v["value"].as_f64())
        });
        let value = value
            .filter(|v| v.is_finite())
            .context("Metric command must end with a number or {\"value\": number}.")?;
        Ok(json!({"stdout":stdout,"value":value}))
    };
    Ok(process)
}
pub fn registry(limits: Limits) -> Result<Registry> {
    registry_with_environment(limits, Environment::default())
}
pub fn registry_with_environment(limits: Limits, environment: Environment) -> Result<Registry> {
    let mut registry = Registry::default();
    registry.session_environment = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    environment.row_settings.install(&mut registry);
    let mut definitions: Vec<_> = [
        ("shell", Behavior::Invoked, vec!["shell"]),
        ("constant", Behavior::Lazy, vec![]),
        ("metric_sample", Behavior::Invoked, vec!["shell"]),
        ("metric", Behavior::Source, vec!["shell"]),
        ("timer", Behavior::Source, vec![]),
        ("watcher", Behavior::Source, vec!["shell"]),
        ("broadcast", Behavior::Invoked, vec![]),
        ("receive", Behavior::Source, vec![]),
        ("alert", Behavior::Invoked, vec![]),
        ("join", Behavior::Join, vec![]),
        ("agent", Behavior::Invoked, vec!["shell"]),
        ("window", Behavior::Application, vec!["application"]),
        ("human", Behavior::Invoked, vec![]),
        ("gate", Behavior::Invoked, vec![]),
    ]
    .into_iter()
    .map(|(name, behavior, capabilities)| {
        use flow_core::{domain::*, graph::*};
        let graph = FlowNet::default();
        let node = NodeSpec {
            id: "schema".into(),
            type_id: format!("local.{name}"),
            flow: Default::default(),
        };
        let mut inputs = pins(&graph, &node, false);
        let mut outputs = pins(&graph, &node, true);
        if behavior == Behavior::Source {
            inputs.clear();
            outputs = vec![
                Pin::new(
                    if name == "metric" { "on_sample" } else { "out" },
                    Type::Signal {
                        args: if name == "timer" {
                            BTreeMap::from([("tick".into(), Type::UInt)])
                        } else {
                            BTreeMap::from([("value".into(), Type::Float)])
                        },
                        open: true,
                    },
                ),
                Pin::new("value", Type::Float),
            ];
        }
        if name == "receive" {
            outputs = vec![
                Pin::new(
                    "out",
                    Type::Signal {
                        args: BTreeMap::new(),
                        open: true,
                    },
                ),
                Pin::new("args", Type::record()),
            ];
        }
        if matches!(name, "human" | "gate") {
            outputs = vec![
                Pin::new(
                    "accepted",
                    Type::Signal {
                        args: BTreeMap::new(),
                        open: true,
                    },
                ),
                Pin::new(
                    "rejected",
                    Type::Signal {
                        args: BTreeMap::new(),
                        open: true,
                    },
                ),
                Pin::new("output", Type::String),
            ];
        }
        if name == "join" {
            let event = Type::Signal {
                args: BTreeMap::new(),
                open: true,
            };
            inputs = vec![
                Pin::new("left", event.clone()),
                Pin::new("right", event.clone()),
                Pin::new("failed", event),
            ];
        }
        Definition {
            id: format!("local.{name}"),
            owner: "local".into(),
            behavior,
            inputs,
            outputs,
            capabilities: capabilities.into_iter().map(String::from).collect(),
            window: None,
            renderer: None,
            properties: {
                let mut properties = BTreeMap::new();
                let mut property = |name: &str, ty: Type, value: Value| {
                    properties.insert(
                        name.into(),
                        Property {
                            ty,
                            default: Some(value),
                            output: false,
                        },
                    );
                };
                if matches!(name, "timer" | "metric" | "watcher" | "receive") {
                    property(
                        "interval_secs",
                        Type::UInt,
                        json!(limits.source_interval_secs),
                    );
                }
                if matches!(name, "receive" | "broadcast") {
                    property("channel", Type::String, json!("events"));
                    property(
                        "signal_type",
                        Type::Any,
                        json!({"kind":"signal","args":{},"open":true}),
                    );
                }
                if name == "broadcast" {
                    property("target", Type::String, json!(""));
                }
                if name == "watcher" {
                    property("threshold", Type::Float, json!(0.));
                    property("above", Type::Bool, json!(true));
                }
                properties
            },
        }
    })
    .collect();
    definitions.extend(crate::environment::definitions());
    registry.mount(
        "local".into(),
        definitions,
        Arc::new(Local::with_environment(limits, environment)),
    )?;
    Ok(registry)
}
