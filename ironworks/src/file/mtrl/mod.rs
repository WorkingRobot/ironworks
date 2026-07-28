//! Structs and utilities for parsing .mtrl files.

mod material;
mod structs;

pub use material::{
	AttributeSet, ColorRow, ColorTable, ColorTableKind, Constant, Material, Sampler, ShaderKey,
	Texture,
};
