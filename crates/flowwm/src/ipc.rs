//! WM services have their own versioned envelope; runtime requests use the host.
use crate::{
    catalog::Catalog,
    client::{Action, DesktopClient},
    extensions::Macro,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
};
pub fn default_socket() -> PathBuf {
    flow_host::ipc::default_socket()
}
pub fn bind(path: &Path) -> Result<UnixListener> {
    flow_host::ipc::bind(path)
}
pub fn send(path: &Path, command: &impl serde::Serialize) -> Result<Value> {
    let command = serde_json::to_value(command)?;
    flow_host::ipc::send(path, &json!({"wm_protocol":1,"command":command}))
}
pub fn serve(listener: UnixListener, client: DesktopClient, macros: Vec<Macro>) {
    let macros = std::sync::Arc::new(macros);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let client = client.clone();
            let macros = macros.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let result = (|| -> Result<Value> {
                    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
                    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
                    let mut line = Vec::new();
                    BufReader::new(stream.try_clone()?)
                        .take(1024 * 1024 + 1)
                        .read_until(b'\n', &mut line)?;
                    ensure!(
                        line.len() <= 1024 * 1024 && line.last() == Some(&b'\n'),
                        "Incomplete WM request"
                    );
                    let request: Value = serde_json::from_slice(&line)?;
                    ensure!(
                        request["wm_protocol"] == 1,
                        "Unsupported WM service protocol"
                    );
                    let value = &request["command"];
                    match value["action"].as_str() {
                        Some("view") => {
                            let view = client.snapshot();
                            let state = &view.state;
                            Ok(
                                json!({"workflows":state.workspaces,"selected_workflow":state.selected_workspace,"nodes":state.nodes,"connections":state.connections}),
                            )
                        }
                        Some("recipes") => {
                            Ok(serde_json::to_value(&client.snapshot().state.recipes)?)
                        }
                        Some("catalog") => Ok(serde_json::to_value(Catalog::new(
                            &client.snapshot(),
                            client.presentation(),
                            &macros,
                        ))?),
                        Some("performance") => {
                            if value["reset"] == true {
                                client.performance.reset();
                            }
                            Ok(serde_json::to_value(client.performance.snapshot())?)
                        }
                        Some("subscribe") => {
                            let notifier = client.clone();
                            let connection =
                                flow_host::ipc::disconnect_signal(&stream, move || {
                                    notifier.wake_ui()
                                })?;
                            let launcher =
                                (value["launcher"] == true).then(|| client.connect_launcher());
                            let mut generation = 0;
                            let mut previous = String::new();
                            while let Some(next) = client.wait_for_change(generation) {
                                if connection.closed.load(std::sync::atomic::Ordering::Acquire) {
                                    break;
                                }
                                generation = next;
                                let catalog = serde_json::to_string(&Catalog::new(
                                    &client.snapshot(),
                                    client.presentation(),
                                    &macros,
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
                                            json!({"launcher":{"query":query}})
                                        )?;
                                    }
                                }
                            }
                            Ok(Value::Null)
                        }
                        Some("status") => anyhow::bail!(
                            "Use the Orchestrator Protocol snapshot command on flow-core.sock"
                        ),
                        _ => client.request(serde_json::from_value::<Action>(value.clone())?),
                    }
                })();
                let reply = match result {
                    Ok(value) => json!({"ok":value}),
                    Err(error) => json!({"error":format!("{error:#}")}),
                };
                let _ = writeln!(stream, "{reply}");
            });
        }
    });
}
