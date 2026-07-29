//! Structs and utilities for parsing .shpk files.

mod package;
mod structs;

pub use package::{
	AliasCluster, DirectX, Key, MaterialParam, NONE, Node, NodeAlias, Pass, Resource, Shader,
	ShaderPackage, Stage, SubCluster,
};

use std::io::Cursor;

use binrw::{BinRead, meta::ReadEndian};

use crate::error::{Error, ErrorValue, Result};

fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("SHPK".into()), reason.into())
}

/// A cursor over the file from `at` onwards.
fn cursor(bytes: &[u8], at: usize) -> Result<Cursor<&[u8]>> {
	match bytes.get(at..) {
		Some(rest) => Ok(Cursor::new(rest)),
		None => Err(invalid(format!(
			"offset {at:#x} is past the end of the file"
		))),
	}
}

/// The extent `count` records of `size` bytes take from `at`, which is where the next table starts.
fn extent(bytes: &[u8], at: usize, count: usize, size: usize, what: &str) -> Result<usize> {
	count
		.checked_mul(size)
		.and_then(|len| at.checked_add(len))
		.filter(|end| *end <= bytes.len())
		.ok_or_else(|| {
			invalid(format!(
				"{what} at {at:#x} declares {count} records, which do not fit in the file"
			))
		})
}

/// Read `count` fixed-size records, slicing to the table's own extent first so a record cannot read
/// past the last one.
fn table<T>(
	bytes: &[u8],
	at: usize,
	count: usize,
	size: usize,
	what: &str,
) -> Result<(Vec<T>, usize)>
where
	T: for<'a> BinRead<Args<'a> = ()> + ReadEndian,
{
	let end = extent(bytes, at, count, size, what)?;
	let mut cursor = Cursor::new(&bytes[at..end]);
	let records = (0..count)
		.map(|_| Ok(T::read(&mut cursor)?))
		.collect::<Result<Vec<_>>>()?;
	Ok((records, end))
}
