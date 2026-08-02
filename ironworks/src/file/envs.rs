//! Structs and utilities shared by the environment set formats.
//!
//! `.envb`, `.obsb` and `.essb` each wrap one `ENVS` section, which holds a timeline per weather.
//! A timeline is split into sets, each animating one thing over the day from its own keyframes.

use getset::{CopyGetters, Getters};

use crate::{
	FileStream,
	error::{Error, ErrorValue, Result},
};

/// The file header, ahead of the section.
const HEADER: usize = 0x0C;

/// Bytes one weather takes.
const WEATHER: usize = 16;

/// Bytes one set takes.
const SET: usize = 12;

/// Switches the section ends with.
const OPTIONS: usize = 12;

/// Marks the fields a keyframe grew, written after the ones it already had.
const EXTENDED: u32 = u32::from_le_bytes(*b"007V");

fn invalid(reason: impl Into<String>) -> Error {
	Error::Invalid(ErrorValue::Other("environment set".into()), reason.into())
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
	bytes
		.get(at..at + 4)
		.and_then(|raw| raw.try_into().ok())
		.map(u32::from_le_bytes)
		.ok_or_else(|| invalid(format!("offset {at:#x} is past the end of the file")))
}

fn f32_at(bytes: &[u8], at: usize) -> Result<f32> {
	u32_at(bytes, at).map(f32::from_bits)
}

/// Where the offset at `field` reaches, which the format writes signed from `base` and may point
/// backwards.
fn seek(bytes: &[u8], base: usize, field: usize) -> Result<usize> {
	let offset = u32_at(bytes, field)? as i32;
	base.checked_add_signed(offset as isize)
		.filter(|&at| at < bytes.len())
		.ok_or_else(|| invalid(format!("offset {offset} from {base:#x} leaves the file")))
}

/// A count the file states, rejected where the bytes behind the table cannot hold that many:
/// collecting a range reserves the whole count before the first read fails.
fn entries(bytes: &[u8], declared: u32, table: usize, stride: usize) -> Result<usize> {
	let count = declared as usize;
	let room = bytes.len().saturating_sub(table) / stride;
	match count <= room {
		true => Ok(count),
		false => Err(invalid(format!("a count of {count} in {room} entries"))),
	}
}

/// A null-terminated string at `at`, or an empty one where the offset does not name a string.
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

/// Everything an `ENVS` section holds.
#[derive(Debug, Getters, CopyGetters)]
pub struct Environments {
	/// The client reads `.envb` at version 6, and the other two at version 4.
	#[get_copy = "pub"]
	version: u32,

	/// Twelve switches, which only `.obsb` sets and none of which are identified.
	#[get_copy = "pub"]
	options: [bool; OPTIONS],

	#[getset(skip)]
	weathers: Vec<Weather>,
}

impl Environments {
	/// The weathers the set covers, in the order it names them.
	pub fn weathers(&self) -> &[Weather] {
		&self.weathers
	}
}

/// One weather's worth of settings.
#[derive(Debug, Getters, CopyGetters)]
pub struct Weather {
	/// A row of `Weather`.
	#[get_copy = "pub"]
	id: u32,

	/// Seconds the sets run over before they repeat.
	#[get_copy = "pub"]
	length: f32,

	#[get_copy = "pub"]
	unknown_a: u32,

	#[get_copy = "pub"]
	unknown_b: f32,

	/// Two assets the weather names, which almost every file the game ships leaves empty.
	#[get = "pub"]
	paths: [String; 2],

	#[getset(skip)]
	sets: Vec<Set>,
}

impl Weather {
	/// What the weather animates, in the order it names them.
	pub fn sets(&self) -> &[Set] {
		&self.sets
	}

	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let table = seek(bytes, at, at)?;
		let count = entries(bytes, u32_at(bytes, at + 4)?, table, SET)?;
		let footer = seek(bytes, at, at + 12)?;

		Ok(Self {
			id: u32_at(bytes, at + 8)?,
			length: f32_at(bytes, footer)?,
			unknown_a: u32_at(bytes, footer + 4)?,
			unknown_b: f32_at(bytes, footer + 8)?,
			paths: [
				seek(bytes, footer, footer + 12)?,
				seek(bytes, footer, footer + 16)?,
			]
			.map(|at| string(bytes, at)),
			sets: (0..count)
				.map(|index| Set::parse(bytes, table + index * SET))
				.collect::<Result<_>>()?,
		})
	}
}

/// One thing a weather animates.
#[derive(Debug, CopyGetters)]
pub struct Set {
	/// What the set animates, which each of the three formats numbers in its own range. It also
	/// says how the rest of a keyframe is laid out.
	#[get_copy = "pub"]
	kind: u32,

	keyframes: Vec<Keyframe>,
}

impl Set {
	/// The keyframes of the set, ascending by time.
	pub fn keyframes(&self) -> &[Keyframe] {
		&self.keyframes
	}

	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let kind = u32_at(bytes, at + 8)?;
		let layout = layout(kind).ok_or_else(|| invalid(format!("unknown kind {kind}")))?;
		let table = seek(bytes, at, at)?;
		let count = entries(bytes, u32_at(bytes, at + 4)?, table, size_of::<i32>())?;

		Ok(Self {
			kind,
			keyframes: (0..count)
				.map(|index| {
					Keyframe::parse(bytes, seek(bytes, table, table + index * 4)?, &layout)
				})
				.collect::<Result<_>>()?,
		})
	}
}

/// One point on a set's timeline.
#[derive(Debug, Getters, CopyGetters)]
pub struct Keyframe {
	/// Seconds since midnight.
	#[get_copy = "pub"]
	time: f32,

	/// The colours the keyframe reaches, in the order it names them.
	#[get = "pub"]
	colours: Vec<Colour>,

	/// The assets the keyframe names, which `.envb` uses for effects and `.essb` for sound.
	#[get = "pub"]
	paths: Vec<String>,

	/// The rest of the keyframe, past its time. What it holds follows the kind of its set, none of
	/// it is identified, and the offsets reaching the colours and paths sit inside it.
	#[get = "pub"]
	unknown: Vec<u8>,
}

impl Keyframe {
	fn parse(bytes: &[u8], at: usize, layout: &Layout) -> Result<Self> {
		let extended = layout
			.extension
			.filter(|_| u32_at(bytes, at + layout.size).is_ok_and(|magic| magic == EXTENDED));

		let colours = layout
			.colours
			.iter()
			.map(|&field| at + field)
			.chain(
				extended
					.into_iter()
					.flat_map(|(_, colours)| colours.iter().map(|&field| at + layout.size + field)),
			)
			.map(|field| Colour::parse(bytes, seek(bytes, at, field)?))
			.collect::<Result<_>>()?;

		let paths = match layout.paths {
			false => Vec::new(),
			true => {
				let table = seek(bytes, at, at + 4)?;
				let count = entries(bytes, u32_at(bytes, at + 8)?, table, size_of::<i32>())?;
				(0..count)
					.map(|index| Ok(string(bytes, seek(bytes, table, table + index * 4)?)))
					.collect::<Result<_>>()?
			}
		};

		let end = at + layout.size + extended.map_or(0, |(size, _)| size);
		let unknown = bytes
			.get(at + 4..end)
			.ok_or_else(|| invalid(format!("keyframe at {at:#x} runs past the end of the file")))?
			.to_vec();

		Ok(Self {
			time: f32_at(bytes, at)?,
			colours,
			paths,
			unknown,
		})
	}
}

/// A colour and the intensity it is scaled by.
#[derive(Debug, Clone, Copy, CopyGetters)]
#[get_copy = "pub"]
pub struct Colour {
	red: u8,
	green: u8,
	blue: u8,
	alpha: u8,
	intensity: f32,
}

impl Colour {
	fn parse(bytes: &[u8], at: usize) -> Result<Self> {
		let [red, green, blue, alpha] = u32_at(bytes, at)?.to_le_bytes();
		Ok(Self {
			red,
			green,
			blue,
			alpha,
			intensity: f32_at(bytes, at + 4)?,
		})
	}
}

/// How the keyframes of one kind are laid out: the bytes one takes, where inside it the colour
/// offsets sit, whether it names assets, and what the fields it grew add.
struct Layout {
	size: usize,
	colours: &'static [usize],
	paths: bool,
	extension: Option<(usize, &'static [usize])>,
}

#[rustfmt::skip]
fn layout(kind: u32) -> Option<Layout> {
	Some(match kind {
		0 => Layout { size: 44, colours: &[4, 20, 24], paths: false, extension: Some((8, &[])) },
		1 => Layout { size: 32, colours: &[4, 8, 12], paths: false, extension: None },
		2 => Layout { size: 28, colours: &[20, 24], paths: false, extension: None },
		3..=5 => Layout { size: 40, colours: &[32], paths: false, extension: None },
		6 => Layout { size: 16, colours: &[], paths: false, extension: Some((28, &[])) },
		7 => Layout { size: 24, colours: &[8, 12], paths: false, extension: None },
		8 => Layout { size: 20, colours: &[], paths: false, extension: None },
		9 => Layout { size: 28, colours: &[], paths: false, extension: None },
		10 => Layout { size: 52, colours: &[20], paths: false, extension: Some((24, &[20])) },
		11 => Layout { size: 48, colours: &[12, 16, 28, 32], paths: true, extension: None },
		12 => Layout { size: 32, colours: &[20], paths: false, extension: None },
		13 => Layout { size: 32, colours: &[4], paths: false, extension: Some((56, &[48])) },
		20 => Layout { size: 12, colours: &[], paths: true, extension: None },
		21 | 29 | 30 | 32 => Layout { size: 8, colours: &[], paths: false, extension: None },
		31 => Layout { size: 12, colours: &[], paths: false, extension: None },
		33 | 35 => Layout { size: 8, colours: &[4], paths: false, extension: None },
		34 => Layout { size: 12, colours: &[4, 8], paths: false, extension: None },
		_ => return None,
	})
}

/// The environment set a file opening with `magic` holds.
pub(super) fn read(mut stream: impl FileStream, magic: &[u8; 4]) -> Result<Environments> {
	let mut bytes = Vec::new();
	stream.read_to_end(&mut bytes)?;

	if bytes.get(..4) != Some(magic) || bytes.get(HEADER..HEADER + 4) != Some(b"ENVS") {
		return Err(invalid(format!(
			"not an {} file",
			String::from_utf8_lossy(magic)
		)));
	}

	// Offsets inside the section are measured from its body, which follows the magic and length.
	let body = HEADER + 8;
	let table = seek(&bytes, body, body + 4)?;
	let count = entries(&bytes, u32_at(&bytes, body + 8)?, table, WEATHER)?;

	let at = seek(&bytes, body, body + 12)?;
	let switches = bytes
		.get(at..at + OPTIONS)
		.ok_or_else(|| invalid(format!("switches at {at:#x} are past the end of the file")))?;

	Ok(Environments {
		version: u32_at(&bytes, body)?,
		options: std::array::from_fn(|index| switches[index] != 0),
		weathers: (0..count)
			.map(|index| Weather::parse(&bytes, table + index * WEATHER))
			.collect::<Result<_>>()?,
	})
}

#[cfg(test)]
mod test {
	use std::io::Cursor;

	use super::read;

	/// Builds a set of `weathers`, each holding one wetness set of one keyframe, with the section
	/// header the format states rather than one the reader assumes.
	fn build(magic: &[u8; 4], weathers: &[u32]) -> Vec<u8> {
		let mut bytes = Vec::from(*magic);
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());

		bytes.extend(*b"ENVS");
		bytes.extend(24u32.to_le_bytes());
		bytes.extend(6u32.to_le_bytes());
		bytes.extend(16u32.to_le_bytes());
		bytes.extend(u32::try_from(weathers.len()).unwrap().to_le_bytes());
		bytes.extend((16 + 16 * weathers.len() as u32 + 56).to_le_bytes());

		let rest = 16 * weathers.len();
		for (index, &weather) in weathers.iter().enumerate() {
			let footer = i32::try_from(rest - 16 * index).unwrap();
			bytes.extend((footer + 20).to_le_bytes());
			bytes.extend(1u32.to_le_bytes());
			bytes.extend(weather.to_le_bytes());
			bytes.extend(footer.to_le_bytes());
		}

		bytes.extend(86400f32.to_le_bytes());
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(1f32.to_le_bytes());
		bytes.extend(4u32.to_le_bytes());
		bytes.extend(4u32.to_le_bytes());

		bytes.extend(12u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());
		bytes.extend(8u32.to_le_bytes());

		bytes.extend(4u32.to_le_bytes());
		bytes.extend(3600f32.to_le_bytes());
		bytes.extend((0..4).flat_map(|index| (index as f32).to_le_bytes()));

		bytes.extend([1u8, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
		bytes
	}

	#[test]
	fn reads_the_weathers_a_set_covers() {
		let file = read(Cursor::new(build(b"ENVB", &[1, 2, 59])), b"ENVB").unwrap();
		assert_eq!(file.version(), 6);
		assert_eq!(file.options()[..4], [true, false, false, true]);

		let ids: Vec<u32> = file.weathers().iter().map(|weather| weather.id()).collect();
		assert_eq!(ids, [1, 2, 59]);
	}

	#[test]
	fn reads_the_keyframes_of_every_set() {
		let file = read(Cursor::new(build(b"ENVB", &[1, 2])), b"ENVB").unwrap();

		let weather = &file.weathers()[1];
		assert_eq!(weather.length(), 86400.);
		assert_eq!(weather.paths(), &[String::new(), String::new()]);

		let sets = weather.sets();
		assert_eq!(sets.len(), 1);
		assert_eq!(sets[0].kind(), 8);

		let keyframes = sets[0].keyframes();
		assert_eq!(keyframes.len(), 1);
		assert_eq!(keyframes[0].time(), 3600.);
		assert!(keyframes[0].colours().is_empty());
		assert_eq!(keyframes[0].unknown().len(), 16);
	}

	/// Every structure is reached by an offset, so nothing may be read on the word of a header
	/// alone.
	#[test]
	fn truncated() {
		let mut bytes = build(b"ENVB", &[1, 2]);
		bytes.truncate(bytes.len() - 1);
		assert!(read(Cursor::new(bytes), b"ENVB").is_err());
	}

	#[test]
	fn rejects_another_format() {
		assert!(read(Cursor::new(build(b"ESSB", &[1])), b"ENVB").is_err());
	}
}
