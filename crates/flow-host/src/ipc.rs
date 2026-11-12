//! Current Orchestrator and Signal Protocol transports. No desktop commands.
use anyhow::{Context, Result, bail, ensure};
use flow_core::protocol::*;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
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
/// A subscription releases its waiting server thread as soon as its client closes.
pub struct ConnectionGuard {
    pub closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stream: UnixStream,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
pub fn disconnect_signal(
    stream: &UnixStream,
    wake: impl FnOnce() + Send + 'static,
) -> Result<ConnectionGuard> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    stream.set_read_timeout(None)?;
    let mut reader = stream.try_clone()?;
    let guard = ConnectionGuard {
        closed: Arc::new(AtomicBool::new(false)),
        stream: stream.try_clone()?,
    };
    let closed = guard.closed.clone();
    thread::spawn(move || {
        let _ = reader.read(&mut [0u8; 1]);
        closed.store(true, Ordering::Release);
        wake();
    });
    Ok(guard)
}
pub fn serve(listener: UnixListener, host: crate::runtime::Host) {
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let host = host.clone();
            thread::spawn(move || {
                let mut stream = stream;
                let result = (|| -> Result<Value> {
                    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                    let mut bytes = Vec::new();
                    BufReader::new(stream.try_clone()?)
                        .take(1024 * 1024 + 1)
                        .read_until(b'\n', &mut bytes)?;
                    ensure!(
                        bytes.len() <= 1024 * 1024 && bytes.last() == Some(&b'\n'),
                        "Request too large or incomplete"
                    );
                    let value: Value = serde_json::from_slice(&bytes)?;
                    if value.get("signal_protocol").is_some() {
                        let signal: SignalFrame = serde_json::from_value(value)?;
                        ensure!(
                            signal.signal_protocol == SIGNAL_VERSION,
                            "Unsupported signal protocol"
                        );
                        let reply = host.request(Request {
                            protocol: ORCHESTRATOR_VERSION,
                            id: signal.id.clone(),
                            expected_revision: None,
                            caller: Some(signal.delivery.source.clone()),
                            command: Command::Broadcast {
                                delivery: signal.delivery,
                            },
                        })?;
                        return Ok(serde_json::to_value(SignalReply {
                            signal_protocol: SIGNAL_VERSION,
                            id: signal.id,
                            revision: reply.revision,
                            accepted: reply
                                .result
                                .as_ref()
                                .and_then(|v| v["accepted"].as_bool())
                                .unwrap_or(false),
                            error: reply.error,
                        })?);
                    }
                    let request: Request = serde_json::from_value(value)
                        .context("Expected a current Orchestrator Protocol request")?;
                    ensure!(
                        request.protocol == ORCHESTRATOR_VERSION,
                        "Unsupported orchestrator protocol {}; expected {}",
                        request.protocol,
                        ORCHESTRATOR_VERSION
                    );
                    if let Command::Subscribe { after } = &request.command {
                        let notifier = host.clone();
                        let connection =
                            disconnect_signal(&stream, move || notifier.wake_subscribers())?;
                        let mut revision = *after;
                        while !connection.closed.load(std::sync::atomic::Ordering::Acquire) {
                            if let Some(after) = revision
                                && host.wait_connected(after, &connection.closed).is_none()
                            {
                                break;
                            }
                            let mut snapshot = request.clone();
                            snapshot.command = Command::Snapshot;
                            snapshot.expected_revision = None;
                            let reply = host.request(snapshot)?;
                            revision = Some(reply.revision);
                            writeln!(stream, "{}", serde_json::to_string(&reply)?)?;
                        }
                        return Ok(Value::Null);
                    }
                    Ok(serde_json::to_value(host.request(request)?)?)
                })();
                let envelope = match result {
                    Ok(value) => serde_json::json!({"ok":value}),
                    Err(e) => serde_json::json!({"error":format!("{e:#}")}),
                };
                let _ = writeln!(stream, "{envelope}");
            });
        }
    });
}
pub fn send(path: &Path, command: &impl serde::Serialize) -> Result<Value> {
    send_timeout(path, command, Duration::from_secs(6))
}
pub fn send_timeout(
    path: &Path,
    command: &impl serde::Serialize,
    timeout: Duration,
) -> Result<Value> {
    let mut stream = UnixStream::connect(path).with_context(|| {
        format!(
            "FlowWM is not running. Start `flowwm start` first (socket: {}).",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{}", serde_json::to_string(command)?)?;
    let mut reply = String::new();
    BufReader::new(stream)
        .take(32 * 1024 * 1024 + 1)
        .read_line(&mut reply)?;
    ensure!(
        reply.len() <= 32 * 1024 * 1024,
        "Runtime response is too large"
    );
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
