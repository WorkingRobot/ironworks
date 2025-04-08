use async_trait::async_trait;
use futures_util::{AsyncRead, AsyncSeek};

use crate::error::Result;

use super::index::Location;

/// Resource adapter to fetch information and data on request for a SqPack instance.
#[async_trait(?Send)]
pub trait Resource {
	/// Get the version string for a given repository.
	async fn version(&self, repository: u8) -> Result<String>;

	/// The type of an index resource.
	type Index: AsyncRead + AsyncSeek;
	/// Fetches the specified index resource.
	async fn index(&self, repository: u8, category: u8, chunk: u8) -> Result<Self::Index>;

	/// The type of an index2 resource.
	type Index2: AsyncRead + AsyncSeek;
	/// Fetches the specified index2 resource.
	async fn index2(&self, repository: u8, category: u8, chunk: u8) -> Result<Self::Index2>;

	/// The type of a file reader resource.
	type File: AsyncRead + AsyncSeek;
	/// Fetch a reader for the specified file from a dat container.
	async fn file(&self, repository: u8, category: u8, location: Location) -> Result<Self::File>;
}
