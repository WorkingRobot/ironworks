use enum_as_inner::EnumAsInner;

use crate::sestring2::SeString;

/// A single field from an Excel database.
#[allow(missing_docs)]
#[derive(Debug, EnumAsInner)]
pub enum Field {
	String(SeString<'static>),

	Bool(bool),

	I8(i8),
	I16(i16),
	I32(i32),
	I64(i64),

	U8(u8),
	U16(u16),
	U32(u32),
	U64(u64),

	F32(f32),
}
