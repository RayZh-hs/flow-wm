//! Variable input groups use stable numbered names, preserving existing wires.
use super::{Pin, Type};
use crate::domain::NodeSpec;
use anyhow::{Result, ensure};
use serde_json::Value;

pub fn group(kind: &str) -> Option<(&'static str, Type)> {
    match kind.strip_prefix("local.").unwrap_or(kind) {
        "list_create" => Some(("item", Type::Any)),
        "on_all_true" => Some(("condition", Type::Bool)),
        "on_all_fired" => Some((
            "signal",
            Type::Signal {
                args: Default::default(),
                open: true,
            },
        )),
        _ => None,
    }
}

pub fn count(node: &NodeSpec) -> usize {
    node.flow
        .properties
        .get("input_count")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .min(256) as usize
}

pub fn validate(node: &NodeSpec) -> Result<()> {
    if group(&node.type_id).is_some() {
        let count = node
            .flow
            .properties
            .get("input_count")
            .and_then(Value::as_u64)
            .unwrap_or(2);
        ensure!(count <= 256, "Input count must be between 0 and 256");
        ensure!(
            count > 0 || super::type_id(node) == "list_create",
            "A condition or signal barrier needs at least one input"
        );
    }
    if node.type_id.starts_with("local.list_")
        && let Some(value) = node.flow.properties.get("item_type")
    {
        let ty: Type = serde_json::from_value(value.clone())?;
        ensure!(!ty.is_signal(), "List items must carry data");
    }
    Ok(())
}

pub fn pins(node: &NodeSpec, pins: &mut Vec<Pin>) {
    if let Some((prefix, mut ty)) = group(&node.type_id) {
        if prefix == "item" {
            ty = node
                .flow
                .properties
                .get("item_type")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(Type::Any);
        }
        for i in 1..=count(node) {
            let name = format!("{prefix}_{i}");
            let ty = if ty == Type::Any {
                node.flow
                    .properties
                    .get(&name)
                    .map(super::infer_type)
                    .unwrap_or(Type::Any)
            } else {
                ty.clone()
            };
            pins.push(Pin {
                property: (!ty.is_signal()).then_some(name.clone()),
                ..Pin::new(&name, ty)
            });
        }
    }
}
