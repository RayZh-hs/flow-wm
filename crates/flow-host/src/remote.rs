//! Configured remote transport. SSH uses argv and quoted arguments, never interpolated commands.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum Remote {
    Unix {
        socket: PathBuf,
    },
    Ssh {
        destination: String,
        #[serde(default = "default_command")]
        command: Vec<String>,
    },
}
fn default_command() -> Vec<String> {
    vec!["flow-core".into(), "--stdio".into()]
}
pub fn configured() -> Result<BTreeMap<String, Remote>> {
    let path = crate::config::directory().join("runtime.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(serde_json::from_value(
        value
            .get("hosts")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )?)
}
impl Remote {
    pub fn request(&self, request: &flow_core::protocol::Request) -> Result<Value> {
        let reply = self.exchange(request)?;
        if !reply["error"].is_null() {
            anyhow::bail!("Remote host: {}", reply["error"]["message"]);
        }
        Ok(reply["result"].clone())
    }
    pub fn control(&self, control: &crate::engine::Control) -> Result<Value> {
        self.exchange(control)
    }
    fn exchange(&self, request: &impl Serialize) -> Result<Value> {
        let reply = match self {
            Self::Unix { socket } => crate::ipc::send_timeout(
                socket,
                request,
                Duration::from_secs(crate::config::runtime_limits()?.remote_timeout_secs),
            )?,
            Self::Ssh {
                destination,
                command,
            } => {
                ensure!(
                    !destination.is_empty()
                        && !destination.starts_with('-')
                        && !destination.chars().any(char::is_whitespace),
                    "Invalid SSH destination"
                );
                ensure!(!command.is_empty(), "Remote host command is empty");
                let quoted = command
                    .iter()
                    .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut child = Command::new("ssh")
                    .args([
                        "-T",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=5",
                        "--",
                        destination,
                        &quoted,
                    ])
                    .process_group(0)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .context("Connect remote Flow host")?;
                let mut input = child.stdin.take().unwrap();
                writeln!(input, "{}", serde_json::to_string(request)?)?;
                drop(input);
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let out = thread::spawn(move || {
                    let mut bytes = vec![];
                    let _ = stdout.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes);
                    bytes
                });
                let err = thread::spawn(move || {
                    let mut bytes = vec![];
                    let _ = stderr.take(65536).read_to_end(&mut bytes);
                    bytes
                });
                let start = Instant::now();
                let timeout =
                    Duration::from_secs(crate::config::runtime_limits()?.remote_timeout_secs);
                let status = loop {
                    if let Some(status) = child.try_wait()? {
                        break status;
                    }
                    if start.elapsed() > timeout {
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                        let _ = child.wait();
                        anyhow::bail!("Remote host disconnected or timed out");
                    }
                    thread::sleep(Duration::from_millis(20));
                };
                let bytes = out.join().unwrap_or_default();
                let stderr = err.join().unwrap_or_default();
                ensure!(
                    status.success(),
                    "Remote host unavailable: {}",
                    String::from_utf8_lossy(&stderr)
                );
                ensure!(
                    bytes.len() <= 32 * 1024 * 1024,
                    "Remote snapshot is too large"
                );
                let envelope: Value = serde_json::from_slice(&bytes)?;
                if let Some(error) = envelope.get("error") {
                    anyhow::bail!("Remote host: {error}");
                }
                envelope["ok"].clone()
            }
        };
        Ok(reply)
    }
}
pub fn call(
    host: &str,
    command: flow_core::protocol::Command,
    request_id: String,
) -> Result<Value> {
    let remotes = configured()?;
    let remote = remotes.get(host).context("Remote host is not configured")?;
    remote.request(&flow_core::protocol::Request {
        protocol: 2,
        id: request_id,
        expected_revision: None,
        caller: None,
        command,
    })
}
pub fn stdio(socket: &std::path::Path) -> Result<()> {
    use std::io::BufRead;
    let mut input = String::new();
    std::io::stdin()
        .lock()
        .take(1024 * 1024)
        .read_line(&mut input)?;
    ensure!(input.ends_with('\n'), "Incomplete remote request");
    let request: Value = serde_json::from_str(&input)?;
    let reply = crate::ipc::send(socket, &request)?;
    println!("{}", serde_json::json!({"ok":reply}));
    Ok(())
}
