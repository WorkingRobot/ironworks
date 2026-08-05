//! Reader for the Havok binary tagfile that animation files embed.

use getset::Getters;

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

/// Read the skeleton the tagfile's animation container names.
pub(super) fn skeleton(data: &[u8]) -> Result<Skeleton> {
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
	Integers(Vec<i64>),
	Floats(Vec<f32>),
	Strings(Vec<String>),
	/// A struct is written a member at a time, each holding every element's value for it.
	Fields(Vec<(String, Value)>),
}

enum Object {
	Skeleton(Skeleton),
	Skeletons(Vec<i64>),
	Other,
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

	fn objects(&mut self) -> Result<Skeleton> {
		let mut remembered = Vec::<Option<Skeleton>>::new();
		let mut named = None;

		loop {
			match self.integer()? {
				FILE_END => break,

				tag @ (OBJECT | OBJECT_REMEMBER) => {
					let class = self.count()?;
					let skeleton = match self.object(class)? {
						Object::Skeleton(skeleton) => Some(skeleton),
						Object::Skeletons(skeletons) => {
							named.get_or_insert(skeletons);
							None
						}
						Object::Other => None,
					};
					if tag == OBJECT_REMEMBER {
						remembered.push(skeleton);
					}
				}

				tag => return Err(invalid(format!("unexpected tag {tag}"))),
			}
		}

		let named = named.ok_or_else(|| invalid("no animation container"))?;
		let &first = named
			.first()
			.ok_or_else(|| invalid("the animation container names no skeleton"))?;
		let skeleton = usize::try_from(first)
			.ok()
			.and_then(|index| remembered.get_mut(index.checked_sub(1)?))
			.and_then(Option::take)
			.ok_or_else(|| invalid(format!("object {first} is not a skeleton")))?;

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

	fn object(&mut self, class: usize) -> Result<Object> {
		let description = self
			.classes
			.get(class)
			.ok_or_else(|| invalid(format!("no description for class {class}")))?;
		let name = description.name.clone();
		let members = description.members.clone();

		let written = self.presence(members.len())?;
		let mut skeleton = Skeleton::default();
		let mut skeletons = Vec::new();

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
					skeletons = references;
				}

				_ => {}
			}
		}

		Ok(match name.as_str() {
			"hkaSkeleton" => Object::Skeleton(skeleton),
			"hkaAnimationContainer" => Object::Skeletons(skeletons),
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
				self.skip(count)?;
				Value::Ignored
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

#[cfg(test)]
mod test {
	use crate::error::Error;

	use super::{Skeleton, skeleton};

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
}
