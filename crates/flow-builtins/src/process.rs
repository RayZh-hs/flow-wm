//! Supervision shared by builtin commands and application adapters.
use crate::local::Limits;
use anyhow::{Context, Result, ensure};
use flow_core::{domain::ExecutionStatus, providers::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Process {
    pub command: Command,
    pub input: Option<String>,
    pub stdout_stream: &'static str,
    pub timeout: Duration,
    pub directory: PathBuf,
    pub writer: bool,
    pub decode: fn(&str) -> Result<Value>,
}
struct Running {
    cancel: Arc<AtomicBool>,
    directory: PathBuf,
    writer: bool,
}
#[derive(Default)]
pub struct Supervisor {
    runs: Arc<Mutex<BTreeMap<String, Running>>>,
}
impl Supervisor {
    pub fn start(
        &self,
        mut process: Process,
        invocation: Invocation,
        limits: &Limits,
        events: EventSink,
        output: Option<OutputSink>,
    ) -> Result<String> {
        let handle = invocation.id.0.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let directory = process
            .directory
            .canonicalize()
            .context("Working directory no longer exists")?;
        let mut runs = self.runs.lock().unwrap();
        ensure!(
            runs.len() < limits.max_concurrent,
            "Execution concurrency limit reached"
        );
        ensure!(!runs.contains_key(&handle), "Invocation is already running");
        ensure!(
            !limits.serialize_working_directories
                || !runs
                    .values()
                    .any(|r| { r.directory == directory && (r.writer || process.writer) }),
            "Working directory is in use by another execution"
        );
        process
            .command
            .current_dir(&directory)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Signal arguments belong to this delivery, never to its parent process.
        for (key, _) in
            std::env::vars_os().filter(|(k, _)| k.as_encoded_bytes().starts_with(b"FLOWWM_ARG_"))
        {
            process.command.env_remove(key);
        }
        process
            .command
            .envs(flow_core::graph::signal_environment(&invocation.args)?);
        process.command.env("FLOWWM_NODE_ID", &invocation.node.0);
        let mut child = process.command.spawn().context("Start builtin execution")?;
        let pid = child.id();
        let stdin = child.stdin.take().unwrap();
        thread::spawn(move || {
            if let Some(input) = process.input {
                let _ = { stdin }.write_all(input.as_bytes());
            }
        });
        let stdout = drain(
            child.stdout.take().unwrap(),
            &invocation,
            process.stdout_stream,
            output.clone(),
            1_048_576,
        );
        let stderr = drain(
            child.stderr.take().unwrap(),
            &invocation,
            "stderr",
            output,
            65_536,
        );
        runs.insert(
            handle.clone(),
            Running {
                cancel: cancel.clone(),
                directory,
                writer: process.writer,
            },
        );
        drop(runs);
        let runs = self.runs.clone();
        let handle_copy = handle.clone();
        let limits = limits.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let mut interrupted = None;
            let mut terminated = false;
            let result = loop {
                if interrupted.is_none()
                    && (cancel.load(Ordering::Acquire) || start.elapsed() >= process.timeout)
                {
                    signal(pid, libc::SIGINT);
                    interrupted = Some(Instant::now());
                }
                if let Some(at) = interrupted {
                    if !terminated
                        && at.elapsed() >= Duration::from_millis(limits.interrupt_grace_ms)
                    {
                        signal(pid, libc::SIGTERM);
                        terminated = true;
                    }
                    if at.elapsed()
                        >= Duration::from_millis(
                            limits.interrupt_grace_ms + limits.terminate_grace_ms,
                        )
                    {
                        signal(pid, libc::SIGKILL);
                    }
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(error) => break Err(error),
                    Ok(None) => thread::sleep(Duration::from_millis(20)),
                }
            };
            // Descendants must not retain the pipes after their owning job exits.
            signal(pid, libc::SIGKILL);
            let stdout = stdout.join().unwrap_or_default();
            let stderr = stderr.join().unwrap_or_default();
            let mut status = if interrupted.is_some() {
                ExecutionStatus::Interrupted
            } else if result.as_ref().is_ok_and(|s| s.success()) {
                ExecutionStatus::Complete
            } else {
                ExecutionStatus::Failed
            };
            let decoded = (process.decode)(&stdout);
            let mut value = match decoded {
                Ok(value) => value,
                Err(error) => {
                    if status == ExecutionStatus::Complete {
                        status = ExecutionStatus::Failed;
                    }
                    json!({"stdout":stdout,"error":error.to_string()})
                }
            };
            if let Some(value) = value.as_object_mut() {
                value.insert("stderr".into(), json!(stderr));
                value.insert(
                    "exit_code".into(),
                    json!(result.ok().and_then(|s| s.code())),
                );
                if interrupted.is_some() {
                    value.insert(
                        "error".into(),
                        json!("Run was stopped or exceeded its time limit"),
                    );
                }
                if value.get("outcome").and_then(Value::as_str) == Some("issue")
                    && status == ExecutionStatus::Complete
                {
                    status = ExecutionStatus::Failed;
                }
            }
            runs.lock().unwrap().remove(&handle_copy);
            events(Completion {
                invocation: invocation.id,
                generation: invocation.generation,
                status,
                result: value,
            });
        });
        Ok(handle)
    }
    pub fn interrupt(&self, handle: &str) -> Result<()> {
        if let Some(run) = self.runs.lock().unwrap().get(handle) {
            run.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
}
fn signal(pid: u32, signal: i32) {
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}
fn drain(
    pipe: impl Read + Send + 'static,
    invocation: &Invocation,
    stream: &'static str,
    output: Option<OutputSink>,
    capacity: usize,
) -> thread::JoinHandle<String> {
    let invocation = invocation.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut captured = Vec::new();
        let mut total = 0usize;
        let mut lines = 0usize;
        loop {
            let mut bytes = Vec::new();
            match (&mut reader).take(256 * 1024).read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            captured.extend_from_slice(&bytes[..bytes.len().min(capacity - captured.len())]);
            total += bytes.len();
            lines += 1;
            if total <= 4 * 1024 * 1024
                && lines <= 4096
                && let Some(output) = &output
            {
                output(Output {
                    invocation: invocation.id.clone(),
                    generation: invocation.generation,
                    stream: stream.into(),
                    line: String::from_utf8_lossy(&bytes).trim_end().into(),
                });
            }
        }
        String::from_utf8_lossy(&captured).into_owned()
    })
}
