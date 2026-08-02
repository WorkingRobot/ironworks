//! Structs and utilities for parsing .lcb files.

use std::io::SeekFrom;

use binrw::{BinRead, binread};
use getset::CopyGetters;

use crate::{FileStream, error::Result};

use super::File;

/// Bytes a group's header takes, magic included.
const HEADER_SIZE: i64 = 20;

/// The boxes a zone clips its lights against.
///
/// A zone's scene names the file it uses.
#[binread]
#[br(little, magic = b"LCB1")]
#[derive(Debug)]
pub struct ClipBoxes {
	#[br(temp, pad_before = 4)]
	count: u32,

	#[br(count = count)]
	groups: Vec<Group>,
}

impl ClipBoxes {
	/// The groups this file carries.
	pub fn groups(&self) -> &[Group] {
		&self.groups
	}
}

impl File for ClipBoxes {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

/// A run of entries.
#[binread]
#[br(little, magic = b"LCC1")]
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

/// The box one light is clipped against.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Entry {
	/// Key of an instance in one of the zone's layers.
	instance: u32,

	/// Reaches the light inside that instance, an index per level of shared group it sits under,
	/// filled from the front and zero the rest of the way.
	members: [u8; 4],

	min: [f32; 3],
	max: [f32; 3],
}

#[cfg(test)]
mod test {
	use std::io::{self, Cursor};

	use crate::{error::Error, file::File};

	use super::ClipBoxes;

	fn boxes(header_size: u32, groups: &[&[u32]]) -> Vec<u8> {
		let mut bytes = Vec::new();
		bytes.extend(b"LCB1");
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(u32::try_from(groups.len()).unwrap().to_le_bytes());

		for entries in groups {
			bytes.extend(b"LCC1");
			bytes.extend(header_size.to_le_bytes());
			bytes.extend(1u32.to_le_bytes());
			bytes.extend(12u32.to_le_bytes());
			bytes.extend(u32::try_from(entries.len()).unwrap().to_le_bytes());
			bytes.extend(vec![0xCC; header_size as usize - 20]);
			for &instance in *entries {
				bytes.extend(instance.to_le_bytes());
				bytes.extend([2, 1, 0, 0]);
				bytes.extend((0..3).flat_map(|axis| (-(axis as f32)).to_le_bytes()));
				bytes.extend((0..3).flat_map(|axis| (axis as f32).to_le_bytes()));
			}
		}
		bytes
	}

	#[test]
	fn empty() {
		assert!(matches!(
			ClipBoxes::read(io::empty()),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn reads_every_group() {
		let file = ClipBoxes::read(Cursor::new(boxes(20, &[&[1, 2], &[], &[3]]))).unwrap();

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
		assert_eq!(entry.min(), [-0., -1., -2.]);
		assert_eq!(entry.max(), [0., 1., 2.]);
	}

	/// Entries start where the group's header says they do, not at a fixed offset.
	#[test]
	fn entries_start_past_the_declared_header() {
		let file = ClipBoxes::read(Cursor::new(boxes(28, &[&[7]]))).unwrap();
		assert_eq!(file.groups()[0].entries()[0].instance(), 7);
	}
}
