use super::*;
use anyhow::bail;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Type {
    Any,
    Bool,
    Int,
    #[serde(rename = "uint")]
    UInt,
    #[serde(rename = "uint8")]
    UInt8,
    Float,
    String,
    List {
        item: Box<Type>,
    },
    Record {
        #[serde(default)]
        fields: BTreeMap<String, Type>,
        #[serde(default)]
        open: bool,
    },
    Named {
        name: String,
    },
    Signal {
        #[serde(default)]
        args: BTreeMap<String, Type>,
        #[serde(default)]
        open: bool,
    },
}
impl Type {
    pub fn trigger() -> Self {
        Self::Signal {
            args: BTreeMap::new(),
            open: false,
        }
    }
    pub fn record() -> Self {
        Self::Record {
            fields: BTreeMap::new(),
            open: true,
        }
    }
    pub fn is_signal(&self) -> bool {
        matches!(self, Self::Signal { .. })
    }
}
impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Any => write!(f, "Any"),
            Self::Bool => write!(f, "Bool"),
            Self::Int => write!(f, "Int"),
            Self::UInt => write!(f, "UInt"),
            Self::UInt8 => write!(f, "UInt8"),
            Self::Float => write!(f, "Float"),
            Self::String => write!(f, "String"),
            Self::Named { name } => write!(f, "{name}"),
            Self::List { item } => write!(f, "List[{item}]"),
            Self::Record { fields, open } | Self::Signal { args: fields, open } => {
                if self.is_signal() {
                    write!(f, "Signal[")?;
                }
                write!(f, "{{")?;
                for (index, (name, ty)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{name}: {ty}")?;
                }
                if *open {
                    if !fields.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..")?;
                }
                write!(f, "}}")?;
                if self.is_signal() {
                    write!(f, "]")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TypeDefinition {
    pub name: String,
    pub representation: Type,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CastDefinition {
    pub from: Type,
    pub to: Type,
    /// Vox expression with a typed `value` binding.
    pub source: String,
}

pub fn validate_type(state: &State, ty: &Type, depth: usize) -> Result<()> {
    let signal = ty.is_signal();
    ensure!(depth < 32, "Recursive or deeply nested type");
    match ty {
        Type::Named { name } => {
            let def = state
                .types
                .iter()
                .find(|d| d.name == *name)
                .with_context(|| format!("Unknown type: {name}"))?;
            ensure!(
                !def.representation.is_signal(),
                "Named types must carry data"
            );
            validate_type(state, &def.representation, depth + 1)?;
        }
        Type::List { item } => {
            ensure!(!item.is_signal(), "Signals cannot be data values");
            validate_type(state, item, depth + 1)?;
        }
        Type::Record { fields, .. } | Type::Signal { args: fields, .. } => {
            let mut env = HashSet::new();
            for (name, ty) in fields {
                validate_name(name)?;
                ensure!(
                    !signal || env.insert(name.to_ascii_uppercase()),
                    "Argument names collide after uppercasing: {name}"
                );
                ensure!(!ty.is_signal(), "Record fields must carry data");
                validate_type(state, ty, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_registry(state: &State) -> Result<()> {
    let mut names = HashSet::new();
    for ty in &state.types {
        ensure!(
            !ty.representation.is_signal(),
            "Named types must carry data"
        );
        ensure!(
            !ty.name.is_empty() && names.insert(&ty.name),
            "Duplicate or empty type name"
        );
        validate_type(state, &ty.representation, 0)?;
    }
    let mut ids = HashSet::new();
    for node in &state.node_types {
        if let Some(window) = &node.window {
            window.validate()?;
        }
        ensure!(
            ![
                "constant",
                "display",
                "break_signal",
                "merge_signal",
                "conditional",
                "vox",
                "python",
                "window",
                "workflow"
            ]
            .contains(&node.id.as_str()),
            "Reserved node type: {}",
            node.id
        );
        ensure!(
            !node.id.is_empty() && ids.insert(&node.id),
            "Duplicate or empty node type"
        );
        for pins in [&node.inputs, &node.outputs] {
            let mut names = HashSet::new();
            for pin in pins {
                validate_name(&pin.name)?;
                ensure!(names.insert(&pin.name), "Duplicate pin");
                validate_type(state, &pin.ty, 0)?;
            }
        }
        for (name, prop) in &node.properties {
            validate_name(name)?;
            validate_type(state, &prop.ty, 0)?;
            ensure!(!prop.ty.is_signal(), "Properties must carry data");
            if let Some(value) = &prop.default {
                check_value(state, value, &prop.ty)?;
            }
        }
    }
    let mut casts = vec![];
    for cast in &state.casts {
        ensure!(
            !casts.contains(&(&cast.from, &cast.to)),
            "Duplicate cast from {} to {}",
            cast.from,
            cast.to
        );
        casts.push((&cast.from, &cast.to));
        validate_type(state, &cast.from, 0)?;
        validate_type(state, &cast.to, 0)?;
        ensure!(
            !cast.from.is_signal() && !cast.to.is_signal(),
            "Register casts between data types; signal casts are structural"
        );
    }
    Ok(())
}

pub fn check_value(state: &State, value: &Value, ty: &Type) -> Result<()> {
    let valid = match ty {
        Type::Any => true,
        Type::Bool => value.is_boolean(),
        Type::Int => value.as_i64().is_some(),
        Type::UInt => value.as_u64().is_some(),
        Type::UInt8 => value.as_u64().is_some_and(|v| u8::try_from(v).is_ok()),
        Type::Float => value.as_f64().is_some_and(f64::is_finite),
        Type::String => value.is_string(),
        Type::Named { name } => {
            check_value(
                state,
                value,
                &state
                    .types
                    .iter()
                    .find(|d| d.name == *name)
                    .context("Unknown type")?
                    .representation,
            )?;
            true
        }
        Type::List { item } => {
            if let Some(values) = value.as_array() {
                for v in values {
                    check_value(state, v, item)?;
                }
                true
            } else {
                false
            }
        }
        Type::Record { fields, open } | Type::Signal { args: fields, open } => {
            if let Some(values) = value.as_object() {
                for (name, ty) in fields {
                    check_value(
                        state,
                        values
                            .get(name)
                            .with_context(|| format!("Missing field: {name}"))?,
                        ty,
                    )?;
                }
                *open || values.len() == fields.len()
            } else {
                false
            }
        }
    };
    ensure!(valid, "Value {value} does not match {ty:?}");
    Ok(())
}

fn structural(state: &State, from: &Type, to: &Type, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    if from == to {
        return true;
    }
    match (from, to) {
        (Type::Any, t) | (t, Type::Any) => !t.is_signal(),
        (Type::Int | Type::UInt | Type::UInt8, Type::Float) | (Type::UInt8, Type::UInt) => true,
        (Type::List { item: a }, Type::List { item: b }) => structural(state, a, b, depth + 1),
        (
            Type::Record {
                fields: a,
                open: ao,
            },
            Type::Record { fields: b, .. },
        )
        | (Type::Signal { args: a, open: ao }, Type::Signal { args: b, .. }) => b
            .iter()
            .all(|(n, t)| a.get(n).is_some_and(|s| can_cast(state, s, t)) || *ao),
        _ => false,
    }
}

pub fn can_cast(state: &State, from: &Type, to: &Type) -> bool {
    structural(state, from, to, 0) || cast_path(state, from, to).is_some()
}

pub(super) fn cast_path<'a>(
    state: &'a State,
    from: &Type,
    to: &Type,
) -> Option<Vec<&'a CastDefinition>> {
    let mut queue = std::collections::VecDeque::from([(from.clone(), vec![])]);
    let mut seen = vec![from.clone()];
    while let Some((ty, path)) = queue.pop_front() {
        for cast in &state.casts {
            if cast.from == ty && !seen.contains(&cast.to) {
                let mut next = path.clone();
                next.push(cast);
                if structural(state, &cast.to, to, 0) {
                    return Some(next);
                }
                seen.push(cast.to.clone());
                queue.push_back((cast.to.clone(), next));
            }
        }
    }
    None
}

pub(super) fn cast_value(state: &State, value: Value, from: &Type, to: &Type) -> Result<Value> {
    if !structural(state, from, to, 0) {
        let path = cast_path(state, from, to).context("No registered cast")?;
        let mut value = value;
        let mut current = from;
        for cast in path {
            value = super::bindings::vox_expression(
                &cast.source,
                &BTreeMap::from([("value".into(), value)]),
                &BTreeMap::from([("value".into(), cast.from.clone())]),
                state,
            )?;
            check_value(state, &value, &cast.to)?;
            current = &cast.to;
        }
        return cast_value(state, value, current, to);
    }
    if let Type::Record { fields, open } | Type::Signal { args: fields, open } = to {
        let object = value.as_object().context("Expected a record")?;
        let source_fields = match from {
            Type::Record { fields, .. } | Type::Signal { args: fields, .. } => Some(fields),
            _ => None,
        };
        let mut result = if *open {
            object.clone()
        } else {
            serde_json::Map::new()
        };
        for (name, ty) in fields {
            let value = object
                .get(name)
                .with_context(|| format!("Missing field: {name}"))?
                .clone();
            result.insert(
                name.clone(),
                cast_value(
                    state,
                    value,
                    source_fields
                        .and_then(|f| f.get(name))
                        .unwrap_or(&Type::Any),
                    ty,
                )?,
            );
        }
        let result = Value::Object(result);
        check_value(state, &result, to)?;
        return Ok(result);
    }
    if let (Type::List { item: a }, Type::List { item: b }) = (from, to) {
        return value
            .as_array()
            .context("Expected a list")?
            .iter()
            .map(|v| cast_value(state, v.clone(), a, b))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }
    if *to == Type::Float
        && let Some(value) = value.as_f64()
    {
        return Ok(json!(value));
    }
    if check_value(state, &value, to).is_err() {
        bail!("Cannot cast {value} from {from:?} to {to:?}");
    }
    Ok(value)
}
