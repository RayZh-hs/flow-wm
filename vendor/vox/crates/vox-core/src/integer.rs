//! Fixed-width integer metadata and arithmetic shared by compilation and execution.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntegerType {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
}

impl IntegerType {
    pub const ALL: [Self; 8] = [
        Self::Int8,
        Self::Int16,
        Self::Int32,
        Self::Int64,
        Self::UInt8,
        Self::UInt16,
        Self::UInt32,
        Self::UInt64,
    ];

    pub const fn bits(self) -> u32 {
        match self {
            Self::Int8 | Self::UInt8 => 8,
            Self::Int16 | Self::UInt16 => 16,
            Self::Int32 | Self::UInt32 => 32,
            Self::Int64 | Self::UInt64 => 64,
        }
    }

    pub const fn signed(self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8",
            Self::Int16 => "Int16",
            Self::Int32 => "Int32",
            Self::Int64 => "Int64",
            Self::UInt8 => "UInt8",
            Self::UInt16 => "UInt16",
            Self::UInt32 => "UInt32",
            Self::UInt64 => "UInt64",
        }
    }

    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Int8 => "i8",
            Self::Int16 => "i16",
            Self::Int32 => "i32",
            Self::Int64 => "i64",
            Self::UInt8 => "u8",
            Self::UInt16 => "u16",
            Self::UInt32 => "u32",
            Self::UInt64 => "u64",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Int" => Some(Self::Int64),
            "UInt" => Some(Self::UInt64),
            _ => Self::ALL.into_iter().find(|ty| ty.name() == name),
        }
    }

    pub fn from_suffix(suffix: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ty| ty.suffix() == suffix)
    }

    pub const fn min(self) -> i128 {
        if self.signed() {
            -(1_i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    pub const fn max(self) -> i128 {
        (1_i128 << (self.bits() - if self.signed() { 1 } else { 0 })) - 1
    }

    pub fn contains(self, value: i128) -> bool {
        value >= self.min() && value <= self.max()
    }

    pub fn common(self, other: Self) -> Option<Self> {
        (self.signed() == other.signed()).then_some(if self.bits() >= other.bits() {
            self
        } else {
            other
        })
    }

    pub fn can_widen_to(self, other: Self) -> bool {
        self.signed() == other.signed() && self.bits() <= other.bits()
    }

    /// Tags 0 and 9 retain the existing Int/UInt ABI representation.
    pub const fn tag(self) -> i32 {
        match self {
            Self::Int64 => 0,
            Self::UInt64 => 9,
            Self::Int8 => 11,
            Self::Int16 => 12,
            Self::Int32 => 13,
            Self::UInt8 => 14,
            Self::UInt16 => 15,
            Self::UInt32 => 16,
        }
    }

    pub fn from_tag(tag: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|ty| ty.tag() == tag)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegerValue {
    ty: IntegerType,
    bits: u64,
}

impl IntegerValue {
    pub fn new(ty: IntegerType, value: i128) -> Option<Self> {
        ty.contains(value)
            .then(|| Self::from_bits(ty, value as u64))
    }

    /// Truncate to the specified width and sign-extend signed integers.
    pub fn from_bits(ty: IntegerType, bits: u64) -> Self {
        let shift = 64 - ty.bits();
        let bits = if ty.signed() {
            (((bits << shift) as i64) >> shift) as u64
        } else {
            (bits << shift) >> shift
        };
        Self { ty, bits }
    }

    pub const fn ty(self) -> IntegerType {
        self.ty
    }
    pub const fn bits(self) -> u64 {
        self.bits
    }
    pub fn number(self) -> i128 {
        if self.ty.signed() {
            self.bits as i64 as i128
        } else {
            self.bits as i128
        }
    }
    pub fn as_float(self) -> f64 {
        if self.ty.signed() {
            self.bits as i64 as f64
        } else {
            self.bits as f64
        }
    }
    pub fn convert(self, ty: IntegerType) -> Option<Self> {
        Self::new(ty, self.number())
    }

    pub fn from_float(ty: IntegerType, value: f64) -> Option<Self> {
        let value = value.trunc();
        if !value.is_finite() || value < ty.min() as f64 || value >= (ty.max() + 1) as f64 {
            return None;
        }
        Self::new(ty, value as i128)
    }

    pub fn negate(self, skip_overflow_check: bool) -> Result<Self, String> {
        if !self.ty.signed() {
            return Err(format!("unary `-` is not defined for {}", self.ty.name()));
        }
        self.finish(-self.number(), skip_overflow_check, "negation overflow")
    }

    fn finish(self, number: i128, skip: bool, error: &str) -> Result<Self, String> {
        if skip {
            Ok(Self::from_bits(self.ty, number as u64))
        } else {
            Self::new(self.ty, number).ok_or_else(|| format!("{} {error}", self.ty.name()))
        }
    }

    pub fn arithmetic(self, op: &str, other: Self, skip: bool) -> Result<Self, String> {
        let ty = self.ty.common(other.ty).ok_or_else(|| {
            format!(
                "operator `{op}` requires explicit conversion between {} and {}",
                self.ty.name(),
                other.ty.name()
            )
        })?;
        let left = self.convert(ty).expect("integer widening");
        let (a, b) = (self.number(), other.number());
        match op {
            "add" => left.finish(a + b, skip, "addition overflow"),
            "subtract" => left.finish(
                a - b,
                skip,
                if ty.signed() {
                    "subtraction overflow"
                } else {
                    "subtraction underflow"
                },
            ),
            "multiply" if !ty.signed() => {
                let product = (a as u128) * (b as u128);
                if skip {
                    Ok(Self::from_bits(ty, product as u64))
                } else if product <= ty.max() as u128 {
                    Ok(Self::from_bits(ty, product as u64))
                } else {
                    Err(format!("{} multiplication overflow", ty.name()))
                }
            }
            "multiply" => left.finish(a * b, skip, "multiplication overflow"),
            "divide" | "remainder" if b == 0 => Err(format!(
                "{} {} by zero",
                ty.name(),
                if op == "divide" {
                    "division"
                } else {
                    "remainder"
                }
            )),
            "divide" => left.finish(a / b, skip, "division overflow"),
            "remainder" => left.finish(a % b, skip, "remainder overflow"),
            _ => Err(format!("unsupported integer operator `{op}`")),
        }
    }
}
