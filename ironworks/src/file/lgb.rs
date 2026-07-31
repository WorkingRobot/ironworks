//! Structs and utilities for parsing .lgb files.

use crate::{FileStream, error::Result};

use super::{File, layer::LayerGroup};

/// The file header, ahead of the first section.
const HEADER: usize = 0x0C;

/// A zone's layer group: where everything in a zone is placed, one file per group.
///
/// A zone ships seven of these beside its models, named for the group each carries: `bg`,
/// `planmap`, `planner`, `planevent`, `planlive`, `sound` and `vfx`.
#[derive(Debug)]
pub struct LayerGroupFile(LayerGroup);

impl LayerGroupFile {
	/// The group the file holds.
	pub fn group(&self) -> &LayerGroup {
		&self.0
	}
}

impl File for LayerGroupFile {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		let mut bytes = Vec::new();
		stream.read_to_end(&mut bytes)?;

		if bytes.get(..4) != Some(b"LGB1") {
			return Err(super::layer::invalid("not an LGB1 file"));
		}

		// The header declares a section count, but every file the game ships declares one and
		// carries one, so the first is the only one read.
		let at = (HEADER..bytes.len().saturating_sub(4))
			.find(|&at| bytes[at..at + 4] == *b"LGP1")
			.ok_or_else(|| super::layer::invalid("no LGP1 section"))?;

		// A section states its own header length, and the older files put eight more bytes in it.
		let size = super::layer::section_size(&bytes, at)?;
		Ok(Self(LayerGroup::parse(&bytes, at, size)?))
	}
}

#[cfg(test)]
mod test {
	use std::io::Cursor;

	use crate::file::{
		File,
		layer::{InstanceData, InstanceKind},
	};

	use super::LayerGroupFile;

	/// Builds a file around one layer holding `instances`, each already laid out.
	///
	/// `section` is the section header's declared length, which is what everything inside it is
	/// measured from; the older files write 32 where the rest write 24.
	fn build(section: usize, instances: &[Vec<u8>]) -> Vec<u8> {
		const LAYER_HEADER: usize = 52;

		let mut bytes = Vec::from(*b"LGB1");
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());

		let at = bytes.len();
		bytes.extend(*b"LGP1");
		bytes.extend((section as u32).to_le_bytes());
		bytes.resize(at + section - 16, 0);

		// Offsets inside the section are measured from the four fields that end its header.
		let heap = bytes.len();
		let table = heap + 16;
		let layer = table + 4;
		let instance_table = layer + LAYER_HEADER;
		let first = instance_table + instances.len() * 4;

		let names = first + instances.iter().map(Vec::len).sum::<usize>();
		bytes.extend(256u32.to_le_bytes());
		bytes.extend(((names - heap) as i32).to_le_bytes());
		bytes.extend(16u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());
		bytes.extend(((layer - table) as i32).to_le_bytes());

		bytes.extend(7u32.to_le_bytes());
		bytes.extend(((names + 6 - layer) as i32).to_le_bytes());
		bytes.extend((LAYER_HEADER as u32).to_le_bytes());
		bytes.extend((instances.len() as u32).to_le_bytes());
		bytes.extend([1, 0, 0, 1]);
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(3u16.to_le_bytes());
		bytes.extend(4u16.to_le_bytes());
		bytes.resize(layer + LAYER_HEADER, 0);

		let mut offset = first;
		for instance in instances {
			bytes.extend(((offset - instance_table) as i32).to_le_bytes());
			offset += instance.len();
		}
		for instance in instances {
			bytes.extend(instance);
		}
		bytes.extend(b"group\0");
		bytes.extend(b"layer\0");
		bytes
	}

	/// The 0x30 bytes every instance opens with, then whatever its kind adds.
	fn instance(kind: i32, id: u32, payload: &[u8]) -> Vec<u8> {
		let mut bytes = Vec::new();
		bytes.extend(kind.to_le_bytes());
		bytes.extend(id.to_le_bytes());
		bytes.extend(0i32.to_le_bytes());
		for value in [1.0f32, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
			bytes.extend(value.to_le_bytes());
		}
		bytes.extend(payload);
		bytes
	}

	fn read(bytes: Vec<u8>) -> LayerGroupFile {
		LayerGroupFile::read(Cursor::new(bytes)).unwrap()
	}

	#[test]
	fn reads_a_layer_and_the_instance_on_it() {
		let file = read(build(24, &[instance(1, 42, &[0; 44])]));
		let group = file.group();
		assert_eq!(group.id(), 256);
		assert_eq!(group.name(), "group");

		let [layer] = group.layers() else {
			panic!("expected one layer")
		};
		assert_eq!(layer.name(), "layer");
		assert_eq!((layer.festival_id(), layer.festival_phase_id()), (3, 4));
		assert!(layer.visible());

		let [placed] = layer.instances() else {
			panic!("expected one instance")
		};
		assert_eq!(placed.kind(), InstanceKind::BgPart);
		assert_eq!(placed.id(), 42);
		assert_eq!(placed.transform().translation(), [1.0, 2.0, 3.0]);
		assert!(matches!(placed.data(), InstanceData::BgPart(_)));
	}

	/// Nothing states an instance's length, so an unmodelled kind is bounded by whatever follows it.
	/// Physis has no such fallback and fails the whole file on a kind it does not model.
	#[test]
	fn an_unmodelled_kind_keeps_its_bytes() {
		let file = read(build(
			24,
			&[instance(39, 1, &[0xAA; 12]), instance(1, 2, &[0; 44])],
		));
		let [weapon, _] = file.group().layers()[0].instances() else {
			panic!("expected two instances")
		};

		assert_eq!(weapon.kind(), InstanceKind::Weapon);
		let InstanceData::Unknown(data) = weapon.data() else {
			panic!("Weapon has no modelled payload, so its bytes should be kept")
		};
		assert_eq!(data, &[0xAA; 12]);
	}

	/// The older zone files declare a 32-byte section header rather than 24, which is why reading
	/// the four trailing fields at a fixed offset finds nothing but padding.
	#[test]
	fn a_longer_section_header_still_finds_its_fields() {
		let file = read(build(32, &[instance(1, 42, &[0; 44])]));
		assert_eq!(file.group().name(), "group");
		assert_eq!(file.group().layers()[0].instances()[0].id(), 42);
	}
}
