//! Trajectory analysis for transport in dense phases: mean-squared displacement
//! and probe-accessible free volume. Pure geometry over [`crate::domain::Trajectory`]
//! and [`crate::domain::Structure`]; no engine is involved.

pub mod free_volume;
pub mod msd;

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::domain::Structure;

pub use free_volume::{FreeVolumeReport, FreeVolumeSpec, free_volume, free_volume_over_frames};
pub use msd::{DiffusionReport, MsdSpec, mean_squared_displacement, msd_csv};

/// Atoms named by a selector: `all`, an element symbol (`C`), or a residue name
/// (`BEN`, matched case-insensitively against the structure's residues).
pub fn select_atoms(structure: &Structure, selector: &str) -> Result<Vec<usize>> {
    let selector = selector.trim();
    if selector.is_empty() || selector.eq_ignore_ascii_case("all") {
        return Ok((0..structure.atoms.len()).collect());
    }
    let mut atoms: Vec<usize> = structure
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| atom.element.eq_ignore_ascii_case(selector))
        .map(|(index, _)| index)
        .collect();
    if atoms.is_empty()
        && let Some(biopolymer) = &structure.biopolymer
    {
        atoms = biopolymer
            .residues
            .iter()
            .filter(|residue| residue.residue_name.eq_ignore_ascii_case(selector))
            .flat_map(|residue| residue.atom_indices.iter().copied())
            .filter(|&atom| atom < structure.atoms.len())
            .collect();
        atoms.sort_unstable();
        atoms.dedup();
    }
    if atoms.is_empty() {
        let residues: BTreeSet<&str> = structure
            .biopolymer
            .iter()
            .flat_map(|b| b.residues.iter().map(|r| r.residue_name.as_str()))
            .collect();
        let hint = if residues.is_empty() {
            "the structure has no residues; use `all` or an element symbol".to_string()
        } else {
            format!(
                "residues present: {}",
                residues.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        bail!("`{selector}` selects no atoms ({hint})");
    }
    Ok(atoms)
}

/// Group `atoms` into molecules: connected components of the structure's bonds
/// restricted to the selection. An atom with no selected bonded neighbour is a
/// molecule of its own, so a bond-free structure yields one molecule per atom.
pub fn molecules_of(structure: &Structure, atoms: &[usize]) -> Vec<Vec<usize>> {
    let n = structure.atoms.len();
    let mut selected = vec![false; n];
    for &atom in atoms {
        if atom < n {
            selected[atom] = true;
        }
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for bond in &structure.bonds {
        if bond.a < n && bond.b < n && selected[bond.a] && selected[bond.b] {
            let (ra, rb) = (find(&mut parent, bond.a), find(&mut parent, bond.b));
            if ra != rb {
                parent[ra] = rb;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for &atom in atoms {
        if atom < n {
            let root = find(&mut parent, atom);
            groups.entry(root).or_default().push(atom);
        }
    }
    let mut molecules: Vec<Vec<usize>> = groups.into_values().collect();
    for molecule in &mut molecules {
        molecule.sort_unstable();
        molecule.dedup();
    }
    molecules.sort_by_key(|molecule| molecule[0]);
    molecules
}

/// Bondi van der Waals radius (Å) of an element, with a generic 1.7 Å for
/// elements outside the common organic/halogen set.
pub fn van_der_waals_radius(element: &str) -> f32 {
    match element {
        "H" => 1.20,
        "C" => 1.70,
        "N" => 1.55,
        "O" => 1.52,
        "F" => 1.47,
        "Si" => 2.10,
        "P" => 1.80,
        "S" => 1.80,
        "Cl" => 1.75,
        "Br" => 1.85,
        "I" => 1.98,
        "He" => 1.40,
        "Ne" => 1.54,
        "Ar" => 1.88,
        _ => 1.70,
    }
}

#[cfg(test)]
mod tests;
