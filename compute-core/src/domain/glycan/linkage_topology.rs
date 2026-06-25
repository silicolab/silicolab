use crate::domain::biopolymer::{Biopolymer, is_carbohydrate_residue};
use crate::domain::structure::Structure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProteinAnchor {
    AsnNd2,
    SerOg,
    ThrOg1,
}

impl ProteinAnchor {
    pub fn residue_name(self) -> &'static str {
        match self {
            ProteinAnchor::AsnNd2 => "ASN",
            ProteinAnchor::SerOg => "SER",
            ProteinAnchor::ThrOg1 => "THR",
        }
    }

    pub fn atom_name(self) -> &'static str {
        match self {
            ProteinAnchor::AsnNd2 => "ND2",
            ProteinAnchor::SerOg => "OG",
            ProteinAnchor::ThrOg1 => "OG1",
        }
    }

    fn from_residue_atom(residue_name: &str, atom_name: &str) -> Option<Self> {
        match (residue_name.trim(), atom_name) {
            ("ASN", "ND2") => Some(ProteinAnchor::AsnNd2),
            ("SER", "OG") => Some(ProteinAnchor::SerOg),
            ("THR", "OG1") => Some(ProteinAnchor::ThrOg1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondLinkage {
    IntraResidue,
    Glycosidic {
        carbon: usize,
        oxygen: usize,
    },
    GlycanProtein {
        anomeric_carbon: usize,
        protein_atom: usize,
        anchor: ProteinAnchor,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossResidueLinkage {
    pub bond_index: usize,
    pub linkage: BondLinkage,
}

pub fn is_anomeric_carbon(name: &str) -> bool {
    matches!(name, "C1" | "C2")
}

pub fn acceptor_oxygen_position(name: &str) -> Option<u8> {
    name.strip_prefix('O')?.parse::<u8>().ok()
}

fn residue_name_of(biopolymer: &Biopolymer, atom_index: usize) -> Option<&str> {
    let residue_index = (*biopolymer.residue_for_atom.get(atom_index)?)?;
    Some(
        biopolymer
            .residues
            .get(residue_index)?
            .residue_name
            .as_str(),
    )
}

fn is_carbohydrate_atom(biopolymer: &Biopolymer, atom_index: usize) -> bool {
    residue_name_of(biopolymer, atom_index)
        .map(is_carbohydrate_residue)
        .unwrap_or(false)
}

pub fn classify_bond(
    structure: &Structure,
    biopolymer: &Biopolymer,
    a: usize,
    b: usize,
) -> BondLinkage {
    let ra = biopolymer.residue_for_atom.get(a).and_then(|r| *r);
    let rb = biopolymer.residue_for_atom.get(b).and_then(|r| *r);
    let (Some(ra), Some(rb)) = (ra, rb) else {
        return BondLinkage::IntraResidue;
    };
    if ra == rb {
        return BondLinkage::IntraResidue;
    }
    if a >= structure.atoms.len() || b >= structure.atoms.len() {
        return BondLinkage::IntraResidue;
    }

    let na = biopolymer.atom_name(a).unwrap_or("");
    let nb = biopolymer.atom_name(b).unwrap_or("");
    let a_carb = is_carbohydrate_atom(biopolymer, a);
    let b_carb = is_carbohydrate_atom(biopolymer, b);

    if a_carb && b_carb {
        if is_anomeric_carbon(na) && acceptor_oxygen_position(nb).is_some() {
            return BondLinkage::Glycosidic {
                carbon: a,
                oxygen: b,
            };
        }
        if is_anomeric_carbon(nb) && acceptor_oxygen_position(na).is_some() {
            return BondLinkage::Glycosidic {
                carbon: b,
                oxygen: a,
            };
        }
        return BondLinkage::IntraResidue;
    }

    if a_carb
        && is_anomeric_carbon(na)
        && let Some(anchor) = protein_anchor_at(biopolymer, b)
    {
        return BondLinkage::GlycanProtein {
            anomeric_carbon: a,
            protein_atom: b,
            anchor,
        };
    }
    if b_carb
        && is_anomeric_carbon(nb)
        && let Some(anchor) = protein_anchor_at(biopolymer, a)
    {
        return BondLinkage::GlycanProtein {
            anomeric_carbon: b,
            protein_atom: a,
            anchor,
        };
    }

    BondLinkage::IntraResidue
}

fn protein_anchor_at(biopolymer: &Biopolymer, atom_index: usize) -> Option<ProteinAnchor> {
    let residue_name = residue_name_of(biopolymer, atom_index)?;
    let atom_name = biopolymer.atom_name(atom_index)?;
    ProteinAnchor::from_residue_atom(residue_name, atom_name)
}

pub fn cross_residue_linkages(
    structure: &Structure,
    biopolymer: &Biopolymer,
) -> Vec<CrossResidueLinkage> {
    structure
        .bonds
        .iter()
        .enumerate()
        .filter_map(|(bond_index, bond)| {
            match classify_bond(structure, biopolymer, bond.a, bond.b) {
                BondLinkage::IntraResidue => None,
                linkage => Some(CrossResidueLinkage {
                    bond_index,
                    linkage,
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::biopolymer::{ChainRecord, ResidueId, ResidueRecord};
    use crate::domain::structure::{Atom, Bond, BondType};
    use nalgebra::Point3;

    fn atom() -> Atom {
        Atom {
            element: "C".to_string(),
            position: Point3::origin(),
            charge: 0.0,
        }
    }

    fn fixture() -> (Structure, Biopolymer) {
        let atoms = vec![atom(), atom(), atom(), atom()];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(2, 1, BondType::Single),
            Bond::with_type(3, 0, BondType::Single),
        ];
        let names = ["ND2", "C1", "C2", "O4"];
        let residue_for_atom = vec![Some(0), Some(1), Some(1), Some(2)];
        let residues = vec![
            ResidueRecord {
                id: ResidueId::new('A', 1, ' '),
                residue_name: "ASN".to_string(),
                atom_indices: vec![0],
                alpha_carbon: None,
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
                is_standard_amino_acid: true,
            },
            ResidueRecord {
                id: ResidueId::new('B', 1, ' '),
                residue_name: "NAG".to_string(),
                atom_indices: vec![1, 2],
                alpha_carbon: None,
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
                is_standard_amino_acid: false,
            },
            ResidueRecord {
                id: ResidueId::new('B', 2, ' '),
                residue_name: "MAN".to_string(),
                atom_indices: vec![3],
                alpha_carbon: None,
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
                is_standard_amino_acid: false,
            },
        ];
        let biopolymer = Biopolymer {
            residues,
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0, 1, 2],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom,
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let structure = Structure::with_bonds("test".to_string(), atoms, bonds);
        (structure, biopolymer)
    }

    #[test]
    fn classifies_each_bond_kind() {
        let (structure, biopolymer) = fixture();
        assert_eq!(
            classify_bond(&structure, &biopolymer, 0, 1),
            BondLinkage::GlycanProtein {
                anomeric_carbon: 1,
                protein_atom: 0,
                anchor: ProteinAnchor::AsnNd2,
            }
        );
        assert_eq!(
            classify_bond(&structure, &biopolymer, 2, 1),
            BondLinkage::IntraResidue
        );
        assert_eq!(
            classify_bond(&structure, &biopolymer, 3, 0),
            BondLinkage::IntraResidue
        );
    }

    #[test]
    fn cross_residue_linkages_skips_intra_residue() {
        let (structure, biopolymer) = fixture();
        let cross = cross_residue_linkages(&structure, &biopolymer);
        assert_eq!(cross.len(), 1);
        assert_eq!(cross[0].bond_index, 0);
    }
}
