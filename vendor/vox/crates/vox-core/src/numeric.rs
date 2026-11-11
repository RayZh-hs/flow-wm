use crate::{
    integer::{IntegerType, IntegerValue},
    value::InlineValue,
};

/// Returns None for nonnumeric operations, allowing each executor's other values.
pub fn binary(
    op: &str,
    left: &InlineValue,
    right: &InlineValue,
    skip: bool,
) -> Option<Result<InlineValue, String>> {
    if let (Some(a), Some(b)) = (left.integer(), right.integer()) {
        if matches!(op, "equal" | "not_equal") {
            let equal = a.ty().common(b.ty()).is_some() && a.number() == b.number();
            return Some(Ok(InlineValue::Bool(if op == "equal" {
                equal
            } else {
                !equal
            })));
        }
        if matches!(op, "less" | "less_equal" | "greater" | "greater_equal") {
            if a.ty().common(b.ty()).is_none() {
                return Some(Err(
                    "ordered comparison requires explicit signed/unsigned conversion".to_owned(),
                ));
            }
            return Some(Ok(InlineValue::Bool(compare(
                op,
                a.number().cmp(&b.number()),
            ))));
        }
        if matches!(op, "add" | "subtract" | "multiply" | "divide" | "remainder") {
            return Some(a.arithmetic(op, b, skip).map(InlineValue::from_integer));
        }
    }
    let (a, b) = match (left, right) {
        (InlineValue::Float(a), InlineValue::Float(b)) => (*a, *b),
        (_, InlineValue::Float(b)) if left.integer().is_some() => (left.integer()?.as_float(), *b),
        (InlineValue::Float(a), _) if right.integer().is_some() => {
            (*a, right.integer()?.as_float())
        }
        _ => return None,
    };
    Some(Ok(match op {
        "add" => InlineValue::Float(a + b),
        "subtract" => InlineValue::Float(a - b),
        "multiply" => InlineValue::Float(a * b),
        "divide" => InlineValue::Float(a / b),
        "remainder" => InlineValue::Float(a % b),
        "less" | "less_equal" | "greater" | "greater_equal" => {
            InlineValue::Bool(a.partial_cmp(&b).is_some_and(|order| compare(op, order)))
        }
        _ => return None,
    }))
}

fn compare(op: &str, order: std::cmp::Ordering) -> bool {
    match op {
        "less" => order.is_lt(),
        "less_equal" => !order.is_gt(),
        "greater" => order.is_gt(),
        "greater_equal" => !order.is_lt(),
        _ => false,
    }
}

pub fn unary(op: &str, value: &InlineValue, skip: bool) -> Option<Result<InlineValue, String>> {
    (op == "negate")
        .then(|| {
            value
                .integer()
                .map(|v| v.negate(skip).map(InlineValue::from_integer))
        })
        .flatten()
}

pub fn builtin(value: &InlineValue, method: &str) -> Option<InlineValue> {
    let integer = value.integer();
    if integer.is_none() && !matches!(value, InlineValue::Float(_)) {
        return None;
    }
    if let Some(ty) = method.strip_prefix("to").and_then(IntegerType::from_name) {
        let converted = if let Some(integer) = integer {
            integer.convert(ty)
        } else if let InlineValue::Float(value) = value {
            IntegerValue::from_float(ty, *value)
        } else {
            None
        };
        return Some(
            converted
                .map(InlineValue::from_integer)
                .unwrap_or(InlineValue::Null),
        );
    }
    match (method, integer) {
        ("toString", Some(integer)) => Some(InlineValue::String(integer.number().to_string())),
        ("toFloat", Some(integer)) => Some(InlineValue::Float(integer.as_float())),
        ("toFloat", None) => Some(value.clone()),
        ("toString", None) => {
            if let InlineValue::Float(value) = value {
                Some(InlineValue::String(value.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}
