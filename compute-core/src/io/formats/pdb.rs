//! Reading and writing of the PDB structure format.
//!
//! Parsing is intentionally permissive: real-world `.pdb` files range from
//! strict fixed-column depositions (RCSB) to loosely formatted exports. We read
//! the columns we can rely on (atom name, residue name, element) by fixed width
//! and fall back to whitespace tokenization for the rest, locating the
//! coordinate triple by scanning for three consecutive decimal numbers.

mod fields;
mod read;
mod write;

#[cfg(test)]
mod tests;

use crate::domain::Structure;

pub use read::{parse_pdb, parse_pdb_document};
pub use write::to_pdb;

/// A parsed PDB file. NMR depositions carry many alternative conformers as
/// `MODEL`/`ENDMDL` blocks; each becomes one [`Structure`] in `models`.
pub struct PdbDocument {
    /// The deposition title (from `TITLE`, falling back to the `HEADER`
    /// classification, then a generic placeholder).
    pub title: String,
    /// The four-character PDB identifier from the `HEADER` record, if present.
    pub pdb_id: Option<String>,
    /// One entry per `MODEL` block, or a single entry for a single-model file.
    pub models: Vec<Structure>,
}
