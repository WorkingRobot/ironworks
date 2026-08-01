//! Structs and utilities for parsing .uwb files.

use binrw::{BinRead, binread};
use getset::CopyGetters;

use crate::{FileStream, error::Result};

use super::File;

/// Bytes a group's header takes, magic included.
const HEADER_SIZE: u32 = 8;

/// The underwater companion to a zone's clip boxes.
///
/// A zone's scene says whether it has one.
#[binread]
#[br(little, magic = b"UWB1")]
#[derive(Debug)]
pub struct Underwater {
	#[br(temp, pad_before = 4)]
	count: u32,

	#[br(count = count)]
	groups: Vec<Group>,
}

impl Underwater {
	/// The groups this file carries.
	pub fn groups(&self) -> &[Group] {
		&self.groups
	}
}

impl File for Underwater {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

/// Twenty values, none of them identified.
#[binread]
#[br(little, magic = b"UWC1")]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Group {
	#[br(temp)]
	size: u32,

	#[br(pad_size_to = size.saturating_sub(HEADER_SIZE))]
	unknown: [f32; 20],
}

#[cfg(test)]
mod test {
	use std::io::{self, Cursor};

	use crate::{error::Error, file::File};

	use super::Underwater;

	fn underwater(size: u32, groups: &[f32]) -> Vec<u8> {
		let mut bytes = Vec::new();
		bytes.extend(b"UWB1");
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(u32::try_from(groups.len()).unwrap().to_le_bytes());

		for &seed in groups {
			bytes.extend(b"UWC1");
			bytes.extend(size.to_le_bytes());
			bytes.extend((0..20).flat_map(|index| (seed + index as f32).to_le_bytes()));
			bytes.extend(vec![0xCC; size as usize - 88]);
		}
		bytes
	}

	#[test]
	fn empty() {
		assert!(matches!(
			Underwater::read(io::empty()),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn reads_every_group() {
		let file = Underwater::read(Cursor::new(underwater(88, &[0., 100.]))).unwrap();

		let groups = file.groups();
		assert_eq!(groups.len(), 2);
		assert_eq!(groups[0].unknown()[19], 19.);
		assert_eq!(groups[1].unknown()[0], 100.);
	}

	/// A group takes the bytes its header says it does, not a fixed count.
	#[test]
	fn a_group_ends_where_it_says_it_does() {
		let file = Underwater::read(Cursor::new(underwater(96, &[0., 100.]))).unwrap();
		assert_eq!(file.groups()[1].unknown()[0], 100.);
	}
}
