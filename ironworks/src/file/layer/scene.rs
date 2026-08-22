use std::io::Cursor;

use getset::{CopyGetters, Getters};

use crate::error::Result;
use crate::file::{File, tmb};

use super::{Colour, LayerGroup, groups, i32_at, invalid, seek, string};

/// Fields the scene header holds, each an offset from the header's own body.
const FIELDS: usize = 16;

/// One entry of the environment list the general fields point at.
const ENVIRONMENT: usize = 24;

/// Slots the general block holds before the environment list it points at begins.
const GENERAL: usize = 24;

/// Where in that block the sun's lean sits.
const SUN_TILT: usize = 4;

/// One entry of the filter list.
const FILTER: usize = 28;

/// One entry of the timeline list, and one of the instance pairs a timeline names.
const TIMELINE: usize = 44;
const ANIMATED: usize = 8;

/// Where the animation handler list sits inside the block the header's ninth slot names.
const HANDLERS: usize = 0x24;

/// The three handler kinds whose bodies are read: a transform the scene repeats forever, a turn
/// about one axis it never stops, and a colour it cycles a light and a surface through.
const REPEAT: i32 = 5;
const SPIN: i32 = 2;
const GLOW: i32 = 6;

/// An environment the scene applies over part of itself.
#[derive(Debug, Getters, CopyGetters)]
pub struct Environment {
	/// The `.envb` the environment is described by.
	#[get = "pub"]
	asset_path: String,

	#[get_copy = "pub"]
	index: i32,

	/// The [`EnvLocation`](super::EnvLocation) instance the environment is centred on.
	#[get_copy = "pub"]
	env_location_instance_id: i32,

	/// The `.essb` the environment is heard through.
	#[get = "pub"]
	sound_asset_path: String,
}

/// One thing a scene animates: which of its own instances move, and the timeline that moves them.
///
/// The timeline is a `TMLB` region inside the same container, which the header points back at with a
/// negative offset rather than laying out as a section of its own.
#[derive(Debug, Getters, CopyGetters)]
pub struct SceneTimeline {
	#[get_copy = "pub"]
	sub_id: i32,

	/// What the timeline is for, as the file names it: `opened`, `Panel8_hide_a`, `Color`.
	#[get = "pub"]
	kind: String,

	/// The actor the timeline drives, against the instance in this scene it drives.
	#[getset(skip)]
	animated: Vec<(i32, i32)>,

	#[get_copy = "pub"]
	auto_play: bool,

	#[get_copy = "pub"]
	looping: bool,

	#[get = "pub"]
	timeline: tmb::Timeline,
}

impl SceneTimeline {
	/// The timelines a scene holds, for a caller that has only an `Option<&Scene>`.
	pub fn of(scene: &Scene) -> &[Self] {
		scene.timelines()
	}

	/// Each actor of the timeline against the instance of this scene it moves.
	pub fn animated(&self) -> &[(i32, i32)] {
		&self.animated
	}

	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let instances = seek(at, i32_at(bytes, at + 8)?)?;
		let count = usize::try_from(i32_at(bytes, at + 12)?)
			.map_err(|_| invalid(format!("a timeline at {at:#x} driving a negative count")))?;
		let room = bytes.len().saturating_sub(instances) / ANIMATED;
		if count > room {
			return Err(invalid(format!(
				"a timeline at {at:#x} driving {count} instances in room for {room}"
			)));
		}
		// Negative, since the region sits ahead of the header that names it.
		let held = at as i64 + i64::from(i32_at(bytes, at + 20)?);
		let start = usize::try_from(held)
			.map_err(|_| invalid(format!("a timeline at {at:#x} reaching past the file")))?;
		let body = bytes
			.get(start..)
			.ok_or_else(|| invalid(format!("a timeline at {at:#x} reaching past the file")))?
			.to_vec();

		Ok(Self {
			sub_id: i32_at(bytes, at)?,
			kind: string(bytes, seek(at, i32_at(bytes, at + 4)?)?),
			animated: (0..count)
				.map(|index| {
					let held = instances + index * ANIMATED;
					Ok((i32_at(bytes, held)?, i32_at(bytes, held + 4)?))
				})
				.collect::<Result<_>>()?,
			auto_play: bytes.get(at + 32).is_some_and(|held| *held != 0),
			looping: bytes.get(at + 33).is_some_and(|held| *held != 0),
			timeline: tmb::Timeline::read(Cursor::new(body))?,
		})
	}
}

/// A motion the scene repeats on its own, with no timeline to play it, on top of wherever the file
/// placed the instances it names.
///
/// A scene lists several kinds of these and gives each its own body; only the repeating transform
/// is read past the kind, so doors, paths and clocks are skipped.
#[derive(Debug, Getters, CopyGetters)]
#[get = "pub"]
pub struct SceneAnimation {
	/// The instances of this scene the motion moves.
	#[getset(skip)]
	instances: Vec<u32>,

	translation: Lane,
	rotation: Lane,
	scale: Lane,
}

/// A turn about one axis the scene never stops, on top of wherever the file placed the instance it
/// names. Unlike a repeating transform it states no reach: it always comes round to where it began.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct SceneSpin {
	/// The instance of this scene the turn moves.
	instance: u32,

	/// Which of the instance's own axes it turns about: nought for x, one for y, two for z.
	axis: u32,

	/// How long one whole turn takes, in the ticks a scene timeline is keyed in, negative where the
	/// turn runs the other way.
	period: f32,
}

/// A colour the scene cycles the instances it names through, on top of whatever colour the file
/// gave them. One lane answers to the models among them and the other to the lights: across the
/// corpus a record naming only models leaves the light lane off in 349 of 363, and one naming only
/// lights leaves the surface lane off in 92 of 101.
#[derive(Debug, Getters, CopyGetters)]
pub struct SceneGlow {
	/// The instances of this scene the colour reaches.
	#[getset(skip)]
	instances: Vec<u32>,

	#[get_copy = "pub"]
	surface: Glow,

	#[get_copy = "pub"]
	light: Glow,
}

/// One lane of a cycled colour. Ten fields past the ones read here state the shape of the swing and
/// are left unread: a flag gating a factor, a period, and a second flag gating a second factor.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Glow {
	/// Whether the lane runs at all.
	active: bool,

	/// Whether the two ends below are a colour of the lane's own. A lane that runs and tints nothing
	/// states white at full strength, and leaves the instance the colour the file gave it.
	tints: bool,

	from: Colour,
	to: Colour,

	/// How long the swing from one end to the other takes, in the ticks a scene timeline is keyed
	/// in.
	period: u32,
}

/// One lane of a repeating motion.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Lane {
	/// Whether the lane swings at all. One that does not still states where it rests.
	active: bool,

	/// How far the swing reaches: world units for a translation, radians for a rotation, and a
	/// factor for a scale, which rests at one rather than at nought.
	amount: [f32; 4],

	/// How long one swing takes, in the ticks a scene timeline is keyed in.
	period: u32,

	/// How long the lane waits before its first swing.
	delay: u32,

	curve: u32,

	/// What the lane does when a swing ends. Nought starts the swing over, which is what a whole
	/// turn wants; the rest swing back.
	wrap: u32,
}

impl SceneAnimation {
	/// The motions a scene repeats, for a caller that has only an `Option<&Scene>`.
	pub fn of(scene: &Scene) -> &[Self] {
		scene.animations()
	}

	/// Each instance of this scene the motion moves.
	pub fn instances(&self) -> &[u32] {
		&self.instances
	}

	/// Reads the handler at `at`, or nothing where its kind is one whose body is not read.
	fn parse(bytes: &[u8], at: usize) -> Result<Option<Self>> {
		if i32_at(bytes, at)? != REPEAT {
			return Ok(None);
		}
		let ids = seek(at, i32_at(bytes, at + 16)?)?;
		let count = usize::try_from(i32_at(bytes, at + 20)?)
			.map_err(|_| invalid(format!("a handler at {at:#x} moving a negative count")))?;
		let instances = bytes
			.get(ids..ids.saturating_add(count))
			.ok_or_else(|| invalid(format!("a handler at {at:#x} reaching past the file")))?
			.iter()
			.map(|&held| u32::from(held))
			.collect();
		let lane = |slot: usize| Lane::parse(bytes, seek(at, i32_at(bytes, at + 32 + slot * 4)?)?);
		Ok(Some(Self {
			instances,
			translation: lane(0)?,
			rotation: lane(1)?,
			scale: lane(2)?,
		}))
	}
}

impl SceneSpin {
	/// The turns a scene never stops, for a caller that has only an `Option<&Scene>`.
	pub fn of(scene: &Scene) -> &[Self] {
		scene.spins()
	}

	/// Reads the handler at `at`, or nothing where its kind is one whose body is not read.
	fn parse(bytes: &[u8], at: usize) -> Result<Option<Self>> {
		if i32_at(bytes, at)? != SPIN {
			return Ok(None);
		}
		Ok(Some(Self {
			instance: i32_at(bytes, at + 16)? as u32,
			axis: i32_at(bytes, at + 20)? as u32,
			period: f32::from_bits(i32_at(bytes, at + 24)? as u32),
		}))
	}
}

impl SceneGlow {
	/// The colours a scene cycles, for a caller that has only an `Option<&Scene>`.
	pub fn of(scene: &Scene) -> &[Self] {
		scene.glows()
	}

	/// Each instance of this scene the colour reaches.
	pub fn instances(&self) -> &[u32] {
		&self.instances
	}

	/// Reads the handler at `at`, or nothing where its kind is one whose body is not read.
	fn parse(bytes: &[u8], at: usize) -> Result<Option<Self>> {
		if i32_at(bytes, at)? != GLOW {
			return Ok(None);
		}
		let ids = seek(at, i32_at(bytes, at + 16)?)?;
		let count = usize::try_from(i32_at(bytes, at + 20)?)
			.map_err(|_| invalid(format!("a handler at {at:#x} colouring a negative count")))?;
		let instances = bytes
			.get(ids..ids.saturating_add(count))
			.ok_or_else(|| invalid(format!("a handler at {at:#x} reaching past the file")))?
			.iter()
			.map(|&held| u32::from(held))
			.collect();
		let lane = |slot: usize| Glow::parse(bytes, seek(at, i32_at(bytes, at + 32 + slot * 4)?)?);
		Ok(Some(Self {
			instances,
			surface: lane(0)?,
			light: lane(1)?,
		}))
	}
}

impl Glow {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		// The same eight bytes an instance states its own colour in, which is what says the three
		// channels are read low byte first.
		let colour = |offset: usize| -> Result<Colour> {
			let rest = bytes
				.get(at + offset..)
				.ok_or_else(|| invalid(format!("a lane at {at:#x} reaching past the file")))?;
			Ok(<Colour as binrw::BinRead>::read(&mut Cursor::new(rest))?)
		};
		Ok(Self {
			active: bytes.get(at).is_some_and(|held| *held != 0),
			tints: bytes.get(at + 1).is_some_and(|held| *held != 0),
			from: colour(4)?,
			to: colour(12)?,
			period: i32_at(bytes, at + 32)? as u32,
		})
	}
}

impl Lane {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let float = |offset| -> Result<f32> {
			Ok(f32::from_bits(i32_at(bytes, at + offset)? as u32))
		};
		Ok(Self {
			active: i32_at(bytes, at)? != 0,
			amount: [float(4)?, float(8)?, float(12)?, float(16)?],
			period: i32_at(bytes, at + 20)? as u32,
			delay: i32_at(bytes, at + 24)? as u32,
			curve: i32_at(bytes, at + 28)? as u32,
			wrap: i32_at(bytes, at + 32)? as u32,
		})
	}
}

/// A place the scene is used from. Several territories can share one scene, and each names the
/// layers it turns on through one of these.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Filter {
	key: u32,

	/// A row of `TerritoryType`.
	territory_type: u16,

	/// A row of `ContentFinderCondition`, and zero where the scene is not entered through one.
	content_finder_condition: u16,
}

/// Everything an `SCN1` section holds: the layer groups laid out inside the file, the paths of the
/// ones kept beside it, and what the scene is drawn and heard with.
#[derive(Debug, Getters)]
#[get = "pub"]
pub struct Scene {
	#[getset(skip)]
	layer_groups: Vec<LayerGroup>,

	/// The `.lgb` files the scene draws its remaining layer groups from.
	layer_group_paths: Vec<String>,

	/// The directory the scene's own assets sit under.
	bg_path: String,

	/// The `.svb` saying which of the scene's models the sky reaches.
	sky_visibility_path: String,

	/// The `.lcb` bounding the scene's lights.
	light_culling_path: String,

	/// The general block whole, a slot at a time, so that what is not named here is still readable.
	/// The offsets the parse above reaches into it - the paths, the environment list - are among
	/// these, and so is everything nothing has yet identified.
	#[getset(skip)]
	general: Vec<u32>,

	#[getset(skip)]
	environments: Vec<Environment>,

	#[getset(skip)]
	filters: Vec<Filter>,

	#[getset(skip)]
	timelines: Vec<SceneTimeline>,

	#[getset(skip)]
	animations: Vec<SceneAnimation>,

	#[getset(skip)]
	spins: Vec<SceneSpin>,

	#[getset(skip)]
	glows: Vec<SceneGlow>,
}

impl Scene {
	/// The layer groups written into the file itself, in the order the header names them.
	pub fn layer_groups(&self) -> &[LayerGroup] {
		&self.layer_groups
	}

	/// The environments the scene applies, in the order it names them.
	pub fn environments(&self) -> &[Environment] {
		&self.environments
	}

	/// The places the scene is used from, in the order it names them.
	pub fn filters(&self) -> &[Filter] {
		&self.filters
	}

	/// What the scene animates, in the order it names them.
	pub fn timelines(&self) -> &[SceneTimeline] {
		&self.timelines
	}

	/// The motions the scene repeats on its own, in the order it names them.
	pub fn animations(&self) -> &[SceneAnimation] {
		&self.animations
	}

	/// The turns the scene never stops, in the order it names them.
	pub fn spins(&self) -> &[SceneSpin] {
		&self.spins
	}

	/// The colours the scene cycles its lights and surfaces through, in the order it names them.
	pub fn glows(&self) -> &[SceneGlow] {
		&self.glows
	}

	/// The general block a slot at a time, for a reader that wants what is not named yet.
	pub fn general(&self) -> &[u32] {
		&self.general
	}

	/// How far the sun's daily circle leans, in degrees, which every zone states for itself:
	/// `sun = (cos t, sin t * cos(this), sin t * sin(this))` for `t = (hour - 6) * 15 degrees`.
	/// Measured against five captures of four zones, exact in every one.
	pub fn sun_tilt_degrees(&self) -> u32 {
		self.general.get(SUN_TILT).copied().unwrap_or_default()
	}

	/// Reads the scene whose section header starts at `at`.
	pub(super) fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		// The older section puts two empty fields ahead of the body.
		let body = match (i32_at(bytes, at + 8)?, i32_at(bytes, at + 12)?) {
			(0, 0) => at + 16,
			_ => at + 8,
		};

		let offsets = (0..FIELDS)
			.map(|slot| i32_at(bytes, body + slot * 4))
			.collect::<Result<Vec<_>>>()?;
		// Rejected where the bytes behind the table cannot hold that many: collecting a range
		// reserves the whole count before the first read fails.
		let count = |declared: i32, table: usize, stride: usize| {
			let count = usize::try_from(declared)
				.map_err(|_| invalid(format!("a scene at {at:#x} declaring {declared} entries")))?;
			let room = bytes.len().saturating_sub(table) / stride;
			match count <= room {
				true => Ok(count),
				false => Err(invalid(format!(
					"a scene at {at:#x} declaring {count} entries in room for {room}"
				))),
			}
		};

		let heaps = (0..count(offsets[1], seek(body, offsets[0])?, 16)?)
			.map(|index| Ok(seek(body, offsets[0])? + index * 16))
			.collect::<Result<Vec<_>>>()?;

		let table = seek(body, offsets[5])?;
		let layer_group_paths = (0..count(offsets[6], table, size_of::<i32>())?)
			.map(|index| {
				Ok(string(
					bytes,
					seek(table, i32_at(bytes, table + index * 4)?)?,
				))
			})
			.collect::<Result<Vec<_>>>()?;

		let list = seek(body, offsets[3])?;
		let entries = seek(list, i32_at(bytes, list)?)?;
		let filters = (0..count(i32_at(bytes, list + 4)?, entries, FILTER)?)
			.map(|index| Filter::parse(bytes, entries + index * FILTER))
			.collect::<Result<Vec<_>>>()?;

		// A scene that animates nothing leaves the offset at nought, and one whose timelines will
		// not read is still a scene: the layers it places are worth having without them.
		let timelines = match offsets[4] > 0 {
			false => Vec::new(),
			true => {
				let list = seek(body, offsets[4])?;
				let entries = seek(list, i32_at(bytes, list)?)?;
				(0..count(i32_at(bytes, list + 4)?, entries, TIMELINE)?)
					.filter_map(|index| SceneTimeline::parse(bytes, entries + index * TIMELINE).ok())
					.collect()
			}
		};

		// The handlers sit inside a block the ninth slot names, past a table nothing has identified.
		// A scene that animates nothing still lays the block out, with the count at nought. Doors
		// and choices are laid out here too, each with a body of its own that nothing reads.
		let handlers: Vec<usize> = match offsets[8] > 0 {
			false => Vec::new(),
			true => {
				let list = seek(body, offsets[8])? + HANDLERS;
				let entries = seek(list, i32_at(bytes, list)?)?;
				(0..count(i32_at(bytes, list + 4)?, entries, size_of::<i32>())?)
					.filter_map(|index| {
						seek(entries, i32_at(bytes, entries + index * 4).ok()?).ok()
					})
					.collect()
			}
		};
		let animations = handlers
			.iter()
			.filter_map(|&at| SceneAnimation::parse(bytes, at).ok().flatten())
			.collect();
		let spins = handlers
			.iter()
			.filter_map(|&at| SceneSpin::parse(bytes, at).ok().flatten())
			.collect();
		let glows = handlers
			.iter()
			.filter_map(|&at| SceneGlow::parse(bytes, at).ok().flatten())
			.collect();

		let general = seek(body, offsets[2])?;
		let path = |offset| -> Result<String> {
			Ok(string(
				bytes,
				seek(general, i32_at(bytes, general + offset)?)?,
			))
		};
		let list = seek(general, i32_at(bytes, general + 8)?)?;
		let environments = (0..count(i32_at(bytes, general + 12)?, list, ENVIRONMENT)?)
			.map(|index| Environment::parse(bytes, list + index * ENVIRONMENT))
			.collect::<Result<Vec<_>>>()?;

		// Whatever the header lays out after a layer group bounds that group's last instance.
		let mut rest = heaps.clone();
		for &offset in &offsets {
			if offset > 0 {
				rest.push(seek(body, offset)?);
			}
		}

		Ok(Self {
			layer_groups: groups(bytes, &heaps, &rest)?,
			layer_group_paths,
			bg_path: path(4)?,
			sky_visibility_path: path(20)?,
			light_culling_path: path(52)?,
			general: (0..GENERAL)
				.map(|slot| {
					bytes
						.get(general + slot * 4..general + slot * 4 + 4)
						.map_or(0, |held| u32::from_le_bytes(held.try_into().expect("four")))
				})
				.collect(),
			environments,
			filters,
			timelines,
			animations,
			spins,
			glows,
		})
	}
}

impl Filter {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let ids = i32_at(bytes, at + 16)? as u32;
		Ok(Self {
			key: i32_at(bytes, at + 4)? as u32,
			territory_type: ids as u16,
			content_finder_condition: (ids >> 16) as u16,
		})
	}
}

impl Environment {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		Ok(Self {
			asset_path: string(bytes, seek(at, i32_at(bytes, at)?)?),
			index: i32_at(bytes, at + 4)?,
			env_location_instance_id: i32_at(bytes, at + 8)?,
			sound_asset_path: string(bytes, seek(at, i32_at(bytes, at + 12)?)?),
		})
	}
}
