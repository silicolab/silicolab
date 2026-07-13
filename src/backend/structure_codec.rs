//! Serialization codec for structures and edit snapshots.
//!
//! Geometry is the heavy part of a project, so rather than spreading it across
//! many normalized rows we serialize a structure into one compact JSON document
//! and zlib-compress it into a single BLOB. A 4-byte uncompressed length is kept
//! alongside the blob so the decompression buffer can be pre-sized. The same
//! codec serializes undo/redo snapshots so persistent history reuses one path.
//!
//! The structure geometry itself rides the shared [`StructurePayload`] mirror
//! (the same one used to ship a job to a worker); this layer adds compression and
//! the snapshot wrapper that also carries the editor's selection.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};

use crate::backend::history::EditSnapshot;
use crate::domain::Structure;
use crate::frontend::AtomSelection;
use compute_core::payload::{StructurePayload, payload_to_structure, structure_to_payload};

/// Payload format tag stored next to each blob. Bump when the on-disk shape of
/// [`StructurePayload`]/`SnapshotPayload` changes incompatibly.
pub const PAYLOAD_FORMAT: i64 = 1;

/// A compressed structure blob plus the length of its uncompressed JSON.
pub struct EncodedBlob {
    pub bytes: Vec<u8>,
    pub uncompressed_len: usize,
}

#[derive(Serialize, Deserialize)]
struct SnapshotPayload {
    structure: StructurePayload,
    source_path: Option<String>,
    save_path: String,
    selection_atoms: Vec<usize>,
    selection_primary: Option<usize>,
}

fn compress(json: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json).context("compress payload")?;
    encoder.finish().context("finish compression")
}

fn decompress(bytes: &[u8], uncompressed_len: usize) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(bytes);
    let mut out = Vec::with_capacity(uncompressed_len);
    decoder
        .read_to_end(&mut out)
        .context("decompress payload")?;
    Ok(out)
}

/// Serialize and compress a structure into a single blob.
pub fn encode_structure(structure: &Structure) -> Result<EncodedBlob> {
    let json =
        serde_json::to_vec(&structure_to_payload(structure)).context("serialize structure")?;
    let uncompressed_len = json.len();
    Ok(EncodedBlob {
        bytes: compress(&json)?,
        uncompressed_len,
    })
}

/// Decompress and deserialize a structure blob.
pub fn decode_structure(bytes: &[u8], uncompressed_len: usize) -> Result<Structure> {
    let json = decompress(bytes, uncompressed_len)?;
    let payload: StructurePayload =
        serde_json::from_slice(&json).context("deserialize structure")?;
    payload_to_structure(payload).context("rebuild structure from payload")
}

/// Serialize and compress an undo/redo snapshot.
pub fn encode_snapshot(snapshot: &EditSnapshot) -> Result<EncodedBlob> {
    let payload = SnapshotPayload {
        structure: structure_to_payload(&snapshot.structure),
        source_path: snapshot
            .source_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        save_path: snapshot.save_path.to_string_lossy().to_string(),
        selection_atoms: snapshot.selection.ordered_indices(),
        selection_primary: snapshot.selection.primary(),
    };
    let json = serde_json::to_vec(&payload).context("serialize snapshot")?;
    let uncompressed_len = json.len();
    Ok(EncodedBlob {
        bytes: compress(&json)?,
        uncompressed_len,
    })
}

/// Decompress and deserialize an undo/redo snapshot.
pub fn decode_snapshot(bytes: &[u8], uncompressed_len: usize) -> Result<EditSnapshot> {
    let json = decompress(bytes, uncompressed_len)?;
    let payload: SnapshotPayload = serde_json::from_slice(&json).context("deserialize snapshot")?;
    Ok(EditSnapshot {
        structure: payload_to_structure(payload.structure)?,
        source_path: payload.source_path.map(PathBuf::from),
        save_path: PathBuf::from(payload.save_path),
        selection: AtomSelection::from_parts(payload.selection_atoms, payload.selection_primary),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType, Structure, UnitCell};
    use nalgebra::Point3;

    #[test]
    fn structure_blob_roundtrips() {
        let structure = Structure::with_cell_and_bonds(
            "ethene",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: -0.1,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(1.34, 0.0, 0.0),
                    charge: 0.1,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Double)],
            UnitCell::from_parameters(10.0, 11.0, 12.0, 90.0, 91.0, 92.0),
        );
        let blob = encode_structure(&structure).unwrap();
        let decoded = decode_structure(&blob.bytes, blob.uncompressed_len).unwrap();
        assert_eq!(decoded.title, "ethene");
        assert_eq!(decoded.atoms.len(), 2);
        assert_eq!(decoded.atoms[1].element, "C");
        assert!((decoded.atoms[1].position.x - 1.34).abs() < 1e-6);
        assert!((decoded.atoms[0].charge + 0.1).abs() < 1e-6);
        assert_eq!(decoded.bonds[0].bond_type, BondType::Double);
        assert!(decoded.cell.is_some());
    }

    #[test]
    fn biopolymer_blob_roundtrips() {
        let pdb = "\
ATOM      1  N   ALA A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ALA A   1       1.400   0.000   0.000  1.00  0.00           C
END
";
        let structure = crate::io::formats::pdb::parse_pdb(pdb).unwrap();
        let blob = encode_structure(&structure).unwrap();
        let decoded = decode_structure(&blob.bytes, blob.uncompressed_len).unwrap();
        let bio = decoded.biopolymer.expect("biopolymer survives");
        assert_eq!(bio.atom_name(0), Some("N"));
        assert_eq!(bio.atom_name(1), Some("CA"));
        assert!(bio.residues[0].is_standard_amino_acid);
    }
}
