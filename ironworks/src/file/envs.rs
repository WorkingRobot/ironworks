//! Structs and utilities shared by the environment set formats.
//!
//! `.envb`, `.obsb` and `.essb` each wrap one `ENVS` section, which holds a set of settings per
//! weather. What a set animates is a tagged union nobody has specified, so only the weather it
//! applies to is read.

use binrw::{BinRead, binread};
use getset::{CopyGetters, Getters};

use crate::{
	FileStream,
	error::{Error, ErrorValue, Result},
};

/// The file header, ahead of the section.
const HEADER: usize = 0x0C;

fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("environment set".into()), reason.into())
}

/// One weather's worth of settings.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Weather {
	/// A row of `Weather`. The two offsets around it reach the settings themselves.
	#[br(pad_before = 8, pad_after = 4)]
	id: u32,
}

/// Everything an `ENVS` section holds.
#[derive(Debug, Getters, CopyGetters)]
pub struct Environments {
	/// The client reads `.envb` at version 6, and the other two at version 4.
	#[get_copy = "pub"]
	version: u32,

	#[getset(skip)]
	weathers: Vec<Weather>,
}

impl Environments {
	/// The weathers the set covers, in the order it names them.
	pub fn weathers(&self) -> &[Weather] {
		&self.weathers
	}
}

/// The environment set a file opening with `magic` holds.
pub(super) fn read(mut stream: impl FileStream, magic: &[u8; 4]) -> Result<Environments> {
	let mut bytes = Vec::new();
	stream.read_to_end(&mut bytes)?;

	if bytes.get(..4) != Some(magic) || bytes.get(HEADER..HEADER + 4) != Some(b"ENVS") {
		return Err(invalid(format!(
			"not an {} file",
			String::from_utf8_lossy(magic)
		)));
	}

	// Offsets inside the section are measured from its body, which follows the magic and length.
	let body = HEADER + 8;
	let mut cursor = std::io::Cursor::new(&bytes[body..]);
	let version = u32::read_le(&mut cursor)?;
	let at = body + u32::read_le(&mut cursor)? as usize;
	let count = u32::read_le(&mut cursor)?;

	let mut cursor = std::io::Cursor::new(
		bytes
			.get(at..)
			.ok_or_else(|| invalid(format!("weathers at {at:#x} are past the end of the file")))?,
	);
	let weathers = (0..count)
		.map(|_| Ok(Weather::read(&mut cursor)?))
		.collect::<Result<Vec<_>>>()?;

	Ok(Environments { version, weathers })
}

#[cfg(test)]
mod test {
	use std::io::Cursor;

	use super::read;

	/// Builds a set of `weathers`, with the section header the format states rather than one the
	/// reader assumes.
	fn build(magic: &[u8; 4], weathers: &[u32]) -> Vec<u8> {
		let mut bytes = Vec::from(*magic);
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());

		bytes.extend(*b"ENVS");
		bytes.extend(24u32.to_le_bytes());
		bytes.extend(6u32.to_le_bytes());
		bytes.extend(16u32.to_le_bytes());
		bytes.extend((weathers.len() as u32).to_le_bytes());
		bytes.extend(0u32.to_le_bytes());

		for &weather in weathers {
			bytes.extend([0u32, 0, weather, 0].map(u32::to_le_bytes).concat());
		}
		bytes
	}

	#[test]
	fn reads_the_weathers_a_set_covers() {
		let set = read(Cursor::new(build(b"ENVB", &[1, 2, 59])), b"ENVB").unwrap();
		assert_eq!(set.version(), 6);
		let ids: Vec<u32> = set.weathers().iter().map(|weather| weather.id()).collect();
		assert_eq!(ids, [1, 2, 59]);
	}

	#[test]
	fn rejects_another_format() {
		assert!(read(Cursor::new(build(b"ESSB", &[1])), b"ENVB").is_err());
	}
}
