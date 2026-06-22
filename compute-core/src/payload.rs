//! Serializable mirror of [`Structure`] for crossing a process boundary.
//!
//! [`Structure`] embeds nalgebra vectors and is the app's universal in-memory
//! type, kept free of serde derives. To send a structure to a worker (a local
//! subprocess or a remote host) it is mirrored into [`StructurePayload`], a flat
//! all-scalar shape that serializes cleanly, and rebuilt on the other side. The
//! on-disk project codec reuses the same mirror, so geometry has one wire shape.

use nalgebra::{Point3, Vector3};
use serde::{Deserialize, Serialize};

use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord,
    SecondaryStructureKind, SecondaryStructureSpan, Structure, UnitCell,
};

/// Flat, all-scalar mirror of a [`Structure`]: title, per-atom element/coords/
/// charges, bonds, an optional unit cell, and optional biopolymer topology.
#[derive(Serialize, Deserialize)]
pub struct StructurePayload {
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

/// Mirror a [`UnitCell`] into its serializable payload (carrying the exact lattice
/// vectors, so a non-orthogonal cell's orientation survives the round trip).
fn cell_to_payload(cell: &UnitCell) -> CellPayload {
    CellPayload {
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
    }
}

/// Rebuild a [`UnitCell`] from its payload.
fn payload_to_cell(cell: CellPayload) -> UnitCell {
    UnitCell {
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
    }
}

/// Mirror a live [`Structure`] into its serializable payload.
pub fn structure_to_payload(structure: &Structure) -> StructurePayload {
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
    let cell = structure.cell.as_ref().map(cell_to_payload);
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

/// Rebuild a live [`Structure`] from its payload. Fails on an inconsistent
/// payload (a truncated or corrupt blob whose flat arrays disagree on the atom
/// count) rather than silently fabricating origin atoms or zero charges to fill
/// the gap — the arrays are written together by [`structure_to_payload`], so a
/// length mismatch means the data is bad.
pub fn payload_to_structure(payload: StructurePayload) -> anyhow::Result<Structure> {
    let count = payload.elements.len();
    anyhow::ensure!(
        payload.coords.len() == count * 3,
        "structure payload has {} coordinate values for {count} atoms (expected {})",
        payload.coords.len(),
        count * 3
    );
    anyhow::ensure!(
        payload.charges.len() == count,
        "structure payload has {} charges for {count} atoms",
        payload.charges.len()
    );
    let atoms = payload
        .elements
        .into_iter()
        .enumerate()
        .map(|(index, element)| {
            let base = index * 3;
            // In-bounds: the lengths were checked against `count` above.
            Atom {
                element,
                position: Point3::new(
                    payload.coords[base],
                    payload.coords[base + 1],
                    payload.coords[base + 2],
                ),
                charge: payload.charges[index],
            }
        })
        .collect();
    let bonds = payload
        .bonds
        .into_iter()
        .map(|bond| Bond::with_type(bond.a, bond.b, bond_type_from_tag(bond.t)))
        .collect();
    let cell = payload.cell.map(payload_to_cell);
    let biopolymer = payload.biopolymer.map(payload_to_biopolymer);
    Ok(Structure {
        title: payload.title,
        atoms,
        bonds,
        cell,
        biopolymer,
    })
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

/// `#[serde(with = ...)]` adapter that serializes a [`Structure`] field through
/// the payload mirror, keeping the live `Structure` in memory while the wire form
/// stays serde-clean.
pub mod structure_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{StructurePayload, payload_to_structure, structure_to_payload};
    use crate::domain::Structure;

    pub fn serialize<S: Serializer>(
        structure: &Structure,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        structure_to_payload(structure).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Structure, D::Error> {
        let payload = StructurePayload::deserialize(deserializer)?;
        payload_to_structure(payload).map_err(serde::de::Error::custom)
    }
}

/// The `Box<Structure>` counterpart of [`structure_serde`], for a boxed structure
/// field — a docking receptor/ligand input, which is boxed to keep its enum small.
pub mod structure_serde_boxed {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{StructurePayload, payload_to_structure, structure_to_payload};
    use crate::domain::Structure;

    // serde's `with` adapter receives the field by reference, and the field's type
    // is `Box<Structure>`; the reference derefs to `&Structure` at the call below.
    #[allow(clippy::borrowed_box)]
    pub fn serialize<S: Serializer>(
        structure: &Box<Structure>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        structure_to_payload(structure).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<Structure>, D::Error> {
        let payload = StructurePayload::deserialize(deserializer)?;
        payload_to_structure(payload)
            .map(Box::new)
            .map_err(serde::de::Error::custom)
    }
}

/// `#[serde(with = ...)]` adapter for an `Option<UnitCell>` field, serializing
/// through the same cell mirror [`StructurePayload`] uses so a nalgebra-backed
/// cell crosses the wire without a serde derive on the domain type.
pub mod cell_serde_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{CellPayload, cell_to_payload, payload_to_cell};
    use crate::domain::UnitCell;

    pub fn serialize<S: Serializer>(
        cell: &Option<UnitCell>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        cell.as_ref().map(cell_to_payload).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<UnitCell>, D::Error> {
        let payload = Option::<CellPayload>::deserialize(deserializer)?;
        Ok(payload.map(payload_to_cell))
    }
}

/// The `Option<Structure>` counterpart of [`structure_serde`], for fields like an
/// optimization's optional optimized geometry.
pub mod structure_serde_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{StructurePayload, payload_to_structure, structure_to_payload};
    use crate::domain::Structure;

    pub fn serialize<S: Serializer>(
        structure: &Option<Structure>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        structure
            .as_ref()
            .map(structure_to_payload)
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Structure>, D::Error> {
        let payload = Option::<StructurePayload>::deserialize(deserializer)?;
        payload
            .map(payload_to_structure)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::{payload_to_structure, structure_to_payload};
    use crate::domain::{Atom, Bond, BondType, Structure, UnitCell};

    #[test]
    fn payload_round_trips_cell_and_bonds() {
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

        let json = serde_json::to_vec(&structure_to_payload(&structure)).unwrap();
        let payload = serde_json::from_slice(&json).unwrap();
        let decoded = payload_to_structure(payload).unwrap();

        assert_eq!(decoded.title, "ethene");
        assert_eq!(decoded.atoms.len(), 2);
        assert_eq!(decoded.atoms[1].element, "C");
        assert!((decoded.atoms[1].position.x - 1.34).abs() < 1e-6);
        assert!((decoded.atoms[0].charge + 0.1).abs() < 1e-6);
        assert_eq!(decoded.bonds[0].bond_type, BondType::Double);
        assert!(decoded.cell.is_some());
    }

    #[test]
    fn payload_round_trips_biopolymer() {
        let pdb = "\
ATOM      1  N   ALA A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ALA A   1       1.400   0.000   0.000  1.00  0.00           C
END
";
        let structure = crate::io::formats::pdb::parse_pdb(pdb).unwrap();
        let json = serde_json::to_vec(&structure_to_payload(&structure)).unwrap();
        let payload = serde_json::from_slice(&json).unwrap();
        let decoded = payload_to_structure(payload).unwrap();

        let bio = decoded.biopolymer.expect("biopolymer survives");
        assert_eq!(bio.atom_name(0), Some("N"));
        assert_eq!(bio.atom_name(1), Some("CA"));
        assert!(bio.residues[0].is_standard_amino_acid);
    }
}
