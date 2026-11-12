//! Headless runtime KDL, following the compositor configuration and include style.
use anyhow::{Context, Result, ensure};
use flow_builtins::local::Limits;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub fn directory() -> PathBuf {
    if let Some(path) = std::env::var_os("FLOWWM_CONFIG_DIR") {
        return path.into();
    }
    directories::BaseDirs::new()
        .expect("Home directory")
        .config_dir()
        .join("flow-wm")
}

#[derive(Default)]
pub struct Configuration {
    pub row_settings: flow_builtins::row_settings::Extension,
    pub host_id: Option<String>,
    pub limits: Limits,
    pub hosts: BTreeMap<String, crate::remote::Remote>,
}
#[derive(knuffel::Decode)]
struct Document {
    #[knuffel(child)]
    row_settings: Option<RowSettings>,
    #[knuffel(child, unwrap(argument))]
    host_id: Option<String>,
    #[knuffel(children(name = "include"))]
    includes: Vec<Include>,
    #[knuffel(children(name = "host"))]
    hosts: Vec<Host>,
    #[knuffel(child, unwrap(argument))]
    source_interval_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    systemd_workspace: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    serialize_working_directories: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    max_concurrent: Option<usize>,
    #[knuffel(child, unwrap(argument))]
    execution_timeout_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    metric_timeout_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    evaluator_timeout_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    interrupt_grace_ms: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    terminate_grace_ms: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    signal_queue_capacity: Option<usize>,
    #[knuffel(child, unwrap(argument))]
    replay_capacity: Option<usize>,
    #[knuffel(child, unwrap(argument))]
    remote_retry_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    remote_timeout_secs: Option<u64>,
    #[knuffel(child, unwrap(argument))]
    remote_batch_size: Option<usize>,
}
#[derive(knuffel::Decode)]
struct RowSettings {
    #[knuffel(child, unwrap(argument))]
    pwd: Option<String>,
    #[knuffel(children(name = "terminal"))]
    terminals: Vec<Terminal>,
}
#[derive(knuffel::Decode)]
struct Terminal {
    #[knuffel(argument)]
    executable: String,
    #[knuffel(arguments)]
    directory_args: Vec<String>,
}
#[derive(knuffel::Decode)]
struct Include {
    #[knuffel(argument)]
    path: String,
}
#[derive(knuffel::Decode)]
struct Host {
    #[knuffel(argument)]
    id: String,
    #[knuffel(child, unwrap(argument))]
    transport: String,
    #[knuffel(child, unwrap(argument))]
    socket: Option<PathBuf>,
    #[knuffel(child, unwrap(argument))]
    destination: Option<String>,
    #[knuffel(child, unwrap(arguments))]
    command: Option<Vec<String>>,
}
pub fn load_from(path: &Path) -> Result<Configuration> {
    fn read(path: &Path, active: &mut BTreeSet<PathBuf>, output: &mut Configuration) -> Result<()> {
        let path = path
            .canonicalize()
            .with_context(|| format!("Read runtime configuration {}", path.display()))?;
        ensure!(
            active.len() < 32 && active.insert(path.clone()),
            "Runtime configuration contains an include cycle or excessive depth"
        );
        let document: Document =
            knuffel::parse(&path.to_string_lossy(), &std::fs::read_to_string(&path)?)
                .map_err(|e| anyhow::anyhow!("Runtime configuration: {e:?}"))?;
        for include in document.includes {
            read(&path.parent().unwrap().join(include.path), active, output)?;
        }
        if let Some(settings) = document.row_settings {
            if let Some(pwd) = settings.pwd {
                output.row_settings.pwd = Some(pwd);
            }
            for terminal in settings.terminals {
                output
                    .row_settings
                    .terminals
                    .insert(terminal.executable, terminal.directory_args);
            }
        }
        if let Some(host) = document.host_id {
            output.host_id = Some(host);
        }
        if let Some(value) = document.source_interval_secs {
            output.limits.source_interval_secs = value;
        }
        if let Some(value) = document.systemd_workspace {
            output.limits.systemd_workspace = value;
        }
        if let Some(value) = document.serialize_working_directories {
            output.limits.serialize_working_directories = value;
        }
        if let Some(value) = document.max_concurrent {
            output.limits.max_concurrent = value;
        }
        if let Some(value) = document.execution_timeout_secs {
            output.limits.execution_timeout_secs = value;
        }
        if let Some(value) = document.metric_timeout_secs {
            output.limits.metric_timeout_secs = value;
        }
        if let Some(value) = document.evaluator_timeout_secs {
            output.limits.evaluator_timeout_secs = value;
        }
        if let Some(value) = document.interrupt_grace_ms {
            output.limits.interrupt_grace_ms = value;
        }
        if let Some(value) = document.terminate_grace_ms {
            output.limits.terminate_grace_ms = value;
        }
        if let Some(value) = document.signal_queue_capacity {
            output.limits.signal_queue_capacity = value;
        }
        if let Some(value) = document.replay_capacity {
            output.limits.replay_capacity = value;
        }
        if let Some(value) = document.remote_retry_secs {
            output.limits.remote_retry_secs = value;
        }
        if let Some(value) = document.remote_timeout_secs {
            output.limits.remote_timeout_secs = value;
        }
        if let Some(value) = document.remote_batch_size {
            output.limits.remote_batch_size = value;
        }
        for host in document.hosts {
            ensure!(
                !host.id.is_empty()
                    && !host.id.contains(':')
                    && !host.id.chars().any(char::is_whitespace),
                "Invalid peer host ID"
            );
            let remote = match host.transport.as_str() {
                "unix" => crate::remote::Remote::Unix {
                    socket: host.socket.context("Unix host needs a socket")?,
                },
                "ssh" => crate::remote::Remote::Ssh {
                    destination: host.destination.context("SSH host needs a destination")?,
                    command: host
                        .command
                        .unwrap_or_else(|| vec!["flow-core".into(), "--stdio".into()]),
                },
                _ => anyhow::bail!("Unknown host transport"),
            };
            output.hosts.insert(host.id, remote);
        }
        active.remove(&path);
        Ok(())
    }
    let mut configuration = Configuration::default();
    if path.exists() {
        read(path, &mut BTreeSet::new(), &mut configuration)?;
    }
    configuration.limits.validate()?;
    Ok(configuration)
}
pub fn load() -> Result<Configuration> {
    ensure!(
        !directory().join("runtime.json").exists(),
        "runtime.json is no longer supported; move its settings to runtime.kdl. The original file has been preserved."
    );
    load_from(&directory().join("runtime.kdl"))
}
pub fn runtime_limits() -> Result<Limits> {
    Ok(load()?.limits)
}
pub fn host_id() -> Result<flow_core::domain::HostId> {
    Ok(load()?.host_id.unwrap_or_else(|| "local".into()).into())
}
