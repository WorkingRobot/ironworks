//! Reader for the Havok binary tagfile that animation files embed.

use std::{
	f32::consts::{FRAC_PI_2, FRAC_PI_4},
	ops::Range,
};

use getset::{CopyGetters, Getters};

use crate::error::{Error, ErrorValue, Result};

const MAGIC: [u8; 8] = [0x1e, 0x0d, 0xb0, 0xca, 0xce, 0xfa, 0x11, 0xd0];

/// Layout the records below are written in, from the file's own file-info record.
const VERSION: i64 = 3;

const FILE_INFO: i64 = 1;
const METADATA: i64 = 2;
const OBJECT: i64 = 3;
const OBJECT_REMEMBER: i64 = 4;
const FILE_END: i64 = 7;

/// Bounds the recursion a class describing itself as one of its own members would otherwise cause.
const MAX_NESTING: usize = 16;

/// Widest spline the basis functions have room for.
const MAX_DEGREE: usize = 4;

const POLAR32: u8 = 0;
const THREECOMP40: u8 = 1;
const THREECOMP48: u8 = 2;
const UNCOMPRESSED: u8 = 5;

fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("Havok tagfile".into()), reason.into())
}

/// A bone's rest transform, in its parent's space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
	/// Position, the fourth component being padding.
	pub translation: [f32; 4],

	/// Rotation, as a quaternion.
	pub rotation: [f32; 4],

	/// Scale, the fourth component being padding.
	pub scale: [f32; 4],
}

/// The bones a skeleton names, and the pose they rest in.
///
/// The three lists describe the same bones in the same order, and are the same length.
#[derive(Debug, Default, Getters)]
pub struct Skeleton {
	/// Name the skeleton was authored under.
	#[get = "pub"]
	name: String,

	/// Bone names, in the order everything else indexes them by.
	#[get = "pub"]
	bones: Vec<String>,

	/// Each bone's parent, or `-1` for a root. A bone is always written after its parent.
	#[get = "pub"]
	parent_indices: Vec<i16>,

	/// Each bone's rest transform.
	#[get = "pub"]
	reference_pose: Vec<Transform>,
}

/// An animation and the skeleton it drives.
#[derive(Debug, Getters, CopyGetters)]
pub struct Binding {
	/// Name the skeleton was authored under, which is not a path.
	#[get = "pub"]
	skeleton: String,

	/// Bone each transform track drives, indexing the skeleton's bones.
	#[get = "pub"]
	bones: Vec<i16>,

	/// How the animation composes with one already playing, `0` being on its own.
	#[get_copy = "pub"]
	blend_hint: i32,

	/// The animation itself.
	#[get = "pub"]
	motion: Motion,
}

/// Transform tracks, sampleable at any time within the animation.
#[derive(Debug, CopyGetters)]
#[get_copy = "pub"]
pub struct Motion {
	/// Length of the animation in seconds.
	duration: f32,

	/// Frames the animation was authored at, the last of which sits at [`duration`](Self::duration).
	frames: u32,

	#[getset(skip)]
	frame_duration: f32,

	#[getset(skip)]
	frames_per_block: u32,

	/// Tracks are split into blocks of consecutive frames, neighbors sharing a frame.
	#[getset(skip)]
	blocks: Vec<Vec<Track>>,
}

impl Motion {
	/// Every transform track at `time` seconds into the animation, clamped to its ends.
	pub fn sample(&self, time: f32) -> Vec<Transform> {
		let last = self.frames.saturating_sub(1);
		let frame = match self.frame_duration > 0.0 {
			true => (time / self.frame_duration).clamp(0.0, last as f32),
			false => 0.0,
		};

		// Blocks overlap by a frame, so the last frame of a whole number of them belongs to the last.
		let span = self.frames_per_block.saturating_sub(1).max(1);
		let block = ((frame as u32 / span) as usize).min(self.blocks.len() - 1);
		let local = frame - (block as u32 * span) as f32;

		self.blocks[block]
			.iter()
			.map(|track| Transform {
				translation: track.translation.at(local),
				rotation: track.rotation.at(local),
				scale: track.scale.at(local),
			})
			.collect()
	}
}

/// Read the skeleton the tagfile's animation container names.
pub(super) fn skeleton(data: &[u8]) -> Result<Skeleton> {
	let (container, mut objects) = walk(data)?;
	let &first = container
		.skeletons
		.first()
		.ok_or_else(|| invalid("the animation container names no skeleton"))?;
	let Some(Object::Skeleton(skeleton)) = resolve(&mut objects, first) else {
		return Err(invalid(format!("object {first} is not a skeleton")));
	};

	let bones = skeleton.bones.len();
	if skeleton.parent_indices.len() != bones || skeleton.reference_pose.len() != bones {
		return Err(invalid(format!(
			"{bones} bones against {} parents and {} transforms",
			skeleton.parent_indices.len(),
			skeleton.reference_pose.len()
		)));
	}

	Ok(skeleton)
}

/// Read every animation the tagfile's container binds, in the order it names them.
pub(super) fn animations(data: &[u8]) -> Result<Vec<Binding>> {
	let (container, mut objects) = walk(data)?;
	container
		.bindings
		.iter()
		.map(|&reference| {
			let Some(Object::Binding(binding)) = resolve(&mut objects, reference) else {
				return Err(invalid(format!(
					"object {reference} is not an animation binding"
				)));
			};
			let Some(Object::Motion(motion)) = resolve(&mut objects, binding.motion) else {
				return Err(invalid(format!(
					"object {} is not a spline compressed animation",
					binding.motion
				)));
			};

			let tracks = motion.blocks.first().map_or(0, Vec::len);
			if binding.bones.len() != tracks {
				return Err(invalid(format!(
					"{tracks} transform tracks against {} bones",
					binding.bones.len()
				)));
			}

			Ok(Binding {
				skeleton: binding.skeleton,
				bones: binding.bones,
				blend_hint: binding.blend_hint,
				motion,
			})
		})
		.collect()
}

fn walk(data: &[u8]) -> Result<(Container, Vec<Option<Object>>)> {
	if data.get(..MAGIC.len()) != Some(&MAGIC) {
		return Err(invalid("not a Havok tagfile"));
	}

	let mut tagfile = Tagfile {
		data,
		offset: MAGIC.len(),
		// Index zero is the absent class, which a root's parent names.
		classes: vec![Class::default()],
		strings: Vec::new(),
		nesting: 0,
	};
	tagfile.classes()?;
	tagfile.objects()
}

/// References are one-based over the objects the file asked to be remembered.
fn resolve(objects: &mut [Option<Object>], reference: i64) -> Option<Object> {
	usize::try_from(reference)
		.ok()
		.and_then(|index| objects.get_mut(index.checked_sub(1)?))
		.and_then(Option::take)
}

#[derive(Default)]
struct Class {
	name: String,
	/// Inherited members first, in the order the presence bitfield covers them.
	members: Vec<Member>,
}

#[derive(Clone)]
struct Member {
	name: String,
	kind: Kind,
	arity: Arity,
	class: Option<String>,
}

#[derive(Clone, Copy)]
enum Kind {
	Void,
	Byte,
	Integer,
	Real,
	/// A run of floats, four wide for a vector and twelve for a transform.
	Vector(usize),
	Object,
	Struct,
	String,
}

#[derive(Clone, Copy)]
enum Arity {
	One,
	Array,
	Tuple(usize),
}

enum Value {
	Ignored,
	Bytes(Range<usize>),
	Integers(Vec<i64>),
	Floats(Vec<f32>),
	Strings(Vec<String>),
	/// A struct is written a member at a time, each holding every element's value for it.
	Fields(Vec<(String, Value)>),
}

enum Object {
	Skeleton(Skeleton),
	Binding(Bound),
	Motion(Motion),
	Container(Container),
	Other,
}

/// The objects the root container names, by reference.
#[derive(Default)]
struct Container {
	skeletons: Vec<i64>,
	bindings: Vec<i64>,
}

/// A binding before the animation it references has been resolved.
#[derive(Default)]
struct Bound {
	skeleton: String,
	bones: Vec<i16>,
	blend_hint: i32,
	motion: i64,
}

/// The fields of a spline compressed animation, before its blocks are decompressed.
#[derive(Default)]
struct Compressed {
	duration: f32,
	frame_duration: f32,
	frames: i64,
	frames_per_block: i64,
	tracks: i64,
	float_tracks: i64,
	block_offsets: Vec<i64>,
	float_block_offsets: Vec<i64>,
	data: Range<usize>,
}

struct Tagfile<'a> {
	data: &'a [u8],
	offset: usize,
	classes: Vec<Class>,
	strings: Vec<String>,
	nesting: usize,
}

impl Tagfile<'_> {
	fn byte(&mut self) -> Result<u8> {
		let byte = *self
			.data
			.get(self.offset)
			.ok_or_else(|| invalid("read past the end of the tagfile"))?;
		self.offset += 1;
		Ok(byte)
	}

	/// Six bits and a sign in the first byte, then seven more for every byte with its high bit set.
	fn integer(&mut self) -> Result<i64> {
		let first = self.byte()?;
		let mut value = i64::from((first >> 1) & 0x3f);
		let mut shift = 6;
		if first & 0x80 != 0 {
			loop {
				let byte = self.byte()?;
				value |= i64::from(byte & 0x7f) << shift;
				shift += 7;
				if shift > 55 {
					return Err(invalid("overlong integer"));
				}
				if byte & 0x80 == 0 {
					break;
				}
			}
		}
		Ok(match first & 1 {
			0 => value,
			_ => -value,
		})
	}

	fn count(&mut self) -> Result<usize> {
		let value = self.integer()?;
		usize::try_from(value).map_err(|_| invalid(format!("count {value} out of range")))
	}

	/// Every element of a run of integers, references or strings writes at least one byte, so a
	/// count past the end of the file is bad data rather than something to allocate for.
	fn bounded(&self, count: usize) -> Result<usize> {
		match count <= self.data.len() - self.offset {
			true => Ok(count),
			false => Err(invalid(format!(
				"{count} elements do not fit in the rest of the tagfile"
			))),
		}
	}

	fn skip(&mut self, size: usize) -> Result<()> {
		self.offset = self
			.offset
			.checked_add(size)
			.filter(|&end| end <= self.data.len())
			.ok_or_else(|| invalid("read past the end of the tagfile"))?;
		Ok(())
	}

	fn floats(&mut self, count: usize) -> Result<Vec<f32>> {
		let start = self.offset;
		self.skip(
			count
				.checked_mul(4)
				.ok_or_else(|| invalid("float run too long"))?,
		)?;
		Ok(self.data[start..self.offset]
			.chunks_exact(4)
			.map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
			.collect())
	}

	fn string(&mut self) -> Result<String> {
		let length = self.integer()?;
		if length == 0 {
			return Ok(String::new());
		}
		// A negative length names a string written earlier by a one-based index, counting the
		// empty string as the first.
		if length < 0 {
			let index =
				usize::try_from(-length - 2).map_err(|_| invalid("bad string reference"))?;
			return self
				.strings
				.get(index)
				.cloned()
				.ok_or_else(|| invalid(format!("string {length} has not been written yet")));
		}

		let start = self.offset;
		self.skip(usize::try_from(length).map_err(|_| invalid("string too long"))?)?;
		let string = String::from_utf8_lossy(&self.data[start..self.offset]).into_owned();
		self.strings.push(string.clone());
		Ok(string)
	}

	/// Class descriptions are variable length and strictly sequential, so the data they precede can
	/// only be reached by reading every one of them.
	fn classes(&mut self) -> Result<()> {
		loop {
			let start = self.offset;
			match self.integer()? {
				FILE_INFO => {
					let version = self.integer()?;
					if version != VERSION {
						return Err(invalid(format!("unsupported tagfile version {version}")));
					}
				}

				METADATA => {
					let class = self.class()?;
					self.classes.push(class);
				}

				// Anything else opens the data, and belongs to it.
				_ => {
					self.offset = start;
					return Ok(());
				}
			}
		}
	}

	fn class(&mut self) -> Result<Class> {
		let name = self.string()?;
		let _version = self.integer()?;

		let parent = self.count()?;
		let mut members = match parent {
			0 => Vec::new(),
			index => self
				.classes
				.get(index)
				.ok_or_else(|| invalid(format!("class {name:?} inherits from a later class")))?
				.members
				.clone(),
		};

		let declared = self.count()?;
		for _ in 0..declared {
			let member = self.member()?;
			members.push(member);
		}

		Ok(Class { name, members })
	}

	fn member(&mut self) -> Result<Member> {
		let name = self.string()?;
		let code = self.integer()?;

		let tuple = match code & 0x20 {
			0 => None,
			_ => Some(self.count()?),
		};
		let arity = match (code & 0x10, tuple) {
			(0, None) => Arity::One,
			(0, Some(count)) => Arity::Tuple(count),
			_ => Arity::Array,
		};

		let kind = match code & 0xf {
			0 => Kind::Void,
			1 => Kind::Byte,
			2 => Kind::Integer,
			3 => Kind::Real,
			width @ 4..=7 => Kind::Vector(usize::try_from(width - 3).unwrap() * 4),
			8 => Kind::Object,
			9 => Kind::Struct,
			10 => Kind::String,
			other => return Err(invalid(format!("unknown member type {other}"))),
		};
		let class = match kind {
			Kind::Object | Kind::Struct => Some(self.string()?),
			_ => None,
		};

		Ok(Member {
			name,
			kind,
			arity,
			class,
		})
	}

	fn objects(&mut self) -> Result<(Container, Vec<Option<Object>>)> {
		let mut remembered = Vec::new();
		let mut container = None;

		loop {
			match self.integer()? {
				FILE_END => break,

				tag @ (OBJECT | OBJECT_REMEMBER) => {
					let class = self.count()?;
					let object = match self.object(class)? {
						Object::Container(named) => {
							container.get_or_insert(named);
							Object::Other
						}
						object => object,
					};
					if tag == OBJECT_REMEMBER {
						remembered.push(Some(object));
					}
				}

				tag => return Err(invalid(format!("unexpected tag {tag}"))),
			}
		}

		let container = container.ok_or_else(|| invalid("no animation container"))?;
		Ok((container, remembered))
	}

	fn object(&mut self, class: usize) -> Result<Object> {
		let description = self
			.classes
			.get(class)
			.ok_or_else(|| invalid(format!("no description for class {class}")))?;
		let name = description.name.clone();
		let members = description.members.clone();

		let written = self.presence(members.len())?;
		let mut skeleton = Skeleton::default();
		let mut container = Container::default();
		let mut bound = Bound::default();
		let mut compressed = Compressed::default();

		for (index, member) in members.iter().enumerate() {
			if !present(&written, index) {
				continue;
			}
			let value = self.value(member)?;

			match (name.as_str(), member.name.as_str(), value) {
				("hkaSkeleton", "name", Value::Strings(mut names)) => {
					skeleton.name = names.pop().unwrap_or_default();
				}

				("hkaSkeleton", "parentIndices", Value::Integers(parents)) => {
					skeleton.parent_indices = parents
						.into_iter()
						.map(|parent| {
							i16::try_from(parent)
								.map_err(|_| invalid(format!("bone parent {parent} out of range")))
						})
						.collect::<Result<_>>()?;
				}

				("hkaSkeleton", "bones", Value::Fields(fields)) => {
					skeleton.bones = fields
						.into_iter()
						.find_map(|(field, value)| match (field.as_str(), value) {
							("name", Value::Strings(names)) => Some(names),
							_ => None,
						})
						.unwrap_or_default();
				}

				("hkaSkeleton", "referencePose", Value::Floats(pose)) => {
					skeleton.reference_pose = pose
						.chunks_exact(12)
						.map(|transform| Transform {
							translation: transform[0..4].try_into().unwrap(),
							rotation: transform[4..8].try_into().unwrap(),
							scale: transform[8..12].try_into().unwrap(),
						})
						.collect();
				}

				("hkaAnimationContainer", "skeletons", Value::Integers(references)) => {
					container.skeletons = references;
				}

				("hkaAnimationContainer", "bindings", Value::Integers(references)) => {
					container.bindings = references;
				}

				("hkaAnimationBinding", "originalSkeletonName", Value::Strings(mut names)) => {
					bound.skeleton = names.pop().unwrap_or_default();
				}

				("hkaAnimationBinding", "animation", Value::Integers(references)) => {
					bound.motion = references.first().copied().unwrap_or_default();
				}

				("hkaAnimationBinding", "transformTrackToBoneIndices", Value::Integers(bones)) => {
					bound.bones = bones
						.into_iter()
						.map(|bone| {
							i16::try_from(bone)
								.map_err(|_| invalid(format!("bone {bone} out of range")))
						})
						.collect::<Result<_>>()?;
				}

				("hkaAnimationBinding", "blendHint", Value::Integers(hint)) => {
					bound.blend_hint = hint
						.first()
						.copied()
						.and_then(|hint| i32::try_from(hint).ok())
						.unwrap_or_default();
				}

				("hkaSplineCompressedAnimation", member, value) => compressed.field(member, value),

				_ => {}
			}
		}

		Ok(match name.as_str() {
			"hkaSkeleton" => Object::Skeleton(skeleton),
			"hkaAnimationContainer" => Object::Container(container),
			"hkaAnimationBinding" => Object::Binding(bound),
			"hkaSplineCompressedAnimation" => Object::Motion(compressed.decompress(self.data)?),
			_ => Object::Other,
		})
	}

	/// One bit per member of the class, saying whether it was written at all.
	fn presence(&mut self, members: usize) -> Result<Vec<u8>> {
		(0..members.div_ceil(8)).map(|_| self.byte()).collect()
	}

	fn value(&mut self, member: &Member) -> Result<Value> {
		match member.arity {
			Arity::One => self.single(member),
			Arity::Array => {
				let count = self.count()?;
				self.repeated(member, count)
			}
			Arity::Tuple(count) => self.repeated(member, count),
		}
	}

	fn single(&mut self, member: &Member) -> Result<Value> {
		Ok(match member.kind {
			Kind::Void => Value::Ignored,
			Kind::Byte => {
				self.skip(1)?;
				Value::Ignored
			}
			Kind::Integer | Kind::Object => Value::Integers(vec![self.integer()?]),
			Kind::Real => Value::Floats(self.floats(1)?),
			Kind::Vector(width) => Value::Floats(self.floats(width)?),
			Kind::Struct => self.fields(member, None)?,
			Kind::String => Value::Strings(vec![self.string()?]),
		})
	}

	fn repeated(&mut self, member: &Member, count: usize) -> Result<Value> {
		Ok(match member.kind {
			Kind::Void => Value::Ignored,
			Kind::Byte => {
				let start = self.offset;
				self.skip(count)?;
				Value::Bytes(start..self.offset)
			}
			Kind::Integer => {
				let _element = self.integer()?;
				let count = self.bounded(count)?;
				Value::Integers((0..count).map(|_| self.integer()).collect::<Result<_>>()?)
			}
			Kind::Object => {
				let count = self.bounded(count)?;
				Value::Integers((0..count).map(|_| self.integer()).collect::<Result<_>>()?)
			}
			Kind::Real => Value::Floats(self.floats(count)?),
			Kind::Vector(width) => Value::Floats(
				self.floats(
					count
						.checked_mul(width)
						.ok_or_else(|| invalid("vector run too long"))?,
				)?,
			),
			Kind::Struct => self.fields(member, Some(count))?,
			Kind::String => {
				let count = self.bounded(count)?;
				Value::Strings((0..count).map(|_| self.string()).collect::<Result<_>>()?)
			}
		})
	}

	/// A struct writes which of its members were written, then each of them. An array of structs
	/// writes that once for the whole array, then every element's value for one member at a time.
	fn fields(&mut self, member: &Member, count: Option<usize>) -> Result<Value> {
		self.nesting += 1;
		if self.nesting > MAX_NESTING {
			return Err(invalid("structs nested too deeply"));
		}

		let class = member
			.class
			.as_deref()
			.ok_or_else(|| invalid("a struct names no class"))?;
		let members = self
			.classes
			.iter()
			.find(|candidate| candidate.name == class)
			.ok_or_else(|| invalid(format!("no description for class {class:?}")))?
			.members
			.clone();

		let written = self.presence(members.len())?;
		let mut fields = Vec::new();
		for (index, field) in members.iter().enumerate() {
			if present(&written, index) {
				let value = match count {
					Some(count) => self.repeated(field, count)?,
					None => self.value(field)?,
				};
				fields.push((field.name.clone(), value));
			}
		}

		self.nesting -= 1;
		Ok(Value::Fields(fields))
	}
}

fn present(bits: &[u8], index: usize) -> bool {
	bits[index / 8] & (1 << (index % 8)) != 0
}

impl Compressed {
	fn field(&mut self, member: &str, value: Value) {
		fn head<T: Default>(values: Vec<T>) -> T {
			values.into_iter().next().unwrap_or_default()
		}

		match (member, value) {
			("duration", Value::Floats(seconds)) => self.duration = head(seconds),
			("frameDuration", Value::Floats(seconds)) => self.frame_duration = head(seconds),
			("numFrames", Value::Integers(count)) => self.frames = head(count),
			("maxFramesPerBlock", Value::Integers(count)) => self.frames_per_block = head(count),
			("numberOfTransformTracks", Value::Integers(count)) => self.tracks = head(count),
			("numberOfFloatTracks", Value::Integers(count)) => self.float_tracks = head(count),
			("blockOffsets", Value::Integers(offsets)) => self.block_offsets = offsets,
			("floatBlockOffsets", Value::Integers(offsets)) => self.float_block_offsets = offsets,
			("data", Value::Bytes(range)) => self.data = range,
			_ => {}
		}
	}

	/// Decompress every block, each of which holds one spline per track over its own frames.
	fn decompress(self, tagfile: &[u8]) -> Result<Motion> {
		let bounded = |value: i64, what: &str| {
			usize::try_from(value).map_err(|_| invalid(format!("{what} {value} out of range")))
		};
		let tracks = bounded(self.tracks, "transform track count")?;
		let float_tracks = bounded(self.float_tracks, "float track count")?;
		let data = tagfile
			.get(self.data)
			.ok_or_else(|| invalid("animation data outside the tagfile"))?;

		let mut blocks = Vec::new();
		for (index, &offset) in self.block_offsets.iter().enumerate() {
			let start = bounded(offset, "block offset")?;
			let mut reader = Reader {
				data,
				offset: start,
			};
			blocks.push(reader.block(tracks, float_tracks)?);

			// The float tracks follow the transform tracks, so where they start says how much of the
			// block the transform tracks were meant to take.
			if let Some(&declared) = self.float_block_offsets.get(index) {
				let read = reader.offset - start;
				if bounded(declared, "float block offset")? != read {
					return Err(invalid(format!(
						"block {index} read {read} bytes of a declared {declared}"
					)));
				}
			}
		}
		if blocks.is_empty() {
			return Err(invalid("an animation with no blocks"));
		}

		Ok(Motion {
			duration: self.duration,
			frames: u32::try_from(self.frames)
				.map_err(|_| invalid(format!("frame count {} out of range", self.frames)))?,
			frame_duration: self.frame_duration,
			frames_per_block: u32::try_from(self.frames_per_block).map_err(|_| {
				invalid(format!(
					"block length {} out of range",
					self.frames_per_block
				))
			})?,
			blocks,
		})
	}
}

/// One bone's translation, rotation and scale over a block of frames.
#[derive(Debug)]
struct Track {
	translation: Vectors,
	rotation: Rotations,
	scale: Vectors,
}

/// A translation or scale curve. The three axes share knots, and an axis holding a single point
/// holds it for the whole block.
#[derive(Debug)]
struct Vectors {
	degree: usize,
	knots: Vec<u8>,
	axes: [Vec<f32>; 3],
}

impl Vectors {
	fn at(&self, frame: f32) -> [f32; 4] {
		let mut value = [0.0; 4];
		for (component, axis) in value.iter_mut().zip(&self.axes) {
			*component = match basis(&self.knots, self.degree, axis.len(), frame) {
				Some((span, weights)) => (0..=self.degree)
					.map(|index| axis[span - index] * weights[index])
					.sum(),
				None => axis[0],
			};
		}
		value
	}
}

/// A rotation curve, holding a single quaternion when the bone does not turn over the block.
#[derive(Debug)]
struct Rotations {
	degree: usize,
	knots: Vec<u8>,
	points: Vec<[f32; 4]>,
}

impl Rotations {
	fn at(&self, frame: f32) -> [f32; 4] {
		let Some((span, weights)) = basis(&self.knots, self.degree, self.points.len(), frame)
		else {
			return self.points[0];
		};

		let mut rotation = [0.0; 4];
		for (index, weight) in weights.iter().enumerate().take(self.degree + 1) {
			for (component, value) in rotation.iter_mut().zip(self.points[span - index]) {
				*component += value * weight;
			}
		}

		// Blending unit quaternions gives a point inside the sphere rather than on it.
		let length = rotation
			.iter()
			.map(|value| value * value)
			.sum::<f32>()
			.sqrt();
		match length > 0.0 {
			true => rotation.map(|component| component / length),
			false => [0.0, 0.0, 0.0, 1.0],
		}
	}
}

/// Weight of each of the `degree + 1` control points a spline blends at `frame`, and the index of
/// the last of them. A curve with too few points for its degree is constant, and has none.
///
/// Basis_ITS1 and GetPoint_NR1 of "Time-efficient NURBS curve evaluation algorithms".
fn basis(
	knots: &[u8],
	degree: usize,
	points: usize,
	frame: f32,
) -> Option<(usize, [f32; MAX_DEGREE + 1])> {
	if points <= degree || knots.len() < points + degree + 1 {
		return None;
	}

	let span = span(knots, degree, points, frame);
	let mut weights = [0.0; MAX_DEGREE + 1];
	weights[0] = 1.0;

	for order in 1..=degree {
		for index in (0..order).rev() {
			let low = f32::from(knots[span - index]);
			let high = f32::from(knots[span + order - index]);
			// Knots repeat at the ends of the curve, where the interval they span carries no weight.
			let scaled = match high > low {
				true => weights[index] * (frame - low) / (high - low),
				false => 0.0,
			};
			weights[index + 1] += weights[index] - scaled;
			weights[index] = scaled;
		}
	}

	Some((span, weights))
}

/// The knot span `frame` falls in, which is algorithm A2.1 of The NURBS Book, 2nd edition.
fn span(knots: &[u8], degree: usize, points: usize, frame: f32) -> usize {
	if frame >= f32::from(knots[points]) {
		return points - 1;
	}

	let (mut low, mut high) = (degree, points);
	let mut middle = (low + high) / 2;
	while frame < f32::from(knots[middle]) || frame >= f32::from(knots[middle + 1]) {
		match frame < f32::from(knots[middle]) {
			true => high = middle,
			false => low = middle,
		}
		// Knots the file wrote out of order would otherwise never narrow the search.
		let next = (low + high) / 2;
		if next == middle {
			break;
		}
		middle = next;
	}

	middle
}

/// Put the component a three-component packing left out back where it belongs, its magnitude being
/// whatever the other three leave of a unit quaternion.
fn compose(components: [f32; 3], missing: usize, negative: bool) -> [f32; 4] {
	let mut rotation = [0.0; 4];
	for (index, component) in rotation.iter_mut().enumerate() {
		if index < missing {
			*component = components[index];
		} else if index > missing {
			*component = components[index - 1];
		}
	}

	let square = 1.0 - components.iter().map(|value| value * value).sum::<f32>();
	rotation[missing] = match square > 0.0 {
		true => square.sqrt(),
		false => 0.0,
	};
	if negative {
		rotation[missing] = -rotation[missing];
	}

	rotation
}

/// Cursor over one animation's compressed data.
struct Reader<'a> {
	data: &'a [u8],
	offset: usize,
}

impl<'a> Reader<'a> {
	fn take(&mut self, size: usize) -> Result<&'a [u8]> {
		let end = self
			.offset
			.checked_add(size)
			.filter(|&end| end <= self.data.len())
			.ok_or_else(|| invalid("read past the end of the animation data"))?;
		let taken = &self.data[self.offset..end];
		self.offset = end;
		Ok(taken)
	}

	fn byte(&mut self) -> Result<u8> {
		Ok(self.take(1)?[0])
	}

	fn short(&mut self) -> Result<u16> {
		Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
	}

	fn word(&mut self) -> Result<u32> {
		Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
	}

	fn real(&mut self) -> Result<f32> {
		Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
	}

	/// Values sit at a multiple of their own width from the start of the animation's data.
	fn align(&mut self, to: usize) -> Result<()> {
		self.offset = self
			.offset
			.checked_next_multiple_of(to)
			.ok_or_else(|| invalid("read past the end of the animation data"))?;
		Ok(())
	}

	/// Every track's masks lead the block, then each track's three curves in turn.
	fn block(&mut self, tracks: usize, float_tracks: usize) -> Result<Vec<Track>> {
		let masks = self.take(
			tracks
				.checked_mul(4)
				.ok_or_else(|| invalid("too many transform tracks"))?,
		)?;
		self.take(float_tracks)?;
		self.align(4)?;

		let mut block = Vec::new();
		for mask in masks.chunks_exact(4) {
			let translation = self.vectors(mask[1], mask[0] & 3, 0.0)?;
			self.align(4)?;
			let rotation = self.rotations(mask[2], (mask[0] >> 2) & 0xf)?;
			self.align(4)?;
			let scale = self.vectors(mask[3], (mask[0] >> 6) & 3, 1.0)?;
			self.align(4)?;

			block.push(Track {
				translation,
				rotation,
				scale,
			});
		}

		Ok(block)
	}

	/// A curve's control point count, degree and knots, which lead every dynamic track.
	fn spline(&mut self) -> Result<(usize, usize, &'a [u8])> {
		let points = usize::from(self.short()?) + 1;
		let degree = usize::from(self.byte()?);
		if degree > MAX_DEGREE {
			return Err(invalid(format!("spline of degree {degree}")));
		}
		let knots = self.take(points + degree + 1)?;
		Ok((points, degree, knots))
	}

	fn vectors(&mut self, present: u8, quantization: u8, identity: f32) -> Result<Vectors> {
		let mut axes = [const { Vec::new() }; 3];

		if present & 0x70 == 0 {
			for (index, axis) in axes.iter_mut().enumerate() {
				axis.push(match present & (1 << index) {
					0 => identity,
					_ => self.real()?,
				});
			}
			return Ok(Vectors {
				degree: 0,
				knots: Vec::new(),
				axes,
			});
		}

		let (points, degree, knots) = self.spline()?;
		self.align(4)?;

		let mut extents = [(0.0, 0.0); 3];
		for (index, axis) in axes.iter_mut().enumerate() {
			if present & (0x10 << index) != 0 {
				extents[index] = (self.real()?, self.real()?);
			} else if present & (1 << index) != 0 {
				axis.push(self.real()?);
			} else {
				axis.push(identity);
			}
		}

		for _ in 0..points {
			for (index, axis) in axes.iter_mut().enumerate() {
				if present & (0x10 << index) == 0 {
					continue;
				}
				let ratio = match quantization {
					0 => f32::from(self.byte()?) / 255.0,
					_ => f32::from(self.short()?) / 65535.0,
				};
				let (low, high) = extents[index];
				axis.push(low + (high - low) * ratio);
			}
		}

		Ok(Vectors {
			degree,
			knots: knots.to_vec(),
			axes,
		})
	}

	fn rotations(&mut self, present: u8, quantization: u8) -> Result<Rotations> {
		if present & 0xf0 == 0 {
			let point = match present & 0xf {
				0 => [0.0, 0.0, 0.0, 1.0],
				_ => self.quaternion(quantization)?,
			};
			return Ok(Rotations {
				degree: 0,
				knots: Vec::new(),
				points: vec![point],
			});
		}

		let (count, degree, knots) = self.spline()?;
		self.align(match quantization {
			POLAR32 | UNCOMPRESSED => 4,
			THREECOMP48 => 2,
			_ => 1,
		})?;

		let mut points = Vec::new();
		for _ in 0..count {
			points.push(self.quaternion(quantization)?);
		}

		Ok(Rotations {
			degree,
			knots: knots.to_vec(),
			points,
		})
	}

	fn quaternion(&mut self, quantization: u8) -> Result<[f32; 4]> {
		match quantization {
			POLAR32 => self.polar32(),
			THREECOMP40 => self.threecomp40(),
			THREECOMP48 => self.threecomp48(),
			UNCOMPRESSED => Ok([self.real()?, self.real()?, self.real()?, self.real()?]),
			other => Err(invalid(format!(
				"unsupported rotation quantization {other}"
			))),
		}
	}

	/// A polar angle pair and a magnitude, with a sign bit for each component.
	fn polar32(&mut self) -> Result<[f32; 4]> {
		let packed = self.word()?;

		let scaled = ((packed >> 18) & 0x3ff) as f32 / 1023.0;
		let last = 1.0 - scaled * scaled;
		let magnitude = (1.0 - last * last).sqrt();

		let angles = (packed & 0x3_ffff) as f32;
		let mut phi = angles.sqrt().floor();
		let mut theta = 0.0;
		if phi > 0.0 {
			theta = FRAC_PI_4 * (angles - phi * phi) / phi;
			phi *= FRAC_PI_2 / 511.0;
		}

		let mut rotation = [
			phi.sin() * theta.cos() * magnitude,
			phi.sin() * theta.sin() * magnitude,
			phi.cos() * magnitude,
			last,
		];
		for (index, component) in rotation.iter_mut().enumerate() {
			if packed & (0x1000_0000 << index) != 0 {
				*component = -*component;
			}
		}

		Ok(rotation)
	}

	/// Three twelve-bit components, the index of the one left out, and its sign.
	fn threecomp40(&mut self) -> Result<[f32; 4]> {
		let bytes = self.take(5)?;
		let packed = u64::from(u32::from_le_bytes(bytes[..4].try_into().unwrap()))
			| (u64::from(bytes[4]) << 32);

		let components = [0, 12, 24]
			.map(|shift| (((packed >> shift) & 0xfff) as i32 - 2047) as f32 * 0.000345436);
		Ok(compose(
			components,
			((packed >> 36) & 3) as usize,
			(packed >> 38) & 1 != 0,
		))
	}

	/// Three fifteen-bit components, the index of the one left out, and its sign.
	fn threecomp48(&mut self) -> Result<[f32; 4]> {
		let bytes = self.take(6)?;
		let packed = [0, 2, 4].map(|at| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()));

		let missing = usize::from(((packed[1] >> 14) & 2) | ((packed[0] >> 15) & 1));
		let negative = packed[2] >> 15 != 0;
		let components =
			packed.map(|value| (i32::from(value & 0x7fff) - 16383) as f32 * 0.000043161);

		Ok(compose(components, missing, negative))
	}
}

#[cfg(test)]
mod test {
	use crate::error::Error;

	use super::{Binding, Skeleton, animations, skeleton};

	/// The tagfile writes every integer as six bits and a sign, then seven bits a byte.
	fn integer(value: i64) -> Vec<u8> {
		let magnitude = value.unsigned_abs();
		let sign = u8::from(value < 0);
		if magnitude < 0x40 {
			return vec![((magnitude as u8) << 1) | sign];
		}

		let mut bytes = vec![0x80 | ((magnitude as u8 & 0x3f) << 1) | sign];
		let mut rest = magnitude >> 6;
		while rest > 0x7f {
			bytes.push(0x80 | (rest as u8 & 0x7f));
			rest >>= 7;
		}
		bytes.push(rest as u8);
		bytes
	}

	fn string(text: &str) -> Vec<u8> {
		let mut bytes = integer(text.len() as i64);
		bytes.extend(text.as_bytes());
		bytes
	}

	/// A member of the given type code, which is a base type plus `0x10` for an array.
	fn member(name: &str, code: i64, class: Option<&str>) -> Vec<u8> {
		let mut bytes = string(name);
		bytes.extend(integer(code));
		if let Some(class) = class {
			bytes.extend(string(class));
		}
		bytes
	}

	fn class(name: &str, parent: i64, members: &[Vec<u8>]) -> Vec<u8> {
		let mut bytes = integer(2);
		bytes.extend(string(name));
		bytes.extend(integer(0));
		bytes.extend(integer(parent));
		bytes.extend(integer(members.len() as i64));
		bytes.extend(members.iter().flatten());
		bytes
	}

	fn transform(translation: f32) -> Vec<u8> {
		let mut values = [0.0f32; 12];
		values[0] = translation;
		values[7] = 1.0;
		values[8..11].fill(1.0);
		values
			.iter()
			.flat_map(|value| value.to_le_bytes())
			.collect()
	}

	/// A container naming one skeleton, whose bones carry the given names and parents. A mapper is
	/// written after it, which the reader has to walk past without reading.
	fn tagfile(
		bones: &[(&str, i64)],
		lock_translation: bool,
		pose: bool,
		mapper: Option<&[i64]>,
	) -> Vec<u8> {
		let mut bytes = super::MAGIC.to_vec();
		bytes.extend(integer(1));
		bytes.extend(integer(3));

		bytes.extend(class(
			"hkaAnimationContainer",
			0,
			&[member("skeletons", 0x18, Some("hkaSkeleton"))],
		));
		bytes.extend(class(
			"hkaSkeleton",
			0,
			&[
				member("name", 0xa, None),
				member("parentIndices", 0x12, None),
				member("bones", 0x19, Some("hkaBone")),
				member("referencePose", 0x16, None),
			],
		));
		bytes.extend(class(
			"hkaBone",
			0,
			&[
				member("name", 0xa, None),
				member("lockTranslation", 0x1, None),
			],
		));
		bytes.extend(class(
			"hkaSkeletonMapper",
			0,
			&[member("mapping", 0x9, Some("hkaSkeletonMapperData"))],
		));
		bytes.extend(class(
			"hkaSkeletonMapperData",
			0,
			&[
				member("skeletonA", 0x8, Some("hkaSkeleton")),
				member("unmappedBones", 0x12, None),
			],
		));

		bytes.extend(integer(4));
		bytes.extend(integer(1));
		bytes.push(0b1);
		bytes.extend(integer(1));
		bytes.extend(integer(2));

		bytes.extend(integer(4));
		bytes.extend(integer(2));
		bytes.push(if pose { 0b1111 } else { 0b0111 });
		bytes.extend(string("test"));
		bytes.extend(integer(bones.len() as i64));
		bytes.extend(integer(4));
		bytes.extend(bones.iter().flat_map(|&(_, parent)| integer(parent)));
		bytes.extend(integer(bones.len() as i64));
		bytes.push(if lock_translation { 0b11 } else { 0b1 });
		bytes.extend(bones.iter().flat_map(|&(name, _)| string(name)));
		if lock_translation {
			bytes.extend(bones.iter().map(|_| 0));
		}
		if pose {
			bytes.extend(integer(bones.len() as i64));
			bytes.extend(
				bones
					.iter()
					.enumerate()
					.flat_map(|(index, _)| transform(index as f32)),
			);
		}

		if let Some(unmapped) = mapper {
			bytes.extend(integer(4));
			bytes.extend(integer(4));
			bytes.push(0b1);
			bytes.push(0b11);
			bytes.extend(integer(2));
			bytes.extend(integer(unmapped.len() as i64));
			bytes.extend(integer(4));
			bytes.extend(unmapped.iter().flat_map(|&bone| integer(bone)));
		}

		bytes.extend(integer(7));
		bytes
	}

	fn read(bytes: Vec<u8>) -> Skeleton {
		skeleton(&bytes).unwrap()
	}

	#[test]
	fn reads_the_skeleton_the_container_names() {
		let file = read(tagfile(&[("n_root", -1), ("j_kao", 0)], false, true, None));
		assert_eq!(file.name(), "test");
		assert_eq!(file.bones(), &["n_root", "j_kao"]);
		assert_eq!(file.parent_indices(), &[-1, 0]);
		assert_eq!(file.reference_pose().len(), 2);
		assert_eq!(file.reference_pose()[1].translation, [1.0, 0.0, 0.0, 0.0]);
		assert_eq!(file.reference_pose()[1].rotation, [0.0, 0.0, 0.0, 1.0]);
		assert_eq!(file.reference_pose()[1].scale, [1.0, 1.0, 1.0, 0.0]);
	}

	/// Each member of a struct holds every element's value for it, rather than each element holding
	/// its own members.
	#[test]
	fn reads_a_struct_a_member_at_a_time() {
		let file = read(tagfile(&[("n_root", -1), ("j_kao", 0)], true, true, None));
		assert_eq!(file.bones(), &["n_root", "j_kao"]);
		assert_eq!(file.parent_indices(), &[-1, 0]);
	}

	/// Names are written once and referred back to by index, the empty string counting as the first.
	#[test]
	fn resolves_a_repeated_name() {
		let mut bytes = tagfile(&[("j_kao", -1)], false, true, None);
		let name = bytes
			.windows(5)
			.position(|window| window == b"j_kao")
			.unwrap();
		bytes.splice(name - 1..name + 5, integer(-4));

		let file = read(bytes);
		assert_eq!(file.bones(), &["hkaSkeleton"]);
	}

	/// A member of an object the reader has no use for still has to be read the way its own class
	/// describes it, or everything after it is lost.
	#[test]
	fn walks_past_an_object_it_does_not_read() {
		let file = read(tagfile(&[("n_root", -1)], false, true, Some(&[11, 12, 13])));
		assert_eq!(file.bones(), &["n_root"]);
	}

	/// A declared count is not a promise, and nothing may be reserved for one the file cannot hold.
	#[test]
	fn rejects_a_count_the_file_cannot_hold() {
		let mut bytes = tagfile(&[("n_root", -1)], false, true, None);
		let count = bytes
			.windows(4)
			.position(|window| window == b"test")
			.unwrap() + 4;
		bytes.splice(count..count + 1, integer(1 << 40));
		assert!(matches!(skeleton(&bytes), Err(Error::Invalid(..))));
	}

	/// Everything downstream indexes the three lists together, so a file that does not write all of
	/// them describes nothing usable.
	#[test]
	fn rejects_a_skeleton_missing_a_list() {
		assert!(matches!(
			skeleton(&tagfile(&[("n_root", -1)], false, false, None)),
			Err(Error::Invalid(..))
		));
	}

	#[test]
	fn rejects_a_foreign_file() {
		assert!(skeleton(b"not a tagfile at all").is_err());
		assert!(skeleton(&[]).is_err());
	}

	#[test]
	fn rejects_an_unknown_version() {
		let mut bytes = tagfile(&[("n_root", -1)], false, true, None);
		bytes[9] = 16;
		assert!(matches!(skeleton(&bytes), Err(Error::Invalid(..))));
	}

	#[test]
	fn rejects_a_truncated_file() {
		let bytes = tagfile(&[("n_root", -1), ("j_kao", 0)], false, true, None);
		for length in [12, bytes.len() / 2, bytes.len() - 8] {
			assert!(skeleton(&bytes[..length]).is_err());
		}
	}

	/// Everything a spline block writes sits at a multiple of four from the block's own start.
	fn pad(bytes: &mut Vec<u8>) {
		// Nothing promises the padding is zeroed.
		bytes.resize(bytes.len().next_multiple_of(4), 0xCC);
	}

	/// A spline's item count, degree and knots, which lead a dynamic curve.
	fn knots(values: &[u8], degree: u8) -> Vec<u8> {
		let items = u16::try_from(values.len() - usize::from(degree) - 2).unwrap();
		let mut bytes = items.to_le_bytes().to_vec();
		bytes.push(degree);
		bytes.extend(values);
		bytes
	}

	fn reals(values: &[f32]) -> Vec<u8> {
		values
			.iter()
			.flat_map(|value| value.to_le_bytes())
			.collect()
	}

	/// A block holding one track, whose three curves are written as given.
	fn block(masks: [u8; 4], curves: &[&[u8]]) -> Vec<u8> {
		let mut bytes = masks.to_vec();
		for curve in curves {
			pad(&mut bytes);
			bytes.extend(*curve);
		}
		pad(&mut bytes);
		bytes
	}

	/// A container binding one animation over `bones`, whose blocks are written verbatim.
	fn animation(bones: &[i64], frames: i64, per_block: i64, blocks: &[Vec<u8>]) -> Vec<u8> {
		let mut bytes = super::MAGIC.to_vec();
		bytes.extend(integer(1));
		bytes.extend(integer(3));

		bytes.extend(class(
			"hkaAnimationContainer",
			0,
			&[
				member("skeletons", 0x18, Some("hkaSkeleton")),
				member("bindings", 0x18, Some("hkaAnimationBinding")),
			],
		));
		bytes.extend(class(
			"hkaAnimationBinding",
			0,
			&[
				member("originalSkeletonName", 0xa, None),
				member("animation", 0x8, Some("hkaAnimation")),
				member("transformTrackToBoneIndices", 0x12, None),
				member("blendHint", 0x2, None),
			],
		));
		bytes.extend(class(
			"hkaSplineCompressedAnimation",
			0,
			&[
				member("duration", 0x3, None),
				member("numberOfTransformTracks", 0x2, None),
				member("numFrames", 0x2, None),
				member("maxFramesPerBlock", 0x2, None),
				member("frameDuration", 0x3, None),
				member("blockOffsets", 0x12, None),
				member("floatBlockOffsets", 0x12, None),
				member("data", 0x11, None),
			],
		));

		// The binding, which is object one, naming the animation that follows it.
		bytes.extend(integer(4));
		bytes.extend(integer(2));
		bytes.push(0b1111);
		bytes.extend(string("test:mdl:n_root"));
		bytes.extend(integer(2));
		bytes.extend(integer(bones.len() as i64));
		bytes.extend(integer(4));
		bytes.extend(bones.iter().flat_map(|&bone| integer(bone)));
		bytes.extend(integer(1));

		let mut data = Vec::new();
		let mut offsets = Vec::new();
		let mut ends = Vec::new();
		for block in blocks {
			offsets.push(data.len() as i64);
			ends.push(block.len() as i64);
			data.extend(block);
			data.resize(data.len().next_multiple_of(16), 0xCC);
		}

		bytes.extend(integer(4));
		bytes.extend(integer(3));
		bytes.push(0b1111_1111);
		bytes.extend(((frames - 1) as f32).to_le_bytes());
		bytes.extend(integer(bones.len() as i64));
		bytes.extend(integer(frames));
		bytes.extend(integer(per_block));
		bytes.extend(1.0f32.to_le_bytes());
		for run in [&offsets, &ends] {
			bytes.extend(integer(run.len() as i64));
			bytes.extend(integer(4));
			bytes.extend(run.iter().flat_map(|&value| integer(value)));
		}
		bytes.extend(integer(data.len() as i64));
		bytes.extend(&data);

		// The container, naming no skeleton and the binding above.
		bytes.extend(integer(4));
		bytes.extend(integer(1));
		bytes.push(0b11);
		bytes.extend(integer(0));
		bytes.extend(integer(1));
		bytes.extend(integer(1));

		bytes.extend(integer(7));
		bytes
	}

	fn bind(bytes: Vec<u8>) -> Binding {
		animations(&bytes).unwrap().pop().unwrap()
	}

	fn close(value: [f32; 4], expected: [f32; 4]) {
		let error = value
			.iter()
			.zip(expected)
			.map(|(a, b)| (a - b).abs())
			.fold(0.0, f32::max);
		assert!(error < 1e-5, "{value:?} against {expected:?}");
	}

	/// Three control points over `[0, 10]`, with a knot at 5, so a linear curve interpolates the
	/// first and last of them and blends a named pair in between.
	fn linear_track(quantization: u8) -> Vec<u8> {
		let mut curve = knots(&[0, 0, 5, 10, 10], 1);
		pad(&mut curve);
		curve.extend(reals(&[1.0, 3.0, 0.0, 2.0, 0.0, 4.0]));
		for point in [[0, 0, 0], [u16::MAX, 0, 0], [0, u16::MAX, 0]] {
			for value in point {
				match quantization {
					0 => curve.push((value >> 8) as u8),
					_ => curve.extend(value.to_le_bytes()),
				}
			}
		}
		block([quantization, 0x70, 0x00, 0x00], &[&curve])
	}

	#[test]
	fn reads_the_animation_the_container_binds() {
		let file = bind(animation(&[7], 11, 11, &[linear_track(0)]));
		assert_eq!(file.skeleton(), "test:mdl:n_root");
		assert_eq!(file.bones(), &[7]);
		assert_eq!(file.blend_hint(), 1);
		assert_eq!(file.motion().frames(), 11);
		assert_eq!(file.motion().duration(), 10.0);
	}

	/// A curve interpolates its end control points, and between two knots it blends the pair the
	/// knot span names, whether its points were quantized to eight bits or sixteen.
	#[test]
	fn blends_the_control_points_a_knot_span_names() {
		for quantization in [0, 1] {
			let file = bind(animation(&[0], 11, 11, &[linear_track(quantization)]));
			let motion = file.motion();

			for (time, expected) in [
				(0.0, [1.0, 0.0, 0.0, 0.0]),
				(2.5, [2.0, 0.0, 0.0, 0.0]),
				(5.0, [3.0, 0.0, 0.0, 0.0]),
				(7.5, [2.0, 1.0, 0.0, 0.0]),
				(10.0, [1.0, 2.0, 0.0, 0.0]),
			] {
				close(motion.sample(time)[0].translation, expected);
			}
		}
	}

	/// The weights a spline blends with sum to one, so a curve whose control points agree holds
	/// their value at every frame of the block, whatever its degree.
	#[test]
	fn weights_a_spline_blends_with_sum_to_one() {
		let mut curve = knots(&[0, 0, 0, 0, 4, 8, 12, 12, 12, 12], 3);
		pad(&mut curve);
		curve.extend(reals(&[2.0, 6.0, 0.0, 1.0, 0.0, 3.0]));
		curve.extend([255; 18]);
		let file = bind(animation(
			&[0],
			13,
			13,
			&[block([0x00, 0x70, 0x00, 0x00], &[&curve])],
		));

		let motion = file.motion();
		for step in 0..=120 {
			close(
				motion.sample(step as f32 / 10.0)[0].translation,
				[6.0, 1.0, 3.0, 0.0],
			);
		}
	}

	/// A track the block writes no curve for holds the identity, which is zero for a translation
	/// and one for a scale.
	#[test]
	fn holds_the_identity_where_a_track_writes_nothing() {
		let file = bind(animation(
			&[0],
			2,
			2,
			&[block(
				[0x00, 0x01, 0x00, 0x02],
				&[&reals(&[9.0]), &[], &reals(&[4.0])],
			)],
		));

		let sample = file.motion().sample(0.0);
		close(sample[0].translation, [9.0, 0.0, 0.0, 0.0]);
		close(sample[0].rotation, [0.0, 0.0, 0.0, 1.0]);
		close(sample[0].scale, [1.0, 4.0, 1.0, 0.0]);
	}

	/// A static rotation of the given quantization, which the mask names in its top four bits.
	fn rotation(quantization: u8, packed: &[u8]) -> Vec<u8> {
		block([quantization << 2, 0x00, 0x01, 0x00], &[&[], packed])
	}

	#[test]
	fn unpacks_a_polar32_rotation() {
		let file = bind(animation(&[0], 2, 2, &[rotation(0, &[57, 48, 240, 58])]));
		close(
			file.motion().sample(0.0)[0].rotation,
			[-0.2793129, -0.04789301, 0.7980568, 0.5317856],
		);
	}

	#[test]
	fn unpacks_a_threecomp40_rotation() {
		let file = bind(animation(
			&[0],
			2,
			2,
			&[rotation(1, &[244, 129, 187, 176, 100])],
		));
		close(
			file.motion().sample(0.0)[0].rotation,
			[-0.5343895, 0.3292005, -0.7214217, -0.2925843],
		);
	}

	#[test]
	fn unpacks_a_threecomp48_rotation() {
		let file = bind(animation(
			&[0],
			2,
			2,
			&[rotation(2, &[32, 206, 16, 39, 255, 191])],
		));
		close(
			file.motion().sample(0.0)[0].rotation,
			[0.1561133, -0.9485411, -0.2754967, 0.0],
		);
	}

	/// Blocks hold consecutive runs of frames and share the frame they meet on, so a time past the
	/// first block's last frame is read out of the second.
	#[test]
	fn samples_the_block_a_frame_falls_in() {
		let first = block(
			[0x00, 0x01, 0x00, 0x00],
			&[&reals(&[1.0]), &[], &reals(&[])],
		);
		let second = block(
			[0x00, 0x01, 0x00, 0x00],
			&[&reals(&[2.0]), &[], &reals(&[])],
		);
		let file = bind(animation(&[0], 7, 4, &[first, second]));

		let motion = file.motion();
		close(motion.sample(0.0)[0].translation, [1.0, 0.0, 0.0, 0.0]);
		close(motion.sample(2.9)[0].translation, [1.0, 0.0, 0.0, 0.0]);
		close(motion.sample(3.0)[0].translation, [2.0, 0.0, 0.0, 0.0]);
		close(motion.sample(6.0)[0].translation, [2.0, 0.0, 0.0, 0.0]);
		// A time past the end holds the last frame rather than reading past the last block.
		close(motion.sample(99.0)[0].translation, [2.0, 0.0, 0.0, 0.0]);
	}

	/// Where the float tracks start says how far the transform tracks were meant to read, so a
	/// block that reads a different number of bytes has gone out of step.
	#[test]
	fn rejects_a_block_that_reads_the_wrong_length() {
		let mut block = linear_track(0);
		block.extend([0; 4]);
		assert!(matches!(
			animations(&animation(&[0], 11, 11, &[block])),
			Err(Error::Invalid(..))
		));
	}

	/// A track count and a bone list that disagree describe nothing that can be posed.
	#[test]
	fn rejects_a_bone_list_the_tracks_do_not_match() {
		assert!(matches!(
			animations(&animation(&[0, 1], 11, 11, &[linear_track(0)])),
			Err(Error::Invalid(..))
		));
	}

	#[test]
	fn rejects_a_spline_too_deep_to_blend() {
		let mut curve = knots(&[0; 9], 7);
		pad(&mut curve);
		let file = animation(&[0], 11, 11, &[block([0x00, 0x70, 0x00, 0x00], &[&curve])]);
		assert!(matches!(animations(&file), Err(Error::Invalid(..))));
	}

	/// Paps declare a skeleton class without ever writing one, so the skeleton reader has nothing
	/// to give back.
	#[test]
	fn a_pap_names_no_skeleton() {
		assert!(matches!(
			skeleton(&animation(&[0], 11, 11, &[linear_track(0)])),
			Err(Error::Invalid(..))
		));
	}
}
