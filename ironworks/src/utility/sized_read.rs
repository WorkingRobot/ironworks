use async_trait::async_trait;
use binrw::{BinRead, BinResult, meta::ReadEndian};
use futures_util::{AsyncRead, AsyncReadExt};

pub trait SizedRead: BinRead {
	const SIZE: usize;
}

#[async_trait(?Send)]
pub trait SizedReadExt: SizedRead {
	async fn read_async(reader: &mut (impl AsyncRead + Unpin)) -> BinResult<Self>
	where
		Self: ReadEndian,
		for<'a> Self::Args<'a>: Default;
}

#[async_trait(?Send)]
impl<R: SizedRead> SizedReadExt for R {
	async fn read_async(reader: &mut (impl AsyncRead + Unpin)) -> BinResult<Self>
	where
		Self: ReadEndian,
		for<'a> Self::Args<'a>: Default,
	{
		let mut buf = vec![0; Self::SIZE];
		reader.read_exact(&mut buf).await?;
		let mut cursor = std::io::Cursor::new(buf);
		R::read(&mut cursor)
	}
}
