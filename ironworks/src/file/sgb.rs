//! Structs and utilities for parsing .sgb files.

use crate::{FileStream, error::Result};

use super::{File, layer::Scene};

/// A shared group: a prefab of instances that a zone can place many times over.
#[derive(Debug)]
pub struct SharedGroupFile(Scene);

impl SharedGroupFile {
	/// The scene the file holds.
	pub fn scene(&self) -> &Scene {
		&self.0
	}
}

impl File for SharedGroupFile {
	fn read(stream: impl FileStream) -> Result<Self> {
		Ok(Self(super::layer::scene(stream, b"SGB1")?))
	}
}

#[cfg(test)]
mod test {
	use std::io::Cursor;

	use crate::file::{File, layer::InstanceData};

	use super::SharedGroupFile;

	/// Builds a scene around one layer group holding one background model, with every string the
	/// scene names written into a heap at the end.
	fn build(magic: &[u8; 4], pad: usize) -> Vec<u8> {
		const LAYER_HEADER: usize = 52;
		const GENERAL: usize = 92;
		const NAMES: [&[u8]; 10] = [
			b"group\0",
			b"layer\0",
			b"bg/dir\0",
			b"a.svb\0",
			b"a.lcb\0",
			b"a.envb\0",
			b"a.essb\0",
			b"a.lgb\0",
			b"first\0",
			b"second\0",
		];
		/// What the handler block spends, so the timelines can follow it.
		const BLOCK: usize = 248;
		/// A timeline region: a `TMLB` holding one `TMDH`.
		const TMLB: usize = 28;

		let mut bytes = Vec::from(*magic);
		bytes.extend(0u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());
		bytes.resize(bytes.len() + pad, 0);

		bytes.extend(*b"SCN1");
		bytes.extend(0u32.to_le_bytes());
		// The older files put two empty fields ahead of the body.
		bytes.resize(bytes.len() + pad, 0);

		let body = bytes.len();
		let general = body + 64;
		let environments = general + GENERAL;
		let resources = environments + 24;
		let heap = resources + 4;
		let table = heap + 16;
		let layer = table + 4;
		let instance_table = layer + LAYER_HEADER;
		let first = instance_table + 4;
		let names = first + 0x30 + 44;
		let handlers = names + NAMES.iter().map(|name| name.len()).sum::<usize>();
		let timelines = handlers + BLOCK;
		let entries = timelines + 2 * TMLB + 8;
		let pairs = entries + 88;

		let mut offsets = [0i32; 16];
		offsets[0] = (heap - body) as i32;
		offsets[1] = 1;
		offsets[2] = (general - body) as i32;
		offsets[4] = (timelines + 2 * TMLB - body) as i32;
		offsets[5] = (resources - body) as i32;
		offsets[6] = 1;
		offsets[8] = (handlers - body) as i32;
		bytes.extend(offsets.map(i32::to_le_bytes).concat());

		let name = |index: usize| {
			(names + NAMES[..index].iter().map(|name| name.len()).sum::<usize>()) as i32
		};
		bytes.resize(general + 4, 0);
		bytes.extend((name(2) - general as i32).to_le_bytes());
		bytes.extend(((environments - general) as i32).to_le_bytes());
		bytes.extend(1i32.to_le_bytes());
		bytes.resize(general + 20, 0);
		bytes.extend((name(3) - general as i32).to_le_bytes());
		bytes.resize(general + 52, 0);
		bytes.extend((name(4) - general as i32).to_le_bytes());
		bytes.resize(general + 88, 0);
		bytes.extend(*b"007V");

		bytes.extend((name(5) - environments as i32).to_le_bytes());
		bytes.extend(2i32.to_le_bytes());
		bytes.extend(9i32.to_le_bytes());
		bytes.extend((name(6) - environments as i32).to_le_bytes());

		bytes.resize(resources, 0);
		bytes.extend(((name(7) - resources as i32) as i32).to_le_bytes());

		bytes.extend(256u32.to_le_bytes());
		bytes.extend((name(0) - heap as i32).to_le_bytes());
		bytes.extend(16u32.to_le_bytes());
		bytes.extend(1u32.to_le_bytes());
		bytes.extend(((layer - table) as i32).to_le_bytes());

		bytes.extend(7u32.to_le_bytes());
		bytes.extend((name(1) - layer as i32).to_le_bytes());
		bytes.extend((LAYER_HEADER as u32).to_le_bytes());
		bytes.extend(1u32.to_le_bytes());
		bytes.extend([1, 0, 0, 1]);
		bytes.resize(layer + LAYER_HEADER, 0);

		bytes.extend(((first - instance_table) as i32).to_le_bytes());
		bytes.extend(1i32.to_le_bytes());
		bytes.extend(11u32.to_le_bytes());
		bytes.resize(first + 0x30 + 44, 0);

		for name in NAMES {
			bytes.extend(name);
		}

		bytes.resize(handlers + 0x24, 0);
		bytes.extend(8i32.to_le_bytes());
		bytes.extend(2i32.to_le_bytes());
		bytes.extend(8i32.to_le_bytes());
		bytes.extend(48i32.to_le_bytes());

		// A turn about one axis, on one of the scene's instances.
		let record = bytes.len();
		bytes.extend(2i32.to_le_bytes());
		bytes.resize(record + 16, 0);
		bytes.extend(12i32.to_le_bytes());
		bytes.extend(1i32.to_le_bytes());
		bytes.extend((-450.0f32).to_le_bytes());
		bytes.resize(record + 40, 0);

		// One repeating transform, moving two of the scene's instances.
		let record = bytes.len();
		bytes.extend(5i32.to_le_bytes());
		bytes.resize(record + 16, 0);
		bytes.extend(44i32.to_le_bytes());
		bytes.extend(2i32.to_le_bytes());
		bytes.resize(record + 32, 0);
		bytes.extend(48i32.to_le_bytes());
		bytes.extend(84i32.to_le_bytes());
		bytes.extend(120i32.to_le_bytes());
		bytes.extend([11, 12, 0, 0]);

		let lane = |active: i32, amount: [f32; 4], period: i32, wrap: i32| {
			let mut held = Vec::from(active.to_le_bytes());
			held.extend(amount.map(f32::to_le_bytes).concat());
			held.extend(period.to_le_bytes());
			held.extend(0i32.to_le_bytes());
			held.extend(0i32.to_le_bytes());
			held.extend(wrap.to_le_bytes());
			held
		};
		bytes.extend(lane(1, [0.0, -1.0, 0.0, 0.0], 180, 1));
		bytes.extend(lane(1, [0.0, std::f32::consts::TAU, 0.0, 0.0], 360, 0));
		bytes.extend(lane(0, [1.0, 1.0, 1.0, 0.0], 30, 0));

		// Two timelines, each with a region of its own ahead of the record that names it, so a
		// record read at the wrong stride reaches the other one's bytes.
		for duration in [480i16, 120] {
			bytes.extend(*b"TMLB");
			bytes.extend((TMLB as u32).to_le_bytes());
			bytes.extend(1u32.to_le_bytes());
			bytes.extend(*b"TMDH");
			bytes.extend(16u32.to_le_bytes());
			bytes.extend([1i16, 0, duration, 3].map(i16::to_le_bytes).concat());
		}
		bytes.extend(8i32.to_le_bytes());
		bytes.extend(2i32.to_le_bytes());

		for (index, (sub_id, auto, looping)) in [(5i32, 1u8, 0u8), (9, 0, 1)].iter().enumerate() {
			let record = entries + index * 44;
			bytes.extend(sub_id.to_le_bytes());
			bytes.extend((name(8 + index) - record as i32).to_le_bytes());
			bytes.extend(((pairs + index * 8) as i32 - record as i32).to_le_bytes());
			bytes.extend(1i32.to_le_bytes());
			bytes.extend(0i32.to_le_bytes());
			bytes.extend(((timelines + index * TMLB) as i32 - record as i32).to_le_bytes());
			bytes.resize(record + 32, 0);
			bytes.extend([*auto, *looping]);
			bytes.resize(record + 44, 0);
		}
		for instance in [11i32, 12] {
			bytes.extend(2i32.to_le_bytes());
			bytes.extend(instance.to_le_bytes());
		}
		bytes
	}

	#[test]
	fn reads_every_timeline_a_scene_names() {
		let file = SharedGroupFile::read(Cursor::new(build(b"SGB1", 0))).unwrap();
		let [first, second] = file.scene().timelines() else {
			panic!("expected two timelines")
		};
		assert_eq!((first.sub_id(), first.kind().as_str()), (5, "first"));
		assert_eq!((first.auto_play(), first.looping()), (true, false));
		assert_eq!(first.animated(), &[(2, 11)]);
		assert_eq!((second.sub_id(), second.kind().as_str()), (9, "second"));
		assert_eq!((second.auto_play(), second.looping()), (false, true));
		assert_eq!(second.animated(), &[(2, 12)]);

		let duration = |held: &super::super::layer::SceneTimeline| {
			held.timeline().items().iter().find_map(|item| match item {
				crate::file::tmb::Item::Header(header) => Some(header.duration()),
				_ => None,
			})
		};
		assert_eq!((duration(first), duration(second)), (Some(480), Some(120)));
	}

	#[test]
	fn reads_a_scene_and_what_it_is_drawn_with() {
		let file = SharedGroupFile::read(Cursor::new(build(b"SGB1", 0))).unwrap();
		let scene = file.scene();
		assert_eq!(scene.bg_path(), "bg/dir");
		assert_eq!(scene.sky_visibility_path(), "a.svb");
		assert_eq!(scene.light_culling_path(), "a.lcb");
		assert_eq!(scene.layer_group_paths(), &["a.lgb"]);

		let [environment] = scene.environments() else {
			panic!("expected one environment")
		};
		assert_eq!(environment.asset_path(), "a.envb");
		assert_eq!(environment.sound_asset_path(), "a.essb");
		assert_eq!(
			(environment.index(), environment.env_location_instance_id()),
			(2, 9)
		);

		let [group] = scene.layer_groups() else {
			panic!("expected one layer group")
		};
		assert_eq!((group.id(), group.name().as_str()), (256, "group"));
		let [layer] = group.layers() else {
			panic!("expected one layer")
		};
		assert_eq!(layer.name(), "layer");
		assert!(matches!(
			layer.instances()[0].data(),
			InstanceData::BgPart(_)
		));
	}

	#[test]
	fn reads_the_motion_a_scene_repeats_on_its_own() {
		let file = SharedGroupFile::read(Cursor::new(build(b"SGB1", 0))).unwrap();
		let [animation] = file.scene().animations() else {
			panic!("expected one repeating motion")
		};
		assert_eq!(animation.instances(), &[11, 12]);
		assert_eq!(animation.translation().amount(), [0.0, -1.0, 0.0, 0.0]);
		assert_eq!(
			(animation.translation().period(), animation.translation().wrap()),
			(180, 1)
		);
		assert!(animation.rotation().active());
		assert_eq!(animation.rotation().period(), 360);
		assert!(!animation.scale().active());
	}

	#[test]
	fn reads_the_turn_a_scene_never_stops() {
		let file = SharedGroupFile::read(Cursor::new(build(b"SGB1", 0))).unwrap();
		let [spin] = file.scene().spins() else {
			panic!("expected one turn")
		};
		assert_eq!((spin.instance(), spin.axis()), (12, 1));
		assert_eq!(spin.period(), -450.0);
	}

	/// The older files pad ahead of the section and put two empty fields ahead of its body.
	#[test]
	fn a_padded_scene_still_finds_its_body() {
		let file = SharedGroupFile::read(Cursor::new(build(b"SGB1", 8))).unwrap();
		assert_eq!(file.scene().layer_groups()[0].name(), "group");
	}

	#[test]
	fn rejects_another_format() {
		assert!(SharedGroupFile::read(Cursor::new(build(b"LVB1", 0))).is_err());
	}
}
