/// Where a shader package's sections sit, as its head states them. The bytecode between the tables
/// and the string block is what a draw asks for a blob at a time.
pub struct Spans {
	pub blobs: u32,
	pub strings: u32,
	pub size: u32,
}

impl Spans {
	pub fn read(head: &[u8]) -> Option<Self> {
		if !head.starts_with(b"ShPk") {
			return None;
		}
		let word = |at: usize| {
			head.get(at..at + 4)
				.map(|held| u32::from_le_bytes(held.try_into().unwrap()))
		};
		let held = Self {
			size: word(12)?,
			blobs: word(16)?,
			strings: word(20)?,
		};
		(held.blobs <= held.strings && held.strings <= held.size).then_some(held)
	}
}
