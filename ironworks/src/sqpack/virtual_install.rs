use async_trait::async_trait;
use futures_util::{AsyncSeekExt, io};

use crate::{
	error::{Error, ErrorValue, Result},
	utility::{TakeSeekable, TakeSeekableExt},
};

use super::{Location, Resource, install::Platform};

#[async_trait(?Send)]
pub trait VirtualFilesystem {
	type File: io::AsyncRead + io::AsyncSeek + Unpin + 'static;

	async fn exists(&self, path: &str) -> bool;
	async fn read_to_string(&self, path: &str) -> std::io::Result<String>;
	async fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
	async fn open(&self, path: &str) -> std::io::Result<Self::File>;
}

/// SqPack resource for reading game data from a virtual FFXIV installation.
#[derive(Debug)]
pub struct VirtualInstall<V: VirtualFilesystem> {
	vfs: V,
	repositories: Vec<Option<String>>,
	platform: Platform,
}

impl<V: VirtualFilesystem> VirtualInstall<V> {
	pub async fn at_sqpack(vfs: V) -> Self {
		let repositories = find_repositories(&vfs).await;

		Self {
			vfs,
			repositories,
			platform: Platform::Win32,
		}
	}

	pub fn vfs(&self) -> &V {
		&self.vfs
	}

	fn build_file_path(
		&self,
		repository: u8,
		category: u8,
		chunk: u8,
		extension: &str,
	) -> Result<String> {
		let platform = match self.platform {
			Platform::Win32 => "win32",
			Platform::PS3 => todo!("PS3 platform"),
			Platform::PS4 => todo!("PS4 platform"),
		};

		let file_name = format!("{category:02x}{repository:02x}{chunk:02x}.{platform}.{extension}");

		let file_path = format!("{}/{}", self.get_repository_name(repository)?, &file_name);

		Ok(file_path)
	}

	fn get_repository_name(&self, repository: u8) -> Result<&String> {
		self.repositories
			.get(usize::from(repository))
			.and_then(|option| option.as_ref())
			.ok_or_else(|| Error::NotFound(ErrorValue::Other(format!("repository {repository}"))))
	}
}

#[async_trait(?Send)]
impl<V: VirtualFilesystem> Resource for VirtualInstall<V> {
	async fn version(&self, repository: u8) -> Result<String> {
		let path = match repository {
			0 => "../ffxivgame.ver".to_owned(),
			repo => {
				let repository_name = self.get_repository_name(repo)?;
				format!("{repository_name}/{repository_name}.ver")
			}
		};

		Ok(self.vfs.read_to_string(&path).await?)
	}

	type Index = io::Cursor<Vec<u8>>;
	async fn index(&self, repository: u8, category: u8, chunk: u8) -> Result<Self::Index> {
		read_index(
			&self.vfs,
			&self.build_file_path(repository, category, chunk, "index")?,
		)
		.await
	}

	type Index2 = io::Cursor<Vec<u8>>;
	async fn index2(&self, repository: u8, category: u8, chunk: u8) -> Result<Self::Index2> {
		read_index(
			&self.vfs,
			&self.build_file_path(repository, category, chunk, "index2")?,
		)
		.await
	}

	type File = TakeSeekable<io::BufReader<V::File>>;
	async fn file(&self, repository: u8, category: u8, location: Location) -> Result<Self::File> {
		let path = self.build_file_path(
			repository,
			category,
			location.chunk(),
			&format!("dat{}", location.data_file()),
		)?;
		let mut file = io::BufReader::new(self.vfs.open(&path).await?);

		let offset = u64::from(location.offset());
		// Resolve the size early in case we need to seek to find the end. Using
		// longhand here so I can shortcut seek failures.
		let size = match location.size() {
			Some(size) => u64::from(size),
			None => file.seek(io::SeekFrom::End(0)).await? - offset,
		};

		file.seek(io::SeekFrom::Start(offset)).await?;

		Ok(file.take_seekable(size)?)
	}
}

async fn find_repositories(vfs: &impl VirtualFilesystem) -> Vec<Option<String>> {
	futures_util::future::join_all((0..=9).map(|index| {
		let name = match index {
			0 => "ffxiv".into(),
			other => format!("ex{other}"),
		};

		async { vfs.exists(&name).await.then_some(name) }
	}))
	.await
}

async fn read_index(vfs: &impl VirtualFilesystem, path: &str) -> Result<io::Cursor<Vec<u8>>> {
	// Read the entire index into memory before returning - we typically need
	// the full dataset anyway, and working directly on a File causes significant
	// slowdowns due to IO syscalls.
	let buffer = vfs.read(&path).await.map_err(|error| match error.kind() {
		io::ErrorKind::NotFound => {
			Error::NotFound(ErrorValue::Other(format!("file path {path:?}")))
		}
		_ => Error::Resource(error.into()),
	})?;
	Ok(io::Cursor::new(buffer))
}
