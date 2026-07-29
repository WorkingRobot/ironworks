use std::fmt;

use binrw::BinRead;
use getset::CopyGetters;

use crate::{FileStream, error::Result, file::File};

use super::{cursor, extent, invalid, structs, table};

/// A package of compiled shaders, as named by a material.
/// Shader bytecode is not kept.
pub struct ShaderPackage {
	version: u32,
	directx: DirectX,
	shaders: Vec<Shader>,
	param_buffer_size: u32,
	material_params: Vec<MaterialParam>,
	param_defaults: Vec<f32>,
	constants: Vec<Resource>,
	samplers: Vec<Resource>,
	textures: Vec<Resource>,
	uavs: Vec<Resource>,
	system_keys: Vec<Key>,
	scene_keys: Vec<Key>,
	material_keys: Vec<Key>,
	subview_defaults: [u32; 2],
	nodes: Vec<Node>,
	aliases: Vec<NodeAlias>,
	clusters: Vec<AliasCluster>,
	blobs_offset: usize,
	bytecode_size: usize,
	strings: Vec<u8>,
}

/// A shader stage a pass does not use.
pub const NONE: u32 = u32::MAX;

/// One combination of key values, and the shaders it selects.
///
/// This is a variant: the values it holds are the conditions its shaders were compiled under, so
/// reading them against the package's key list says what a shader is for rather than merely which
/// number it has.
#[derive(Debug)]
pub struct Node {
	id: u32,
	keys: Vec<u32>,
	passes: Vec<Pass>,
}

impl Node {
	pub fn id(&self) -> u32 {
		self.id
	}

	/// A value for each of the package's keys, in the order it declares them: system, then scene,
	/// then material, then the two subview keys.
	pub fn keys(&self) -> &[u32] {
		&self.keys
	}

	pub fn passes(&self) -> &[Pass] {
		&self.passes
	}
}

/// One pass of a variant, and the shader each stage runs for it. A stage the pass does not use is
/// [`NONE`], as are the three later stages on a package that predates them.
#[derive(Debug, Clone, Copy)]
pub struct Pass {
	id: u32,
	vertex: u32,
	pixel: u32,
	geometry: u32,
	hull: u32,
	domain: u32,
}

impl Pass {
	pub fn id(&self) -> u32 {
		self.id
	}

	/// The shader each stage runs, in the order the package stores its own shader lists.
	pub fn stages(&self) -> [u32; 5] {
		[
			self.vertex,
			self.pixel,
			self.hull,
			self.domain,
			self.geometry,
		]
	}

	pub fn vertex(&self) -> u32 {
		self.vertex
	}

	pub fn pixel(&self) -> u32 {
		self.pixel
	}
}

/// A selector that resolves to a node rather than carrying its own key values.
#[derive(Debug, Clone, Copy)]
pub struct NodeAlias {
	selector: u32,
	node: u32,
}

impl NodeAlias {
	pub fn selector(&self) -> u32 {
		self.selector
	}

	/// Which node the selector stands for.
	pub fn node(&self) -> u32 {
		self.node
	}
}

/// A grouping of aliases. Not sure what this is for.
#[derive(Debug)]
pub struct AliasCluster {
	id: u32,
	subclusters: Vec<SubCluster>,
}

impl AliasCluster {
	pub fn id(&self) -> u32 {
		self.id
	}

	pub fn subclusters(&self) -> &[SubCluster] {
		&self.subclusters
	}
}

#[derive(Debug)]
pub struct SubCluster {
	index: u16,
	alias_count: u16,
	data: Vec<u32>,
}

impl SubCluster {
	pub fn index(&self) -> u16 {
		self.index
	}

	pub fn alias_count(&self) -> u16 {
		self.alias_count
	}

	pub fn data(&self) -> &[u32] {
		&self.data
	}
}

impl ShaderPackage {
	pub fn version(&self) -> u32 {
		self.version
	}

	/// Which DirectX the shaders were compiled for.
	pub fn directx(&self) -> DirectX {
		self.directx
	}

	/// Every shader, vertex stage first, in the order the file lists them.
	pub fn shaders(&self) -> &[Shader] {
		&self.shaders
	}

	/// Bytes the material parameter buffer takes, which the parameters below carve up.
	pub fn param_buffer_size(&self) -> u32 {
		self.param_buffer_size
	}

	/// The layout of the material parameter buffer. A material's constants name these by id.
	pub fn material_params(&self) -> &[MaterialParam] {
		&self.material_params
	}

	/// The shader's defaults for the whole parameter buffer, or empty where it carries none.
	/// Indexed by float, so a parameter's byte offset over four indexes into it.
	pub fn param_defaults(&self) -> &[f32] {
		&self.param_defaults
	}

	/// Constant buffers the package binds.
	pub fn constants(&self) -> &[Resource] {
		&self.constants
	}

	/// Samplers the package binds.
	pub fn samplers(&self) -> &[Resource] {
		&self.samplers
	}

	/// Textures the package binds.
	pub fn textures(&self) -> &[Resource] {
		&self.textures
	}

	/// Unordered access views the package binds.
	pub fn uavs(&self) -> &[Resource] {
		&self.uavs
	}

	/// Keys the game sets from engine state.
	pub fn system_keys(&self) -> &[Key] {
		&self.system_keys
	}

	/// Keys the game sets per scene.
	pub fn scene_keys(&self) -> &[Key] {
		&self.scene_keys
	}

	/// Keys a material sets, which is what a material's shader keys select against.
	pub fn material_keys(&self) -> &[Key] {
		&self.material_keys
	}

	/// Defaults for the two subview keys.
	pub fn subview_defaults(&self) -> [u32; 2] {
		self.subview_defaults
	}

	/// Every combination of key values the package knows, and the shaders each one selects. This is
	/// what turns a shader from a number into the variant it is: the values a node holds are the
	/// conditions the source was compiled under.
	pub fn nodes(&self) -> &[Node] {
		&self.nodes
	}

	/// Selectors standing in for a node, where a combination resolves to one already listed.
	pub fn aliases(&self) -> &[NodeAlias] {
		&self.aliases
	}

	pub fn clusters(&self) -> &[AliasCluster] {
		&self.clusters
	}

	/// Where the blob section begins in the file the package was read from. A shader's
	/// [`blob_offset`](Shader::blob_offset) is relative to this, so the two locate its bytecode in
	/// the original bytes.
	pub fn blobs_offset(&self) -> usize {
		self.blobs_offset
	}

	/// Bytes of compiled bytecode the file carries.
	pub fn bytecode_size(&self) -> usize {
		self.bytecode_size
	}

	/// The name of a resource, or `None` where it points outside the string block.
	pub fn name(&self, resource: &Resource) -> Option<&str> {
		let start = to_usize(resource.string_offset);
		let end = start.checked_add(usize::from(resource.string_length))?;
		std::str::from_utf8(self.strings.get(start..end)?).ok()
	}

	/// The shader's default for a material parameter, or `None` where the package carries no
	/// defaults or the parameter points outside the buffer.
	pub fn param_default(&self, param: &MaterialParam) -> Option<&[f32]> {
		let start = usize::from(param.byte_offset) / 4;
		let len = usize::from(param.byte_size) / 4;
		self.param_defaults.get(start..start.checked_add(len)?)
	}
}

impl ShaderPackage {
	pub fn parse(bytes: &[u8]) -> Result<Self> {
		let mut head = cursor(bytes, 0)?;
		let header = structs::Header::read(&mut head)?;
		let mut at = to_usize(u32::try_from(head.position()).expect("header is small"));

		if bytes.len() < to_usize(header.total_size) {
			return Err(invalid(format!(
				"file declares {} bytes but carries {}",
				header.total_size,
				bytes.len()
			)));
		}

		let blobs = to_usize(header.blobs_offset);
		let strings_at = to_usize(header.strings_offset);
		if blobs > strings_at || strings_at > bytes.len() {
			return Err(invalid(format!(
				"blob section {blobs:#x} and string block {strings_at:#x} do not fall in the file in that order"
			)));
		}

		let mut shaders = Vec::new();
		for (stage, count) in [
			(Stage::Vertex, header.vertex_count),
			(Stage::Pixel, header.pixel_count),
			(Stage::Hull, header.hull_count),
			(Stage::Domain, header.domain_count),
			(Stage::Geometry, header.geometry_count),
		] {
			for _ in 0..count {
				let mut entry = cursor(bytes, at)?;
				let shader = structs::Shader::read_args(&mut entry, (header.version,))?;
				at += to_usize(u32::try_from(entry.position()).expect("shader header is small"));

				let constants = usize::from(shader.constant_count);
				let samplers = constants + usize::from(shader.sampler_count);
				let textures = samplers + usize::from(shader.texture_count);
				let bound = textures + usize::from(shader.uav_count);
				shaders.push(Shader {
					stage,
					blob_offset: shader.blob_offset,
					blob_size: shader.blob_size,
					bounds: [constants, samplers, textures],
					resources: resources(bytes, &mut at, bound, "shader resource table")?,
				});
			}
		}

		let (material_params, end) = table::<structs::MaterialParam>(
			bytes,
			at,
			usize::from(header.material_param_count),
			structs::MaterialParam::SIZE,
			"material parameter table",
		)?;
		at = end;

		let param_defaults = match header.has_param_defaults {
			0 => Vec::new(),
			_ => {
				let size = to_usize(header.material_params_size);
				let end = extent(bytes, at, 1, size, "material parameter defaults")?;
				let values = bytes[at..end]
					.chunks_exact(4)
					.map(|word| f32::from_le_bytes(word.try_into().expect("chunk is four bytes")))
					.collect();
				at = end;
				values
			}
		};

		let constants = resources(
			bytes,
			&mut at,
			to_usize(header.constant_count),
			"constant table",
		)?;
		let samplers = resources(
			bytes,
			&mut at,
			usize::from(header.sampler_count),
			"sampler table",
		)?;
		let textures = resources(
			bytes,
			&mut at,
			usize::from(header.texture_count),
			"texture table",
		)?;
		let uavs = resources(
			bytes,
			&mut at,
			to_usize(header.uav_count),
			"unordered access view table",
		)?;

		let system_keys = keys(bytes, &mut at, header.system_key_count, "system key table")?;
		let scene_keys = keys(bytes, &mut at, header.scene_key_count, "scene key table")?;
		let material_keys = keys(
			bytes,
			&mut at,
			header.material_key_count,
			"material key table",
		)?;

		let end = extent(bytes, at, 2, 4, "subview key defaults")?;
		let subview_defaults = [word(bytes, at), word(bytes, at + 4)];
		at = end;

		let (nodes, aliases, clusters, at) = read_selectors(bytes, at, &header)?;

		// Nothing in the file declares its own size, so this is the only thing that catches a table
		// walked with the wrong stride: everything above must land exactly where the blobs begin.
		if at != blobs {
			return Err(invalid(format!(
				"tables end at {at:#x}, where the blob section starts at {blobs:#x}"
			)));
		}

		Ok(Self {
			version: header.version,
			directx: DirectX::from(header.directx),
			shaders,
			param_buffer_size: header.material_params_size,
			material_params: material_params
				.into_iter()
				.map(|param| MaterialParam {
					id: param.id,
					byte_offset: param.byte_offset,
					byte_size: param.byte_size,
				})
				.collect(),
			param_defaults,
			constants,
			samplers,
			textures,
			uavs,
			system_keys,
			scene_keys,
			material_keys,
			subview_defaults,
			nodes,
			aliases,
			clusters,
			blobs_offset: blobs,
			bytecode_size: strings_at - blobs,
			strings: bytes[strings_at..].to_vec(),
		})
	}
}

impl File for ShaderPackage {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		let mut bytes = Vec::new();
		stream.read_to_end(&mut bytes)?;
		Self::parse(&bytes)
	}
}

impl fmt::Debug for ShaderPackage {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("ShaderPackage")
			.field("version", &format_args!("{:#06x}", self.version))
			.field("directx", &self.directx)
			.field("shaders", &self.shaders.len())
			.field("material_params", &self.material_params.len())
			.field("constants", &self.constants.len())
			.field("samplers", &self.samplers.len())
			.field("textures", &self.textures.len())
			.field("bytecode_size", &self.bytecode_size)
			.finish_non_exhaustive()
	}
}

/// Which DirectX a package's shaders were compiled for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectX {
	Dx9,
	Dx11,
	/// A tag ironworks does not recognise.
	Unknown([u8; 4]),
}

impl From<[u8; 4]> for DirectX {
	fn from(value: [u8; 4]) -> Self {
		match &value {
			b"DX9\0" => Self::Dx9,
			b"DX11" => Self::Dx11,
			_ => Self::Unknown(value),
		}
	}
}

/// Which pipeline stage a shader runs at.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
	Vertex,
	Pixel,
	Hull,
	Domain,
	Geometry,
}

/// One compiled shader.
#[derive(Debug, Clone, CopyGetters)]
pub struct Shader {
	#[get_copy = "pub"]
	stage: Stage,

	/// Where the bytecode sits within the blob section, and how long it is. A vertex shader's blob
	/// opens with an extra header, 4 bytes under DX9 and 8 bytes under DX11.
	#[get_copy = "pub"]
	blob_offset: u32,
	#[get_copy = "pub"]
	blob_size: u32,

	/// Where the flat resource list divides, as a running total: constants, samplers, unordered
	/// access views, then textures.
	bounds: [usize; 3],

	resources: Vec<Resource>,
}

impl Shader {
	/// What this shader binds: constant buffers, then samplers, unordered access views and textures.
	pub fn resources(&self) -> &[Resource] {
		&self.resources
	}

	/// The constant buffers this shader binds. A slot means nothing without the kind, and nothing
	/// without the shader either: the same buffer sits at different slots in different shaders of
	/// one package.
	pub fn constants(&self) -> &[Resource] {
		&self.resources[..self.bounds[0]]
	}

	/// The samplers this shader binds.
	pub fn samplers(&self) -> &[Resource] {
		&self.resources[self.bounds[0]..self.bounds[1]]
	}

	/// The textures this shader binds.
	pub fn textures(&self) -> &[Resource] {
		&self.resources[self.bounds[1]..self.bounds[2]]
	}

	/// The unordered access views this shader binds.
	pub fn uavs(&self) -> &[Resource] {
		&self.resources[self.bounds[2]..]
	}
}

/// A constant buffer, sampler, texture or unordered access view.
///
/// The name lives in the package's string block; [`ShaderPackage::name`] resolves it.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Resource {
	/// The crc32 of the resource's name.
	id: u32,

	string_offset: u32,
	string_length: u16,

	/// Zero for a constant buffer or sampler and one for a texture.
	is_texture: u16,

	/// The register the resource binds to.
	slot: u16,

	/// Size in registers of 16 bytes for a constant buffer, and 1 otherwise.
	size: u16,
}

/// One value in the material parameter buffer.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct MaterialParam {
	/// The crc32 of the param's name. A material's constants use the same ids.
	id: u32,

	/// Byte offset into the parameter buffer, and the length taken from it. 12 is a vec3.
	byte_offset: u16,
	byte_size: u16,
}

/// A switch selecting between shader variants.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Key {
	/// The crc32 of the key's name.
	id: u32,

	/// The value used where nothing sets the key.
	default_value: u32,
}

fn resources(bytes: &[u8], at: &mut usize, count: usize, what: &str) -> Result<Vec<Resource>> {
	let (records, end) =
		table::<structs::Resource>(bytes, *at, count, structs::Resource::SIZE, what)?;
	*at = end;
	Ok(records
		.into_iter()
		.map(|record| Resource {
			id: record.id,
			string_offset: record.string_offset,
			string_length: record.string_length,
			is_texture: record.is_texture,
			slot: record.slot,
			size: record.size,
		})
		.collect())
}

fn keys(bytes: &[u8], at: &mut usize, count: u32, what: &str) -> Result<Vec<Key>> {
	let (records, end) =
		table::<structs::Key>(bytes, *at, to_usize(count), structs::Key::SIZE, what)?;
	*at = end;
	Ok(records
		.into_iter()
		.map(|record| Key {
			id: record.id,
			default_value: record.default_value,
		})
		.collect())
}

/// Read the node, alias and cluster tables, returning them and where they end.
fn read_selectors(
	bytes: &[u8],
	mut at: usize,
	header: &structs::Header,
) -> Result<(Vec<Node>, Vec<NodeAlias>, Vec<AliasCluster>, usize)> {
	let tessellated = header.version >= structs::VERSION_TESSELLATION;
	// The later generation widened the slot table and gave a pass a shader for each of the three
	// stages it gained.
	let node_prefix = if tessellated { 32 } else { 24 };
	let pass_words = if tessellated { 6 } else { 3 };

	// A node carries a value for every key the package declares, plus the two subview keys.
	let key_count = to_usize(header.system_key_count)
		+ to_usize(header.scene_key_count)
		+ to_usize(header.material_key_count)
		+ 2;

	let mut nodes = Vec::with_capacity(to_usize(header.node_count));
	for _ in 0..header.node_count {
		let head = extent(bytes, at, 1, node_prefix + key_count * 4, "node")?;
		let count = to_usize(word(bytes, at + 4));
		let end = extent(bytes, head, count, pass_words * 4, "node pass table")?;

		let keys = (0..key_count)
			.map(|index| word(bytes, at + node_prefix + index * 4))
			.collect();
		let passes = (0..count)
			.map(|index| {
				let pass = head + index * pass_words * 4;
				let stage = |step: usize| match step < pass_words {
					true => word(bytes, pass + step * 4),
					false => NONE,
				};
				Pass {
					id: stage(0),
					vertex: stage(1),
					pixel: stage(2),
					geometry: stage(3),
					hull: stage(4),
					domain: stage(5),
				}
			})
			.collect();
		nodes.push(Node {
			id: word(bytes, at),
			keys,
			passes,
		});
		at = end;
	}

	let (records, end) = table::<structs::Alias>(
		bytes,
		at,
		to_usize(header.node_alias_count),
		structs::Alias::SIZE,
		"node alias table",
	)?;
	at = end;
	let aliases = records
		.into_iter()
		.map(|record| NodeAlias {
			selector: record.selector,
			node: record.node,
		})
		.collect();

	let mut clusters = Vec::with_capacity(to_usize(header.node_alias_cluster_count));
	for _ in 0..header.node_alias_cluster_count {
		const CLUSTER: usize = 16;
		const SUB_CLUSTER: usize = 392;
		let head = extent(bytes, at, 1, CLUSTER, "node alias cluster")?;
		// The sub-cluster count is the third word of the header; the fourth is unidentified. Taking
		// the wrong one reads a hash as a count.
		let count = to_usize(word(bytes, at + 8));
		let end = extent(
			bytes,
			head,
			count,
			SUB_CLUSTER,
			"node alias sub-cluster table",
		)?;
		let subclusters = (0..count)
			.map(|index| {
				let sub = head + index * SUB_CLUSTER;
				SubCluster {
					index: half(bytes, sub),
					alias_count: half(bytes, sub + 2),
					data: (0..97)
						.map(|word_at| word(bytes, sub + 4 + word_at * 4))
						.collect(),
				}
			})
			.collect();
		clusters.push(AliasCluster {
			id: word(bytes, at),
			subclusters,
		});
		at = end;
	}

	Ok((nodes, aliases, clusters, at))
}

/// The half-word at `at`, which every caller has already bounds-checked.
fn half(bytes: &[u8], at: usize) -> u16 {
	u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two bytes"))
}

/// The word at `at`, which every caller has already bounds-checked.
fn word(bytes: &[u8], at: usize) -> u32 {
	u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn to_usize(value: u32) -> usize {
	usize::try_from(value).expect("u32 fits usize")
}
