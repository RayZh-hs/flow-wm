//! Bridge invoked application nodes to the owning host's desktop service.
use anyhow::Result;
use flow_core::providers::Invocation;
use serde_json::json;
use std::path::PathBuf;

pub struct Desktop(pub PathBuf);

pub fn socket() -> PathBuf {
    std::env::var_os("FLOWWM_DESKTOP_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::ipc::default_socket().with_file_name("flowwm-desktop.sock"))
}

impl flow_builtins::applications::Launcher for Desktop {
    fn launch(&self, invocation: &Invocation) -> Result<()> {
        crate::ipc::send(
            &self.0,
            &json!({"desktop_protocol":1,"command":"launch","node":invocation.node,"invocation":invocation}),
        )?;
        Ok(())
    }
}
