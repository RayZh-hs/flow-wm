use crate::engine::{Control, Engine};
use crate::{catalog::Catalog, extensions::Macro};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub fn default_socket() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")))
        .join("flowwm.sock")
}
pub fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("FlowWM is already running at {}", path.display());
        }
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::getuid() },
            "Refusing to replace a socket not owned by this user"
        );
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path).context("Create FlowWM control socket")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
pub fn serve(listener: UnixListener, engine: Engine, macros: Vec<Macro>) {
    let macros = std::sync::Arc::new(macros);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                break;
            };
            let engine = engine.clone();
            let macros = macros.clone();
            thread::spawn(move || serve_client(stream, engine, &macros));
        }
    });
}
fn serve_client(mut stream: UnixStream, engine: Engine, macros: &[Macro]) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let result = (|| -> Result<Value> {
        let mut bytes = vec![];
        std::io::Read::take(BufReader::new(stream.try_clone()?), 1024 * 1024)
            .read_until(b'\n', &mut bytes)?;
        ensure!(
            bytes.last() == Some(&b'\n'),
            "Request is too large or incomplete"
        );
        let value: Value = serde_json::from_slice(&bytes)?;
        if value.get("signal_protocol").is_some() {
            let frame: flow_core::protocol::SignalFrame = serde_json::from_value(value)?;
            ensure!(
                frame.signal_protocol == flow_core::protocol::SIGNAL_VERSION,
                "Unsupported signal protocol"
            );
            let reply: flow_core::protocol::Reply =
                serde_json::from_value(engine.request(Control::Orchestrate {
                    request: Box::new(flow_core::protocol::Request {
                        protocol: 2,
                        id: frame.id.clone(),
                        expected_revision: None,
                        caller: Some(frame.delivery.source.clone()),
                        command: flow_core::protocol::Command::Broadcast {
                            delivery: frame.delivery,
                        },
                    }),
                })?)?;
            return Ok(serde_json::to_value(flow_core::protocol::SignalReply {
                signal_protocol: 1,
                id: frame.id,
                revision: reply.revision,
                accepted: reply
                    .result
                    .as_ref()
                    .and_then(|v| v["accepted"].as_bool())
                    .unwrap_or(false),
                error: reply.error,
            })?);
        }
        if value["op"] == "snapshot" {
            return Ok(serde_json::to_value(engine.snapshot())?);
        }
        if value["op"] == "attach" {
            ensure!(value["protocol"] == 2, "Unsupported client protocol");
            let commands = engine.connect_frontend();
            let disconnected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let closed = disconnected.clone();
            let wake = engine.clone();
            let mut peer = stream.try_clone()?;
            peer.set_read_timeout(None)?;
            thread::spawn(move || {
                let mut byte = [0];
                let _ = std::io::Read::read(&mut peer, &mut byte);
                closed.store(true, std::sync::atomic::Ordering::Release);
                wake.wake_ui();
            });
            let mut generation = 0;
            let mut previous = None;
            while let Some(next) = engine.wait_for_change(generation) {
                generation = next;
                if disconnected.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let snapshot = engine.snapshot();
                let changed = previous != Some(snapshot.revision);
                previous = Some(snapshot.revision);
                let frame = ClientFrame {
                    snapshot: changed.then_some(snapshot),
                    commands: commands.try_iter().collect(),
                };
                if frame.snapshot.is_some() || !frame.commands.is_empty() {
                    writeln!(stream, "{}", serde_json::to_string(&frame)?)?;
                }
            }
            return Ok(Value::Null);
        }
        if value["op"] == "presentation" {
            engine.set_presentation(serde_json::from_value(value["value"].clone())?);
            return Ok(Value::Null);
        }
        if value.get("protocol").is_some() {
            let request: flow_core::protocol::Request = serde_json::from_value(value)?;
            if matches!(
                request.command,
                flow_core::protocol::Command::Subscribe { .. }
            ) {
                let mut generation = 0;
                let mut previous = None;
                while let Some(next) = engine.wait_for_change(generation) {
                    generation = next;
                    let mut snapshot_request = request.clone();
                    snapshot_request.command = flow_core::protocol::Command::Snapshot;
                    snapshot_request.expected_revision = None;
                    let reply = engine.request(Control::Orchestrate {
                        request: Box::new(snapshot_request),
                    })?;
                    let encoded = serde_json::to_string(&reply)?;
                    if previous.as_ref() != Some(&encoded) {
                        writeln!(stream, "{encoded}")?;
                        previous = Some(encoded);
                    }
                }
                return Ok(Value::Null);
            }
            return engine.request(Control::Orchestrate {
                request: Box::new(request),
            });
        }
        if value["op"] == "performance" {
            if value["reset"] == true {
                engine.performance.reset();
            }
            return Ok(serde_json::to_value(engine.performance.snapshot())?);
        }
        if value["op"] == "subscribe" {
            let launcher = (value["launcher"] == true).then(|| engine.connect_launcher());
            let mut generation = 0;
            let mut previous = String::new();
            while let Some(next) = engine.wait_for_change(generation) {
                generation = next;
                let catalog = serde_json::to_string(&Catalog::new(
                    &engine.snapshot(),
                    engine.presentation(),
                    macros,
                ))?;
                if catalog != previous {
                    writeln!(stream, "{catalog}")?;
                    previous = catalog;
                }
                if let Some(launcher) = &launcher {
                    for query in launcher.try_iter() {
                        writeln!(
                            stream,
                            "{}",
                            serde_json::json!({"launcher": {"query": query}})
                        )?;
                    }
                }
            }
            return Ok(Value::Null);
        }
        if value["op"] == "catalog" {
            return Ok(serde_json::to_value(Catalog::new(
                &engine.snapshot(),
                engine.presentation(),
                macros,
            ))?);
        }
        engine.request(serde_json::from_value::<Control>(value)?)
    })();
    let reply = match result {
        Ok(value) => serde_json::json!({"ok":value}),
        Err(e) => serde_json::json!({"error":format!("{e:#}")}),
    };
    let _ = writeln!(stream, "{reply}");
}
pub fn send(path: &Path, command: &impl serde::Serialize) -> Result<Value> {
    let mut stream = UnixStream::connect(path).with_context(|| {
        format!(
            "FlowWM is not running. Start `flowwm start` first (socket: {}).",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    writeln!(stream, "{}", serde_json::to_string(command)?)?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let mut value: Value = serde_json::from_str(&reply)?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        bail!("{error}");
    }
    Ok(value["ok"].take())
}

pub fn host_socket() -> PathBuf {
    std::env::var_os("FLOWWM_HOST_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_socket().with_file_name("flow-core.sock"))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ClientFrame {
    pub snapshot: Option<crate::engine::Snapshot>,
    pub commands: Vec<crate::engine::UiCommand>,
}
