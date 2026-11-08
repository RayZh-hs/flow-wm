use super::*;
use anyhow::bail;
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use vox_core::{
    opt::OptimizationLevel,
    source::SourceText,
    value::{HandleData, InlineValue, RuntimeValue},
};
use vox_runtime::Runtime;

pub(super) fn vox_expression(source: &str, bindings: &Record) -> Result<Value> {
    let mut text = "script flowwm.binding;\n".to_string();
    for (name, value) in bindings {
        validate_name(name)?;
        text.push_str(&format!("val {name} = {};\n", literal(value)?));
    }
    text.push_str(source);
    let mut runtime = Runtime::default();
    let artifact = runtime.load_script(
        SourceText::new("flowwm/binding.vox", 1, text),
        Some(OptimizationLevel::NOpt),
    )?;
    let value = runtime.run_script(artifact, &[])?;
    runtime_json(&runtime, value)
}

fn literal(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        Value::String(s) => {
            let mut literal = String::from("\"");
            for ch in s.chars() {
                match ch {
                    '"' => literal.push_str("\\\""),
                    '\\' => literal.push_str("\\\\"),
                    '$' => literal.push_str("\\$"),
                    '\n' => literal.push_str("\\n"),
                    '\r' => literal.push_str("\\r"),
                    '\t' => literal.push_str("\\t"),
                    c if c.is_control() => literal.push_str(&format!("\\u{{{:x}}}", c as u32)),
                    c => literal.push(c),
                }
            }
            literal.push('"');
            literal
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(literal)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Object(values) => {
            let mut fields = vec![];
            for (name, value) in values {
                validate_name(name)?;
                fields.push(format!("{name} = {}", literal(value)?));
            }
            if fields.is_empty() {
                "{}".into()
            } else {
                format!("{{ {}, }}", fields.join(", "))
            }
        }
    })
}
fn runtime_json(runtime: &Runtime, value: RuntimeValue) -> Result<Value> {
    Ok(match value {
        RuntimeValue::Handle(handle) | RuntimeValue::Inline(InlineValue::Handle(handle)) => {
            data_json(
                runtime
                    .get_handle_data(handle)
                    .map_err(anyhow::Error::msg)?,
            )?
        }
        RuntimeValue::Inline(InlineValue::Int(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::UInt(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::Float(v)) => {
            ensure!(v.is_finite(), "Non-finite Vox output");
            json!(v)
        }
        RuntimeValue::Inline(InlineValue::Bool(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::String(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::Null) => Value::Null,
        RuntimeValue::Inline(InlineValue::Tuple(v)) if v.is_empty() => json!({}),
        RuntimeValue::Inline(InlineValue::Tuple(v)) => Value::Array(
            v.into_iter()
                .map(|v| runtime_json(runtime, RuntimeValue::Inline(v)))
                .collect::<Result<_>>()?,
        ),
        RuntimeValue::Inline(InlineValue::Record(v)) => Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, runtime_json(runtime, RuntimeValue::Inline(v))?)))
                .collect::<Result<_>>()?,
        ),
    })
}
fn data_json(value: HandleData) -> Result<Value> {
    Ok(match value {
        HandleData::Int(v) => json!(v),
        HandleData::UInt(v) => json!(v),
        HandleData::Float(v) => {
            ensure!(v.is_finite(), "Non-finite Vox output");
            json!(v)
        }
        HandleData::Bool(v) => json!(v),
        HandleData::String(v) => json!(v),
        HandleData::Null => Value::Null,
        HandleData::Tuple(v) if v.is_empty() => json!({}),
        HandleData::List(v) | HandleData::Tuple(v) => {
            Value::Array(v.into_iter().map(data_json).collect::<Result<_>>()?)
        }
        HandleData::Record(v) => Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, data_json(v)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

pub(super) fn python(
    node: &Node,
    bindings: &Record,
    args: &Value,
    cancel: &AtomicBool,
) -> Result<Value> {
    const BRIDGE: &str = "import json, os, sys\nb = json.load(sys.stdin)\nscope = dict(b['bindings'])\nscope['outputs'] = {}\nexec(compile(b['source'], '<FlowWM Python>', 'exec'), scope)\nwith os.fdopen(3, 'w') as result:\n json.dump(scope['outputs'], result, allow_nan=False)\n";
    // Keep program stdout separate from the result channel, and bound both pipes.
    let (result_read, result_write) = std::os::unix::net::UnixStream::pair()?;
    let result_fd = std::os::fd::AsRawFd::as_raw_fd(&result_write);
    let mut command = Command::new("python3");
    command
        .args(["-c", BRIDGE])
        .current_dir(&node.cwd)
        .envs(signal_environment(args)?)
        .env("FLOWWM_NODE_ID", &node.id)
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
    let payload = serde_json::to_vec(&json!({"bindings": bindings, "source": node.command}))?;
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
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(60) {
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
