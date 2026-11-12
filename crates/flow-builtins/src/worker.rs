//! Run evaluations in an owned process group so Stop also interrupts Vox loops.
use crate::evaluate_with_limits;
use anyhow::{Context, Result, ensure};
use flow_core::{domain::FlowNet, graph::*};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct Request {
    limits: crate::local::Limits,
    graph: FlowNet,
    context: EvaluationContext,
    root: String,
    signal: Option<(String, Value)>,
}

/// Internal CLI entrypoint; the result travels on fd 3, separate from user code.
pub fn run_worker() -> Result<()> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        "Graph snapshot is too large"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    let result = evaluate_with_limits(
        request.graph,
        request.context,
        &request.root,
        request
            .signal
            .as_ref()
            .map(|(p, a)| (p.as_str(), a.clone())),
        Arc::new(AtomicBool::new(false)),
        request.limits,
    )
    .map_err(|e| format!("{e:#}"));
    let output = unsafe { File::from_raw_fd(3) };
    serde_json::to_writer(output, &result)?;
    Ok(())
}

pub fn evaluate_isolated(
    executable: &std::path::Path,
    limits: &crate::local::Limits,
    graph: FlowNet,
    context: EvaluationContext,
    root: &str,
    signal: Option<(String, Value)>,
    cancel: Arc<AtomicBool>,
) -> Result<Evaluation> {
    let bytes = serde_json::to_vec(&Request {
        limits: limits.clone(),
        graph,
        context,
        root: root.into(),
        signal,
    })?;
    ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        "Graph snapshot is too large"
    );
    let (reader, writer) = UnixStream::pair()?;
    let fd = writer.as_raw_fd();
    let mut command = Command::new(executable);
    command
        .arg("evaluate-worker")
        .env("FLOWWM_GRAPH_WORKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::fcntl(3, libc::F_SETFD, 0);
            Ok(())
        });
    }
    let mut child = command.spawn().context("Start Vox graph evaluator")?;
    drop(writer);
    let mut input = child.stdin.take().unwrap();
    let input = thread::spawn(move || input.write_all(&bytes));
    let mut stderr = child.stderr.take().unwrap();
    let err = thread::spawn(move || {
        let mut bytes = vec![];
        let _ = (&mut stderr).take(64_001).read_to_end(&mut bytes);
        bytes
    });
    let out = thread::spawn(move || {
        let mut bytes = vec![];
        let _ = reader.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    let mut interrupted = false;
    let mut interrupted_at = None;
    let status = loop {
        if (cancel.load(Ordering::Relaxed)
            || start.elapsed() > Duration::from_secs(limits.evaluator_timeout_secs))
            && interrupted_at.is_none()
        {
            interrupted = true;
            interrupted_at = Some(Instant::now());
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGINT);
            }
        }
        if let Some(at) = interrupted_at {
            if at.elapsed() >= Duration::from_millis(limits.interrupt_grace_ms) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGTERM);
                }
            }
            if at.elapsed()
                >= Duration::from_millis(limits.interrupt_grace_ms + limits.terminate_grace_ms)
            {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = input.join();
    let stderr = err.join().unwrap_or_default();
    let bytes = out.join().unwrap_or_default();
    ensure!(!interrupted, "Graph evaluation stopped or timed out");
    ensure!(
        status.success(),
        "Graph evaluator failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    ensure!(bytes.len() <= 4 * 1024 * 1024, "Graph result is too large");
    serde_json::from_slice::<std::result::Result<Evaluation, String>>(&bytes)?
        .map_err(anyhow::Error::msg)
}
