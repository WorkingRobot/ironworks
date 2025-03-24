//! Structs and utilities for parsing .exd files.

use std::io::{Cursor, Read, Seek};

use binrw::{BinRead, BinResult, Endian, binread};

use crate::{
	FileStream,
	error::{Error, ErrorValue, Result},
};

use super::file::File;

/// An Excel data page. One or more pages form the full dataset for an Excel
/// sheet. Metadata for sheets is contained in an associated .exh Excel header file.
#[binread]
#[derive(Debug)]
#[br(big, magic = b"EXDF")]
pub struct ExcelData {
	_version: u16,
	// unknown1: u16,
	#[br(pad_before = 2, temp)]
	index_size: u32,

	// unknown2: [u16; 10],
	/// Vector of rows contained within this page.
	#[br(
		pad_before = 20,
		count = index_size / RowDefinition::SIZE,
	)]
	pub rows: Vec<RowDefinition>,

	#[br(parse_with = current_position)]
	pub data_offset: u64,

	#[br(parse_with = until_eof)]
	pub data: Vec<u8>,
}

impl ExcelData {
	/// Fetch the slice of data associated with the specified row. If this data
	/// page is for a sheet with subrows, this will include all child rows of the
	/// specified row. Otherwise, it will contain the row and any trailing string data.
	pub fn row_data(&self, row_id: u32) -> Result<&[u8]> {
		let (row_header, offset) = self.row_meta(row_id)?;

		// Get a slice of the row's data
		let length: usize = row_header.data_size.try_into().unwrap();
		Ok(&self.data[offset..offset + length])
	}

	/// Fetch the slice of data associated with the specified subrow.
	pub fn subrow_data(&self, row_id: u32, subrow_id: u16) -> Result<&[u8]> {
		let (row_header, offset) = self.row_meta(row_id)?;

		// Subrows invariably do not support unstructured data (i.e. strings), and
		// are laid out in subrow order. As such, it's safe to assume that evenly
		// splitting the row's data by it's subrow count will give us what we want.
		let subrow_size =
			usize::try_from(row_header.data_size / u32::from(row_header.row_count)).unwrap();

		// Subrow IDs do not always match their index within the row - loop over
		// subrows and find the subrow with a matching ID.
		let mut cursor = Cursor::new(&self.data);
		let maybe_subrow_offset = (0..row_header.row_count)
			// TODO: map->try_find whenever _that_ stabilises
			.find_map(|index| -> Option<Result<_>> {
				let subrow_offset = offset + subrow_size * usize::try_from(index).unwrap();
				cursor.set_position(subrow_offset.try_into().unwrap());
				match SubrowHeader::read(&mut cursor) {
					Err(e) => Some(Err(e.into())),
					Ok(v) => match v.id == subrow_id {
						true => Some(Ok(subrow_offset)),
						false => None,
					},
				}
			});

		let subrow_offset = match maybe_subrow_offset {
			// A subrow header read failed
			Some(Err(error)) => return Err(error),
			// No subrows matched at all
			None => {
				return Err(Error::NotFound(ErrorValue::Row {
					row: row_id,
					subrow: subrow_id,
					sheet: None,
				}));
			}
			// A match was found
			Some(Ok(subrow_offset)) => subrow_offset,
		};

		// Get the slice of subrow data.
		Ok(&self.data[subrow_offset + SubrowHeader::SIZE..subrow_offset + subrow_size])
	}

	// TODO: This is a hacky implementation for use in excel's sheet iterator. Remove once iterator is rewritten to be less insane.
	pub(crate) fn subrow_max(&self, row_id: u32) -> Result<u16> {
		let (row_header, offset) = self.row_meta(row_id)?;

		let subrow_size =
			usize::try_from(row_header.data_size / u32::from(row_header.row_count)).unwrap();

		let mut cursor = Cursor::new(&self.data);
		(0..row_header.row_count)
			.map(move |index| -> Result<_> {
				let subrow_offset = offset + subrow_size * usize::try_from(index).unwrap();
				cursor.set_position(subrow_offset.try_into().unwrap());
				Ok(SubrowHeader::read(&mut cursor)?)
			})
			.fold(Ok(0), |a, b| {
				a.and_then(|a| b.map(|b| std::cmp::max(a, b.id)))
			})
	}

	fn row_meta(&self, row_id: u32) -> Result<(RowHeader, usize)> {
		let row_definition = self.row_definition(row_id)?;

		// Get a cursor to the start of the row.
		let mut cursor = Cursor::new(&self.data);
		cursor.set_position(u64::from(row_definition.offset) - self.data_offset);

		// Read in the header.
		let row_header = RowHeader::read(&mut cursor)?;

		Ok((row_header, cursor.position().try_into().unwrap()))
	}

	fn row_definition(&self, row_id: u32) -> Result<&RowDefinition> {
		// In all likelihood the row can be found simply by indexing
		// the vector based on the ID's offset from the first row.
		let first_row_id = self.rows.get(0).map_or(0, |row| row.id);
		if let Some(row_index) = row_id.checked_sub(first_row_id).map(|i| i as usize) {
			if row_index < self.rows.len() && self.rows[row_index].id == row_id {
				return Ok(&self.rows[row_index]);
			}
		}

		// If not, scan to find the row.
		self.rows.iter().find(|row| row.id == row_id).ok_or({
			Error::NotFound(ErrorValue::Row {
				row: row_id,
				subrow: 0,
				sheet: None,
			})
		})
	}
}

impl File for ExcelData {
	fn read(mut stream: impl FileStream) -> Result<Self> {
		Ok(<Self as BinRead>::read(&mut stream)?)
	}
}

/// Metadata of a row contained in a page.
#[binread]
#[derive(Debug)]
#[br(big)]
pub struct RowDefinition {
	/// Primary key ID of this row.
	pub id: u32,
	pub offset: u32,
}

impl RowDefinition {
	pub const SIZE: u32 = 8;
}

fn current_position<R: Read + Seek>(reader: &mut R, _: Endian, _: ()) -> BinResult<u64> {
	Ok(reader.stream_position()?)
}

fn until_eof<R: Read + Seek>(reader: &mut R, _: Endian, _: ()) -> BinResult<Vec<u8>> {
	let mut v = Vec::new();
	reader.read_to_end(&mut v)?;
	Ok(v)
}

#[binread]
#[derive(Debug)]
#[br(big)]
pub struct RowHeader {
	pub data_size: u32,
	pub row_count: u16,
}

impl RowHeader {
	pub const SIZE: usize = 6;
}

#[binread]
#[derive(Debug)]
#[br(big)]
pub struct SubrowHeader {
	pub id: u16,
}

impl SubrowHeader {
	pub const SIZE: usize = 2;
}
