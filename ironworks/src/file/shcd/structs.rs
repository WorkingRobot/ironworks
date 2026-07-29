use binrw::binread;

/// Versions from this one on carry textures and unordered access views, and a word per shader entry.
pub const VERSION_RESOURCES: u32 = 0x0601;

/// The file's root header. The version is three bytes wide, the fourth carrying the stage, so a
/// version reads a byte narrower than the .shpk one it otherwise resembles.
#[binread]
#[br(little, magic = b"ShCd")]
#[derive(Debug)]
pub struct Header {
	pub version: [u8; 3],
	pub stage: u8,

	pub directx: [u8; 4],

	pub total_size: u32,

	pub blob_offset: u32,
	pub strings_offset: u32,
}

#[binread]
#[br(little, import(version: u32))]
#[derive(Debug)]
pub struct Shader {
	pub blob_offset: u32,
	pub blob_size: u32,

	pub constant_count: u16,
	pub sampler_count: u16,

	#[br(if(version >= VERSION_RESOURCES))]
	pub uav_count: u16,
	#[br(if(version >= VERSION_RESOURCES))]
	pub texture_count: u16,

	#[br(if(version >= VERSION_RESOURCES))]
	_unknown: u32,
}

/// A constant buffer, sampler, texture or unordered access view.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy)]
pub struct Resource {
	pub id: u32,
	pub string_offset: u32,
	pub string_length: u16,
	pub kind: u16,
	pub slot: u16,
	pub size: u16,
}

impl Resource {
	pub const SIZE: usize = 16;
}
