//! All desktop launches resolve a fresh row environment before starting the client.
use anyhow::{Context, Result, ensure};
use flow_core::{domain::*, environment::Env, graph::*, protocol::Command};
use flow_host::client::Client;
use gtk::{gio, glib, prelude::*};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

pub fn row_environment(host: &Client, workflow: &WorkflowId) -> Result<Env> {
    let mut env = serde_json::from_value(host.command_as(
        Command::RowEnvironment {
            workflow: workflow.clone(),
        },
        Some(workflow.clone()),
    )?)?;
    // The independent host can start before the compositor, or outlive a login.
    // Only desktop connections come from this process: keep the host's application
    // environment (including XDG paths outside Dux's private profile) and row settings.
    // Do this before evaluating a node's env input so explicit overrides still win.
    refresh_desktop_environment(&mut env);
    Ok(env)
}

pub fn refresh_desktop_environment(env: &mut Env) {
    let session = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    refresh_session_environment(
        env,
        &session,
        crate::config::flow()
            .session_environment
            .as_deref()
            .unwrap_or_default(),
    );
}

fn refresh_session_environment(env: &mut Env, session: &Env, names: &[String]) {
    for name in names {
        if let Some(value) = session.get(name) {
            env.insert(name.clone(), value.clone());
        } else {
            env.remove(name);
        }
    }
}

pub fn selected_environment(socket: &std::path::Path) -> Result<Env> {
    let view = crate::ipc::send(socket, &json!({"action":"view"}))?;
    let workflow = view["selected_workflow"]
        .as_str()
        .context("No row selected")?;
    row_environment(
        &Client::new(flow_host::ipc::host_socket()),
        &workflow.into(),
    )
}

pub fn node_environment(host: &Client, workflow: &WorkflowId, node: &str) -> Result<Env> {
    let runtime = host.snapshot_as(Some(workflow.clone()))?;
    let graph = runtime.lookup(workflow, Some(workflow))?.graph.clone();
    let env = row_environment(host, workflow)?;
    if !graph
        .connections
        .iter()
        .any(|e| e.target.node == node && e.target.pin == "env")
    {
        return Ok(env);
    }
    let worker: PathBuf = serde_json::from_value(host.command(Command::Info)?["worker"].clone())?;
    let result = flow_builtins::worker::evaluate_isolated(
        &worker,
        &flow_host::config::runtime_limits()?,
        graph,
        EvaluationContext {
            input: Some("env".into()),
            workflow: workflow.clone(),
            environment: env,
            published: runtime.queue.published,
        },
        node,
        None,
        Arc::new(AtomicBool::new(false)),
    )?;
    ensure!(
        result.errors.is_empty(),
        "Environment could not be evaluated: {:?}",
        result.errors
    );
    Ok(serde_json::from_value(result.answer)?)
}

pub fn command(arguments: &[String], env: &Env) -> Result<()> {
    let executable = arguments.first().context("Choose a command to launch")?;
    let extension = flow_host::config::load()?.row_settings;
    let extra = extension.terminal_arguments(executable, env);
    let mut command = std::process::Command::new(executable);
    command
        .args(&extra)
        .args(&arguments[1..])
        .env_clear()
        .envs(env);
    let mut child = command
        .spawn()
        .with_context(|| format!("Launch {executable}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn application(id: &str, env: &Env) -> Result<()> {
    application_in(id, env, None)
}

pub fn application_in(id: &str, env: &Env, directory: Option<&std::path::Path>) -> Result<()> {
    let id = if id.ends_with(".desktop") {
        id.to_owned()
    } else {
        format!("{id}.desktop")
    };
    let entry =
        gio_unix::DesktopAppInfo::new(&id).context("Application desktop entry is unavailable")?;
    let filename = entry
        .filename()
        .context("Application desktop entry has no file")?;
    let keyfile = glib::KeyFile::new();
    keyfile.load_from_file(filename, glib::KeyFileFlags::NONE)?;
    // D-Bus activation cannot supply a different environment for each launch.
    keyfile.set_boolean("Desktop Entry", "DBusActivatable", false);
    let directory = directory
        .map(std::fs::canonicalize)
        .transpose()
        .context("Working directory no longer exists")?;
    let mut terminal_env = env.clone();
    if let Some(directory) = &directory {
        keyfile.set_string("Desktop Entry", "Path", &directory.to_string_lossy());
        terminal_env.insert(
            "FLOWWM_ROWSET_PWD".into(),
            directory.to_string_lossy().into_owned(),
        );
    }
    let executable = entry.executable();
    let args = flow_host::config::load()?
        .row_settings
        .terminal_arguments(&executable.to_string_lossy(), &terminal_env);
    if !args.is_empty() {
        let commandline = format!(
            "{} {}",
            keyfile.string("Desktop Entry", "Exec")?,
            args.iter()
                .map(|s| glib::shell_quote(s).to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        );
        keyfile.set_string("Desktop Entry", "Exec", &commandline);
    }
    let app = gio_unix::DesktopAppInfo::from_keyfile(&keyfile)
        .context("Application desktop entry has an invalid launch command")?;
    let context = gio::AppLaunchContext::new();
    for (name, _) in std::env::vars_os() {
        context.unsetenv(name);
    }
    for (name, value) in env {
        context.setenv(name, value);
    }
    app.launch(&[], Some(&context))?;
    Ok(())
}
