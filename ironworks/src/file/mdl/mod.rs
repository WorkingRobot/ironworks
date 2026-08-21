//! Structs and utilities for parsing .mdl files.

mod container;
mod lods;
mod mesh;
mod model;
mod structs;

pub use {
	container::ModelContainer,
	lods::Lods,
	mesh::{Mesh, Submesh, VertexAttribute, VertexValues},
	model::{Lod, MeshKind, Model, Shape},
	structs::{VertexAttributeKind, VertexFormat},
};
