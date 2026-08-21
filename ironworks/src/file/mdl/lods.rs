use std::ops::Range;

/// Where each detail level's geometry sits in a model file, as its head states it.
pub struct Lods {
	vertex: [u32; 3],
	index: [u32; 3],
	size: [u32; 3],
}

impl Lods {
	/// How many bytes at the head of a model file state this.
	pub const SIZE: u64 = 0x44;

	pub fn read(head: &[u8]) -> Option<Self> {
		let word = |at: usize| {
			head.get(at..at + 4)
				.map(|held| u32::from_le_bytes(held.try_into().unwrap()))
		};
		let three = |at: usize| Some([word(at)?, word(at + 4)?, word(at + 8)?]);
		Some(Self {
			vertex: three(0x10)?,
			index: three(0x1c)?,
			size: three(0x34)?,
		})
	}

	/// The level nearest `lod` the file holds geometry for, coarser first, the way a scene falls
	/// back when the level it would draw is not there.
	pub fn level(&self, lod: u8) -> u8 {
		let lod = usize::from(lod).min(2);
		u8::try_from(
			(lod..3)
				.chain((0..lod).rev())
				.find(|level| self.size[*level] > 0)
				.unwrap_or(lod),
		)
		.unwrap_or(0)
	}

	/// Where the geometry begins. Everything before it is read whichever level is drawn.
	pub fn head(&self) -> Option<u32> {
		self.vertex
			.iter()
			.chain(&self.index)
			.copied()
			.filter(|at| u64::from(*at) >= Self::SIZE)
			.min()
	}

	/// The bytes one level spans, its vertices through its indices.
	pub fn span(&self, level: u8) -> Option<Range<u32>> {
		let level = usize::from(level);
		let (start, index) = (*self.vertex.get(level)?, *self.index.get(level)?);
		let end = index.checked_add(*self.size.get(level)?)?;
		(start > 0 && index >= start && end > start).then_some(start..end)
	}

	/// Rewrite the head so a reader finds `level`'s geometry directly after it.
	pub fn keep(&self, head: &mut [u8], level: u8) {
		let start = self.head().unwrap_or_default();
		let mut put = |at: usize, value: u32| head[at..at + 4].copy_from_slice(&value.to_le_bytes());
		for lod in 0..3 {
			put(0x10 + lod * 4, start);
			put(
				0x1c + lod * 4,
				match usize::from(level) == lod {
					true => start + (self.index[lod] - self.vertex[lod]),
					false => start,
				},
			);
		}
	}
}
