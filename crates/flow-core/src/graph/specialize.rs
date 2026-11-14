//! Resolve template bindings from connections without persisting inferred schemas.
use super::{Connection, Pin, Type};
use std::collections::{BTreeMap, HashSet};

pub fn specialized_pins(
    id: &str,
    output: bool,
    connections: &[Connection],
    declared: &impl Fn(&str, bool) -> Vec<Pin>,
    kind: &impl Fn(&str) -> String,
    source: &impl Fn(&str) -> String,
) -> Vec<Pin> {
    resolve(
        id,
        output,
        connections,
        declared,
        kind,
        source,
        &mut HashSet::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve(
    id: &str,
    output: bool,
    connections: &[Connection],
    declared: &impl Fn(&str, bool) -> Vec<Pin>,
    kind: &impl Fn(&str) -> String,
    source: &impl Fn(&str) -> String,
    active: &mut HashSet<(String, bool)>,
) -> Vec<Pin> {
    let mut pins = declared(id, output);
    let key = (id.to_owned(), output);
    if !active.insert(key.clone()) {
        return pins;
    }
    let mut incoming = |name: &str| {
        let types: Vec<_> = connections
            .iter()
            .filter(|edge| edge.target.node == id && edge.target.pin == name)
            .filter_map(|edge| {
                resolve(
                    &edge.source.node,
                    true,
                    connections,
                    declared,
                    kind,
                    source,
                    active,
                )
                .into_iter()
                .find(|pin| pin.name == edge.source.pin)
                .map(|pin| pin.ty)
            })
            .collect();
        let first = types.first()?.clone();
        if types.iter().all(|ty| *ty == first) {
            return Some(first);
        }
        // A fan-in guarantees only fields shared by every incoming event.
        if let Type::Signal { mut args, .. } = first {
            args.retain(|name, ty| types.iter().all(|other| matches!(other, Type::Signal { args, .. } if args.get(name) == Some(ty))));
            Some(Type::Signal { args, open: true })
        } else {
            Some(Type::Any)
        }
    };
    let node_kind = kind(id);
    let node_kind = node_kind.strip_prefix("local.").unwrap_or(&node_kind);
    let signal = if matches!(
        node_kind,
        "break_signal" | "merge_signal" | "conditional" | "alert" | "vox" | "python"
    ) {
        incoming("in")
    } else {
        None
    };
    let value = if node_kind == "display" {
        declared(id, false)
            .into_iter()
            .find(|pin| pin.name == "value" && pin.ty != Type::Any)
            .map(|pin| pin.ty)
            .or_else(|| incoming("value").filter(|ty| !ty.is_signal()))
    } else {
        None
    };
    let record = if node_kind == "merge_signal" {
        incoming("args").or_else(|| {
            declared(id, false)
                .into_iter()
                .find(|pin| pin.name == "args")
                .map(|pin| pin.ty)
        })
    } else {
        None
    };
    for pin in &mut pins {
        let inferred = match (node_kind, output, pin.name.as_str()) {
            ("display", _, "value") => value.clone(),
            ("break_signal", false, "in") => signal.clone(),
            ("break_signal", true, "args") => signal.as_ref().and_then(|ty| match ty {
                Type::Signal { args, open } => Some(Type::Record {
                    fields: args.clone(),
                    open: *open,
                }),
                _ => None,
            }),
            ("conditional" | "alert" | "vox" | "python", false, "in")
            | ("conditional", true, "true" | "false")
            | ("alert" | "vox" | "python", true, "out") => signal.clone(),
            ("merge_signal", false, "in") => signal.clone(),
            ("merge_signal", false, "args") => record.clone(),
            ("merge_signal", true, "out") => {
                let (mut args, mut open) = match &signal {
                    Some(Type::Signal { args, open }) => (args.clone(), *open),
                    _ => (BTreeMap::new(), true),
                };
                if let Some(Type::Record {
                    fields,
                    open: record_open,
                }) = &record
                {
                    args.extend(fields.clone());
                    open |= record_open;
                }
                Some(Type::Signal { args, open })
            }
            (_, false, _) if pin.ty == Type::Any => {
                incoming(&pin.name).filter(|ty| !ty.is_signal())
            }
            _ => None,
        };
        if let Some(ty) = inferred {
            pin.ty = ty;
            // A connected template's old placeholder is not a typed default.
            if connections
                .iter()
                .any(|edge| edge.target.node == id && edge.target.pin == pin.name)
                && !output
            {
                pin.default = None;
            }
        }
    }
    if output && node_kind == "vox" {
        let inputs = resolve(id, false, connections, declared, kind, source, active);
        if let Some(fields) = super::vox_outputs(&source(id), &inputs) {
            pins.retain(|pin| {
                pin.name != "value" || pin.ty != Type::Any || fields.contains_key("value")
            });
            for (name, ty) in fields {
                if let Some(pin) = pins.iter_mut().find(|pin| pin.name == name) {
                    if pin.ty == Type::Any {
                        pin.ty = ty;
                    }
                } else {
                    pins.push(Pin::new(&name, ty));
                }
            }
        }
    }
    active.remove(&key);
    pins
}
