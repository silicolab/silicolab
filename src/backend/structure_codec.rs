//! Serialization codec for structures and edit snapshots.
//!
//! Geometry is the heavy part of a project, so rather than spreading it across
//! many normalized rows we serialize a structure into one compact JSON document
//! and zlib-compress it into a single BLOB. A 4-byte uncompressed length is kept
//! alongside the blob so the decompression buffer can be pre-sized. The same
//! codec serializes undo/redo snapshots so persistent history reuses one path.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use nalgebra::{Point3, Vector3};
use serde::{Deserialize, Serialize};

use crate::backend::history::EditSnapshot;
use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord,
    SecondaryStructureKind, SecondaryStructureSpan, Structure, UnitCell,
};
use crate::frontend::AtomSelection;

/// Payload format tag stored next to each blob. Bump when the on-disk shape of
/// [`StructurePayload`]/[`SnapshotPayload`] changes incompatibly.
pub const PAYLOAD_FORMAT: i64 = 1;

/// A compressed structure blob plus the length of its uncompressed JSON.
pub struct EncodedBlob {
    pub bytes: Vec<u8>,
    pub uncompressed_len: usize,
}

#[derive(Serialize, Deserialize)]
struct StructurePayload {
    title: String,
    /// Per-atom element symbols (length N).
    elements: Vec<String>,
    /// Flattened cartesian coordinates (length 3N: x0, y0, z0, x1, ...).
    coords: Vec<f32>,
    /// Per-atom partial charges (length N).
    charges: Vec<f32>,
    bonds: Vec<BondPayload>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    cell: Option<CellPayload>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    biopolymer: Option<BiopolymerPayload>,
}

#[derive(Serialize, Deserialize)]
struct BondPayload {
    a: usize,
    b: usize,
    t: u8,
}

#[derive(Serialize, Deserialize)]
struct CellPayload {
    a: f32,
    b: f32,
    c: f32,
    alpha: f32,
    beta: f32,
    gamma: f32,
    vectors: [[f32; 3]; 3],
}

#[derive(Serialize, Deserialize)]
struct BiopolymerPayload {
    residues: Vec<ResiduePayload>,
    chains: Vec<ChainPayload>,
    secondary: Vec<SecondaryPayload>,
    residue_for_atom: Vec<Option<usize>>,
    atom_name_for_atom: Vec<Option<String>>,
}

#[derive(Serialize, Deserialize)]
struct ResiduePayload {
    chain_id: char,
    sequence_number: i32,
    insertion_code: char,
    name: String,
    atoms: Vec<usize>,
    alpha_carbon: Option<usize>,
    is_standard: bool,
}

#[derive(Serialize, Deserialize)]
struct ChainPayload {
    id: char,
    residues: Vec<usize>,
}

#[derive(Serialize, Deserialize)]
struct SecondaryPayload {
    kind: u8,
    start: ResidueKeyPayload,
    end: ResidueKeyPayload,
}

#[derive(Serialize, Deserialize)]
struct ResidueKeyPayload {
    chain_id: char,
    sequence_number: i32,
    insertion_code: char,
}

#[derive(Serialize, Deserialize)]
struct SnapshotPayload {
    structure: StructurePayload,
    source_path: Option<String>,
    save_path: String,
    selection_atoms: Vec<usize>,
    selection_primary: Option<usize>,
}

fn bond_type_tag(bond_type: BondType) -> u8 {
    match bond_type {
        BondType::Single => 0,
        BondType::Double => 1,
        BondType::Triple => 2,
        BondType::Aromatic => 3,
    }
}

fn bond_type_from_tag(tag: u8) -> BondType {
    match tag {
        1 => BondType::Double,
        2 => BondType::Triple,
        3 => BondType::Aromatic,
        _ => BondType::Single,
    }
}

fn secondary_kind_tag(kind: SecondaryStructureKind) -> u8 {
    match kind {
        SecondaryStructureKind::Helix => 0,
        SecondaryStructureKind::Sheet => 1,
    }
}

fn secondary_kind_from_tag(tag: u8) -> SecondaryStructureKind {
    match tag {
        1 => SecondaryStructureKind::Sheet,
        _ => SecondaryStructureKind::Helix,
    }
}

fn structure_to_payload(structure: &Structure) -> StructurePayload {
    let mut elements = Vec::with_capacity(structure.atoms.len());
    let mut coords = Vec::with_capacity(structure.atoms.len() * 3);
    let mut charges = Vec::with_capacity(structure.atoms.len());
    for atom in &structure.atoms {
        elements.push(atom.element.clone());
        coords.push(atom.position.x);
        coords.push(atom.position.y);
        coords.push(atom.position.z);
        charges.push(atom.charge);
    }
    let bonds = structure
        .bonds
        .iter()
        .map(|bond| BondPayload {
            a: bond.a,
            b: bond.b,
            t: bond_type_tag(bond.bond_type),
        })
        .collect();
    let cell = structure.cell.as_ref().map(|cell| CellPayload {
        a: cell.a,
        b: cell.b,
        c: cell.c,
        alpha: cell.alpha,
        beta: cell.beta,
        gamma: cell.gamma,
        vectors: [
            [cell.vectors[0].x, cell.vectors[0].y, cell.vectors[0].z],
            [cell.vectors[1].x, cell.vectors[1].y, cell.vectors[1].z],
            [cell.vectors[2].x, cell.vectors[2].y, cell.vectors[2].z],
        ],
    });
    let biopolymer = structure.biopolymer.as_ref().map(biopolymer_to_payload);
    StructurePayload {
        title: structure.title.clone(),
        elements,
        coords,
        charges,
        bonds,
        cell,
        biopolymer,
    }
}

fn biopolymer_to_payload(biopolymer: &Biopolymer) -> BiopolymerPayload {
    BiopolymerPayload {
        residues: biopolymer
            .residues
            .iter()
            .map(|residue| ResiduePayload {
                chain_id: residue.id.chain_id,
                sequence_number: residue.id.sequence_number,
                insertion_code: residue.id.insertion_code,
                name: residue.residue_name.clone(),
                atoms: residue.atom_indices.clone(),
                alpha_carbon: residue.alpha_carbon,
                is_standard: residue.is_standard_amino_acid,
            })
            .collect(),
        chains: biopolymer
            .chains
            .iter()
            .map(|chain| ChainPayload {
                id: chain.id,
                residues: chain.residue_indices.clone(),
            })
            .collect(),
        secondary: biopolymer
            .secondary_structures
            .iter()
            .map(|span| SecondaryPayload {
                kind: secondary_kind_tag(span.kind),
                start: ResidueKeyPayload {
                    chain_id: span.start.chain_id,
                    sequence_number: span.start.sequence_number,
                    insertion_code: span.start.insertion_code,
                },
                end: ResidueKeyPayload {
                    chain_id: span.end.chain_id,
                    sequence_number: span.end.sequence_number,
                    insertion_code: span.end.insertion_code,
                },
            })
            .collect(),
        residue_for_atom: biopolymer.residue_for_atom.clone(),
        atom_name_for_atom: biopolymer.atom_name_for_atom.clone(),
    }
}

fn payload_to_structure(payload: StructurePayload) -> Structure {
    let atoms = payload
        .elements
        .into_iter()
        .enumerate()
        .map(|(index, element)| {
            let base = index * 3;
            Atom {
                element,
                position: Point3::new(
                    payload.coords.get(base).copied().unwrap_or(0.0),
                    payload.coords.get(base + 1).copied().unwrap_or(0.0),
                    payload.coords.get(base + 2).copied().unwrap_or(0.0),
                ),
                charge: payload.charges.get(index).copied().unwrap_or(0.0),
            }
        })
        .collect();
    let bonds = payload
        .bonds
        .into_iter()
        .map(|bond| Bond::with_type(bond.a, bond.b, bond_type_from_tag(bond.t)))
        .collect();
    let cell = payload.cell.map(|cell| UnitCell {
        a: cell.a,
        b: cell.b,
        c: cell.c,
        alpha: cell.alpha,
        beta: cell.beta,
        gamma: cell.gamma,
        vectors: [
            Vector3::new(cell.vectors[0][0], cell.vectors[0][1], cell.vectors[0][2]),
            Vector3::new(cell.vectors[1][0], cell.vectors[1][1], cell.vectors[1][2]),
            Vector3::new(cell.vectors[2][0], cell.vectors[2][1], cell.vectors[2][2]),
        ],
    });
    let biopolymer = payload.biopolymer.map(payload_to_biopolymer);
    Structure {
        title: payload.title,
        atoms,
        bonds,
        cell,
        biopolymer,
    }
}

fn payload_to_biopolymer(payload: BiopolymerPayload) -> Biopolymer {
    Biopolymer {
        residues: payload
            .residues
            .into_iter()
            .map(|residue| ResidueRecord {
                id: ResidueId::new(
                    residue.chain_id,
                    residue.sequence_number,
                    residue.insertion_code,
                ),
                residue_name: residue.name,
                atom_indices: residue.atoms,
                alpha_carbon: residue.alpha_carbon,
                is_standard_amino_acid: residue.is_standard,
            })
            .collect(),
        chains: payload
            .chains
            .into_iter()
            .map(|chain| ChainRecord {
                id: chain.id,
                residue_indices: chain.residues,
            })
            .collect(),
        secondary_structures: payload
            .secondary
            .into_iter()
            .map(|span| SecondaryStructureSpan {
                kind: secondary_kind_from_tag(span.kind),
                start: ResidueId::new(
                    span.start.chain_id,
                    span.start.sequence_number,
                    span.start.insertion_code,
                ),
                end: ResidueId::new(
                    span.end.chain_id,
                    span.end.sequence_number,
                    span.end.insertion_code,
                ),
            })
            .collect(),
        residue_for_atom: payload.residue_for_atom,
        atom_name_for_atom: payload.atom_name_for_atom,
    }
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
    Ok(payload_to_structure(payload))
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
        structure: payload_to_structure(payload.structure),
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
