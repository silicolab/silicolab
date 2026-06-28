//! Minimal single-residue protein fixtures for the PTM workflow tests. Each
//! fixture carries the biopolymer overlay the host resolver and condense path
//! require, plus just the atoms a given anchor needs.
#![cfg(test)]

use nalgebra::Point3;

use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord, Structure,
};

/// Backbone atom indices to record on the fixture residue (needed for N-terminus
/// resolution and outward directions).
pub(crate) struct Backbone {
    pub alpha: Option<usize>,
    pub nitrogen: Option<usize>,
    pub carbon: Option<usize>,
    pub oxygen: Option<usize>,
}

/// Build a one-residue, one-chain protein from named atoms and single bonds.
pub(crate) fn single_residue(
    residue_name: &str,
    atoms: &[(&str, &str, [f32; 3])],
    bonds: &[(usize, usize)],
    backbone: Backbone,
) -> Structure {
    let names = atoms
        .iter()
        .map(|(name, ..)| Some(name.to_string()))
        .collect();
    let atom_records = atoms
        .iter()
        .map(|(_, element, p)| Atom {
            element: element.to_string(),
            position: Point3::new(p[0], p[1], p[2]),
            charge: 0.0,
        })
        .collect();
    let bond_records = bonds
        .iter()
        .map(|&(a, b)| Bond::with_type(a, b, BondType::Single))
        .collect();
    let residue = ResidueRecord {
        id: ResidueId::new('A', 1, ' '),
        residue_name: residue_name.to_string(),
        atom_indices: (0..atoms.len()).collect(),
        alpha_carbon: backbone.alpha,
        backbone_nitrogen: backbone.nitrogen,
        backbone_carbon: backbone.carbon,
        backbone_oxygen: backbone.oxygen,
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
        atom_name_for_atom: names,
    };
    let mut structure = Structure::with_bonds(residue_name.to_string(), atom_records, bond_records);
    structure.biopolymer = Some(biopolymer);
    structure
}

/// Side-chain backbone (N/CA/C/O at indices 0..=3) shared by the fixtures.
pub(crate) fn sidechain_backbone() -> Backbone {
    Backbone {
        alpha: Some(1),
        nitrogen: Some(0),
        carbon: Some(2),
        oxygen: Some(3),
    }
}

/// The target residue still carries the named anchor atom after a modification.
pub(crate) fn residue_has_atom(structure: &Structure, residue: ResidueId, atom_name: &str) -> bool {
    let bio = structure.biopolymer.as_ref().expect("overlay");
    bio.residues
        .iter()
        .find(|record| record.id == residue)
        .is_some_and(|record| {
            record
                .atom_indices
                .iter()
                .any(|&index| bio.atom_name(index) == Some(atom_name))
        })
}

/// True if a bond joins atoms named `first` and `second` (in either order).
pub(crate) fn junction(structure: &Structure, first: &str, second: &str) -> bool {
    let bio = structure.biopolymer.as_ref().expect("overlay");
    structure.bonds.iter().any(|bond| {
        let (a, b) = (bio.atom_name(bond.a), bio.atom_name(bond.b));
        (a == Some(first) && b == Some(second)) || (a == Some(second) && b == Some(first))
    })
}
