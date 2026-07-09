pub mod dictionary;
pub mod iupac;
pub mod linkage_topology;
pub mod patches;
pub mod reducing_end;
pub mod templates;

use crate::domain::ResidueId;
use crate::domain::biopolymer::Biopolymer;
use crate::domain::structure::Structure;

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlycanTree {
    pub nodes: Vec<GlycanResidue>,
    pub root: NodeId,
    /// The aglycon named by the reducing-end linkage (`GlcNAc(b1-N)`), when the
    /// notation states one. Cross-checked against the requested glycosylation.
    pub aglycon: Option<GlycosylationKind>,
}

/// Which protein side chain a glycan's reducing end condenses onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlycosylationKind {
    NLinked,
    OLinked,
}

impl GlycosylationKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::NLinked => "N-linked",
            Self::OLinked => "O-linked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlycanResidue {
    pub mono: Monosaccharide,
    pub parent: Option<NodeId>,
    pub linkage: Option<Linkage>,
    pub children: Vec<(Linkage, NodeId)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monosaccharide {
    pub kind: SugarKind,
    pub ring: RingForm,
    pub config: AbsConfig,
    pub anomer: Anomer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Linkage {
    pub anomer: Anomer,
    pub child_pos: u8,
    pub parent_pos: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SugarKind {
    Glc,
    Gal,
    Man,
    Fuc,
    Xyl,
    GlcNAc,
    GalNAc,
    ManNAc,
    Neu5Ac,
    Neu5Gc,
    GlcA,
    IdoA,
    GalA,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anomer {
    Alpha,
    Beta,
    Unknown,
}

impl Anomer {
    pub fn name(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::Unknown => "unspecified",
        }
    }

    /// The `a`/`b`/`?` character this anomer takes inside a linkage.
    pub fn symbol(self) -> char {
        match self {
            Self::Alpha => 'a',
            Self::Beta => 'b',
            Self::Unknown => '?',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingForm {
    Pyranose,
    Furanose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsConfig {
    D,
    L,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aglycon {
    NLinked {
        asn: ResidueId,
        anchor_atom: String,
    },
    OLinked {
        ser_thr: ResidueId,
        anchor_atom: String,
    },
}

pub fn infer_attachment(structure: &Structure) -> Option<Aglycon> {
    let biopolymer = structure.biopolymer.as_ref()?;
    if !biopolymer.is_compatible_with_atom_count(structure.atoms.len()) {
        return None;
    }

    if let Some(aglycon) = attachment_from_bonds(structure, biopolymer) {
        return Some(aglycon);
    }
    attachment_from_proximity(structure, biopolymer)
}

const MAX_GLYCOSIDIC_BOND_LENGTH: f32 = 2.0;

fn attachment_from_bonds(structure: &Structure, biopolymer: &Biopolymer) -> Option<Aglycon> {
    linkage_topology::cross_residue_linkages(structure, biopolymer)
        .into_iter()
        .find_map(|cross| match cross.linkage {
            BondLinkage::GlycanProtein {
                protein_atom,
                anchor,
                ..
            } => aglycon_at_protein_atom(biopolymer, protein_atom, anchor),
            _ => None,
        })
}

fn attachment_from_proximity(structure: &Structure, biopolymer: &Biopolymer) -> Option<Aglycon> {
    let reducing_c1 = reducing_end_anomeric_carbon(structure, biopolymer)?;
    let c1_position = structure.atoms.get(reducing_c1)?.position;

    let mut best: Option<(f32, usize, ProteinAnchor)> = None;
    for (atom_index, _) in structure.atoms.iter().enumerate() {
        let residue_name = match residue_name_for(biopolymer, atom_index) {
            Some(name) => name,
            None => continue,
        };
        let atom_name = match biopolymer.atom_name(atom_index) {
            Some(name) => name,
            None => continue,
        };
        let Some(anchor) = protein_anchor_for(residue_name, atom_name) else {
            continue;
        };
        let position = match structure.atoms.get(atom_index) {
            Some(atom) => atom.position,
            None => continue,
        };
        let distance = (position - c1_position).norm();
        if distance <= MAX_GLYCOSIDIC_BOND_LENGTH
            && best.map(|(d, _, _)| distance < d).unwrap_or(true)
        {
            best = Some((distance, atom_index, anchor));
        }
    }

    let (_, protein_atom, anchor) = best?;
    aglycon_at_protein_atom(biopolymer, protein_atom, anchor)
}

fn protein_anchor_for(residue_name: &str, atom_name: &str) -> Option<ProteinAnchor> {
    match (residue_name.trim(), atom_name) {
        ("ASN", "ND2") => Some(ProteinAnchor::AsnNd2),
        ("SER", "OG") => Some(ProteinAnchor::SerOg),
        ("THR", "OG1") => Some(ProteinAnchor::ThrOg1),
        _ => None,
    }
}

fn reducing_end_anomeric_carbon(structure: &Structure, biopolymer: &Biopolymer) -> Option<usize> {
    let root_residue_index = biopolymer.residues.iter().position(|residue| {
        crate::domain::biopolymer::is_carbohydrate_residue(&residue.residue_name)
    })?;
    let root = &biopolymer.residues[root_residue_index];
    root.atom_indices.iter().copied().find(|&atom_index| {
        atom_index < structure.atoms.len() && biopolymer.atom_name(atom_index) == Some("C1")
    })
}

fn aglycon_at_protein_atom(
    biopolymer: &Biopolymer,
    protein_atom: usize,
    anchor: ProteinAnchor,
) -> Option<Aglycon> {
    let residue_index = (*biopolymer.residue_for_atom.get(protein_atom)?)?;
    let residue = biopolymer.residues.get(residue_index)?;
    let anchor_atom = anchor.atom_name().to_string();
    match anchor {
        ProteinAnchor::AsnNd2 => Some(Aglycon::NLinked {
            asn: residue.id.clone(),
            anchor_atom,
        }),
        ProteinAnchor::SerOg | ProteinAnchor::ThrOg1 => Some(Aglycon::OLinked {
            ser_thr: residue.id.clone(),
            anchor_atom,
        }),
        // Non-glycosidic anchors are not N-/O-linked aglycons; ignore them.
        _ => None,
    }
}

fn residue_name_for(biopolymer: &Biopolymer, atom_index: usize) -> Option<&str> {
    let residue_index = (*biopolymer.residue_for_atom.get(atom_index)?)?;
    Some(
        biopolymer
            .residues
            .get(residue_index)?
            .residue_name
            .as_str(),
    )
}

pub use dictionary::{MonosaccharideEntry, entry_for, lookup, supported_tokens};
pub use iupac::{parse, to_iupac};
pub(crate) use linkage_topology::ProteinAnchor;
pub use linkage_topology::{
    BondLinkage, CrossResidueLinkage, classify_bond, cross_residue_linkages, is_anomeric_carbon,
};
pub use reducing_end::{canonical_anomer, resolve_root_anomer};
pub use templates::{CoordinationSite, RingTemplate, TemplateAtom, TemplateBond, ring_template};

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use crate::domain::biopolymer::{ChainRecord, ResidueRecord};
    use crate::domain::structure::{Atom, Bond, BondType};
    use nalgebra::Point3;

    fn carbon(x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn residue(name: &str, seq: i32, atom_indices: Vec<usize>, standard: bool) -> ResidueRecord {
        ResidueRecord {
            id: ResidueId::new('A', seq, ' '),
            residue_name: name.to_string(),
            atom_indices,
            alpha_carbon: None,
            backbone_nitrogen: None,
            backbone_carbon: None,
            backbone_oxygen: None,
            is_standard_amino_acid: standard,
        }
    }

    fn nlinked_structure(with_bond: bool) -> Structure {
        let atoms = vec![carbon(0.0, 0.0, 0.0), carbon(1.4, 0.0, 0.0)];
        let bonds = if with_bond {
            vec![Bond::with_type(0, 1, BondType::Single)]
        } else {
            Vec::new()
        };
        let biopolymer = Biopolymer {
            residues: vec![
                residue("ASN", 1, vec![0], true),
                residue("NAG", 2, vec![1], false),
            ],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0, 1],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom: vec![Some(0), Some(1)],
            atom_name_for_atom: vec![Some("ND2".to_string()), Some("C1".to_string())],
        };
        let mut structure = Structure::with_bonds("attachment".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    #[test]
    fn infers_n_linked_attachment_from_bond() {
        let structure = nlinked_structure(true);
        let attachment = infer_attachment(&structure).expect("attachment");
        match attachment {
            Aglycon::NLinked { asn, anchor_atom } => {
                assert_eq!(asn, ResidueId::new('A', 1, ' '));
                assert_eq!(anchor_atom, "ND2");
            }
            other => panic!("expected N-linked, got {other:?}"),
        }
    }

    #[test]
    fn infers_n_linked_attachment_from_proximity() {
        let structure = nlinked_structure(false);
        let attachment = infer_attachment(&structure).expect("attachment");
        assert!(matches!(attachment, Aglycon::NLinked { .. }));
    }
}
