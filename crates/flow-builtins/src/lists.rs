//! Pure list templates. Pin specialization is shared by the runtime and editor.
use anyhow::{Context, Result, bail, ensure};
use flow_core::{graph::*, providers::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const NODES: &[(&str, &str, &str)] = &[
    (
        "list_create",
        "Make list",
        "Build an ordered typed list from a variable number of inputs",
    ),
    (
        "list_append",
        "Append to list",
        "Append one item to a typed list",
    ),
    (
        "list_concat",
        "Concatenate lists",
        "Combine two lists of the same item type",
    ),
    (
        "list_get",
        "List item",
        "Read the item at a zero-based index",
    ),
    (
        "list_set",
        "Set list item",
        "Replace the item at a zero-based index",
    ),
    (
        "list_remove",
        "Remove list item",
        "Remove the item at a zero-based index",
    ),
    ("list_length", "List length", "Count the items in a list"),
    (
        "list_slice",
        "Slice list",
        "Take a range starting at a zero-based index",
    ),
    (
        "list_contains",
        "List contains",
        "Test whether a list contains an item",
    ),
    (
        "list_reverse",
        "Reverse list",
        "Reverse the order of a typed list",
    ),
];

pub fn definitions() -> Vec<Definition> {
    NODES
        .iter()
        .map(|(id, _, _)| {
            let list = Type::List {
                item: Box::new(Type::Any),
            };
            let mut inputs = vec![];
            let mut properties = BTreeMap::new();
            if *id == "list_create" {
                properties.insert(
                    "input_count".into(),
                    Property {
                        ty: Type::UInt,
                        default: Some(json!(2)),
                        output: false,
                    },
                );
            } else {
                inputs.push(Pin::new("list", list.clone()));
            }
            if matches!(*id, "list_append" | "list_set" | "list_contains") {
                inputs.push(Pin {
                    property: Some("item".into()),
                    ..Pin::new("item", Type::Any)
                });
                properties.insert(
                    "item".into(),
                    Property {
                        ty: Type::Any,
                        default: None,
                        output: false,
                    },
                );
            }
            if *id == "list_concat" {
                inputs.push(Pin::new("other", list.clone()));
            }
            for name in match *id {
                "list_get" | "list_set" | "list_remove" => vec!["index"],
                "list_slice" => vec!["start", "count"],
                _ => vec![],
            } {
                inputs.push(Pin {
                    property: Some(name.into()),
                    default: Some(json!(0)),
                    ..Pin::new(name, Type::UInt)
                });
                properties.insert(
                    name.into(),
                    Property {
                        ty: Type::UInt,
                        default: Some(json!(0)),
                        output: false,
                    },
                );
            }
            let ty = match *id {
                "list_get" => Type::Any,
                "list_length" => Type::UInt,
                "list_contains" => Type::Bool,
                _ => list,
            };
            Definition {
                id: format!("local.{id}"),
                owner: "local".into(),
                behavior: Behavior::Lazy,
                inputs,
                outputs: vec![Pin::new("value", ty)],
                properties,
                capabilities: Default::default(),
                window: None,
                renderer: None,
            }
        })
        .collect()
}

pub fn read(invocation: &Invocation) -> Result<Value> {
    let inputs = &invocation.properties["data_inputs"];
    let kind = invocation
        .type_id
        .strip_prefix("local.")
        .unwrap_or(&invocation.type_id);
    if kind == "list_create" {
        let count = invocation
            .properties
            .get("input_count")
            .and_then(Value::as_u64)
            .unwrap_or(2);
        ensure!(count <= 256, "List supports at most 256 inputs");
        let items = (1..=count)
            .map(|i| {
                inputs
                    .get(format!("item_{i}"))
                    .cloned()
                    .context("Missing list item")
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(json!({"value": items}));
    }
    let mut list = inputs["list"]
        .as_array()
        .context("Expected a list")?
        .clone();
    let index = |name: &str| -> Result<usize> {
        usize::try_from(
            inputs[name]
                .as_u64()
                .context("Expected an unsigned index")?,
        )
        .context("Index is too large")
    };
    let value = match kind {
        "list_length" => json!(list.len()),
        "list_contains" => json!(list.contains(&inputs["item"])),
        "list_get" => list
            .get(index("index")?)
            .context("List index out of bounds")?
            .clone(),
        "list_append" => {
            list.push(inputs["item"].clone());
            json!(list)
        }
        "list_concat" => {
            list.extend(
                inputs["other"]
                    .as_array()
                    .context("Expected a list")?
                    .iter()
                    .cloned(),
            );
            json!(list)
        }
        "list_set" | "list_remove" => {
            let i = index("index")?;
            ensure!(i < list.len(), "List index out of bounds");
            if kind == "list_set" {
                list[i] = inputs["item"].clone();
            } else {
                list.remove(i);
            }
            json!(list)
        }
        "list_slice" => {
            let start = index("start")?;
            ensure!(start <= list.len(), "Slice start out of bounds");
            json!(&list[start..start.saturating_add(index("count")?).min(list.len())])
        }
        "list_reverse" => {
            list.reverse();
            json!(list)
        }
        _ => bail!("Unknown list operation"),
    };
    Ok(json!({"value":value}))
}
