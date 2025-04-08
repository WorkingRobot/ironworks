use std::io::SeekFrom;

use binrw::binread;
use futures_util::{AsyncRead, AsyncSeek, AsyncSeekExt};

use crate::{
	error::Result,
	sqpack::block::{BlockHeader, BlockMetadata, BlockStream},
	utility::{SizedRead, SizedReadExt},
};

use super::shared::Header;

#[binread]
#[derive(Debug)]
#[br(little)]
struct BlockInfo {
	offset: u32,
	_input_size: u16,
	output_size: u16,
}

impl SizedRead for BlockInfo {
	const SIZE: usize = 8;
}

pub async fn read<R: AsyncRead + AsyncSeek + Unpin>(
	mut reader: R,
	offset: u32,
	header: Header,
) -> Result<BlockStream<R>> {
	// Eagerly read the block info.
	let mut blocks = Vec::with_capacity(header.block_count.try_into().unwrap());
	for _ in 0..header.block_count {
		let block = BlockInfo::read_async(&mut reader).await?;
		blocks.push(block);
	}

	let mut metadata = Vec::with_capacity(blocks.len());

	// Read in the block headers to build the metadata needed for the reader.
	let mut previous = 0usize;
	for info in &blocks {
		let output_offset = previous;
		previous += usize::from(info.output_size);

		let header_offset = offset + info.offset;
		reader.seek(SeekFrom::Start(header_offset.into())).await?;
		let header = BlockHeader::read_async(&mut reader).await?;

		let meta = BlockMetadata {
			input_offset: (header_offset + header.size).try_into().unwrap(),
			input_size: header.compressed_size.try_into().unwrap(),
			output_offset,
			output_size: info.output_size.into(),
		};
		metadata.push(meta);
	}

	Ok(BlockStream::new(reader, 0, metadata))
}
