//! Structs and utilities for parsing .dic files.

use std::{
	fmt::Debug,
	io::{Read, Seek, SeekFrom},
};

use binrw::{BinRead, BinResult, Endian, binread};
use getset::CopyGetters;

use crate::{FileStream, error::Result};

use super::File;

const MAPS: u64 = 0x8124;
const MAP_SIZE: u64 = 0x200;

/// Offsets, lengths, kind and block map, which the tables are stated against the end of.
const LIST_HEADER: u64 = 44 + MAP_SIZE;

/// A dictionary of words the game matches chat against, shipped as the vulgar word filter.
///
/// Nothing in the file names a language: one holds every language its region ships, folded to a
/// canonical case and width and held as a trie rather than as text.
#[binread]
#[br(little)]
pub struct WordDictionary {
	#[br(temp, pad_before = 12)]
	skip_bits: [u8; 0x2000],

	// Which character map, one-based, folds each block of 256 code points.
	#[br(temp, seek_before = SeekFrom::Start(0x800c))]
	map_blocks: [u8; 0x100],

	#[br(temp, assert(map_count <= 0x100, "{map_count} character maps for 256 blocks"))]
	map_count: u32,

	#[br(temp)]
	list_offsets: [u32; 5],

	#[br(temp, count = map_count)]
	maps: Vec<[u16; 0x100]>,

	#[br(calc = skipped(&skip_bits))]
	skipped: Vec<char>,

	#[br(calc = replacements(&map_blocks, &maps))]
	replacements: Vec<(char, char)>,

	#[br(parse_with = lists, args(list_offsets, MAPS + u64::from(map_count) * MAP_SIZE))]
	lists: Vec<WordList>,
}

impl WordDictionary {
	/// Characters dropped from a phrase before it is matched, which are what someone would break a
	/// word up with.
	pub fn skipped(&self) -> &[char] {
		&self.skipped
	}

	/// Characters read as another before matching, as (character, what it is read as). Covers case,
	/// fullwidth forms and the kana the words are written in.
	pub fn replacements(&self) -> &[(char, char)] {
		&self.replacements
	}

	/// The word lists the file holds. Up to three ship at once: the words themselves, and the
	/// phrases that contain one but are let through anyway.
	pub fn lists(&self) -> &[WordList] {
		&self.lists
	}
}

impl Debug for WordDictionary {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("WordDictionary")
			.field("skipped.len", &self.skipped.len())
			.field("replacements.len", &self.replacements.len())
			.field("lists", &self.lists)
			.finish()
	}
}

/// One list of words.
#[derive(CopyGetters)]
pub struct WordList {
	/// Whether a phrase matching this list is filtered, rather than let through.
	#[get_copy = "pub"]
	blocked: bool,

	words: Vec<String>,
}

impl WordList {
	/// Every word, ordered by first character and then by the trie.
	pub fn words(&self) -> &[String] {
		&self.words
	}
}

impl Debug for WordList {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("WordList")
			.field("blocked", &self.blocked)
			.field("words.len", &self.words.len())
			.finish()
	}
}

fn skipped(bits: &[u8; 0x2000]) -> Vec<char> {
	(0..0x10000u32)
		.filter(|point| (bits[*point as usize / 8] >> (point % 8)) & 1 == 1)
		.filter_map(char::from_u32)
		.collect()
}

fn replacements(blocks: &[u8; 0x100], maps: &[[u16; 0x100]]) -> Vec<(char, char)> {
	blocks
		.iter()
		.enumerate()
		.filter_map(|(block, &map)| {
			let map = maps.get(usize::from(map.checked_sub(1)?))?;
			Some((block as u32, map))
		})
		.flat_map(|(block, map)| {
			map.iter().enumerate().filter_map(move |(low, &folded)| {
				let from = char::from_u32((block << 8) | low as u32)?;
				let to = char::from_u32(folded.into())?;
				(from != to).then_some((from, to))
			})
		})
		.collect()
}

fn lists<R: Read + Seek>(
	reader: &mut R,
	endian: Endian,
	(offsets, maps_end): ([u32; 5], u64),
) -> BinResult<Vec<WordList>> {
	let file_end = reader.seek(SeekFrom::End(0))?;
	offsets
		.into_iter()
		.filter(|&offset| offset != 0)
		.map(|offset| list(reader, endian, u64::from(offset), maps_end, file_end))
		.collect()
}

fn list<R: Read + Seek>(
	reader: &mut R,
	endian: Endian,
	at: u64,
	maps_end: u64,
	file_end: u64,
) -> BinResult<WordList> {
	if at < maps_end || at + LIST_HEADER > file_end {
		return Err(binrw::Error::AssertFail {
			pos: at,
			message: format!("a list at {at} lies outside {maps_end}..{file_end}"),
		});
	}

	reader.seek(SeekFrom::Start(at))?;
	let header = ListHeader::read_options(reader, endian, ())?;

	// A list left behind after its words were dropped keeps stale extents, so no table's end can be
	// taken for where the next one starts.
	let base = at + LIST_HEADER;
	let mut table = |index: usize| -> BinResult<Vec<u8>> {
		let start = base + u64::from(header.offsets[index]);
		let end = start + u64::from(header.lengths[index]);
		if end > file_end {
			return Err(binrw::Error::AssertFail {
				pos: at,
				message: format!("table {index} spans {start}..{end}, past the file"),
			});
		}

		reader.seek(SeekFrom::Start(start))?;
		let mut bytes = vec![0; usize::try_from(end - start).unwrap()];
		reader.read_exact(&mut bytes)?;
		Ok(bytes)
	};

	let trie = Trie {
		inner: points(&table(1)?),
		chara: points(&table(2)?),
		word: points(&table(3)?),
		nodes: table(4)?
			.chunks_exact(16)
			.map(|node| Node {
				multiple: field(node, 0) != 0,
				siblings: field(node, 4),
				child: field(node, 8),
				offset: field(node, 12),
			})
			.collect(),
	};

	let begin = points(&table(0)?);
	let mut words = Vec::new();
	let mut visited = vec![false; trie.nodes.len()];
	for (block, &mapped) in header.blocks.iter().enumerate() {
		if mapped == 0 {
			continue;
		}
		let start = usize::from(mapped) * 0x100;
		for low in 0..0x100 {
			let Some(&node) = begin.get(start + low) else {
				break;
			};
			if node == 0 {
				continue;
			}
			let point = char::from_u32(((block as u32) << 8) | low as u32)
				.unwrap_or(char::REPLACEMENT_CHARACTER);
			trie.walk(usize::from(node), point.into(), &mut visited, &mut words);
		}
	}

	Ok(WordList {
		blocked: header.kind == 1,
		words,
	})
}

#[binread]
#[br(little)]
struct ListHeader {
	offsets: [u32; 5],
	lengths: [u32; 5],
	kind: u32,
	blocks: [u16; 0x100],
}

struct Node {
	multiple: bool,
	siblings: usize,
	child: usize,
	offset: usize,
}

struct Trie {
	inner: Vec<u16>,
	chara: Vec<u16>,
	word: Vec<u16>,
	nodes: Vec<Node>,
}

enum Step {
	Node(usize, String),
	Word(String),
}

impl Trie {
	/// Every node is entered once, so one reached twice is a cycle and the walk stops there.
	fn walk(&self, root: usize, prefix: String, visited: &mut [bool], words: &mut Vec<String>) {
		let mut stack = vec![Step::Node(root, prefix)];
		while let Some(step) = stack.pop() {
			let (index, prefix) = match step {
				Step::Word(word) => {
					words.push(word);
					continue;
				}
				Step::Node(index, prefix) => (index, prefix),
			};

			let Some(node) = self.nodes.get(index) else {
				continue;
			};
			if std::mem::replace(&mut visited[index], true) {
				continue;
			}

			let at = stack.len();
			for sibling in 0..node.siblings {
				let Some(text) = self.text(node, sibling) else {
					break;
				};
				let word = prefix.clone() + &text;
				stack.push(
					match sibling
						.checked_add(node.child)
						.and_then(|at| self.inner.get(at))
					{
						Some(&child) if node.child != 0 && child != 0 => {
							Step::Node(usize::from(child), word)
						}
						_ => Step::Word(word),
					},
				);
			}
			stack[at..].reverse();
		}
	}

	fn text(&self, node: &Node, sibling: usize) -> Option<String> {
		let start = node.offset / 2;
		if !node.multiple {
			// A zero adds nothing to the word its parents spelled.
			let point = *self.chara.get(start.checked_add(sibling)?)?;
			return Some(decode(&[point][..usize::from(point != 0)]));
		}

		// Nodes naming a run of characters ship with one sibling apiece.
		let run = self.word.get(start..).filter(|_| sibling == 0)?;
		let end = run
			.iter()
			.position(|&point| point == 0)
			.unwrap_or(run.len());
		Some(decode(&run[..end]))
	}
}

fn points(bytes: &[u8]) -> Vec<u16> {
	bytes
		.chunks_exact(2)
		.map(|point| u16::from_le_bytes(point.try_into().unwrap()))
		.collect()
}

fn field(bytes: &[u8], at: usize) -> usize {
	u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

fn decode(points: &[u16]) -> String {
	char::decode_utf16(points.iter().copied())
		.map(|point| point.unwrap_or(char::REPLACEMENT_CHARACTER))
		.collect()
}

impl File for WordDictionary {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

#[cfg(test)]
mod test {
	use std::io::{self, Cursor};

	use crate::{error::Error, file::File};

	use super::WordDictionary;

	/// A character map per block, identity but for the folds named, and the blocks the trie starts
	/// at, packed into the fixed header.
	fn dictionary(
		maps: &[(u8, &[(u8, u16)])],
		skip: &[u32],
		lists: &[(usize, Vec<u8>)],
	) -> Vec<u8> {
		let mut bytes = vec![0; 0x8124];

		for &point in skip {
			bytes[12 + point as usize / 8] |= 1 << (point % 8);
		}

		for (index, (block, _)) in maps.iter().enumerate() {
			bytes[0x800c + usize::from(*block)] = index as u8 + 1;
		}
		write(&mut bytes, 0x810c, &[maps.len() as u32]);

		let mut at = 0x8124 + maps.len() as u32 * 0x200;
		let mut offsets = [0; 5];
		for (slot, list) in lists {
			offsets[*slot] = at;
			at += list.len() as u32;
		}
		write(&mut bytes, 0x8110, &offsets);

		for (block, folds) in maps {
			let mut map = (0..0x100)
				.map(|low| u16::from(*block) << 8 | low)
				.collect::<Vec<_>>();
			for &(low, folded) in *folds {
				map[usize::from(low)] = folded;
			}
			bytes.extend(points(&map));
		}

		for (_, list) in lists {
			bytes.extend(list);
		}
		bytes
	}

	/// Blocks name where in the begin table each block of 256 code points starts, and the tables are
	/// laid out in the order the header states them.
	fn list(kind: u32, blocks: &[(u8, u16)], tables: [&[u8]; 5]) -> Vec<u8> {
		let mut bytes = Vec::new();

		let mut at = 0;
		for table in tables {
			bytes.extend(u32::try_from(at).unwrap().to_le_bytes());
			at += table.len();
		}
		for table in tables {
			bytes.extend(u32::try_from(table.len()).unwrap().to_le_bytes());
		}
		bytes.extend(kind.to_le_bytes());

		let mut map = vec![0; 0x100];
		for &(block, mapped) in blocks {
			map[usize::from(block)] = mapped;
		}
		bytes.extend(points(&map));

		bytes.extend(tables.concat());
		bytes
	}

	fn points(points: &[u16]) -> Vec<u8> {
		points
			.iter()
			.flat_map(|point| point.to_le_bytes())
			.collect()
	}

	fn nodes(nodes: &[(u32, u32, u32, u32)]) -> Vec<u8> {
		nodes
			.iter()
			.flat_map(|&(multiple, siblings, child, offset)| {
				[multiple, siblings, child, offset].map(u32::to_le_bytes)
			})
			.flatten()
			.collect()
	}

	fn write(bytes: &mut [u8], at: usize, values: &[u32]) {
		for (index, value) in values.iter().enumerate() {
			let at = at + index * 4;
			bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
		}
	}

	/// `ab` and `ac` hang off one node reading a character apiece, `dog` off one reading a run.
	fn words() -> Vec<u8> {
		let mut begin = vec![0; 0x200];
		begin[0x100 + 0x61] = 1;
		begin[0x100 + 0x64] = 2;

		list(
			1,
			&[(0, 1)],
			[
				&points(&begin),
				&[],
				&points(&[0x62, 0x63]),
				&points(&[0x6f, 0x67, 0]),
				&nodes(&[(0, 0, 0, 0), (0, 2, 0, 0), (1, 1, 0, 0)]),
			],
		)
	}

	#[test]
	fn empty() {
		assert!(matches!(
			WordDictionary::read(io::empty()),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn truncated() {
		let mut bytes = dictionary(&[(0, &[])], &[], &[(0, words())]);
		bytes.truncate(bytes.len() - 2);
		assert!(matches!(
			WordDictionary::read(Cursor::new(bytes)),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn reads_words_a_character_and_a_run_at_a_time() {
		let file = WordDictionary::read(Cursor::new(dictionary(&[(0, &[])], &[], &[(0, words())])))
			.unwrap();

		assert_eq!(file.lists().len(), 1);
		assert!(file.lists()[0].blocked());
		assert_eq!(file.lists()[0].words(), ["ab", "ac", "dog"]);
	}

	/// The lists a file holds are named by slot, and the game leaves gaps between them.
	#[test]
	fn reads_a_list_from_a_later_slot() {
		let mut begin = vec![0; 0x200];
		begin[0x100 + 0x61] = 1;
		let allowed = list(
			0,
			&[(0, 1)],
			[
				&points(&begin),
				&[],
				&points(&[0x62]),
				&[],
				&nodes(&[(0, 0, 0, 0), (0, 1, 0, 0)]),
			],
		);

		let bytes = dictionary(&[(0, &[])], &[], &[(0, words()), (4, allowed)]);
		let file = WordDictionary::read(Cursor::new(bytes)).unwrap();

		assert_eq!(file.lists().len(), 2);
		assert!(file.lists()[0].blocked());
		assert!(!file.lists()[1].blocked());
		assert_eq!(file.lists()[1].words(), ["ab"]);
	}

	#[test]
	fn reads_the_characters_dropped_and_folded_before_matching() {
		let bytes = dictionary(&[(0, &[(0x41, 0x61)])], &[0x20, 0x3000], &[(0, words())]);
		let file = WordDictionary::read(Cursor::new(bytes)).unwrap();

		assert_eq!(file.skipped(), ['\u{20}', '\u{3000}']);
		assert_eq!(file.replacements(), [('A', 'a')]);
	}

	/// A list left in place after its words were dropped keeps stale extents, which name no trie and
	/// must not be read as one.
	#[test]
	fn reads_no_words_from_a_block_pointing_past_the_begin_table() {
		let stale = list(
			0,
			&[(0, 8)],
			[&points(&[0; 4]), &[], &[], &[], &nodes(&[(0, 0, 0, 0)])],
		);

		let bytes = dictionary(&[(0, &[])], &[], &[(0, words()), (3, stale)]);
		let file = WordDictionary::read(Cursor::new(bytes)).unwrap();

		assert_eq!(file.lists().len(), 2);
		assert!(file.lists()[1].words().is_empty());
	}

	#[test]
	fn rejects_a_table_running_past_the_file() {
		let mut bytes = dictionary(&[(0, &[])], &[], &[(0, words())]);
		let lengths = 0x8324 + 20;
		write(&mut bytes, lengths, &[0x1000]);
		assert!(matches!(
			WordDictionary::read(Cursor::new(bytes)),
			Err(Error::Resource(_))
		));
	}

	/// Nothing in the file stops a node naming one of its own parents.
	#[test]
	fn stops_on_a_cycle() {
		let mut begin = vec![0; 0x200];
		begin[0x100 + 0x61] = 1;
		let looping = list(
			1,
			&[(0, 1)],
			[
				&points(&begin),
				&points(&[0, 1]),
				&points(&[0x62]),
				&[],
				&nodes(&[(0, 0, 0, 0), (0, 1, 1, 0)]),
			],
		);

		let file = WordDictionary::read(Cursor::new(dictionary(&[(0, &[])], &[], &[(0, looping)])))
			.unwrap();
		assert!(file.lists()[0].words().is_empty());
	}
}
