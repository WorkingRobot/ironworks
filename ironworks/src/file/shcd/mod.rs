//! Structs and utilities for parsing .shcd files.

mod code;
mod structs;

pub use code::{DirectX, Resource, ShaderCode, Stage};

use std::io::Cursor;

use crate::error::{Error, ErrorValue, Result};

fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("SHCD".into()), reason.into())
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
