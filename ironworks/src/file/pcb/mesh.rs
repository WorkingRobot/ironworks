use std::io::Cursor;

use binrw::BinRead;
use getset::CopyGetters;

use crate::{FileStream, error::Result, file::File};

use super::{
	invalid,
	structs::{self, BoundingBox, MaterialWidth, Primitive},
};

/// A collision mesh, held as a binary tree of nodes reached by byte offset.
///
/// Every node bounds the geometry of the subtree below it, and only the leaves carry any.
#[derive(Debug)]
pub struct Mesh {
	version: u32,
	root: Node,
}

impl Mesh {
	/// Read a mesh whose primitives carry a material mask of the given width. [`File::read`] reads
	/// the width off the node extents instead.
	pub fn read_with(mut stream: impl FileStream, width: MaterialWidth) -> Result<Self> {
		let mut bytes = Vec::new();
		stream.read_to_end(&mut bytes)?;
		Self::parse(&bytes, width)
	}

	/// Version of the mesh structure. The client reads `1` and `4` alike and treats `0` as a legacy
	/// form, which is not supported here.
	pub fn version(&self) -> u32 {
		self.version
	}

	/// The node the rest of the tree hangs off, bounding the entire mesh.
	pub fn root(&self) -> &Node {
		&self.root
	}

	pub(super) fn parse(bytes: &[u8], width: MaterialWidth) -> Result<Self> {
		let header = structs::Header::read(&mut Cursor::new(bytes))?;
		if header.kind != 0 {
			return Err(invalid(format!(
				"expected a mesh, found a list of {} entries",
				header.kind
			)));
		}
		if !matches!(header.version, 1 | 4) {
			return Err(invalid(format!(
				"unsupported structure version {}",
				header.version
			)));
		}

		let mut budget = header.total_child_nodes.saturating_add(1);
		Ok(Self {
			version: header.version,
			root: node(bytes, structs::Header::SIZE, width, &mut budget)?,
		})
	}
}

impl File for Mesh {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		let mut bytes = Vec::new();
		stream.read_to_end(&mut bytes)?;
		Self::parse(&bytes, width_of(&bytes))
	}
}

/// Bytes a node spends before its vertices.
const NODE: usize = 0x30;

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
	Some(usize::from(u16::from_le_bytes(
		bytes.get(at..at + 2)?.try_into().ok()?,
	)))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
	Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Where one node's triangles start and how many of them there are, read without knowing how wide
/// each of them is written.
struct Extent {
	start: usize,
	geometry: usize,
	primitives: usize,
}

fn extents(bytes: &[u8], start: usize, into: &mut Vec<Extent>, budget: &mut u32) -> Option<()> {
	*budget = budget.checked_sub(1)?;
	let compressed = u16_at(bytes, start + 0x28)?;
	let primitives = u16_at(bytes, start + 0x2a)?;
	let raw = u16_at(bytes, start + 0x2c)?;
	into.push(Extent {
		start,
		geometry: start + NODE + raw * 12 + compressed * 6,
		primitives,
	});
	for at in [start + 8, start + 12] {
		let offset = i32_at(bytes, at)?;
		if offset != 0 {
			let child = start.checked_add(usize::try_from(offset).ok()?)?;
			extents(bytes, child, into, budget)?;
		}
	}
	Some(())
}

/// Which width a mesh's primitives are written at, off the room each node leaves its triangles.
///
/// A node's geometry runs up to the next node in the file, so a width that overruns that is not the
/// one the mesh was written at. Nodes are padded apart by a run of zeroes long enough to hold the
/// difference, so where both widths fit, the wide reading is the one that makes a triangle with no
/// corners out of that padding.
pub(super) fn width_of(bytes: &[u8]) -> MaterialWidth {
	let mut budget = i32_at(bytes, 8)
		.and_then(|count| u32::try_from(count).ok())
		.unwrap_or(0)
		.saturating_add(1);
	let mut held = Vec::new();
	if extents(bytes, structs::Header::SIZE, &mut held, &mut budget).is_none() {
		return MaterialWidth::Wide;
	}

	let mut starts: Vec<usize> = held.iter().map(|extent| extent.start).collect();
	starts.sort_unstable();
	let limit = |extent: &Extent| {
		starts
			.get(starts.partition_point(|&start| start <= extent.start))
			.copied()
			.unwrap_or(bytes.len())
	};

	let wide = held
		.iter()
		.all(|extent| extent.geometry + extent.primitives * 12 <= limit(extent));
	let blank = held.iter().any(|extent| {
		(0..extent.primitives).any(|at| {
			let start = extent.geometry + at * 12;
			bytes
				.get(start..start + 12)
				.is_some_and(|held| held.iter().all(|byte| *byte == 0))
		})
	});

	match wide && !blank {
		true => MaterialWidth::Wide,
		false => MaterialWidth::Narrow,
	}
}

/// One node of a mesh's tree, bounding the geometry of everything below it.
#[derive(Debug, CopyGetters)]
pub struct Node {
	/// Zero in every version 1 mesh. A version 4 mesh writes `0x1_0000_0000` in every node.
	#[get_copy = "pub"]
	unknown_a: u64,

	/// The volume this node and its children cover.
	#[get_copy = "pub"]
	bounds: BoundingBox,

	vertices: Vec<[f32; 3]>,
	primitives: Vec<Primitive>,
	children: Vec<Node>,
}

impl Node {
	/// Every vertex this node's triangles are drawn from, in the order their indices count in.
	/// Positions written relative to [`bounds`](Self::bounds) are resolved.
	pub fn vertices(&self) -> &[[f32; 3]] {
		&self.vertices
	}

	/// The triangles this node carries, which is none for anything but a leaf.
	pub fn primitives(&self) -> &[Primitive] {
		&self.primitives
	}

	/// The nodes hanging off this one, of which there are at most two.
	pub fn children(&self) -> &[Node] {
		&self.children
	}
}

/// The tree is walked by offset rather than read in order, as nodes are padded apart. `budget`
/// bounds the walk to the node count the file declares.
fn node(bytes: &[u8], start: usize, width: MaterialWidth, budget: &mut u32) -> Result<Node> {
	*budget = budget
		.checked_sub(1)
		.ok_or_else(|| invalid("more nodes than the header declares"))?;

	let rest = bytes.get(start..).ok_or_else(|| {
		invalid(format!(
			"node offset {start:#x} is past the end of the file"
		))
	})?;
	let raw = structs::Node::read_args(&mut Cursor::new(rest), (width,))?;

	let min = raw.bounds.min();
	let max = raw.bounds.max();
	let scale: [f32; 3] = std::array::from_fn(|axis| (max[axis] - min[axis]) / f32::from(u16::MAX));
	let vertices = raw
		.raw_vertices
		.into_iter()
		.chain(raw.compressed_vertices.iter().map(|vertex| {
			std::array::from_fn(|axis| min[axis] + scale[axis] * f32::from(vertex[axis]))
		}))
		.collect();

	// Only a forward offset can be followed, which is also what keeps the walk from looping.
	let mut children = Vec::new();
	for offset in raw.child_offsets {
		if offset == 0 {
			continue;
		}
		let at = usize::try_from(offset)
			.ok()
			.and_then(|offset| start.checked_add(offset))
			.ok_or_else(|| invalid(format!("node at {start:#x} points at itself or earlier")))?;
		children.push(node(bytes, at, width, budget)?);
	}

	Ok(Node {
		unknown_a: raw.unknown_a,
		bounds: raw.bounds,
		vertices,
		primitives: raw.primitives,
		children,
	})
}

#[cfg(test)]
mod test {
	use std::io::Cursor;

	use crate::{error::Error, file::File};

	use super::{MaterialWidth, Mesh, structs};

	fn header(child_nodes: u32, primitives: u32) -> Vec<u8> {
		let mut bytes = Vec::new();
		for field in [0, 1, child_nodes, primitives] {
			bytes.extend(field.to_le_bytes());
		}
		bytes
	}

	fn node(
		children: (i32, i32),
		bounds: ([f32; 3], [f32; 3]),
		raw: &[[f32; 3]],
		compressed: &[[u16; 3]],
		primitives: &[([u8; 3], u64)],
		width: MaterialWidth,
	) -> Vec<u8> {
		let mut bytes = vec![0; 8];
		bytes.extend(children.0.to_le_bytes());
		bytes.extend(children.1.to_le_bytes());
		for axis in bounds.0.into_iter().chain(bounds.1) {
			bytes.extend(axis.to_le_bytes());
		}
		for count in [compressed.len(), primitives.len(), raw.len()] {
			bytes.extend(u16::try_from(count).unwrap().to_le_bytes());
		}
		bytes.extend([0; 2]);

		for axis in raw.iter().flatten() {
			bytes.extend(axis.to_le_bytes());
		}
		for axis in compressed.iter().flatten() {
			bytes.extend(axis.to_le_bytes());
		}
		for &(indices, material) in primitives {
			bytes.extend(indices);
			bytes.push(0x20);
			match width {
				MaterialWidth::Narrow => {
					bytes.extend(u16::try_from(material).unwrap().to_le_bytes())
				}
				MaterialWidth::Wide => bytes.extend(material.to_le_bytes()),
			}
		}
		bytes
	}

	const UNIT: ([f32; 3], [f32; 3]) = ([0.0; 3], [1.0; 3]);
	const TRIANGLE: [[f32; 3]; 3] = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

	fn leaf(width: MaterialWidth) -> Vec<u8> {
		node((0, 0), UNIT, &TRIANGLE, &[], &[([0, 1, 2], 0x7004)], width)
	}

	#[test]
	fn reads_a_leaf_only_file() {
		let mut bytes = header(0, 1);
		bytes.extend(leaf(MaterialWidth::Wide));

		let file = Mesh::read(Cursor::new(bytes)).unwrap();
		let root = file.root();
		assert_eq!(file.version(), 1);
		assert!(root.children().is_empty());
		assert_eq!(root.vertices(), TRIANGLE);
		assert_eq!(root.primitives().len(), 1);
		assert_eq!(root.primitives()[0].indices(), [0, 1, 2]);
		assert_eq!(root.primitives()[0].material(), 0x7004);
	}

	/// Nodes are padded apart by an arbitrary run of zeroes, so the tree can only be walked by the
	/// offsets it declares.
	#[test]
	fn reads_both_children_across_the_padding_between_nodes() {
		let first = leaf(MaterialWidth::Wide);
		let second = node(
			(0, 0),
			UNIT,
			&[],
			&[[0, 0, 0], [65535, 0, 0]],
			&[([0, 1, 1], 0x2004), ([1, 0, 1], 0x2004)],
			MaterialWidth::Wide,
		);

		let root = 0x30 + 24;
		let mut bytes = header(2, 3);
		bytes.extend(node(
			(i32::try_from(root).unwrap(), 0),
			UNIT,
			&[],
			&[],
			&[],
			MaterialWidth::Wide,
		));
		bytes.extend([0; 24]);
		let sibling = root + first.len() + 8;
		bytes.extend(first);
		bytes.extend([0; 8]);
		bytes.extend(second);

		let offsets = i32::try_from(sibling).unwrap().to_le_bytes();
		bytes[structs::Header::SIZE + 0xc..structs::Header::SIZE + 0x10].copy_from_slice(&offsets);

		let file = Mesh::read(Cursor::new(bytes)).unwrap();
		let children = file.root().children();
		assert!(file.root().primitives().is_empty());
		assert_eq!(children.len(), 2);
		assert_eq!(children[0].vertices(), TRIANGLE);
		assert_eq!(children[1].primitives().len(), 2);
		assert_eq!(children[1].primitives()[1].indices(), [1, 0, 1]);
	}

	/// Positions written against the node's bounds resolve to its corners at either extreme.
	#[test]
	fn decodes_quantised_vertices() {
		let bounds = ([0.0, -65535.0, -2.0], [65535.0, 0.0, 65533.0]);
		let mut bytes = header(0, 0);
		bytes.extend(node(
			(0, 0),
			bounds,
			&[[1.0; 3]],
			&[[0, 0, 0], [65535, 65535, 65535], [32768, 0, 0]],
			&[],
			MaterialWidth::Wide,
		));

		let file = Mesh::read(Cursor::new(bytes)).unwrap();
		let vertices = file.root().vertices();
		assert_eq!(vertices[0], [1.0; 3]);
		assert_eq!(vertices[1], bounds.0);
		assert_eq!(vertices[2], bounds.1);
		assert_eq!(vertices[3][0], 32768.0);
	}

	/// Nothing in the file says which width the material mask is written at.
	#[test]
	fn reads_either_material_width() {
		let mut narrow = header(0, 1);
		narrow.extend(leaf(MaterialWidth::Narrow));
		let file = Mesh::read(Cursor::new(narrow.clone())).unwrap();
		assert_eq!(file.root().primitives()[0].material(), 0x7004);
		assert_eq!(file.root().primitives()[0].unknown_a(), 0x20);

		// The wide reading of the same bytes runs past the end of the file.
		assert!(matches!(
			Mesh::read_with(Cursor::new(narrow), MaterialWidth::Wide),
			Err(Error::Resource(_))
		));
	}

	/// Where a node's padding is long enough to hold the primitives a wide reading would take from
	/// it, the padding being blank is what tells the two apart.
	#[test]
	fn reads_a_narrow_node_the_wide_width_would_fit_inside() {
		let mut bytes = header(0, 2);
		bytes.extend(node(
			(0, 0),
			UNIT,
			&TRIANGLE,
			&[],
			&[([0, 1, 2], 0x7004), ([2, 1, 0], 0x7002)],
			MaterialWidth::Narrow,
		));
		bytes.extend([0; 12]);

		let file = Mesh::read(Cursor::new(bytes)).unwrap();
		let primitives = file.root().primitives();
		assert_eq!(primitives.len(), 2);
		assert_eq!(primitives[1].indices(), [2, 1, 0]);
		assert_eq!(primitives[1].material(), 0x7002);
	}

	/// The client reads version 4 the way it reads 1, and nothing describes the legacy zero.
	#[test]
	fn rejects_the_legacy_version() {
		let mut bytes = header(0, 1);
		bytes.extend(leaf(MaterialWidth::Wide));
		bytes[4] = 4;
		bytes[structs::Header::SIZE + 4] = 1;
		let file = Mesh::read(Cursor::new(bytes.clone())).unwrap();
		assert_eq!(file.root().unknown_a(), 0x1_0000_0000);

		bytes[4] = 0;
		assert!(matches!(
			Mesh::read(Cursor::new(bytes)),
			Err(Error::Invalid(..))
		));
	}

	#[test]
	fn rejects_a_truncated_node() {
		let mut bytes = header(0, 1);
		bytes.extend(leaf(MaterialWidth::Wide));
		bytes.truncate(bytes.len() - 16);
		assert!(matches!(
			Mesh::read(Cursor::new(bytes)),
			Err(Error::Resource(_))
		));
	}

	#[test]
	fn rejects_a_child_offset_pointing_back_at_its_parent() {
		let mut bytes = header(1, 1);
		bytes.extend(leaf(MaterialWidth::Wide));
		bytes[structs::Header::SIZE + 8..structs::Header::SIZE + 0xc]
			.copy_from_slice(&(-16i32).to_le_bytes());
		assert!(matches!(
			Mesh::read(Cursor::new(bytes)),
			Err(Error::Invalid(..))
		));
	}

	/// The declared node count is what bounds the walk.
	#[test]
	fn rejects_more_nodes_than_the_header_declares() {
		let child = 0x30;
		let mut bytes = header(0, 0);
		for _ in 0..2 {
			bytes.extend(node((child, 0), UNIT, &[], &[], &[], MaterialWidth::Wide));
		}
		bytes.extend(node((0, 0), UNIT, &[], &[], &[], MaterialWidth::Wide));
		assert!(matches!(
			Mesh::read(Cursor::new(bytes)),
			Err(Error::Invalid(..))
		));
	}
}
