use super::*;
use vox_core::{
    opt::OptimizationLevel,
    source::SourceText,
    value::{HandleData, InlineValue, RuntimeValue},
};
use vox_runtime::Runtime;

pub(super) fn vox_expression(
    source: &str,
    bindings: &Record,
    types: &BTreeMap<String, Type>,
    state: &State,
) -> Result<Value> {
    let mut text = "script flowwm.binding;\n".to_string();
    for (name, value) in bindings {
        validate_name(name)?;
        text.push_str(&format!(
            "val {name} = {};\n",
            literal(value, types.get(name), state)?
        ));
    }
    text.push_str(source);
    let mut runtime = Runtime::default();
    let artifact = runtime.load_script(
        SourceText::new("flowwm/binding.vox", 1, text),
        Some(OptimizationLevel::NOpt),
    )?;
    let value = runtime.run_script(artifact, &[])?;
    runtime_json(&runtime, value)
}

fn literal(value: &Value, ty: Option<&Type>, state: &State) -> Result<String> {
    if let Some(Type::Named { name }) = ty {
        let representation = &state
            .types
            .iter()
            .find(|t| t.name == *name)
            .context("Unknown type")?
            .representation;
        return literal(value, Some(representation), state);
    }
    if let Some(ty @ (Type::UInt | Type::UInt8)) = ty {
        check_value(state, value, ty)?;
        return Ok(format!(
            "{value}{}",
            if *ty == Type::UInt8 { "u8" } else { "u" }
        ));
    }
    Ok(match value {
        Value::Null => "null".into(),
        Value::Number(n) if n.is_u64() && !n.is_i64() => format!("{n}u"),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        Value::String(s) => {
            let mut literal = String::from("\"");
            for ch in s.chars() {
                match ch {
                    '"' => literal.push_str("\\\""),
                    '\\' => literal.push_str("\\\\"),
                    '$' => literal.push_str("\\$"),
                    '\n' => literal.push_str("\\n"),
                    '\r' => literal.push_str("\\r"),
                    '\t' => literal.push_str("\\t"),
                    c if c.is_control() => literal.push_str(&format!("\\u{{{:x}}}", c as u32)),
                    c => literal.push(c),
                }
            }
            literal.push('"');
            literal
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|v| literal(
                    v,
                    match ty {
                        Some(Type::List { item }) => Some(item.as_ref()),
                        _ => None,
                    },
                    state
                ))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Object(values) => {
            let mut fields = vec![];
            for (name, value) in values {
                validate_name(name)?;
                let field_type = match ty {
                    Some(Type::Record { fields, .. } | Type::Signal { args: fields, .. }) => {
                        fields.get(name)
                    }
                    _ => None,
                };
                fields.push(format!("{name} = {}", literal(value, field_type, state)?));
            }
            if fields.is_empty() {
                "{}".into()
            } else {
                format!("{{ {}, }}", fields.join(", "))
            }
        }
    })
}
fn runtime_json(runtime: &Runtime, value: RuntimeValue) -> Result<Value> {
    Ok(match value {
        RuntimeValue::Handle(handle) | RuntimeValue::Inline(InlineValue::Handle(handle)) => {
            data_json(
                runtime
                    .get_handle_data(handle)
                    .map_err(anyhow::Error::msg)?,
            )?
        }
        RuntimeValue::Inline(InlineValue::Int(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::UInt(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::Integer(v)) => json!(v.number()),
        RuntimeValue::Inline(InlineValue::Float(v)) => {
            ensure!(v.is_finite(), "Non-finite Vox output");
            json!(v)
        }
        RuntimeValue::Inline(InlineValue::Bool(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::String(v)) => json!(v),
        RuntimeValue::Inline(InlineValue::Null) => Value::Null,
        RuntimeValue::Inline(InlineValue::Tuple(v)) if v.is_empty() => json!({}),
        RuntimeValue::Inline(InlineValue::Tuple(v)) => Value::Array(
            v.into_iter()
                .map(|v| runtime_json(runtime, RuntimeValue::Inline(v)))
                .collect::<Result<_>>()?,
        ),
        RuntimeValue::Inline(InlineValue::Record(v)) => Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, runtime_json(runtime, RuntimeValue::Inline(v))?)))
                .collect::<Result<_>>()?,
        ),
    })
}
fn data_json(value: HandleData) -> Result<Value> {
    Ok(match value {
        HandleData::Int(v) => json!(v),
        HandleData::UInt(v) => json!(v),
        HandleData::Integer(v) => json!(v.number()),
        HandleData::Float(v) => {
            ensure!(v.is_finite(), "Non-finite Vox output");
            json!(v)
        }
        HandleData::Bool(v) => json!(v),
        HandleData::String(v) => json!(v),
        HandleData::Null => Value::Null,
        HandleData::Tuple(v) if v.is_empty() => json!({}),
        HandleData::List(v) | HandleData::Tuple(v) => {
            Value::Array(v.into_iter().map(data_json).collect::<Result<_>>()?)
        }
        HandleData::Record(v) => Value::Object(
            v.into_iter()
                .map(|(k, v)| Ok((k, data_json(v)?)))
                .collect::<Result<_>>()?,
        ),
    })
}
