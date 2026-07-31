use std::io::Cursor;

use binrw::{BinRead, binread};
use getset::{CopyGetters, Getters};

use crate::error::Result;

use super::{invalid, seek, string};

/// Bytes every instance starts with, before its type-specific payload.
const PREFIX: usize = 0x30;

/// What an instance is, as the discriminant in front of it says.
///
/// The client names all 94 slots; the ones below carrying no payload in [`InstanceData`] are named
/// but unmodelled, and their bytes are kept in [`InstanceData::Unknown`].
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum InstanceKind {
	None = 0,
	BgPart = 1,
	Attribute = 2,
	Light = 3,
	Vfx = 4,
	PositionMarker = 5,
	SharedGroup = 6,
	Sound = 7,
	EventNpc = 8,
	BattleNpc = 9,
	RoutePath = 10,
	Character = 11,
	Aetheryte = 12,
	EnvSpace = 13,
	Gathering = 14,
	HelperObject = 15,
	Treasure = 16,
	Clip = 17,
	ClipCtrlPoint = 18,
	ClipCamera = 19,
	ClipLight = 20,
	ClipReserve00 = 21,
	ClipReserve01 = 22,
	ClipReserve02 = 23,
	ClipReserve03 = 24,
	ClipReserve04 = 25,
	ClipReserve05 = 26,
	ClipReserve06 = 27,
	ClipReserve07 = 28,
	ClipReserve08 = 29,
	ClipReserve09 = 30,
	ClipReserve10 = 31,
	ClipReserve11 = 32,
	ClipReserve12 = 33,
	ClipReserve13 = 34,
	ClipReserve14 = 35,
	CutAssetOnlySelectable = 36,
	Player = 37,
	Monster = 38,
	Weapon = 39,
	PopRange = 40,
	ExitRange = 41,
	Lvb = 42,
	MapRange = 43,
	NaviMeshRange = 44,
	EventObject = 45,
	DemiHuman = 46,
	EnvLocation = 47,
	ControlPoint = 48,
	EventRange = 49,
	RestBonusRange = 50,
	QuestMarker = 51,
	Timeline = 52,
	ObjectBehaviorSet = 53,
	Movie = 54,
	ScenarioExd = 55,
	ScenarioText = 56,
	CollisionBox = 57,
	DoorRange = 58,
	LineVfx = 59,
	SoundEnvSet = 60,
	CutActionTimeline = 61,
	CharaScene = 62,
	CutAction = 63,
	EquipPreset = 64,
	ClientPath = 65,
	ServerPath = 66,
	GimmickRange = 67,
	TargetMarker = 68,
	ChairMarker = 69,
	ClickableRange = 70,
	PrefetchRange = 71,
	FateRange = 72,
	PartyMember = 73,
	KeepRange = 74,
	SphereCastRange = 75,
	IndoorObject = 76,
	OutdoorObject = 77,
	EditGroup = 78,
	StableChocobo = 79,
	Unknown80 = 80,
	Unknown81 = 81,
	Unknown82 = 82,
	Decal = 83,
	Unknown84 = 84,
	Unknown85 = 85,
	ColliderLayer7 = 86,
	ColliderLayer8 = 87,
	ColliderLayer9 = 88,
	ColliderLayer10 = 89,
	CullingBox = 90,
	Unknown91 = 91,
	Unknown92 = 92,
	Unknown93 = 93,
}

/// A position, an orientation in radians, and a scale.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Transform {
	translation: [f32; 3],
	/// Euler angles, applied X then Y then Z.
	rotation: [f32; 3],
	scale: [f32; 3],
}

/// A colour with an intensity multiplier, so it can exceed what the four bytes alone express.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Colour {
	red: u8,
	green: u8,
	blue: u8,
	alpha: u8,
	intensity: f32,
}

/// One thing placed on a [`Layer`](super::Layer).
#[derive(Debug, Getters, CopyGetters)]
pub struct Instance {
	#[get_copy = "pub"]
	kind: InstanceKind,

	/// Unique within the layer group. Other instances refer to one by this.
	#[get_copy = "pub"]
	id: u32,

	#[get = "pub"]
	name: String,

	#[get_copy = "pub"]
	transform: Transform,

	#[get = "pub"]
	data: InstanceData,
}

impl Instance {
	pub(super) fn parse(bytes: &[u8], at: usize, end: usize) -> Result<Self> {
		let head = bytes
			.get(at..end.max(at))
			.filter(|region| region.len() >= PREFIX)
			.ok_or_else(|| invalid(format!("instance at {at:#x} is past the end of the file")))?;

		let mut cursor = Cursor::new(head);
		let kind = InstanceKind::read(&mut cursor)?;
		let id = u32::read_le(&mut cursor)?;
		let name = i32::read_le(&mut cursor)?;
		let transform = Transform::read(&mut cursor)?;

		Ok(Self {
			kind,
			id,
			name: string(bytes, seek(at, name)?),
			transform,
			data: InstanceData::parse(bytes, at, &head[PREFIX..], kind)?,
		})
	}
}

/// The payload behind an [`Instance`], read according to its [`InstanceKind`].
///
/// A kind the game ships but nobody has specified lands in [`Unknown`](Self::Unknown) with its
/// bytes intact, so an unmodelled type costs that payload rather than the file.
#[derive(Debug)]
pub enum InstanceData {
	/// The instance carries no payload.
	None,
	BgPart(BgPart),
	Light(LightSource),
	Vfx(Vfx),
	PositionMarker(PositionMarker),
	EventNpc(EventNpc),
	Character(Character),
	Aetheryte(Aetheryte),
	Treasure(Treasure),
	EventObject(EventObject),
	QuestMarker(QuestMarker),
	CollisionBox(CollisionBox),
	LineVfx(LineVfx),
	TargetMarker(TargetMarker),
	ChairMarker(ChairMarker),
	CullingBox(CullingBox),
	/// A payload with no reading yet, as the bytes between the instance's prefix and whatever
	/// follows it.
	///
	/// Nothing in the format states an instance's length, so this runs to the next structure and is
	/// an upper bound rather than an exact size: it takes in any string heap that sits between the
	/// two. For the last instance of a layer that is the layer's whole heap.
	Unknown(Vec<u8>),
}

impl InstanceData {
	fn parse(bytes: &[u8], at: usize, payload: &[u8], kind: InstanceKind) -> Result<Self> {
		// A payload reaching back into the file for a string measures the offset from the instance,
		// so the whole file is passed alongside the slice.
		let mut cursor = Cursor::new(payload);
		Ok(match kind {
			InstanceKind::None => Self::None,
			InstanceKind::BgPart => Self::BgPart(BgPart::parse(bytes, at, &mut cursor)?),
			InstanceKind::Light => Self::Light(LightSource::parse(bytes, at, &mut cursor)?),
			InstanceKind::Vfx => Self::Vfx(Vfx::parse(bytes, at, &mut cursor)?),
			InstanceKind::PositionMarker => {
				Self::PositionMarker(PositionMarker::read(&mut cursor)?)
			}
			InstanceKind::EventNpc => Self::EventNpc(EventNpc::read(&mut cursor)?),
			InstanceKind::Character => Self::Character(Character::read(&mut cursor)?),
			InstanceKind::Aetheryte => Self::Aetheryte(Aetheryte::read(&mut cursor)?),
			InstanceKind::Treasure => Self::Treasure(Treasure::read(&mut cursor)?),
			InstanceKind::EventObject => Self::EventObject(EventObject::read(&mut cursor)?),
			InstanceKind::QuestMarker => Self::QuestMarker(QuestMarker::read(&mut cursor)?),
			InstanceKind::CollisionBox => {
				Self::CollisionBox(CollisionBox::parse(bytes, at, &mut cursor)?)
			}
			InstanceKind::LineVfx => Self::LineVfx(LineVfx::read(&mut cursor)?),
			InstanceKind::TargetMarker => Self::TargetMarker(TargetMarker::read(&mut cursor)?),
			InstanceKind::ChairMarker => Self::ChairMarker(ChairMarker::read(&mut cursor)?),
			InstanceKind::CullingBox => Self::CullingBox(CullingBox::read(&mut cursor)?),
			_ => Self::Unknown(payload.to_vec()),
		})
	}
}

/// How a background model's collision is built.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ModelCollision {
	None = 0,
	Replace = 1,
	Box = 2,
}

/// Whether a model overrides the shadow setting it would otherwise inherit.
#[binread]
#[br(little, repr = u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShadowMode {
	ForceOff = 0,
	ForceOn = 1,
	Inherit = 2,
}

/// A piece of zone scenery: the commonest instance by a wide margin.
#[derive(Debug, Getters, CopyGetters)]
pub struct BgPart {
	#[get = "pub"]
	asset_path: String,
	#[get = "pub"]
	collision_asset_path: String,
	#[get_copy = "pub"]
	collision: ModelCollision,
	#[get_copy = "pub"]
	collision_material_mask: u64,
	#[get_copy = "pub"]
	collision_material_id: u64,
	#[get_copy = "pub"]
	visible: bool,
	#[get_copy = "pub"]
	world_light_shadow_mode: ShadowMode,
	#[get_copy = "pub"]
	object_light_shadow_mode: ShadowMode,
	/// Distance at which the model stops being drawn.
	#[get_copy = "pub"]
	fade_out_distance: f32,
	#[get_copy = "pub"]
	bounding_sphere_size: f32,
}

#[binread]
#[br(little)]
struct BgPartFields {
	asset_path: i32,
	collision_asset_path: i32,
	collision: ModelCollision,
	collision_material_mask_low: u32,
	collision_material_id_low: u32,
	collision_material_mask_high: u32,
	collision_material_id_high: u32,
	_collision_config: i32,
	#[br(map = |raw: u8| raw != 0)]
	visible: bool,
	world_light_shadow_mode: ShadowMode,
	#[br(pad_after = 1)]
	object_light_shadow_mode: ShadowMode,
	fade_out_distance: f32,
	bounding_sphere_size: f32,
}

fn pair(low: u32, high: u32) -> u64 {
	u64::from(low) | (u64::from(high) << 32)
}

impl BgPart {
	fn parse(bytes: &[u8], at: usize, cursor: &mut Cursor<&[u8]>) -> Result<Self> {
		let fields = BgPartFields::read(cursor)?;
		Ok(Self {
			asset_path: string(bytes, seek(at, fields.asset_path)?),
			collision_asset_path: string(bytes, seek(at, fields.collision_asset_path)?),
			collision: fields.collision,
			collision_material_mask: pair(
				fields.collision_material_mask_low,
				fields.collision_material_mask_high,
			),
			collision_material_id: pair(
				fields.collision_material_id_low,
				fields.collision_material_id_high,
			),
			visible: fields.visible,
			world_light_shadow_mode: fields.world_light_shadow_mode,
			object_light_shadow_mode: fields.object_light_shadow_mode,
			fade_out_distance: fields.fade_out_distance,
			bounding_sphere_size: fields.bounding_sphere_size,
		})
	}
}

/// The shape a light throws.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum LightKind {
	None = 0,
	World = 1,
	Point = 2,
	Spot = 3,
	Flat = 4,
	Line = 5,
	Specular = 6,
}

/// How a point light spreads.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PointLightKind {
	Sphere = 0,
	Hemisphere = 1,
}

/// A placed light.
#[derive(Debug, Getters, CopyGetters)]
pub struct LightSource {
	#[get_copy = "pub"]
	kind: LightKind,
	#[get_copy = "pub"]
	attenuation: f32,
	#[get_copy = "pub"]
	range: f32,
	#[get_copy = "pub"]
	point_light_kind: PointLightKind,
	#[get_copy = "pub"]
	attenuation_cone_coefficient: f32,
	#[get_copy = "pub"]
	spot_angle: f32,
	/// A texture the light projects, where it has one.
	#[get = "pub"]
	texture_path: String,
	#[get_copy = "pub"]
	colour: Colour,
	#[get_copy = "pub"]
	specular_highlights: bool,
	#[get_copy = "pub"]
	bg_part_shadows: bool,
	#[get_copy = "pub"]
	character_shadows: bool,
}

#[binread]
#[br(little)]
struct LightFields {
	kind: LightKind,
	attenuation: f32,
	range: f32,
	point_light_kind: PointLightKind,
	attenuation_cone_coefficient: f32,
	spot_angle: f32,
	texture_path: i32,
	#[br(pad_after = 4)]
	colour: Colour,
	#[br(map = |raw: u8| raw != 0)]
	specular_highlights: bool,
	#[br(map = |raw: u8| raw != 0)]
	bg_part_shadows: bool,
	#[br(map = |raw: u8| raw != 0)]
	character_shadows: bool,
}

impl LightSource {
	fn parse(bytes: &[u8], at: usize, cursor: &mut Cursor<&[u8]>) -> Result<Self> {
		let fields = LightFields::read(cursor)?;
		Ok(Self {
			kind: fields.kind,
			attenuation: fields.attenuation,
			range: fields.range,
			point_light_kind: fields.point_light_kind,
			attenuation_cone_coefficient: fields.attenuation_cone_coefficient,
			spot_angle: fields.spot_angle,
			texture_path: string(bytes, seek(at, fields.texture_path)?),
			colour: fields.colour,
			specular_highlights: fields.specular_highlights,
			bg_part_shadows: fields.bg_part_shadows,
			character_shadows: fields.character_shadows,
		})
	}
}

/// A placed visual effect.
#[derive(Debug, Getters, CopyGetters)]
pub struct Vfx {
	#[get = "pub"]
	asset_path: String,
	#[get_copy = "pub"]
	soft_particle_fade_range: f32,
	#[get_copy = "pub"]
	colour: Colour,
	#[get_copy = "pub"]
	auto_play: bool,
	#[get_copy = "pub"]
	no_far_clip: bool,
	/// Where the effect begins and finishes fading, near and far.
	#[get_copy = "pub"]
	fade_near: [f32; 2],
	#[get_copy = "pub"]
	fade_far: [f32; 2],
}

#[binread]
#[br(little)]
struct VfxFields {
	asset_path: i32,
	soft_particle_fade_range: f32,
	colour: Colour,
	#[br(map = |raw: u8| raw != 0)]
	auto_play: bool,
	#[br(map = |raw: u8| raw != 0, pad_after = 6)]
	no_far_clip: bool,
	fade_near: [f32; 2],
	fade_far: [f32; 2],
}

impl Vfx {
	fn parse(bytes: &[u8], at: usize, cursor: &mut Cursor<&[u8]>) -> Result<Self> {
		let fields = VfxFields::read(cursor)?;
		Ok(Self {
			asset_path: string(bytes, seek(at, fields.asset_path)?),
			soft_particle_fade_range: fields.soft_particle_fade_range,
			colour: fields.colour,
			auto_play: fields.auto_play,
			no_far_clip: fields.no_far_clip,
			fade_near: fields.fade_near,
			fade_far: fields.fade_far,
		})
	}
}

/// What a position marker marks.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PositionMarkerKind {
	DebugZonePop = 1,
	DebugJump = 2,
	NaviMesh = 3,
	LowQualityEvent = 4,
}

/// A marker the tools place, carrying a comment in two languages.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct PositionMarker {
	kind: PositionMarkerKind,
	comment_jp_offset: u32,
	comment_en_offset: u32,
}

/// The base an instance is spawned from, as a row of the sheet its kind names.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct GameObject {
	/// A row of `ENpcBase`, `BNpcBase`, `Aetheryte`, `EObj` or `Treasure`, whichever the
	/// instance's kind names.
	base_id: u32,
}

/// A game object that can be placed as a character.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Character {
	object: GameObject,
	unknown: [u32; 6],
}

/// An event NPC, which the `Level` sheet also projects.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct EventNpc {
	character: Character,
	unknown: [u32; 3],
}

/// An aetheryte, as a row of `Aetheryte`.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Aetheryte {
	object: GameObject,
	/// The instance this one is attached to, or zero.
	bound_instance_id: u32,
	unknown: u32,
}

/// A treasure coffer, as a row of `Treasure`.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Treasure {
	object: GameObject,
}

/// An interactable object, as a row of `EObj`.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct EventObject {
	object: GameObject,
	bound_instance_id: u32,
	unknown: u8,
}

/// A quest marker, wholly unread.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct QuestMarker {
	unknown: [u32; 2],
}

/// The shape a trigger volume takes.
#[binread]
#[br(little, repr = u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TriggerShape {
	None = 0,
	Box = 1,
	Sphere = 2,
	Cylinder = 3,
	Plane = 4,
	Mesh = 5,
	PlaneTwoSided = 6,
}

/// A volume that reports what enters it. The ranges are built on this.
#[binread]
#[br(little)]
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct TriggerBox {
	shape: TriggerShape,
	priority: i16,
	#[br(map = |raw: u8| raw != 0, pad_after = 5)]
	enabled: bool,
}

/// A trigger volume that also carries collision.
#[derive(Debug, Getters, CopyGetters)]
pub struct CollisionBox {
	#[get_copy = "pub"]
	trigger: TriggerBox,
	#[get_copy = "pub"]
	collision_material_mask: u64,
	#[get_copy = "pub"]
	collision_material_id: u64,
	#[get = "pub"]
	collision_asset_path: String,
}

#[binread]
#[br(little)]
struct CollisionBoxFields {
	trigger: TriggerBox,
	collision_material_mask_low: u32,
	collision_material_id_low: u32,
	collision_material_mask_high: u32,
	collision_material_id_high: u32,
	#[br(pad_after = 3)]
	_layer_mask: u8,
	collision_asset_path: i32,
}

impl CollisionBox {
	fn parse(bytes: &[u8], at: usize, cursor: &mut Cursor<&[u8]>) -> Result<Self> {
		let fields = CollisionBoxFields::read(cursor)?;
		Ok(Self {
			trigger: fields.trigger,
			collision_material_mask: pair(
				fields.collision_material_mask_low,
				fields.collision_material_mask_high,
			),
			collision_material_id: pair(
				fields.collision_material_id_low,
				fields.collision_material_id_high,
			),
			collision_asset_path: string(bytes, seek(at, fields.collision_asset_path)?),
		})
	}
}

/// Which dotted line a boundary is drawn with.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum LineStyle {
	Red = 1,
	Blue = 2,
	RedFar = 3,
}

/// The dotted lines marking a zone boundary.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct LineVfx {
	style: LineStyle,
}

/// What a target marker anchors.
#[binread]
#[br(little, repr = i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TargetMarkerKind {
	Target = 0,
	Nameplate = 1,
	LookAt = 2,
	BodyDynamics = 3,
	Root = 4,
	Unknown5 = 5,
	Unknown6 = 6,
}

/// Where the interface anchors itself on an object.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct TargetMarker {
	nameplate_offset_y: f32,
	kind: TargetMarkerKind,
}

/// Whether a seat is sat on or lain on.
#[binread]
#[br(little, repr = u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChairKind {
	Chair = 0,
	Bed = 1,
}

/// Somewhere a character can sit, and which sides they may take.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct ChairMarker {
	#[br(map = |raw: u8| raw != 0)]
	left: bool,
	#[br(map = |raw: u8| raw != 0)]
	right: bool,
	#[br(map = |raw: u8| raw != 0)]
	back: bool,
	kind: ChairKind,
}

/// A volume outside which the scenery it covers is not drawn.
#[binread]
#[br(little)]
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct CullingBox {
	unknown: u32,
}
