//! The variable table: headers parsed from bytes, and typed handles that read a [`Row`].

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::defs::{MAX_DESC, MAX_STRING, VarType};
use crate::header::{Header, LayoutError, VAR_HEADER_LEN, cp1252_string, i32_at};
use crate::row::Row;

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for bool {}
    impl Sealed for i32 {}
    impl Sealed for u32 {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// A Rust type a variable can be read as.
pub trait VarValue: sealed::Sealed + Copy {
    /// The value read from a row that is too short or from an older generation.
    const ZERO: Self;
    /// Size of the value in bytes.
    const SIZE: usize;
    /// Whether a variable of type `ty` can be read as this type.
    fn accepts(ty: VarType) -> bool;
    /// Decodes the value from exactly [`VarValue::SIZE`] little-endian bytes.
    fn from_le(bytes: &[u8]) -> Self;
    /// Encodes the value into exactly [`VarValue::SIZE`] bytes.
    fn write_le(self, dst: &mut [u8]);
}

macro_rules! num_value {
    ($t:ty, $zero:expr, $size:expr, $($ty:path),+) => {
        impl VarValue for $t {
            const ZERO: Self = $zero;
            const SIZE: usize = $size;
            fn accepts(ty: VarType) -> bool {
                matches!(ty, $($ty)|+)
            }
            fn from_le(bytes: &[u8]) -> Self {
                bytes.try_into().map_or($zero, <$t>::from_le_bytes)
            }
            fn write_le(self, dst: &mut [u8]) {
                dst.copy_from_slice(&self.to_le_bytes());
            }
        }
    };
}

num_value!(u8, 0, 1, VarType::Char);
num_value!(i32, 0, 4, VarType::Int, VarType::BitField);
num_value!(u32, 0, 4, VarType::BitField, VarType::Int);
num_value!(f32, 0.0, 4, VarType::Float);
num_value!(f64, 0.0, 8, VarType::Double);

impl VarValue for bool {
    const ZERO: Self = false;
    const SIZE: usize = 1;
    fn accepts(ty: VarType) -> bool {
        ty == VarType::Bool
    }
    fn from_le(bytes: &[u8]) -> Self {
        bytes.first().is_some_and(|&b| b != 0)
    }
    fn write_le(self, dst: &mut [u8]) {
        dst[0] = u8::from(self);
    }
}

/// Why a variable could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VarError {
    /// The table has no variable of that name (the car may lack it).
    Missing,
    /// The variable has another type.
    WrongType {
        /// The type the table lists.
        found: VarType,
    },
    /// A scalar was asked for but the variable is an array.
    NotScalar {
        /// The array length.
        count: usize,
    },
}

impl std::fmt::Display for VarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VarError::Missing => write!(f, "variable not present"),
            VarError::WrongType { found } => write!(f, "variable has type {found:?}"),
            VarError::NotScalar { count } => write!(f, "variable is an array of {count}"),
        }
    }
}

impl std::error::Error for VarError {}

/// One variable header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarHeader {
    /// The type.
    pub ty: VarType,
    /// Offset of the value in a row, in bytes.
    pub offset: usize,
    /// Array length (1 for a scalar).
    pub count: usize,
    /// Whether the values count time.
    pub count_as_time: bool,
    /// The name.
    pub name: String,
    /// The description.
    pub desc: String,
    /// The unit.
    pub unit: String,
}

/// The variables of a layout, by name.
#[derive(Clone, Debug)]
pub struct VarTable {
    vars: Vec<VarHeader>,
    by_name: HashMap<String, usize>,
    skipped: Vec<(String, &'static str)>,
    generation: u64,
}

impl VarTable {
    /// Parses the variable headers. `bytes` holds the data the offsets of `header` point into
    /// (the whole image, or a copy of the header and variable header region at the same
    /// offsets). A header that cannot be used is skipped and listed in [`VarTable::skipped`].
    pub fn parse(bytes: &[u8], header: &Header) -> Result<VarTable, LayoutError> {
        let start =
            usize::try_from(header.var_header_offset).map_err(|_| LayoutError::OutOfRange {
                field: "var_header_offset",
                value: i64::from(header.var_header_offset),
            })?;
        let num = usize::try_from(header.num_vars).map_err(|_| LayoutError::OutOfRange {
            field: "num_vars",
            value: i64::from(header.num_vars),
        })?;
        let end = num
            .checked_mul(VAR_HEADER_LEN)
            .and_then(|n| n.checked_add(start))
            .filter(|&e| e <= bytes.len())
            .ok_or(LayoutError::PastEnd {
                field: "var_header_offset",
            })?;
        let buf_len = usize::try_from(header.buf_len).unwrap_or(0);

        let mut table = VarTable {
            vars: Vec::with_capacity(num),
            by_name: HashMap::with_capacity(num),
            skipped: Vec::new(),
            generation: 0,
        };
        for at in (start..end).step_by(VAR_HEADER_LEN) {
            let raw = &bytes[at..at + VAR_HEADER_LEN];
            let name = cp1252_string(&raw[16..16 + MAX_STRING]);
            let skip = |table: &mut VarTable, reason: &'static str| {
                table.skipped.push((name.clone(), reason));
            };
            let Some(ty) = VarType::from_i32(i32_at(raw, 0)) else {
                skip(&mut table, "unknown type");
                continue;
            };
            let (Ok(offset), Ok(count)) = (
                usize::try_from(i32_at(raw, 4)),
                usize::try_from(i32_at(raw, 8)),
            ) else {
                skip(&mut table, "negative offset or count");
                continue;
            };
            if count < 1 {
                skip(&mut table, "count below one");
                continue;
            }
            if name.is_empty() {
                skip(&mut table, "empty name");
                continue;
            }
            let fits = count
                .checked_mul(ty.size())
                .and_then(|n| n.checked_add(offset))
                .is_some_and(|e| e <= buf_len);
            if !fits {
                skip(&mut table, "past the end of the row");
                continue;
            }
            if table.by_name.contains_key(&name) {
                skip(&mut table, "duplicate name");
                continue;
            }
            let var = VarHeader {
                ty,
                offset,
                count,
                count_as_time: raw[12] != 0,
                desc: cp1252_string(&raw[48..48 + MAX_DESC]),
                unit: cp1252_string(&raw[112..112 + MAX_STRING]),
                name: name.clone(),
            };
            table.by_name.insert(name, table.vars.len());
            table.vars.push(var);
        }
        Ok(table)
    }

    /// The same table with its generation set; handles resolved from it carry that generation.
    pub fn with_generation(mut self, generation: u64) -> VarTable {
        self.generation = generation;
        self
    }

    /// The variable header called `name`.
    pub fn get(&self, name: &str) -> Option<&VarHeader> {
        self.by_name.get(name).map(|&i| &self.vars[i])
    }

    /// Resolves a scalar variable (count 1) as `T`.
    pub fn find<T: VarValue>(&self, name: &str) -> Result<Var<T>, VarError> {
        let h = self.get(name).ok_or(VarError::Missing)?;
        if !T::accepts(h.ty) {
            return Err(VarError::WrongType { found: h.ty });
        }
        if h.count != 1 {
            return Err(VarError::NotScalar { count: h.count });
        }
        Ok(Var {
            offset: h.offset,
            generation: self.generation,
            _t: PhantomData,
        })
    }

    /// Resolves an array variable as `T`.
    pub fn find_array<T: VarValue>(&self, name: &str) -> Result<ArrayVar<T>, VarError> {
        let h = self.get(name).ok_or(VarError::Missing)?;
        if !T::accepts(h.ty) {
            return Err(VarError::WrongType { found: h.ty });
        }
        Ok(ArrayVar {
            offset: h.offset,
            count: h.count,
            generation: self.generation,
            _t: PhantomData,
        })
    }

    /// The usable variables, in table order.
    pub fn iter(&self) -> impl Iterator<Item = &VarHeader> {
        self.vars.iter()
    }

    /// Number of usable variables.
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// Whether the table has no usable variable.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// The headers that were skipped, with the reason.
    pub fn skipped(&self) -> &[(String, &'static str)] {
        &self.skipped
    }

    /// The generation: bumped by the caller at each attach and reset.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

fn read_at<T: VarValue>(row: &Row, offset: usize, generation: u64) -> T {
    if generation != row.generation() {
        debug_assert!(
            false,
            "variable handle used with a row of another generation"
        );
        return T::ZERO;
    }
    offset
        .checked_add(T::SIZE)
        .and_then(|end| row.bytes().get(offset..end))
        .map_or(T::ZERO, T::from_le)
}

/// A handle on a scalar variable.
#[derive(Clone, Copy, Debug)]
pub struct Var<T> {
    offset: usize,
    generation: u64,
    _t: PhantomData<T>,
}

impl<T: VarValue> Var<T> {
    /// Reads the value from `row`. A handle of an older generation than the row reads as zero
    /// (and fails a debug assertion): resolve it again after a reset.
    pub fn read(&self, row: &Row) -> T {
        read_at(row, self.offset, self.generation)
    }
}

/// A handle on an array variable.
#[derive(Clone, Copy, Debug)]
pub struct ArrayVar<T> {
    offset: usize,
    count: usize,
    generation: u64,
    _t: PhantomData<T>,
}

impl<T: VarValue> ArrayVar<T> {
    /// Length of the array.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Element `i` of the array in `row`; `None` when `i` is past the array.
    pub fn get(&self, row: &Row, i: usize) -> Option<T> {
        if i >= self.count {
            return None;
        }
        Some(read_at(row, self.offset + i * T::SIZE, self.generation))
    }

    /// All elements of the array in `row`.
    pub fn iter<'a>(&self, row: &'a Row) -> impl Iterator<Item = T> + 'a {
        let (offset, generation) = (self.offset, self.generation);
        (0..self.count).map(move |i| read_at(row, offset + i * T::SIZE, generation))
    }
}
