//! Structs and utilities shared by the zone layer formats.
//!
//! A layer group is three nested offset tables ending in one tagged union: the group names its
//! layers, a layer names its instances, and an instance's leading discriminant says how to read the
//! rest of it. `.lgb` holds a group directly; `.sgb` and `.lvb` wrap one in a scene.

mod instance;

pub use instance::{
	Aetheryte, BgPart, ChairKind, ChairMarker, Character, CollisionBox, Colour, CullingBox,
	EventNpc, EventObject, GameObject, Instance, InstanceData, InstanceKind, LightKind,
	LightSource, LineStyle, LineVfx, PointLightKind, PositionMarker, PositionMarkerKind,
	QuestMarker, ShadowMode, TargetMarker, TargetMarkerKind, Transform, Treasure, TriggerBox,
	TriggerShape, Vfx,
};

use binrw::BinRead;
use getset::{CopyGetters, Getters};

use crate::error::{Error, ErrorValue, Result};

pub(super) fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("layer group".into()), reason.into())
}

/// The last four bytes of a section header, which every offset inside it is measured from.
///
/// A section declares its own header length rather than a fixed one, and the four fields below sit
/// at its end, so this is where they begin.
const QUARTET: usize = 16;

fn i32_at(bytes: &[u8], at: usize) -> Result<i32> {
	bytes
		.get(at..at + 4)
		.and_then(|raw| raw.try_into().ok())
		.map(i32::from_le_bytes)
		.ok_or_else(|| invalid(format!("offset {at:#x} is past the end of the file")))
}

/// An offset relative to `base`, which the format writes signed and may point backwards.
fn seek(base: usize, offset: i32) -> Result<usize> {
	base.checked_add_signed(offset as isize)
		.ok_or_else(|| invalid(format!("offset {offset} from {base:#x} leaves the file")))
}

/// A null-terminated string at `at`, or an empty one where the offset does not name a string.
///
/// An absent name is written as offset zero, which lands on the structure itself rather than on the
/// heap, so it is read as empty rather than as a failure.
fn string(bytes: &[u8], at: usize) -> String {
	let Some(rest) = bytes.get(at..) else {
		return String::new();
	};
	let end = rest
		.iter()
		.position(|&byte| byte == 0)
		.unwrap_or(rest.len());
	String::from_utf8_lossy(&rest[..end]).into_owned()
}

/// A section's own header length, which is what its trailing fields are measured from.
pub(super) fn section_size(bytes: &[u8], at: usize) -> Result<usize> {
	let size = i32_at(bytes, at + 4)?;
	usize::try_from(size)
		.ok()
		.filter(|&size| size >= QUARTET)
		.ok_or_else(|| invalid(format!("section at {at:#x} declares a size of {size}")))
}

/// One layer group: everything a single `LGP1` section or an embedded scene group holds.
#[derive(Debug, Getters, CopyGetters)]
pub struct LayerGroup {
	/// Which of the seven groups this is. The zone files each carry one, so the id says which file
	/// it came from: 256 is `bg`, 258 `Planner`, 262 `Sound`.
	#[get_copy = "pub"]
	id: i32,

	#[get = "pub"]
	name: String,

	#[getset(skip)]
	layers: Vec<Layer>,
}

impl LayerGroup {
	/// The layers of the group, in the order its offset table names them.
	pub fn layers(&self) -> &[Layer] {
		&self.layers
	}

	/// Reads the group whose section header starts at `at`.
	///
	/// `size` is the section header's own length, which is where the four trailing fields are found
	/// and what everything inside the section is measured from.
	pub(super) fn parse(bytes: &[u8], at: usize, size: usize) -> Result<Self> {
		let heap = at
			.checked_add(size)
			.and_then(|end| end.checked_sub(QUARTET))
			.ok_or_else(|| invalid(format!("section at {at:#x} declares a size of {size}")))?;

		let id = i32_at(bytes, heap)?;
		let name = string(bytes, seek(heap, i32_at(bytes, heap + 4)?)?);
		let table = seek(heap, i32_at(bytes, heap + 8)?)?;
		let count = i32_at(bytes, heap + 12)?;
		let count = usize::try_from(count)
			.map_err(|_| invalid(format!("section at {at:#x} declares {count} layers")))?;

		let starts = (0..count)
			.map(|index| seek(table, i32_at(bytes, table + index * 4)?))
			.collect::<Result<Vec<_>>>()?;

		// An instance runs to whatever structure follows it, so the layers have to be laid out
		// before any of their instances can be read.
		let mut plans = Vec::with_capacity(starts.len());
		let mut bounds = vec![bytes.len()];
		bounds.extend(&starts);
		for &start in &starts {
			let plan = LayerPlan::parse(bytes, start)?;
			bounds.extend(&plan.instances);
			plans.push(plan);
		}
		bounds.sort_unstable();
		bounds.dedup();

		let layers = plans
			.into_iter()
			.map(|plan| plan.read(bytes, &bounds))
			.collect::<Result<Vec<_>>>()?;

		Ok(Self { id, name, layers })
	}
}

/// Fields of a layer header the format reads back, and where its instances start.
struct LayerPlan {
	at: usize,
	instances: Vec<usize>,
}

/// A layer header's fixed leading fields. The header's full length is [`instance_table`], which
/// grew by eight bytes at some point, so nothing past these may be read at a fixed offset.
///
/// [`instance_table`]: Header::instance_table
#[derive(BinRead)]
#[br(little)]
struct Header {
	id: u32,
	name: i32,
	/// The layer header's own length, and so where the instance offset table begins.
	instance_table: i32,
	instance_count: i32,
	#[br(map = |raw: u8| raw != 0)]
	visible: bool,
	_tool_mode_read_only: u8,
	_bush_layer: u8,
	_ps3_visible: u8,
	_layer_set_referenced: i32,
	festival_id: u16,
	festival_phase_id: u16,
}

impl LayerPlan {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let header = Self::header(bytes, at)?;
		let table = seek(at, header.instance_table)?;
		let count = usize::try_from(header.instance_count).map_err(|_| {
			invalid(format!(
				"layer at {at:#x} declares {} instances",
				header.instance_count
			))
		})?;

		let instances = (0..count)
			.map(|index| seek(table, i32_at(bytes, table + index * 4)?))
			.collect::<Result<Vec<_>>>()?;

		Ok(Self { at, instances })
	}

	fn header(bytes: &[u8], at: usize) -> Result<Header> {
		let mut cursor = std::io::Cursor::new(
			bytes
				.get(at..)
				.ok_or_else(|| invalid(format!("layer at {at:#x} is past the end of the file")))?,
		);
		Ok(Header::read(&mut cursor)?)
	}

	fn read(self, bytes: &[u8], bounds: &[usize]) -> Result<Layer> {
		let header = Self::header(bytes, self.at)?;
		let instances = self
			.instances
			.iter()
			.map(|&start| {
				// Nothing declares an instance's length, so it runs to whatever comes next: the
				// following instance, the following layer, or the end of the file.
				let end = match bounds.binary_search(&start) {
					Ok(index) => bounds.get(index + 1),
					Err(index) => bounds.get(index),
				};
				Instance::parse(bytes, start, end.copied().unwrap_or(bytes.len()))
			})
			.collect::<Result<Vec<_>>>()?;

		Ok(Layer {
			id: header.id,
			name: string(bytes, seek(self.at, header.name)?),
			visible: header.visible,
			festival_id: header.festival_id,
			festival_phase_id: header.festival_phase_id,
			instances,
		})
	}
}

/// One layer of a group, and the instances placed on it.
#[derive(Debug, Getters, CopyGetters)]
pub struct Layer {
	#[get_copy = "pub"]
	id: u32,

	#[get = "pub"]
	name: String,

	/// Whether the layer is shown without anything having to switch it on.
	#[get_copy = "pub"]
	visible: bool,

	/// Rows of `GameMain.Festival`. The layer is shown only while this festival is running; zero
	/// for a layer that is not seasonal.
	#[get_copy = "pub"]
	festival_id: u16,

	#[get_copy = "pub"]
	festival_phase_id: u16,

	#[getset(skip)]
	instances: Vec<Instance>,
}

impl Layer {
	/// Everything placed on the layer, in the order its offset table names them.
	pub fn instances(&self) -> &[Instance] {
		&self.instances
	}
}
