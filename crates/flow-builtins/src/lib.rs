//! Builtin platform providers, kept outside the portable core.
pub mod environment;
pub mod python;
pub mod row_settings;
pub mod worker;
use anyhow::Result;
use flow_core::{
    domain::{FlowNet, NodeSpec},
    graph::*,
};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};
pub struct Scripts(pub local::Limits);
impl ScriptEvaluator for Scripts {
    fn evaluate(
        &self,
        node: &NodeSpec,
        bindings: &Record,
        args: &Value,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        python::python(node, bindings, args, cancel, &self.0)
    }
}
pub fn evaluate(
    state: FlowNet,
    context: EvaluationContext,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
) -> Result<Evaluation> {
    evaluate_with_limits(
        state,
        context,
        root,
        signal,
        cancel,
        local::Limits::default(),
    )
}
pub fn evaluate_with_limits(
    state: FlowNet,
    context: EvaluationContext,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
    limits: local::Limits,
) -> Result<Evaluation> {
    flow_core::scheduler::evaluate(
        state,
        context,
        root,
        signal,
        cancel,
        local::registry(limits.clone())?,
        Arc::new(Scripts(limits)),
    )
}
pub mod agent;
pub mod applications;
pub mod external;
pub mod local;
pub mod process;
pub mod sessions;
pub mod sources;
