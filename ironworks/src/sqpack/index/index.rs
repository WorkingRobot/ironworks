use std::sync::{Arc, Mutex};

use binrw::BinRead;
use getset::CopyGetters;

use crate::{
	error::{Error, ErrorValue, Result},
	sqpack::Resource,
};

use super::{index1::Index1, index2::Index2, shared::FileMetadata};

/// How many absent chunk IDs to look past before treating a category as exhausted.
/// Live data leaves holes - global/latest ships `ex2/bg` as chunks 0-3 and 5, and `ex5/bg`
/// as 0-3 and 5-8 - and a resource may be remote, so this walks over holes without
/// probing the whole ID space.
const CHUNK_MISS_TOLERANCE: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexHash {
	/// `.index`
	Split(u64),
	/// `.index2`
	Whole(u32),
}

impl IndexHash {
	pub fn of(path: &str) -> (Option<Self>, Self) {
		(
			Index1::hash(path).map(Self::Split),
			Self::Whole(Index2::hash(path)),
		)
	}

	/// The hash of a directory on its own (upper half of [`Split`](Self::Split))
	pub fn directory(path: &str) -> u32 {
		super::crc::crc32(path.as_bytes())
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IndexEntry {
	pub repository: u8,
	pub category: u8,
	pub chunk: u8,
	pub hash: IndexHash,
}

/// Specifier of a file location within a SqPack category.
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Location {
	/// SqPack chunk the file is in, i.e. `0000XX.win32.dat1`.
	chunk: u8,
	/// Data file the file is in, i.e. `000000.win32.datX`.
	data_file: u8,
	/// Offset within the targeted data file that the file starts at.
	offset: u64,
	/// Estimated size of the target file, if known. This will typically err on
	/// the larger side, as files commonly have some amount of padding at the end.
	size: Option<u64>,
}

#[derive(Debug)]
pub struct Index<R> {
	repository: u8,
	category: u8,

	resource: Arc<R>,
	max_chunk: Mutex<Option<u16>>,
	chunks: Mutex<Vec<Option<Arc<IndexChunk>>>>,
}

impl<R: Resource> Index<R> {
	pub fn new(repository: u8, category: u8, resource: Arc<R>) -> Result<Self> {
		Ok(Self {
			repository,
			category,
			resource,
			max_chunk: None.into(),
			chunks: Vec::new().into(),
		})
	}

	/// Every file this category records, across all of its chunks.
	pub fn entries(&self) -> Result<Vec<IndexEntry>> {
		let mut entries = Vec::new();
		for chunk in self.chunks() {
			let (index, chunk) = chunk?;
			let repository = self.repository;
			let category = self.category;
			match &*chunk {
				IndexChunk::Index1(file) => entries.extend(file.hashes().map(|hash| IndexEntry {
					repository,
					category,
					chunk: index,
					hash: IndexHash::Split(hash),
				})),
				IndexChunk::Index2(file) => entries.extend(file.hashes().map(|hash| IndexEntry {
					repository,
					category,
					chunk: index,
					hash: IndexHash::Whole(hash),
				})),
			}
		}
		Ok(entries)
	}

	/// Locate a file by its index hash.
	pub fn find_hash(&self, hash: IndexHash) -> Result<Location> {
		let location = self.chunks().find_map(|chunk| {
			let (index, chunk) = match chunk {
				Ok(value) => value,
				Err(error) => return Some(Err(error)),
			};

			chunk.find_hash(hash).map(|(meta, size)| {
				Ok(Location {
					chunk: index,
					data_file: meta.data_file_id,
					offset: meta.offset,
					size,
				})
			})
		});

		match location {
			None => Err(Error::NotFound(ErrorValue::Other(format!("hash {hash:?}")))),
			Some(result) => result,
		}
	}

	pub fn find(&self, path: &str) -> Result<Location> {
		let location = self.chunks().find_map(|chunk| {
			let (index, chunk) = match chunk {
				Ok(value) => value,
				Err(error) => return Some(Err(error)),
			};

			match chunk.find(path) {
				Err(Error::NotFound(_)) => None,
				Err(error) => Some(Err(error)),
				Ok((meta, size)) => Some(Ok(Location {
					chunk: index,
					data_file: meta.data_file_id,
					offset: meta.offset,
					size,
				})),
			}
		});

		match location {
			None => Err(Error::NotFound(ErrorValue::Path(path.into()))),
			Some(result) => result,
		}
	}

	fn chunks(&self) -> impl Iterator<Item = Result<(u8, Arc<IndexChunk>)>> + '_ {
		// Get the max known chunk ID. If we don't know it, we want to loop the full potential ID space (u8).
		let guard = self.max_chunk.lock().unwrap();
		let max_chunk = guard.unwrap_or(256);
		drop(guard);

		(0u16..max_chunk)
			.scan(0u16, |misses, index| {
				let index_usize = usize::from(index);
				let index_u8 = u8::try_from(index).unwrap();

				// If we've already decided about this chunk index, use that.
				let guard = self.chunks.lock().unwrap();
				if let Some(chunk) = guard.get(index_usize) {
					return Some(chunk.clone().map(|chunk| Ok((index_u8, chunk))));
				}
				drop(guard);

				// Try to build a new chunk.
				let chunk = IndexChunk::new(
					self.repository,
					self.category,
					index.try_into().unwrap(),
					&*self.resource,
				);

				match chunk {
					// Found an index - save it out to the cache.
					Ok(chunk) => {
						*misses = 0;
						let mut guard = self.chunks.lock().unwrap();
						// The lock was released while reading, so another lookup may have stored this
						// chunk already.
						if guard.len() == index_usize {
							guard.push(Some(chunk.into()));
						}
						Some(
							guard
								.get(index_usize)
								.and_then(|chunk| chunk.clone())
								.map(|chunk| Ok((index_u8, chunk))),
						)
					}

					// No index at this ID. Chunk IDs are not contiguous in live data, so keep
					// looking; only a run of misses means we have run off the end.
					Err(Error::NotFound(_)) => {
						*misses += 1;
						let mut guard = self.chunks.lock().unwrap();
						if guard.len() == index_usize {
							guard.push(None);
						}
						drop(guard);

						match *misses >= CHUNK_MISS_TOLERANCE {
							// Remember where the last real chunk was so we stop here next time.
							true => {
								*self.max_chunk.lock().unwrap() = Some(index + 1 - *misses);
								None
							}
							false => Some(None),
						}
					}

					// Some other error occured, surface it.
					Err(error) => Some(Some(Err(error))),
				}
			})
			.flatten()
	}
}

#[derive(Debug)]
enum IndexChunk {
	Index1(Index1),
	Index2(Index2),
}

impl IndexChunk {
	fn new<R: Resource>(repository: u8, category: u8, chunk: u8, resource: &R) -> Result<Self> {
		let index1 = resource
			.index(repository, category, chunk)
			.and_then(|mut reader| Ok(IndexChunk::Index1(Index1::read(&mut reader)?)));

		match index1 {
			Err(Error::NotFound(_)) => resource
				.index2(repository, category, chunk)
				.and_then(|mut reader| Ok(IndexChunk::Index2(Index2::read(&mut reader)?))),
			result => result,
		}
	}

	fn find(&self, path: &str) -> Result<(FileMetadata, Option<u64>)> {
		match self {
			Self::Index1(index) => index.find(path),
			Self::Index2(index) => index.find(path),
		}
	}

	fn find_hash(&self, hash: IndexHash) -> Option<(FileMetadata, Option<u64>)> {
		match (self, hash) {
			(Self::Index1(index), IndexHash::Split(hash)) => index.find_hash(hash),
			(Self::Index2(index), IndexHash::Whole(hash)) => index.find_hash(hash),
			_ => None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::IndexHash;

	#[test]
	fn directory_matches_the_upper_half_of_a_split_hash() {
		for (dir, file) in [
			("music/ffxiv", "BGM_Null.scd"),
			("exd", "root.exl"),
			("common/savedata", "anything.dat"),
		] {
			let Some(IndexHash::Split(split)) = IndexHash::of(&format!("{dir}/{file}")).0 else {
				panic!("no split hash for {dir}/{file}");
			};
			assert_eq!(IndexHash::directory(dir), (split >> 32) as u32, "{dir}");
		}
	}
}
