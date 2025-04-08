use futures_util::io;
use futures_util::{AsyncSeekExt, ready};
use std::{cmp, task::Poll};

pub trait TakeSeekableExt: io::AsyncRead + io::AsyncSeek + Unpin + Sized {
	async fn take_seekable(self, limit: u64) -> io::Result<TakeSeekable<Self>>;
}

impl<R: io::AsyncRead + io::AsyncSeek + Unpin> TakeSeekableExt for R {
	async fn take_seekable(mut self, limit: u64) -> io::Result<TakeSeekable<Self>> {
		let offset = self.stream_position().await?;
		Ok(TakeSeekable {
			inner: self,
			current: 0,
			offset,
			limit,
		})
	}
}

/// Reader adapter which limits the bytes read from an underlying reader, and provides seeking capabilities.
///
/// This struct is created by calling `TakeSeekableExt::take_seekable` on a seekable reader.
#[derive(Debug)]
pub struct TakeSeekable<R> {
	inner: R,
	current: u64,
	offset: u64,
	limit: u64,
}

impl<R: io::AsyncRead> io::AsyncRead for TakeSeekable<R> {
	fn poll_read(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
		buf: &mut [u8],
	) -> Poll<std::io::Result<usize>> {
		// Don't call into inner reader at all at EOF because it may still block
		if self.current >= self.limit {
			return Poll::Ready(Ok(0));
		}

		let remaining = self.limit - self.current;

		let max = cmp::min(buf.len() as u64, remaining) as usize;
		let bytes_read = ready!(self.get_mut().inner.poll_read(cx, &mut buf[..max]))?;
		assert!(
			bytes_read as u64 <= remaining,
			"number of read bytes exceeds limit"
		);
		self.current += bytes_read as u64;
		Poll::Ready(Ok(bytes_read))
	}
}

impl<S: io::AsyncSeek> io::AsyncSeek for TakeSeekable<S> {
	fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
		let (base, position) = match position {
			io::SeekFrom::Start(position) => {
				let inner_offset = self
					.inner
					.seek(io::SeekFrom::Start(self.offset + position))?;
				self.current = inner_offset - self.offset;
				return Ok(self.current);
			}
			io::SeekFrom::Current(position) => (self.current, position),
			io::SeekFrom::End(position) => (self.limit, position),
		};

		let ioffset = i128::from(base)
			.checked_add(position.into())
			.ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::InvalidInput,
					"invalid seek to an overflowing position",
				)
			})?;

		if ioffset < 0 {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"invalid seek to a negative position",
			));
		}

		let inner_offset = self.inner.seek(io::SeekFrom::Start(
			self.offset + u64::try_from(ioffset).unwrap(),
		))?;

		self.current = inner_offset - self.offset;
		Ok(self.current)
	}
}
