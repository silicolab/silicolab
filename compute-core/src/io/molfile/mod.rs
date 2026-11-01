//! MDL Molfile V3000 reading and writing for 2D [`Sketch`]es.
//!
//! This covers the connection table a depiction needs: coordinates, bond orders,
//! charges, attachment points (`*`), and SRU Sgroups with their brackets. Reading
//! returns warnings for what a sketch cannot hold and an error for what would
//! change the chemistry if dropped. Molfile is a sketch format here, not a
//! [`crate::io::structure_format::StructureFormat`]: a 3D `Structure` has no
//! attachment points or repeat units.

mod reader;
mod syntax;
#[cfg(test)]
mod tests;
mod writer;

use crate::domain::sketch::Sketch;

pub use reader::read_molfile;
pub use writer::write_molfile;

/// A sketch read from a molfile.
#[derive(Debug, Clone, PartialEq)]
pub struct MolfileSketch {
    /// The header's molecule name, possibly empty.
    pub title: String,
    pub sketch: Sketch,
    /// Content of the file that the sketch does not preserve.
    pub warnings: Vec<String>,
}
