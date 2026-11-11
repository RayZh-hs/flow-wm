//! Owned Linux execution provider. Interrupt is SIGINT followed by configured escalation.
use anyhow::{Context, Result, ensure};
use flow_core::{domain::*, providers::*};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub max_concurrent: usize,
    pub execution_timeout_secs: u64,
    pub metric_timeout_secs: u64,
    pub evaluator_timeout_secs: u64,
    pub interrupt_grace_ms: u64,
    pub terminate_grace_ms: u64,
    pub signal_queue_capacity: usize,
    pub replay_capacity: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            execution_timeout_secs: 3600,
            metric_timeout_secs: 20,
            evaluator_timeout_secs: 60,
            interrupt_grace_ms: 1000,
            terminate_grace_ms: 1000,
            signal_queue_capacity: 4096,
            replay_capacity: 65536,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.max_concurrent > 0
                && self.execution_timeout_secs > 0
                && self.evaluator_timeout_secs > 0
                && self.signal_queue_capacity > 0
                && self.replay_capacity > 0,
            "Runtime limits must be positive"
        );
        Ok(())
    }
}
#[derive(Default)]
pub struct Local {
    limits: Limits,
    runs: Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>,
}
impl Local {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            ..Default::default()
        }
    }
}
impl ExecutionProvider for Local {
    fn invoke(&self, invocation: Invocation, events: EventSink) -> Result<String> {
        let handle = invocation.id.0.clone();
        if invocation.type_id == "local.constant" {
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status: ExecutionStatus::Complete,
                result: json!({"value":invocation.properties.get("value")}),
            });
            return Ok(handle);
        }
        ensure!(
            invocation.type_id == "local.shell",
            "Unsupported local provider node"
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let mut runs = self.runs.lock().unwrap();
        ensure!(
            runs.len() < self.limits.max_concurrent,
            "Execution concurrency limit reached"
        );
        ensure!(!runs.contains_key(&handle), "Invocation is already running");
        let source = invocation
            .properties
            .get("command")
            .and_then(|v| v.as_str())
            .context("Shell requires a command")?;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-lc", source])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = invocation.properties.get("cwd").and_then(|v| v.as_str()) {
            command.current_dir(cwd);
        }
        for (key, _) in
            std::env::vars_os().filter(|(k, _)| k.as_encoded_bytes().starts_with(b"FLOWWM_ARG_"))
        {
            command.env_remove(key);
        }
        command.envs(flow_core::graph::signal_environment(&invocation.args)?);
        command.env("FLOWWM_NODE_ID", &invocation.node.0);
        let mut child = command.spawn().context("Start shell invocation")?;
        let pid = child.id();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(1_048_577).read_to_end(&mut bytes);
            bytes
        });
        let err = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.take(65537).read_to_end(&mut bytes);
            bytes
        });
        runs.insert(handle.clone(), cancel.clone());
        drop(runs);
        let runs = self.runs.clone();
        let handle_copy = handle.clone();
        let limits = self.limits.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let mut interrupted = None;
            let mut terminated = false;
            let result = loop {
                if interrupted.is_none()
                    && (cancel.load(Ordering::Acquire)
                        || start.elapsed() >= Duration::from_secs(limits.execution_timeout_secs))
                {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGINT);
                    }
                    interrupted = Some(Instant::now());
                }
                if let Some(at) = interrupted {
                    if !terminated
                        && at.elapsed() >= Duration::from_millis(limits.interrupt_grace_ms)
                    {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGTERM);
                        }
                        terminated = true;
                    }
                    if at.elapsed()
                        >= Duration::from_millis(
                            limits.interrupt_grace_ms + limits.terminate_grace_ms,
                        )
                    {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => break Err(error),
                    Ok(None) => thread::sleep(Duration::from_millis(20)),
                }
            };
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let stdout = out.join().unwrap_or_default();
            let stderr = err.join().unwrap_or_default();
            let status = if interrupted.is_some() {
                ExecutionStatus::Interrupted
            } else if result.as_ref().is_ok_and(|s| s.success()) {
                ExecutionStatus::Complete
            } else {
                ExecutionStatus::Failed
            };
            runs.lock().unwrap().remove(&handle_copy);
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status,
                result: json!({"stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr), "exit_code":result.ok().and_then(|s|s.code())}),
            });
        });
        Ok(handle)
    }
    fn interrupt(&self, handle: &str) -> Result<()> {
        if let Some(cancel) = self.runs.lock().unwrap().get(handle) {
            cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
}
pub fn registry(limits: Limits) -> Result<Registry> {
    let mut registry = Registry::default();
    registry.mount(
        "local".into(),
        vec![
            Definition {
                id: "local.shell".into(),
                owner: "local".into(),
                behavior: Behavior::Invoked,
                inputs: vec![],
                outputs: vec![],
                capabilities: std::collections::BTreeSet::from(["shell".into()]),
                window: None,
                renderer: None,
            },
            Definition {
                id: "local.constant".into(),
                owner: "local".into(),
                behavior: Behavior::Lazy,
                inputs: vec![],
                outputs: vec![],
                capabilities: Default::default(),
                window: None,
                renderer: None,
            },
        ],
        Arc::new(Local::new(limits)),
    )?;
    Ok(registry)
}
