use std::io::Empty;

use futures_util::{AsyncRead, AsyncReadExt, AsyncSeek};

use crate::error::{Error, ErrorValue, Result};

use super::shared::Header;

pub async fn read<R: AsyncRead + AsyncSeek + Unpin>(
	mut reader: R,
	header: Header,
) -> Result<Empty> {
	let mut buf = vec![0; header.raw_file_size.try_into().unwrap()];
	reader.read_exact(&mut buf).await?;

	// .take(header.raw_file_size.into())
	// .read_to_end(&mut buf)?;

	// TODO: if type 1 and first 64 == second 64, RSF
	//       if type 1 and first 64 == [0..], empty

	// Empty files can't be read as-is - they're either entirely invalid, or need
	// further processing that doesn't belong in sqpack specifically.
	Err(Error::Invalid(
		ErrorValue::File(buf),
		String::from("Empty file"),
	))
}
