use anyhow::{Result, anyhow};
use nalgebra::Vector3;

use crate::domain::glycan::{self, ProteinAnchor};
use crate::domain::{Biopolymer, BondType, ResidueId, Structure};
use crate::engines::forcefield;
use crate::workflows::assembly::condense::{self, AcceptorSpec, DonorSpec};

use super::builder::glycan_to_structure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlycosylationKind {
    NLinked,
    OLinked,
}

pub fn glycosylate_protein(
    protein: &Structure,
    glycan_notation: &str,
    anchor: ResidueId,
    kind: GlycosylationKind,
) -> Result<Structure> {
    let protein_bio = protein
        .biopolymer
        .as_ref()
        .filter(|bio| bio.is_compatible_with_atom_count(protein.atoms.len()))
        .ok_or_else(|| anyhow!("protein has no biopolymer overlay"))?;

    let anchor_residue_index = protein_bio
        .residues
        .iter()
        .position(|residue| residue.id == anchor)
        .ok_or_else(|| anyhow!("anchor residue not found at {anchor:?}"))?;
    let anchor_residue_name = protein_bio.residues[anchor_residue_index]
        .residue_name
        .trim()
        .to_string();

    let anchor_atom_name = anchor_atom_name(&anchor_residue_name, kind)
        .ok_or_else(|| anyhow!("{anchor_residue_name} is not a valid {kind:?} anchor residue"))?;
    let anchor_atom = atom_in_residue(protein_bio, anchor_residue_index, anchor_atom_name)
        .ok_or_else(|| anyhow!("anchor residue is missing atom {anchor_atom_name}"))?;
    let anchor_hydrogen = anchor_hydrogen_atom(protein_bio, anchor_residue_index, kind);
    let anchor_outward =
        anchor_outward_direction(protein, protein_bio, anchor_residue_index, anchor_atom);

    let glycan = glycan_to_structure(glycan_notation, None)?;
    let glycan_bio = glycan
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("glycan has no biopolymer overlay"))?;

    let reducing_c1 = reducing_end_anomeric_carbon(&glycan, glycan_bio)
        .ok_or_else(|| anyhow!("glycan has no reducing-end anomeric carbon"))?;
    let leaving_oxygen = glycan_atom_by_name(&glycan, glycan_bio, reducing_c1, "O1");
    let leaving_hydrogen = glycan_atom_by_name(&glycan, glycan_bio, reducing_c1, "HO1");
    let donor_outward = donor_outward_direction(&glycan, reducing_c1, leaving_oxygen);

    let anchor_element = match kind {
        GlycosylationKind::NLinked => "N",
        GlycosylationKind::OLinked => "O",
    };
    let bond_length =
        forcefield::equilibrium_bond_length("C", anchor_element, BondType::Single).unwrap_or(1.45);

    let acceptor = AcceptorSpec {
        anchor_atom,
        remove: anchor_hydrogen.into_iter().collect(),
        outward: anchor_outward,
    };
    let donor = DonorSpec {
        donor_atom: reducing_c1,
        remove: [leaving_oxygen, leaving_hydrogen]
            .into_iter()
            .flatten()
            .collect(),
        outward: donor_outward,
    };

    condense::attach_fragment(
        protein,
        acceptor,
        &glycan,
        donor,
        bond_length,
        BondType::Single,
        "glycan",
    )
}

fn anchor_atom_name(residue_name: &str, kind: GlycosylationKind) -> Option<&'static str> {
    match (kind, residue_name) {
        (GlycosylationKind::NLinked, "ASN") => Some(ProteinAnchor::AsnNd2.atom_name()),
        (GlycosylationKind::OLinked, "SER") => Some(ProteinAnchor::SerOg.atom_name()),
        (GlycosylationKind::OLinked, "THR") => Some(ProteinAnchor::ThrOg1.atom_name()),
        _ => None,
    }
}

fn atom_in_residue(
    biopolymer: &Biopolymer,
    residue_index: usize,
    atom_name: &str,
) -> Option<usize> {
    let residue = biopolymer.residues.get(residue_index)?;
    residue
        .atom_indices
        .iter()
        .copied()
        .find(|&index| biopolymer.atom_name(index) == Some(atom_name))
}

fn anchor_hydrogen_atom(
    biopolymer: &Biopolymer,
    residue_index: usize,
    kind: GlycosylationKind,
) -> Option<usize> {
    let candidates: &[&str] = match kind {
        GlycosylationKind::NLinked => &["HD21", "HD22", "1HD2", "2HD2"],
        GlycosylationKind::OLinked => &["HG", "HG1", "HO", "HOG", "HOG1"],
    };
    candidates
        .iter()
        .find_map(|name| atom_in_residue(biopolymer, residue_index, name))
}

fn anchor_outward_direction(
    structure: &Structure,
    biopolymer: &Biopolymer,
    residue_index: usize,
    anchor_atom: usize,
) -> Vector3<f32> {
    let neighbor = ["CG", "CB", "CA"]
        .iter()
        .find_map(|name| atom_in_residue(biopolymer, residue_index, name));
    match neighbor {
        Some(carbon) => (structure.atoms[anchor_atom].position - structure.atoms[carbon].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    }
}

fn reducing_end_anomeric_carbon(structure: &Structure, biopolymer: &Biopolymer) -> Option<usize> {
    let root_index = biopolymer.residues.iter().position(|residue| {
        crate::domain::biopolymer::is_carbohydrate_residue(&residue.residue_name)
    })?;
    let root = &biopolymer.residues[root_index];
    root.atom_indices.iter().copied().find(|&index| {
        index < structure.atoms.len()
            && biopolymer
                .atom_name(index)
                .map(glycan::is_anomeric_carbon)
                .unwrap_or(false)
    })
}

fn glycan_atom_by_name(
    structure: &Structure,
    biopolymer: &Biopolymer,
    anomeric_carbon: usize,
    name: &str,
) -> Option<usize> {
    let residue_index = (*biopolymer.residue_for_atom.get(anomeric_carbon)?)?;
    let residue = biopolymer.residues.get(residue_index)?;
    residue
        .atom_indices
        .iter()
        .copied()
        .find(|&index| index < structure.atoms.len() && biopolymer.atom_name(index) == Some(name))
}

fn donor_outward_direction(
    structure: &Structure,
    anomeric_carbon: usize,
    leaving_oxygen: Option<usize>,
) -> Vector3<f32> {
    match leaving_oxygen {
        Some(oxygen) => (structure.atoms[oxygen].position
            - structure.atoms[anomeric_carbon].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::glycan::{Aglycon, infer_attachment};
    use crate::domain::{Atom, AtomCategory, Bond, ChainRecord, ResidueRecord};
    use nalgebra::Point3;

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn asn_structure() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.45, 0.0, 0.0),
            atom("C", 2.9, 0.0, 0.0),
            atom("O", 3.6, 1.0, 0.0),
            atom("C", 3.6, -1.2, 0.0),
            atom("O", 3.0, -2.3, 0.0),
            atom("N", 4.9, -1.2, 0.0),
            atom("H", 5.4, -2.0, 0.0),
        ];
        let names = ["N", "CA", "CB", "O", "CG", "OD1", "ND2", "HD21"];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(1, 3, BondType::Single),
            Bond::with_type(2, 4, BondType::Single),
            Bond::with_type(4, 5, BondType::Single),
            Bond::with_type(4, 6, BondType::Single),
            Bond::with_type(6, 7, BondType::Single),
        ];
        let residue = ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: "ASN".to_string(),
            atom_indices: (0..atoms.len()).collect(),
            alpha_carbon: Some(1),
            backbone_nitrogen: Some(0),
            backbone_carbon: Some(2),
            backbone_oxygen: Some(3),
            is_standard_amino_acid: true,
        };
        let biopolymer = Biopolymer {
            residues: vec![residue],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom: vec![Some(0); atoms.len()],
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("asn".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    fn ser_structure() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.45, 0.0, 0.0),
            atom("C", 2.9, 0.0, 0.0),
            atom("O", 3.6, 1.0, 0.0),
            atom("O", 3.6, -1.2, 0.0),
            atom("H", 4.5, -1.2, 0.0),
        ];
        let names = ["N", "CA", "CB", "O", "OG", "HG"];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(1, 3, BondType::Single),
            Bond::with_type(2, 4, BondType::Single),
            Bond::with_type(4, 5, BondType::Single),
        ];
        let residue = ResidueRecord {
            id: ResidueId::new('A', 1, ' '),
            residue_name: "SER".to_string(),
            atom_indices: (0..atoms.len()).collect(),
            alpha_carbon: Some(1),
            backbone_nitrogen: Some(0),
            backbone_carbon: Some(2),
            backbone_oxygen: Some(3),
            is_standard_amino_acid: true,
        };
        let biopolymer = Biopolymer {
            residues: vec![residue],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom: vec![Some(0); atoms.len()],
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("ser".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    fn junction_present(structure: &Structure) -> bool {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure.bonds.iter().any(|bond| {
            let na = bio.atom_name(bond.a);
            let nb = bio.atom_name(bond.b);
            (na == Some("ND2") && nb == Some("C1")) || (na == Some("C1") && nb == Some("ND2"))
        })
    }

    fn o_junction_present(structure: &Structure) -> bool {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure.bonds.iter().any(|bond| {
            let na = bio.atom_name(bond.a);
            let nb = bio.atom_name(bond.b);
            (na == Some("OG") && nb == Some("C1")) || (na == Some("C1") && nb == Some("OG"))
        })
    }

    #[test]
    fn glycosylates_asn_with_glcnac() {
        let protein = asn_structure();
        let glycan = glycan_to_structure("GlcNAc", None).expect("glycan");
        let glycan_atom_count = glycan.atoms.len();

        let result = glycosylate_protein(
            &protein,
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            GlycosylationKind::NLinked,
        )
        .expect("glycosylation");

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + glycan_atom_count - 2
        );
        assert!(junction_present(&result), "ND2-C1 junction bond present");

        let carbohydrate = (0..result.atoms.len())
            .filter(|&i| result.atom_category(i) == AtomCategory::Carbohydrate)
            .count();
        assert!(carbohydrate > 0, "glycan atoms classify Carbohydrate");

        let attachment = infer_attachment(&result).expect("attachment inferred");
        match attachment {
            Aglycon::NLinked { asn, anchor_atom } => {
                assert_eq!(asn, ResidueId::new('A', 1, ' '));
                assert_eq!(anchor_atom, "ND2");
            }
            other => panic!("expected N-linked, got {other:?}"),
        }
    }

    #[test]
    fn glycosylates_ser_with_glcnac_o_linked() {
        let protein = ser_structure();
        let glycan = glycan_to_structure("GlcNAc", None).expect("glycan");
        let glycan_atom_count = glycan.atoms.len();

        let result = glycosylate_protein(
            &protein,
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            GlycosylationKind::OLinked,
        )
        .expect("glycosylation");

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + glycan_atom_count - 2
        );
        assert!(o_junction_present(&result), "OG-C1 junction bond present");

        let attachment = infer_attachment(&result).expect("attachment inferred");
        match attachment {
            Aglycon::OLinked {
                ser_thr,
                anchor_atom,
            } => {
                assert_eq!(ser_thr, ResidueId::new('A', 1, ' '));
                assert_eq!(anchor_atom, "OG");
            }
            other => panic!("expected O-linked, got {other:?}"),
        }
    }

    #[test]
    fn junction_round_trips_through_pdb() {
        let protein = asn_structure();
        let result = glycosylate_protein(
            &protein,
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            GlycosylationKind::NLinked,
        )
        .expect("glycosylation");

        let serialized = crate::io::formats::pdb::to_pdb(&result).expect("serialize glycoprotein");
        assert!(
            serialized.lines().any(|line| line.starts_with("LINK")),
            "to_pdb emits LINK for the junction"
        );

        let reparsed =
            crate::io::formats::pdb::parse_pdb(&serialized).expect("reparse glycoprotein");
        assert!(
            junction_present(&reparsed),
            "ND2-C1 junction survives the PDB round trip"
        );
    }
}
