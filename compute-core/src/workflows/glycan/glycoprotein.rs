use anyhow::{Result, anyhow};
use nalgebra::Vector3;

use crate::domain::glycan::{self, ProteinAnchor, TemplateAtom};
use crate::domain::{Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, Structure};
use crate::engines::forcefield;
use crate::workflows::assembly::stitch::{self, AcceptorSite, DonorSite};

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

    let template_atoms: Vec<TemplateAtom> = glycan
        .atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| TemplateAtom {
            name: glycan_bio
                .atom_name(index)
                .map(str::to_string)
                .unwrap_or_default(),
            element: atom.element.clone(),
            position: atom.position,
        })
        .collect();

    let donor_outward = donor_outward_direction(&glycan, reducing_c1, leaving_oxygen);
    let anchor_element = match kind {
        GlycosylationKind::NLinked => "N",
        GlycosylationKind::OLinked => "O",
    };
    let bond_length =
        forcefield::equilibrium_bond_length("C", anchor_element, BondType::Single).unwrap_or(1.45);

    let child_bonds: Vec<(usize, usize)> = glycan.bonds.iter().map(|b| (b.a, b.b)).collect();
    let placement = stitch::place_fragment(
        &template_atoms,
        &child_bonds,
        DonorSite {
            anomeric_atom: reducing_c1,
            outward: donor_outward,
        },
        AcceptorSite {
            oxygen_atom: anchor_atom,
            outward: anchor_outward,
        },
        protein.atoms[anchor_atom].position,
        bond_length,
        anchor_outward,
    );

    merge(
        protein,
        protein_bio,
        &glycan,
        glycan_bio,
        &placement,
        anchor_atom,
        anchor_hydrogen,
        reducing_c1,
        leaving_oxygen,
        leaving_hydrogen,
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

#[allow(clippy::too_many_arguments)]
fn merge(
    protein: &Structure,
    protein_bio: &Biopolymer,
    glycan: &Structure,
    glycan_bio: &Biopolymer,
    placement: &stitch::FragmentPlacement,
    anchor_atom: usize,
    anchor_hydrogen: Option<usize>,
    reducing_c1: usize,
    leaving_oxygen: Option<usize>,
    leaving_hydrogen: Option<usize>,
) -> Result<Structure> {
    let removed_glycan: Vec<bool> = (0..glycan.atoms.len())
        .map(|index| Some(index) == leaving_oxygen || Some(index) == leaving_hydrogen)
        .collect();
    let removed_protein: Vec<bool> = (0..protein.atoms.len())
        .map(|index| Some(index) == anchor_hydrogen)
        .collect();

    let mut atoms: Vec<Atom> = Vec::new();
    let mut atom_names: Vec<Option<String>> = Vec::new();
    let mut residue_for_atom: Vec<Option<usize>> = Vec::new();
    let mut protein_remap = vec![usize::MAX; protein.atoms.len()];

    for (index, atom) in protein.atoms.iter().enumerate() {
        if removed_protein[index] {
            continue;
        }
        protein_remap[index] = atoms.len();
        atoms.push(atom.clone());
        atom_names.push(protein_bio.atom_name(index).map(str::to_string));
        residue_for_atom.push(*protein_bio.residue_for_atom.get(index).unwrap_or(&None));
    }

    let protein_residue_count = protein_bio.residues.len();
    let mut glycan_remap = vec![usize::MAX; glycan.atoms.len()];
    for (index, placed) in placement.atoms.iter().enumerate() {
        if removed_glycan[index] {
            continue;
        }
        glycan_remap[index] = atoms.len();
        atoms.push(Atom {
            element: placed.element.clone(),
            position: placed.position,
            charge: glycan.atoms[index].charge,
        });
        atom_names.push(glycan_bio.atom_name(index).map(str::to_string));
        let residue = glycan_bio
            .residue_for_atom
            .get(index)
            .and_then(|r| *r)
            .map(|r| r + protein_residue_count);
        residue_for_atom.push(residue);
    }

    let mut bonds: Vec<Bond> = Vec::new();
    for bond in &protein.bonds {
        let (a, b) = (
            protein_remap.get(bond.a).copied().unwrap_or(usize::MAX),
            protein_remap.get(bond.b).copied().unwrap_or(usize::MAX),
        );
        if a != usize::MAX && b != usize::MAX {
            bonds.push(Bond::with_type(a, b, bond.bond_type));
        }
    }
    for bond in &glycan.bonds {
        let (a, b) = (
            glycan_remap.get(bond.a).copied().unwrap_or(usize::MAX),
            glycan_remap.get(bond.b).copied().unwrap_or(usize::MAX),
        );
        if a != usize::MAX && b != usize::MAX {
            bonds.push(Bond::with_type(a, b, bond.bond_type));
        }
    }

    let junction_protein = protein_remap[anchor_atom];
    let junction_c1 = glycan_remap[reducing_c1];
    if junction_protein == usize::MAX || junction_c1 == usize::MAX {
        return Err(anyhow!("junction atoms were removed during merge"));
    }
    bonds.push(Bond::with_type(
        junction_protein,
        junction_c1,
        BondType::Single,
    ));

    let mut residues = protein_bio.residues.clone();
    for residue in &mut residues {
        remap_residue(residue, &protein_remap);
    }
    for glycan_residue in &glycan_bio.residues {
        let mut residue = glycan_residue.clone();
        remap_residue(&mut residue, &glycan_remap);
        residues.push(residue);
    }

    let chains = merge_chains(protein_bio, glycan_bio, protein_residue_count);

    let biopolymer = Biopolymer {
        residues,
        chains,
        secondary_structures: protein_bio.secondary_structures.clone(),
        residue_for_atom,
        atom_name_for_atom: atom_names,
    };

    let title = format!("{}+glycan", protein.title);
    let mut structure = Structure::with_bonds(title, atoms, bonds);
    structure.biopolymer = Some(biopolymer);
    structure.cell = protein.cell.clone();

    Ok(structure)
}

fn remap_residue(residue: &mut crate::domain::ResidueRecord, remap: &[usize]) {
    residue.atom_indices = residue
        .atom_indices
        .iter()
        .filter_map(|&old| remap.get(old).copied())
        .filter(|&new| new != usize::MAX)
        .collect();
    residue.alpha_carbon = remap_optional(residue.alpha_carbon, remap);
    residue.backbone_nitrogen = remap_optional(residue.backbone_nitrogen, remap);
    residue.backbone_carbon = remap_optional(residue.backbone_carbon, remap);
    residue.backbone_oxygen = remap_optional(residue.backbone_oxygen, remap);
}

fn remap_optional(index: Option<usize>, remap: &[usize]) -> Option<usize> {
    let old = index?;
    let new = remap.get(old).copied()?;
    if new == usize::MAX { None } else { Some(new) }
}

fn merge_chains(
    protein_bio: &Biopolymer,
    glycan_bio: &Biopolymer,
    protein_residue_count: usize,
) -> Vec<ChainRecord> {
    let mut chains = protein_bio.chains.clone();
    for glycan_chain in &glycan_bio.chains {
        chains.push(ChainRecord {
            id: glycan_chain.id,
            residue_indices: glycan_chain
                .residue_indices
                .iter()
                .map(|&index| index + protein_residue_count)
                .collect(),
        });
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::glycan::{Aglycon, infer_attachment};
    use crate::domain::{AtomCategory, ResidueRecord};
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
