//! Builtin platform providers, kept outside the portable core.
pub mod python;
pub mod worker;
use anyhow::Result;
use flow_core::{
    graph::*,
    model::{Node, State},
};
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};
pub struct Scripts;
impl ScriptEvaluator for Scripts {
    fn evaluate(
        &self,
        node: &Node,
        bindings: &Record,
        args: &Value,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        python::python(node, bindings, args, cancel)
    }
}
pub fn evaluate(
    state: State,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
) -> Result<Evaluation> {
    evaluate_with(state, root, signal, cancel, Arc::new(Scripts))
}
pub mod agent;
pub mod local;
pub mod process;
