use anyhow::{Context, Result, bail, ensure};
use flow_core::{domain::NodeSpec, graph::*};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
pub fn python(
    node: &NodeSpec,
    bindings: &Record,
    args: &Value,
    cancel: &AtomicBool,
    limits: &crate::local::Limits,
) -> Result<Value> {
    const BRIDGE: &str = "import json, os, sys\nb = json.load(sys.stdin)\nscope = dict(b['bindings'])\nscope['outputs'] = {}\nexec(compile(b['source'], '<FlowWM Python>', 'exec'), scope)\nwith os.fdopen(3, 'w') as result:\n json.dump(scope['outputs'], result, allow_nan=False)\n";
    // Keep program stdout separate from the result channel, and bound both pipes.
    let (result_read, result_write) = std::os::unix::net::UnixStream::pair()?;
    let result_fd = std::os::fd::AsRawFd::as_raw_fd(&result_write);
    let mut command = Command::new("python3");
    command
        .args(["-c", BRIDGE])
        .current_dir(
            node.flow
                .properties
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("."),
        )
        .envs(signal_environment(args)?)
        .env("FLOWWM_NODE_ID", &node.id.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let own_group = std::env::var_os("FLOWWM_GRAPH_WORKER").is_none();
    if own_group {
        command.process_group(0);
    }
    for (key, _) in
        std::env::vars_os().filter(|(k, _)| k.as_encoded_bytes().starts_with(b"FLOWWM_ARG_"))
    {
        command.env_remove(key);
    }
    command.envs(signal_environment(args)?);
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(result_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::fcntl(3, libc::F_SETFD, 0);
            Ok(())
        });
    }
    let mut child = command.spawn().context("Start Python binding")?;
    drop(result_write);
    let payload = serde_json::to_vec(
        &json!({"bindings": bindings, "source": node.flow.properties.get("command").and_then(Value::as_str).unwrap_or("")}),
    )?;
    let mut stdin = child.stdin.take().unwrap();
    let input = thread::spawn(move || stdin.write_all(&payload));
    let mut stderr = child.stderr.take().unwrap();
    let err = thread::spawn(move || {
        let mut bytes = vec![];
        let _ = (&mut stderr).take(64_001).read_to_end(&mut bytes);
        bytes
    });
    let out = thread::spawn(move || {
        let mut bytes = vec![];
        let _ = result_read.take(1_048_577).read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed)
            || start.elapsed() > Duration::from_secs(limits.evaluator_timeout_secs)
        {
            if own_group {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            } else {
                let _ = child.kill();
            }
            let _ = child.wait();
            bail!("Python evaluation stopped or timed out");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    // A script may have left descendants holding pipes. Reap its owned group.
    if own_group {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    input
        .join()
        .map_err(|_| anyhow::anyhow!("Python input worker failed"))??;
    let stderr = err.join().unwrap_or_default();
    ensure!(
        status.success(),
        "Python binding failed: {}",
        String::from_utf8_lossy(&stderr)
    );
    let bytes = out.join().unwrap_or_default();
    ensure!(bytes.len() <= 1_048_576, "Python result is too large");
    serde_json::from_slice(&bytes).context("Python outputs must be a JSON record")
}
