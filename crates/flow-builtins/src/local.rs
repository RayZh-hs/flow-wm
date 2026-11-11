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
    pub desktop_branches_follow_rows: bool,
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
            desktop_branches_follow_rows: false,
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
            self.max_concurrent > 0
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
    pub config_directory: Option<PathBuf>,
    pub data_directory: Option<PathBuf>,
    pub control_socket: Option<PathBuf>,
    pub cli: Option<PathBuf>,
}
type Factory = fn(&Invocation, &Environment, &Limits) -> Result<Process>;
pub struct Local {
    limits: Limits,
    environment: Environment,
    processes: Supervisor,
    factories: BTreeMap<String, Factory>,
    decisions: Mutex<BTreeMap<String, (Invocation, EventSink)>>,
}
impl Local {
    pub fn new(limits: Limits) -> Self {
        Self::with_environment(limits, Environment::default())
    }
    pub fn with_environment(limits: Limits, environment: Environment) -> Self {
        Self {
            decisions: Mutex::new(BTreeMap::new()),
            limits,
            environment,
            processes: Supervisor::default(),
            factories: BTreeMap::from([
                ("local.shell".into(), shell as Factory),
                ("local.metric".into(), metric as Factory),
                ("local.agent".into(), crate::agent::process as Factory),
                ("local.application".into(), application as Factory),
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
    fn read(&self, invocation: Invocation) -> Result<Value> {
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
            decisions.insert(handle.clone(), (invocation, events));
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
        process.command.env(
            "FLOWWM_INPUTS",
            invocation
                .properties
                .get("inputs")
                .unwrap_or(&json!([]))
                .to_string(),
        );
        if let Some(socket) = &self.environment.control_socket {
            process.command.env("FLOWWM_SOCKET", socket);
        }
        if let Some(cli) = &self.environment.cli {
            process.command.env("FLOWWM_CLI", cli);
        }
        if let Some(inputs) = invocation.properties.get("data_inputs") {
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
    command.args(["-lc", source]);
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
fn metric(invocation: &Invocation, environment: &Environment, limits: &Limits) -> Result<Process> {
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
fn application(invocation: &Invocation, _: &Environment, limits: &Limits) -> Result<Process> {
    let application = text(&invocation.properties, "application")
        .context("Application identifier is required")?;
    let mut command = Command::new("gtk-launch");
    command.arg(application);
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "QT_PLUGIN_PATH",
        "NIXPKGS_QT6_QML_IMPORT_PATH",
    ] {
        if let Some(original) = std::env::var_os(format!("DMS_ORIG_{key}")) {
            if original.is_empty() {
                command.env_remove(key);
            } else {
                command.env(key, original);
            }
        }
    }
    Ok(Process {
        command,
        input: None,
        stdout_stream: "stdout",
        timeout: Duration::from_secs(limits.execution_timeout_secs),
        directory: directory(invocation),
        writer: false,
        decode: |stdout| Ok(json!({"stdout":stdout})),
    })
}
pub fn registry(limits: Limits) -> Result<Registry> {
    registry_with_environment(limits, Environment::default())
}
pub fn registry_with_environment(limits: Limits, environment: Environment) -> Result<Registry> {
    let mut registry = Registry::default();
    let definitions = [
        ("shell", Behavior::Invoked, vec!["shell"]),
        ("constant", Behavior::Lazy, vec![]),
        ("metric", Behavior::Invoked, vec!["shell"]),
        ("agent", Behavior::Invoked, vec!["shell"]),
        ("application", Behavior::Invoked, vec!["desktop"]),
        ("human", Behavior::Invoked, vec![]),
        ("gate", Behavior::Invoked, vec![]),
    ]
    .into_iter()
    .map(|(name, behavior, capabilities)| Definition {
        id: format!("local.{name}"),
        owner: "local".into(),
        behavior,
        inputs: vec![],
        outputs: vec![],
        capabilities: capabilities.into_iter().map(String::from).collect(),
        window: None,
        renderer: None,
    })
    .collect();
    registry.mount(
        "local".into(),
        definitions,
        Arc::new(Local::with_environment(limits, environment)),
    )?;
    Ok(registry)
}
