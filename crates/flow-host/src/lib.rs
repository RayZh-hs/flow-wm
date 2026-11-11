//! Independent runtime host and compatibility adapters for FlowWM clients.
pub use flow_core::model;
pub mod agent;
pub mod catalog;
pub mod config;
pub mod engine;
pub mod extensions;
pub mod groups;
pub mod ipc;
pub mod niri;
pub mod performance;
mod processes;
pub mod store;
pub mod graph {
    pub use flow_builtins::evaluate;
    pub use flow_builtins::worker::run_worker;
    pub use flow_core::graph::*;
    pub fn evaluate_isolated(
        state: crate::model::State,
        root: &str,
        signal: Option<(String, serde_json::Value)>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> anyhow::Result<Evaluation> {
        let executable = crate::config::worker_executable()?;
        flow_builtins::worker::evaluate_isolated(
            &executable,
            &crate::config::runtime_limits()?,
            state,
            root,
            signal,
            cancel,
        )
    }
}
pub mod desktop_provider;
mod projection;
pub mod remote;
