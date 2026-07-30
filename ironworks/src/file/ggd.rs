//! Structs and utilities for parsing .ggd files.

use std::io::{Read, Seek, SeekFrom};

use binrw::{BinRead, BinResult, Endian, binread};
use getset::CopyGetters;
use half::f16;

use crate::{FileStream, error::Result};

use super::File;

/// Version whose header carries sixteen trailing bytes the other does not.
const LONG_HEADER: u16 = 0x0800;

/// Where the grass of one terrain plate grows, at one level of detail.
///
/// A plate ships three of these beside its models, as `<plate>_h`, `_m` and `_l` in the zone's
/// `grass` directory. The grid divides the plate into chunks, each holding a run of placements
/// whose fields are undecoded and are handed out as the bytes they occupy.
#[binread]
#[br(little, magic = b" dgg")]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct GrassGrid {
	/// `0x0800` in almost every file the game ships, `0x0402` in the rest, which hold sixteen fewer
	/// bytes of header.
	version: u16,

	/// `0x0200` in every file the game ships.
	unknown_a: u16,

	#[br(temp)]
	chunk_count: u16,

	/// `0x0555` in every file the game ships.
	unknown_b: u16,

	// Slots past chunk_count hold stale offsets, so only the declared ones are followed.
	#[br(temp)]
	chunk_offsets: [u32; 8],

	/// Read from half precision.
	#[br(map = |raw: [u16; 12]| raw.map(|bits| f16::from_bits(bits).to_f32()))]
	unknown_c: [f32; 12],

	unknown_d: [f32; 3],

	/// Only `0x0800` files carry these sixteen bytes.
	#[br(if(version == LONG_HEADER))]
	unknown_e: Option<[u8; 16]>,

	#[br(parse_with = chunks, args(chunk_count.into(), chunk_offsets))]
	#[getset(skip)]
	chunks: Vec<Chunk>,
}

impl GrassGrid {
	/// Every chunk of the grid, in the order the header names them.
	pub fn chunks(&self) -> &[Chunk] {
		&self.chunks
	}
}

/// One chunk of a grid, and the placements that fall inside it.
#[binread]
#[br(little, magic = b"dgs\0")]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Chunk {
	/// Bounds of the chunk, in world units.
	min: [f32; 3],
	max: [f32; 3],

	#[getset(skip)]
	record_counts: [u16; 36],

	#[br(count = record_counts.iter().copied().map(usize::from).sum::<usize>() * Self::RECORD_SIZE)]
	#[getset(skip)]
	records: Vec<u8>,
}

impl Chunk {
	/// Bytes one record occupies.
	pub const RECORD_SIZE: usize = 26;

	/// The counts the record run is sized from. What a position in the array names is unidentified.
	pub fn record_counts(&self) -> &[u16; 36] {
		&self.record_counts
	}

	/// The records, as [`RECORD_SIZE`](Self::RECORD_SIZE) byte units. Their fields are undecoded.
	pub fn records(&self) -> &[u8] {
		&self.records
	}
}

impl File for GrassGrid {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

/// Reads the chunks the header declares, each at the offset naming it.
fn chunks<R: Read + Seek>(
	reader: &mut R,
	endian: Endian,
	(count, offsets): (usize, [u32; 8]),
) -> BinResult<Vec<Chunk>> {
	if count > offsets.len() {
		return Err(binrw::Error::AssertFail {
			pos: 8,
			message: format!(
				"{count} chunks declared, but only {} are named",
				offsets.len()
			),
		});
	}

	offsets[..count]
		.iter()
		.map(|offset| {
			reader.seek(SeekFrom::Start((*offset).into()))?;
			Chunk::read_options(reader, endian, ())
		})
		.collect()
}

#[cfg(test)]
mod test {
	use std::io::{self, Cursor};

	use crate::{error::Error, file::File};

	use super::{Chunk, GrassGrid};

	/// A chunk, as its bounds and the counts naming how many records follow.
	fn chunk(min: [f32; 3], counts: &[(usize, u16)]) -> Vec<u8> {
		let mut bytes = Vec::from(*b"dgs\0");
		for axis in min {
			bytes.extend(axis.to_le_bytes());
		}
		for axis in min {
			bytes.extend((axis + 32.0).to_le_bytes());
		}

		let mut slots = [0u16; 36];
		for &(at, count) in counts {
			slots[at] = count;
		}
		for slot in slots {
			bytes.extend(slot.to_le_bytes());
		}

		let records = slots.iter().map(|c| usize::from(*c)).sum::<usize>();
		bytes.extend(std::iter::repeat_n(0xab, records * Chunk::RECORD_SIZE));
		bytes
	}

	/// A grid holding the given chunks, laid out one after another past the header.
	fn grid(version: u16, stale: &[u32], chunks: &[Vec<u8>]) -> Vec<u8> {
		let header = if version == 0x0800 { 0x60 } else { 0x50 };

		let mut offsets = [0u32; 8];
		let mut at = u32::try_from(header).unwrap();
		for (slot, chunk) in offsets.iter_mut().zip(chunks) {
			*slot = at;
			at += u32::try_from(chunk.len()).unwrap();
		}
		for (slot, value) in offsets.iter_mut().skip(chunks.len()).zip(stale) {
			*slot = *value;
		}

		let mut bytes = Vec::from(*b" dgg");
		bytes.extend(version.to_le_bytes());
		bytes.extend(0x0200u16.to_le_bytes());
		bytes.extend(u16::try_from(chunks.len()).unwrap().to_le_bytes());
		bytes.extend(0x0555u16.to_le_bytes());
		for offset in offsets {
			bytes.extend(offset.to_le_bytes());
		}
		// 0.25, 0.5 and ten more half-precision values.
		for raw in [0x3400u16, 0x3800] {
			bytes.extend(raw.to_le_bytes());
		}
		bytes.extend([0; 20]);
		for axis in [1.0f32, 2.0, 3.0] {
			bytes.extend(axis.to_le_bytes());
		}
		if version == 0x0800 {
			bytes.extend([0x5f; 8]);
			bytes.extend([0, 0x33, 0, 0, 0, 0, 0, 0]);
		}
		assert_eq!(bytes.len(), header);

		for chunk in chunks {
			bytes.extend(chunk);
		}
		bytes
	}

	#[test]
	fn empty() {
		assert!(matches!(
			GrassGrid::read(io::empty()),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn reads_a_chunk_at_each_declared_offset() {
		let file = GrassGrid::read(Cursor::new(grid(
			0x0800,
			&[],
			&[
				chunk([0.0, 0.0, 0.0], &[(0, 3)]),
				chunk([64.0, 0.0, 0.0], &[(0, 1), (1, 2)]),
			],
		)))
		.unwrap();

		assert_eq!(file.version(), 0x0800);
		assert_eq!(file.unknown_c()[..2], [0.25, 0.5]);
		assert_eq!(file.unknown_d(), [1.0, 2.0, 3.0]);
		assert_eq!(
			file.unknown_e().unwrap()[..9],
			[0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0]
		);

		assert_eq!(file.chunks().len(), 2);
		assert_eq!(file.chunks()[1].min(), [64.0, 0.0, 0.0]);
		assert_eq!(file.chunks()[1].max(), [96.0, 32.0, 32.0]);
		assert_eq!(file.chunks()[1].record_counts()[..2], [1, 2]);
		assert_eq!(file.chunks()[1].records().len(), 3 * Chunk::RECORD_SIZE);
	}

	/// `0x0402` files hold sixteen fewer bytes of header, putting their first chunk at 0x50.
	#[test]
	fn reads_the_short_header_version() {
		let file = GrassGrid::read(Cursor::new(grid(
			0x0402,
			&[],
			&[chunk([5.0, 6.0, 7.0], &[(0, 2)])],
		)))
		.unwrap();

		assert_eq!(file.unknown_c()[..2], [0.25, 0.5]);
		assert_eq!(file.unknown_d(), [1.0, 2.0, 3.0]);
		assert_eq!(file.unknown_e(), None);
		assert_eq!(file.chunks()[0].min(), [5.0, 6.0, 7.0]);
		assert_eq!(file.chunks()[0].records().len(), 2 * Chunk::RECORD_SIZE);
	}

	/// Records are counted from every slot, not only the first few that files usually fill.
	#[test]
	fn sums_the_whole_count_array() {
		let file = GrassGrid::read(Cursor::new(grid(
			0x0800,
			&[],
			&[chunk([0.0; 3], &[(0, 1), (5, 2), (30, 4)])],
		)))
		.unwrap();

		let chunk = &file.chunks()[0];
		assert_eq!(chunk.record_counts()[30], 4);
		assert_eq!(chunk.records().len(), 7 * Chunk::RECORD_SIZE);
	}

	/// The run ends where the counts say, not at the end of the file.
	#[test]
	fn ignores_bytes_past_the_last_record() {
		let mut bytes = grid(0x0800, &[], &[chunk([0.0; 3], &[(0, 1)])]);
		bytes.extend([0xcd; 40]);

		let file = GrassGrid::read(Cursor::new(bytes)).unwrap();
		assert_eq!(file.chunks()[0].records().len(), Chunk::RECORD_SIZE);
	}

	/// Offsets past the declared count are stale, and pointing them anywhere must not matter.
	#[test]
	fn ignores_stale_offsets() {
		let file = GrassGrid::read(Cursor::new(grid(
			0x0800,
			&[0xdead, 4, 0],
			&[chunk([0.0; 3], &[(0, 1)])],
		)))
		.unwrap();
		assert_eq!(file.chunks().len(), 1);
	}

	#[test]
	fn truncated_records() {
		let mut bytes = grid(0x0800, &[], &[chunk([0.0; 3], &[(0, 2)])]);
		bytes.truncate(bytes.len() - 1);
		assert!(matches!(
			GrassGrid::read(Cursor::new(bytes)),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn more_chunks_than_the_header_can_name() {
		let mut bytes = grid(0x0800, &[], &[chunk([0.0; 3], &[(0, 1)])]);
		bytes[8..10].copy_from_slice(&9u16.to_le_bytes());
		assert!(matches!(
			GrassGrid::read(Cursor::new(bytes)),
			Err(Error::Resource(_))
		));
	}
}
