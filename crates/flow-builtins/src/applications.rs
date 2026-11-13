//! Application launch requests cross into the desktop without owning its windows.
use anyhow::{Context, Result};
use flow_core::{domain::ExecutionStatus, providers::*};
use serde_json::json;

pub trait Launcher: Send + Sync {
    fn launch(&self, invocation: &Invocation) -> Result<()>;
}

pub fn invoke(
    invocation: Invocation,
    environment: &crate::local::Environment,
    events: EventSink,
) -> Result<String> {
    let launcher = environment
        .applications
        .clone()
        .context("Desktop application launcher is unavailable")?;
    let handle = invocation.id.0.clone();
    std::thread::spawn(move || {
        let result = launcher.launch(&invocation);
        events(Completion {
            invocation: invocation.id,
            generation: invocation.generation,
            status: if result.is_ok() {
                ExecutionStatus::Complete
            } else {
                ExecutionStatus::Failed
            },
            result: result
                .map(|_| json!({}))
                .unwrap_or_else(|e| json!({"error":e.to_string()})),
        });
    });
    Ok(handle)
}
