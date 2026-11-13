//! Live terminal presentation for the same supervised agent invocation.
use anyhow::{Result, ensure};
use std::{
    path::Path,
    process::{Command, Stdio},
};
pub const APP_PREFIX: &str = "dev.flowwm.agent.v1.";
pub const RUNNER: &str = include_str!("runner.py");
const VIEWER: &str = include_str!("viewer.py");
const SDK: &str = include_str!("../../../../sdk/python/flow_sdk.py");
const NATIVE: &str = include_str!("native.py");
const HOOK: &str = include_str!("hook.py");

pub fn open_native(
    config: Option<&Path>,
    spec: &flow_core::domain::NodeSpec,
    env: &flow_core::environment::Env,
) -> Result<()> {
    let settings = super::harness::Harnesses::load(config)?;
    let harness = settings.select(
        spec.flow
            .properties
            .get("harness")
            .and_then(serde_json::Value::as_str),
    )?;
    let cwd = crate::row_settings::directory(&spec.flow.properties, env);
    let title = spec
        .flow
        .properties
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Agent session");
    let cwd = cwd.canonicalize()?;
    let node = &spec.id.0;
    ensure!(
        !node.is_empty()
            && node
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid agent node identity"
    );
    let app_id = format!("{APP_PREFIX}{node}");
    let args = settings
        .terminal
        .iter()
        .map(|s| {
            s.replace("{app_id}", &app_id)
                .replace("{title}", title)
                .replace("{cwd}", &cwd.to_string_lossy())
        })
        .collect::<Vec<_>>();
    let request = serde_json::json!({"node":node,"harness":harness.id,"adapter":harness.adapter,"command":harness.command,"interactive":harness.interactive,
        "prompt":spec.flow.properties.get("command").and_then(serde_json::Value::as_str).unwrap_or_default(),
        "writable":spec.flow.properties.get("writable").and_then(serde_json::Value::as_bool).unwrap_or(false),"hook":format!("{SDK}\n{HOOK}")});
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .args([
            "python3",
            "-c",
            &format!("{SDK}\n{NATIVE}"),
            &request.to_string(),
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::environment::apply(&mut command, env)?;
    if let Some(config) = config {
        command.env("FLOWWM_CONFIG_DIR", config);
    }
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
pub fn open(
    config: Option<&Path>,
    node: &str,
    title: &str,
    cwd: &Path,
    env: &flow_core::environment::Env,
) -> Result<()> {
    ensure!(
        !node.is_empty()
            && node
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid agent node identity"
    );
    let settings = super::harness::Harnesses::load(config)?;
    let app_id = format!("{APP_PREFIX}{node}");
    let cwd = cwd.canonicalize()?;
    let args = settings
        .terminal
        .iter()
        .map(|s| {
            s.replace("{app_id}", &app_id)
                .replace("{title}", title)
                .replace("{cwd}", &cwd.to_string_lossy())
        })
        .collect::<Vec<_>>();
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .args(["python3", "-c", VIEWER, node])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::environment::apply(&mut command, env)?;
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
