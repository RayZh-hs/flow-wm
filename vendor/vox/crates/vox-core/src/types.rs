use crate::integer::IntegerType;
use crate::source::ModulePath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoxType {
    Int,
    UInt,
    Integer(IntegerType),
    Float,
    Bool,
    String,
    List(Box<VoxType>),
    Tuple(Vec<VoxType>),
    Record(Vec<RecordField>),
    Nullable(Box<VoxType>),
    DynTrait(QualifiedTypeName),
    Named(QualifiedTypeName),
    TypeParameter(String),
    OpaqueSurface(String),
}

impl VoxType {
    pub fn integer(ty: IntegerType) -> Self {
        match ty {
            IntegerType::Int64 => Self::Int,
            IntegerType::UInt64 => Self::UInt,
            _ => Self::Integer(ty),
        }
    }

    pub fn integer_type(&self) -> Option<IntegerType> {
        match self {
            Self::Int => Some(IntegerType::Int64),
            Self::UInt => Some(IntegerType::UInt64),
            Self::Integer(ty) => Some(*ty),
            Self::OpaqueSurface(name) => IntegerType::from_name(name),
            _ => None,
        }
    }

    pub fn opaque_surface(raw: impl Into<String>) -> Self {
        Self::OpaqueSurface(raw.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordField {
    pub name: String,
    pub ty: VoxType,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct QualifiedTypeName {
    pub module: ModulePath,
    pub name: String,
}
