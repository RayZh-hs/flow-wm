use std::collections::BTreeMap;

use crate::ids::HandleId;
use crate::integer::{IntegerType, IntegerValue};

/// Reserved record-payload field used to carry nominal struct identity across
/// MIR, wasm, and package boundaries. Vox identifiers cannot contain NUL.
pub const NOMINAL_TYPE_FIELD: &str = "\0vox.type";

#[derive(Debug, Clone, PartialEq)]
pub enum InlineValue {
    Int(i64),
    UInt(u64),
    Integer(IntegerValue),
    Float(f64),
    Bool(bool),
    String(String),
    Tuple(Vec<InlineValue>),
    Record(BTreeMap<String, InlineValue>),
    Handle(HandleId),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HandleData {
    Int(i64),
    UInt(u64),
    Integer(IntegerValue),
    Float(f64),
    Bool(bool),
    String(String),
    List(Vec<HandleData>),
    Tuple(Vec<HandleData>),
    Record(BTreeMap<String, HandleData>),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleSummary {
    pub type_name: String,
    pub summary: String,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    Inline(InlineValue),
    Handle(HandleId),
}

impl InlineValue {
    pub fn integer(&self) -> Option<IntegerValue> {
        match self {
            Self::Int(v) => Some(IntegerValue::from_bits(IntegerType::Int64, *v as u64)),
            Self::UInt(v) => Some(IntegerValue::from_bits(IntegerType::UInt64, *v)),
            Self::Integer(v) => Some(*v),
            _ => None,
        }
    }

    pub fn from_integer(value: IntegerValue) -> Self {
        match value.ty() {
            IntegerType::Int64 => Self::Int(value.bits() as i64),
            IntegerType::UInt64 => Self::UInt(value.bits()),
            _ => Self::Integer(value),
        }
    }
}
