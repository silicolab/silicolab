//! A molecule after force-field typing: every atom carries an atom type and a
//! partial charge, the type table carries nonbonded parameters, and each bonded
//! interaction carries its own parameters. This is what an organic force-field
//! typer (GAFF through antechamber, CGenFF, OPLS) hands back for one molecule,
//! kept engine-neutral so a repeat-unit template can be tiled into a chain
//! topology without re-running the typer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Nonbonded parameters of one atom type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtomTypeParams {
    pub name: String,
    pub mass_u: f64,
    pub sigma_nm: f64,
    pub epsilon_kj_mol: f64,
}

/// One typed atom, in molecule order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedAtom {
    pub name: String,
    pub type_name: String,
    pub residue_name: String,
    pub residue_number: i32,
    pub charge: f64,
    pub mass_u: f64,
}

/// A bonded interaction with explicit parameters, atoms 1-based in molecule
/// order. `func` is the GROMACS interaction function number the typer chose;
/// `params` are the numbers that followed it, in file order (e.g. `r0, k` for a
/// harmonic bond; `phase, k, multiplicity` for a periodic dihedral).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BondedInteraction {
    pub atoms: Vec<u32>,
    pub func: i32,
    pub params: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedMolecule {
    pub name: String,
    /// Bonded neighbours excluded from nonbonded interactions (`nrexcl`).
    pub nrexcl: u32,
    /// Which typer and settings produced this (e.g. `acpype gaff2 bcc`).
    pub source: String,
    pub atom_types: Vec<AtomTypeParams>,
    pub atoms: Vec<TypedAtom>,
    pub bonds: Vec<BondedInteraction>,
    pub pairs: Vec<BondedInteraction>,
    pub angles: Vec<BondedInteraction>,
    /// Proper and improper dihedrals together; the function number tells them
    /// apart (GAFF propers are 9, impropers 4).
    pub dihedrals: Vec<BondedInteraction>,
}

impl TypedMolecule {
    pub fn net_charge(&self) -> f64 {
        self.atoms.iter().map(|atom| atom.charge).sum()
    }

    pub fn atom_type(&self, name: &str) -> Option<&AtomTypeParams> {
        self.atom_types.iter().find(|t| t.name == name)
    }

    /// How many atoms carry each type, in type-name order.
    pub fn type_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::new();
        for atom in &self.atoms {
            *counts.entry(atom.type_name.as_str()).or_insert(0) += 1;
        }
        counts
    }
}
