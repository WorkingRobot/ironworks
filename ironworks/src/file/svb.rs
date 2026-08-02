//! Structs and utilities for parsing .svb files.

use std::io::SeekFrom;

use binrw::{BinRead, binread};
use getset::CopyGetters;

use crate::{FileStream, error::Result};

use super::File;

/// Bytes a group's header takes, magic included.
const HEADER_SIZE: i64 = 20;

/// How the sky reads through a zone's instances.
///
/// A zone's scene names the file it uses.
#[binread]
#[br(little, magic = b"SVB1")]
#[derive(Debug)]
pub struct SkyVisibility {
	#[br(temp, pad_before = 4)]
	count: u32,

	#[br(count = count)]
	groups: Vec<Group>,
}

impl SkyVisibility {
	/// The groups this file carries.
	pub fn groups(&self) -> &[Group] {
		&self.groups
	}
}

impl File for SkyVisibility {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

/// A run of entries.
#[binread]
#[br(little, magic = b"SVC1")]
#[derive(Debug, CopyGetters)]
pub struct Group {
	#[br(temp)]
	header_size: u32,

	/// Zero in every file the game ships.
	#[get_copy = "pub"]
	unknown_a: u32,

	/// Twelve in every file the game ships.
	#[get_copy = "pub"]
	unknown_b: u32,

	#[br(temp)]
	count: u32,

	#[br(seek_before = SeekFrom::Current(i64::from(header_size) - HEADER_SIZE), count = count)]
	entries: Vec<Entry>,
}

impl Group {
	/// The entries this group carries.
	pub fn entries(&self) -> &[Entry] {
		&self.entries
	}
}

/// What one instance does to the sky.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Entry {
	/// Key of an instance in one of the zone's layers.
	instance: u32,

	/// Reaches the part of that instance the entry applies to, an index per level of shared group
	/// it sits under, filled from the front and zero the rest of the way.
	members: [u8; 4],

	/// How much of the sky reaches it, over `0.0..=1.0`.
	visibility: f32,
}

#[cfg(test)]
mod test {
	use std::io::{self, Cursor};

	use crate::{error::Error, file::File};

	use super::SkyVisibility;

	fn visibility(header_size: u32, groups: &[&[u32]]) -> Vec<u8> {
		let mut bytes = Vec::new();
		bytes.extend(b"SVB1");
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(u32::try_from(groups.len()).unwrap().to_le_bytes());

		for entries in groups {
			bytes.extend(b"SVC1");
			bytes.extend(header_size.to_le_bytes());
			bytes.extend(1u32.to_le_bytes());
			bytes.extend(12u32.to_le_bytes());
			bytes.extend(u32::try_from(entries.len()).unwrap().to_le_bytes());
			bytes.extend(vec![0xCC; header_size as usize - 20]);
			for &instance in *entries {
				bytes.extend(instance.to_le_bytes());
				bytes.extend([2, 1, 0, 0]);
				bytes.extend(0.5f32.to_le_bytes());
			}
		}
		bytes
	}

	#[test]
	fn empty() {
		assert!(matches!(
			SkyVisibility::read(io::empty()),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn reads_every_group() {
		let file = SkyVisibility::read(Cursor::new(visibility(20, &[&[1, 2], &[], &[3]]))).unwrap();

		let groups = file.groups();
		assert_eq!(groups.len(), 3);
		assert_eq!(groups[0].unknown_a(), 1);
		assert_eq!(groups[0].unknown_b(), 12);
		assert_eq!(
			groups[0]
				.entries()
				.iter()
				.map(|entry| entry.instance())
				.collect::<Vec<_>>(),
			[1, 2]
		);
		assert!(groups[1].entries().is_empty());

		let entry = groups[2].entries()[0];
		assert_eq!(entry.members(), [2, 1, 0, 0]);
		assert_eq!(entry.visibility(), 0.5);
	}

	/// Entries start where the group's header says they do, not at a fixed offset.
	#[test]
	fn entries_start_past_the_declared_header() {
		let file = SkyVisibility::read(Cursor::new(visibility(28, &[&[7]]))).unwrap();
		assert_eq!(file.groups()[0].entries()[0].instance(), 7);
	}
}
