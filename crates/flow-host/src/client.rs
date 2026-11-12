//! Rust client of the current public protocol.
use anyhow::{Result, bail, ensure};
use flow_core::{domain::RuntimeState, protocol::*};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};
#[derive(Clone)]
pub struct Client {
    pub socket: PathBuf,
}
impl Client {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
    pub fn request(&self, request: &Request) -> Result<Reply> {
        let reply: Reply = serde_json::from_value(crate::ipc::send(&self.socket, request)?)?;
        ensure!(
            reply.protocol == ORCHESTRATOR_VERSION && reply.id == request.id,
            "Invalid orchestrator response identity or version"
        );
        Ok(reply)
    }
    pub fn command(&self, command: Command) -> Result<Value> {
        self.command_as(command, None)
    }
    pub fn command_as(
        &self,
        command: Command,
        caller: Option<flow_core::domain::WorkflowId>,
    ) -> Result<Value> {
        let reply = self.request(&Request {
            protocol: ORCHESTRATOR_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            expected_revision: None,
            caller,
            command,
        })?;
        if let Some(error) = reply.error {
            bail!("{}: {}", error.code, error.message);
        }
        Ok(reply.result.unwrap_or(Value::Null))
    }
    pub fn snapshot_as(
        &self,
        caller: Option<flow_core::domain::WorkflowId>,
    ) -> Result<RuntimeState> {
        Ok(serde_json::from_value(
            self.command_as(Command::Snapshot, caller)?,
        )?)
    }
    pub fn snapshot(&self) -> Result<RuntimeState> {
        Ok(serde_json::from_value(self.command(Command::Snapshot)?)?)
    }
    pub fn subscribe(&self, receive: impl FnMut(Reply) -> Result<()>) -> Result<()> {
        self.subscribe_connected(|_| Ok(()), receive)
    }
    pub fn subscribe_connected(
        &self,
        connected: impl FnOnce(&UnixStream) -> Result<()>,
        mut receive: impl FnMut(Reply) -> Result<()>,
    ) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket)?;
        connected(&stream)?;
        let request = Request {
            protocol: ORCHESTRATOR_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            expected_revision: None,
            caller: None,
            command: Command::Subscribe { after: None },
        };
        writeln!(stream, "{}", serde_json::to_string(&request)?)?;
        let mut stream = BufReader::new(stream);
        loop {
            let mut line = Vec::new();
            (&mut stream)
                .take(32 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            ensure!(!line.is_empty(), "Host subscription disconnected");
            ensure!(
                line.len() <= 32 * 1024 * 1024 && line.last() == Some(&b'\n'),
                "Incomplete or oversized host subscription frame"
            );
            let reply: Reply = serde_json::from_slice(&line)?;
            ensure!(
                reply.protocol == ORCHESTRATOR_VERSION && reply.id == request.id,
                "Unsupported or mismatched host subscription reply"
            );
            receive(reply)?;
        }
    }
}
