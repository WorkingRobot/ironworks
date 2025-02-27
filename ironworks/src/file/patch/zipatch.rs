use std::{
	io::SeekFrom,
	sync::{Arc, Mutex},
};

use binrw::BinRead;
use derivative::Derivative;

use crate::{FileStream, error::Result, file::File};

use super::chunk::Chunk;

const ZIPATCH_MAGIC: &[u8; 12] = b"\x91ZIPATCH\x0D\x0A\x1A\x0A";

/// ZiPatch incremental patch file format.
///
/// This file format contains a significant number of small headers interspersed
/// between payloads, and will _heavily_ benefit from buffered reading or
/// memory-mapped files.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct ZiPatch {
	#[derivative(Debug = "ignore")]
	stream: Arc<Mutex<Box<dyn FileStream>>>,
}

impl ZiPatch {
	/// Get an iterator over the chunks within this patch file.
	pub fn chunks(&self) -> ChunkIterator {
		ChunkIterator::new(self.stream.clone())
	}
}

impl File for ZiPatch {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		// Check the magic in the header
		let mut magic = [0u8; ZIPATCH_MAGIC.len()];
		stream.read_exact(&mut magic)?;

		if &magic != ZIPATCH_MAGIC {
			todo!("error message")
		}

		// Rest of the file is chunks that we'll read lazily.
		Ok(Self {
			// TODO: I'm really not happy with this incantation
			stream: Arc::new(Mutex::new(Box::new(stream))),
		})
	}
}

/// Iterator over the chunks within a patch file.
///
/// Chunks are read lazily from the source stream over the course of iteration.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct ChunkIterator {
	#[derivative(Debug = "ignore")]
	stream: Arc<Mutex<Box<dyn FileStream>>>,
	offset: u64,
	complete: bool,
}

impl ChunkIterator {
	fn new(stream: Arc<Mutex<Box<dyn FileStream>>>) -> Self {
		ChunkIterator {
			stream,
			offset: ZIPATCH_MAGIC.len().try_into().unwrap(),
			complete: false,
		}
	}

	fn read_chunk(&mut self) -> Result<Chunk> {
		let mut handle = self.stream.lock().unwrap();

		// Seek to last known offset - in a tight loop this is effectively a noop,
		// but need to make sure if there's stuff jumping around.
		// TODO: lots of jumping around would be catastrophic for read performance - it'd be nice to be able to request something cloneable, so i.e. file handles could be cloned between chunk iterators, rather than trying to share access to a single one - but i'm not sure how to mode that without major refactors.
		handle.seek(SeekFrom::Start(self.offset))?;

		let size = u32::read_be(&mut *handle)?;

		let chunk = Chunk::read_args(&mut *handle, (size,))?;

		// Update iterator offset to the start of the next chunk. `size` only represents
		// the size of the chunk data itself, so the +12 is to account for the other
		// fields in the container.
		self.offset += u64::from(size) + 12;

		// TODO: check the hash? is it worth it? I'd need to relock the stream for that...

		Ok(chunk)
	}
}

impl Iterator for ChunkIterator {
	type Item = Result<Chunk>;

	fn next(&mut self) -> Option<Self::Item> {
		if self.complete {
			return None;
		}

		let chunk = self.read_chunk();

		if let Ok(Chunk::EndOfFile) = chunk {
			self.complete = true;
		}

		Some(chunk)
	}
}
