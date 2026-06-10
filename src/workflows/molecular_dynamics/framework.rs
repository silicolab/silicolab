//! Build an engine-neutral [`MdTopology`] for a covalent framework material — a
//! 2D nanosheet, and by extension any bonded periodic structure.
//!
//! Unlike [`MdTopology::from_structure`](super::topology::MdTopology::from_structure),
//! which handles only bond-free monatomic systems, this consumes the structure's
//! own bond graph. Two models are offered:
//!
//! * [`FrameworkMode::Rigid`] — every atom is a frozen Lennard-Jones site. The
//!   topology carries no bonded terms; it lists explicit 1-2/1-3 nonbonded
//!   exclusions so close bonded neighbors do not blow up the LJ energy. The atoms
//!   are held fixed by a freeze group at run time. Works for any element with
//!   tabulated parameters ([`super::materials`]).
//! * [`FrameworkMode::Flexible`] — bonds, angles and Ryckaert-Bellemans dihedrals
//!   are derived from the bond graph and parameterized from a force field. Only
//!   available for chemistry with bonded parameters (the aromatic-carbon family);
//!   it errors otherwise so the caller can fall back to the rigid model.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::domain::{Bond, Structure, chemistry::normalized_symbol};

use super::materials::{self, CustomTypes, ElementParameterization};
use super::topology::{BondedTerm, MdTopology, MoleculeAtom, MoleculeRun, MoleculeType, Species};

/// The molecule/residue name a framework topology's single all-atom molecule
/// uses.
const FRAMEWORK_MOLECULE: &str = "SHT";

/// How a covalent framework is modeled when building its topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameworkMode {
    /// Frozen framework: Lennard-Jones sites held fixed, with explicit
    /// exclusions and no bonded terms. Works for any supported element.
    Rigid,
    /// Flexible framework: bonds/angles/dihedrals from the bond graph,
    /// parameterized from a force field. Aromatic carbon only.
    Flexible,
}

impl FrameworkMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rigid => "Rigid (frozen)",
            Self::Flexible => "Flexible (bonded)",
        }
    }
}

impl MdTopology {
    /// Build the engine-neutral topology for a covalent framework material,
    /// modeled either rigidly (frozen LJ sites with explicit exclusions) or
    /// flexibly (bonds/angles/dihedrals from the bond graph).
    ///
    /// Errors when the structure is empty, contains an element with no framework
    /// parameters, or — in [`FrameworkMode::Flexible`] — uses chemistry with no
    /// bonded parameters (the caller should fall back to [`FrameworkMode::Rigid`]).
    pub fn framework(structure: &Structure, mode: FrameworkMode) -> Result<Self> {
        Self::framework_with_custom(structure, mode, &CustomTypes::default())
    }

    /// Like [`framework`](Self::framework), but elements absent from the built-in
    /// tables are accepted when `custom` supplies a matching atom type (named
    /// after the element symbol). For those elements no built-in `[atomtypes]`
    /// entry is emitted — the renderer `#include`s the user's force field, which
    /// provides it. A custom type whose name matches a built-in one overrides it
    /// the same way.
    ///
    /// Custom parameters only enable the **rigid** model; flexible (bonded)
    /// modeling still requires built-in carbon-family bonded parameters, because
    /// the bonded terms cannot be auto-derived for arbitrary chemistry.
    pub fn framework_with_custom(
        structure: &Structure,
        mode: FrameworkMode,
        custom: &CustomTypes,
    ) -> Result<Self> {
        if structure.atoms.is_empty() {
            bail!("cannot build a framework topology for a structure with no atoms");
        }

        // Resolve every atom to an atom type first, so an unparameterized element
        // fails once with a clear, actionable message. Built-in types contribute
        // a Lennard-Jones species; custom (and overridden) types are provided by
        // the user's `#include`, so no species is emitted for them.
        let mut type_names: Vec<String> = Vec::with_capacity(structure.atoms.len());
        let mut species: Vec<Species> = Vec::new();
        let mut has_custom_type = false;
        for atom in &structure.atoms {
            let parameterization = materials::parameterize_element(&atom.element, custom)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no framework force-field parameters for element `{}`; built-in support \
                         covers {:?}. Supply a custom force field that defines an atom type named \
                         `{}`.",
                        normalized_symbol(&atom.element),
                        materials::supported_elements(),
                        normalized_symbol(&atom.element),
                    )
                })?;
            type_names.push(parameterization.type_name().to_string());
            match parameterization {
                ElementParameterization::BuiltIn {
                    atom_type: mat,
                    overridden,
                } => {
                    if !overridden && !species.iter().any(|s| s.element == mat.type_name) {
                        species.push(Species {
                            element: mat.type_name.to_string(),
                            atomic_number: mat.atomic_number,
                            mass_u: mat.mass_u,
                            charge: 0.0,
                            sigma_angstrom: mat.sigma_angstrom,
                            epsilon_kj_mol: mat.epsilon_kj_mol,
                        });
                    }
                }
                ElementParameterization::Custom { .. } => has_custom_type = true,
            }
        }

        let adjacency = bond_adjacency(structure.atoms.len(), &structure.bonds);

        let atoms: Vec<MoleculeAtom> = type_names
            .iter()
            .zip(&structure.atoms)
            .map(|(type_name, atom)| MoleculeAtom::new(type_name, type_name, atom.charge))
            .collect();

        let mut molecule = MoleculeType {
            name: FRAMEWORK_MOLECULE.to_string(),
            nrexcl: 0,
            atoms,
            settle: None,
            bonds: Vec::new(),
            pairs: Vec::new(),
            angles: Vec::new(),
            dihedrals: Vec::new(),
            impropers: Vec::new(),
            exclusions: Vec::new(),
        };

        let mut defaults = None;
        let mut bonded_params = Vec::new();

        match mode {
            FrameworkMode::Rigid => {
                // No bonds for grompp to derive exclusions from, so list the 1-2
                // and 1-3 neighbors explicitly. The sheet is frozen, so no
                // bonded terms are needed to hold it together.
                molecule.exclusions = framework_exclusions(&adjacency);
            }
            FrameworkMode::Flexible => {
                if has_custom_type {
                    bail!(
                        "flexible (bonded) modeling is only available for built-in carbon-family \
                         chemistry; elements supplied by a custom force field must use the rigid \
                         (frozen) model"
                    );
                }
                let present: Vec<&str> = species.iter().map(|s| s.element.as_str()).collect();
                let ff = materials::flexible_force_field(&present).ok_or_else(|| {
                    anyhow::anyhow!(
                        "flexible (bonded) modeling has no force-field parameters for {}; \
                         use the rigid (frozen) model instead",
                        distinct_elements(structure).join(", ")
                    )
                })?;
                molecule.nrexcl = 3;
                molecule.bonds = framework_bonds(&structure.bonds);
                molecule.angles = framework_angles(&adjacency);
                molecule.dihedrals = framework_dihedrals(&adjacency);
                defaults = Some(ff.defaults);
                bonded_params = ff.bonded_params;
            }
        }

        let title = structure.title.lines().next().unwrap_or("").trim();
        Ok(Self {
            title: if title.is_empty() {
                "SilicoLab framework".to_string()
            } else {
                title.to_string()
            },
            species,
            molecules: vec![molecule],
            composition: vec![MoleculeRun {
                molecule: FRAMEWORK_MOLECULE.to_string(),
                count: 1,
            }],
            defaults,
            bonded_params,
            inline_force_field: None,
        })
    }
}

/// Build a 0-based neighbor adjacency list from a bond list, with each atom's
/// neighbors sorted and de-duplicated.
fn bond_adjacency(atom_count: usize, bonds: &[Bond]) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::new(); atom_count];
    for bond in bonds {
        if bond.a == bond.b || bond.a >= atom_count || bond.b >= atom_count {
            continue;
        }
        adjacency[bond.a].push(bond.b);
        adjacency[bond.b].push(bond.a);
    }
    for neighbors in &mut adjacency {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    adjacency
}

/// Index-only harmonic bonds (func 1) from the bond list, 1-based, de-duplicated
/// and orientation-normalized (`a < b`).
fn framework_bonds(bonds: &[Bond]) -> Vec<BondedTerm> {
    let mut seen = BTreeSet::new();
    for bond in bonds {
        if bond.a == bond.b {
            continue;
        }
        seen.insert((bond.a.min(bond.b), bond.a.max(bond.b)));
    }
    seen.into_iter()
        .map(|(a, b)| BondedTerm {
            atoms: vec![a as u32 + 1, b as u32 + 1],
            func: 1,
        })
        .collect()
}

/// Index-only harmonic angles (func 1): every pair of bonds sharing a central
/// atom. 1-based, with the two end atoms ordered so each angle appears once.
fn framework_angles(adjacency: &[Vec<usize>]) -> Vec<BondedTerm> {
    let mut angles = Vec::new();
    for (center, neighbors) in adjacency.iter().enumerate() {
        for i in 0..neighbors.len() {
            for j in (i + 1)..neighbors.len() {
                angles.push(BondedTerm {
                    atoms: vec![
                        neighbors[i] as u32 + 1,
                        center as u32 + 1,
                        neighbors[j] as u32 + 1,
                    ],
                    func: 1,
                });
            }
        }
    }
    angles
}

/// Index-only Ryckaert-Bellemans proper dihedrals (func 3): every i-j-k-l path
/// over a central bond (j,k). 1-based, de-duplicated. Each central bond is
/// visited once (`j < k`), so a dihedral and its reverse are not both emitted.
fn framework_dihedrals(adjacency: &[Vec<usize>]) -> Vec<BondedTerm> {
    let mut seen = BTreeSet::new();
    for (j, neighbors_j) in adjacency.iter().enumerate() {
        for &k in neighbors_j {
            if j >= k {
                continue;
            }
            for &i in neighbors_j {
                if i == k {
                    continue;
                }
                for &l in &adjacency[k] {
                    if l == j || l == i {
                        continue;
                    }
                    seen.insert((i, j, k, l));
                }
            }
        }
    }
    seen.into_iter()
        .map(|(i, j, k, l)| BondedTerm {
            atoms: vec![i as u32 + 1, j as u32 + 1, k as u32 + 1, l as u32 + 1],
            func: 3,
        })
        .collect()
}

/// Per-atom 1-2 and 1-3 nonbonded exclusions (1-based) for a rigid framework.
/// `exclusions[i]` lists every atom bonded to atom `i+1` (1-2) or bonded to one
/// of those (1-3), excluding the atom itself.
fn framework_exclusions(adjacency: &[Vec<usize>]) -> Vec<Vec<u32>> {
    adjacency
        .iter()
        .enumerate()
        .map(|(i, neighbors)| {
            let mut excluded = BTreeSet::new();
            for &j in neighbors {
                excluded.insert(j);
                for &k in &adjacency[j] {
                    if k != i {
                        excluded.insert(k);
                    }
                }
            }
            excluded.remove(&i);
            excluded.into_iter().map(|j| j as u32 + 1).collect()
        })
        .collect()
}

/// Distinct element symbols in a structure, normalized, in first-seen order.
fn distinct_elements(structure: &Structure) -> Vec<String> {
    let mut elements: Vec<String> = Vec::new();
    for atom in &structure.atoms {
        let symbol = normalized_symbol(&atom.element);
        if !elements.contains(&symbol) {
            elements.push(symbol);
        }
    }
    elements
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::*;
    use crate::domain::{Atom, BondType};

    /// A closed ring of `elements`, each atom bonded to its two ring neighbors.
    /// Positions are placed on a circle; the framework builder ignores them but
    /// they keep the structure well-formed.
    fn ring(elements: &[&str]) -> Structure {
        let n = elements.len();
        let atoms: Vec<Atom> = elements
            .iter()
            .enumerate()
            .map(|(i, element)| {
                let theta = (i as f32) / (n as f32) * std::f32::consts::TAU;
                Atom {
                    element: element.to_string(),
                    position: Point3::new(theta.cos(), theta.sin(), 0.0),
                    charge: 0.0,
                }
            })
            .collect();
        let bonds = (0..n)
            .map(|i| Bond::with_type(i, (i + 1) % n, BondType::Single))
            .collect();
        Structure::with_bonds("ring", atoms, bonds)
    }

    #[test]
    fn rigid_framework_excludes_neighbors_and_has_no_bonds() {
        let benzene = ring(&["C"; 6]);
        let topo = MdTopology::framework(&benzene, FrameworkMode::Rigid).unwrap();

        // One CJ species, one all-atom molecule, frozen (no bonded terms).
        assert_eq!(topo.species.len(), 1);
        assert_eq!(topo.species[0].element, "CJ");
        assert!(topo.defaults.is_none());
        assert!(topo.bonded_params.is_empty());
        let mol = &topo.molecules[0];
        assert_eq!(mol.nrexcl, 0);
        assert!(!mol.has_bonded_terms());
        assert_eq!(mol.atoms.len(), 6);
        // In a 6-ring every atom has two 1-2 and two 1-3 neighbors -> 4 exclusions.
        assert_eq!(mol.exclusions.len(), 6);
        assert!(mol.exclusions.iter().all(|e| e.len() == 4));
        assert_eq!(
            topo.composition,
            vec![MoleculeRun {
                molecule: "SHT".into(),
                count: 1,
            }]
        );
        assert_eq!(topo.atom_count(), 6);
    }

    #[test]
    fn flexible_carbon_framework_has_bonds_angles_and_dihedrals() {
        let benzene = ring(&["C"; 6]);
        let topo = MdTopology::framework(&benzene, FrameworkMode::Flexible).unwrap();

        let mol = &topo.molecules[0];
        assert_eq!(mol.nrexcl, 3);
        // 6 ring bonds, 6 angles (one per central atom), 6 ring dihedrals.
        assert_eq!(mol.bonds.len(), 6);
        assert_eq!(mol.angles.len(), 6);
        assert_eq!(mol.dihedrals.len(), 6);
        assert!(
            mol.exclusions.is_empty(),
            "grompp derives exclusions from nrexcl"
        );
        assert_eq!(topo.defaults.unwrap().comb_rule, 3);
        assert!(
            topo.bonded_params
                .iter()
                .any(|p| p.kind == "bondtypes" && p.atoms == "CJ CJ")
        );
    }

    #[test]
    fn flexible_rejects_chemistry_without_bonded_parameters() {
        let mos2 = ring(&["Mo", "S", "Mo", "S"]);
        let err = MdTopology::framework(&mos2, FrameworkMode::Flexible)
            .unwrap_err()
            .to_string();
        assert!(err.contains("rigid"), "should steer to rigid: {err}");
    }

    #[test]
    fn rigid_framework_supports_non_carbon_chemistry() {
        let mos2 = ring(&["Mo", "S", "Mo", "S"]);
        let topo = MdTopology::framework(&mos2, FrameworkMode::Rigid).unwrap();
        let names: Vec<&str> = topo.species.iter().map(|s| s.element.as_str()).collect();
        assert!(names.contains(&"Mo") && names.contains(&"S"));
    }

    #[test]
    fn framework_rejects_unsupported_element() {
        let gold = ring(&["Au", "Au", "Au"]);
        let err = MdTopology::framework(&gold, FrameworkMode::Rigid)
            .unwrap_err()
            .to_string();
        assert!(err.contains("element `Au`"), "unexpected error: {err}");
    }

    #[test]
    fn duplicate_and_self_bonds_are_ignored() {
        // A defensive check: a stray self-bond or duplicate must not produce a
        // degenerate bonded term.
        let mut s = ring(&["C"; 4]);
        s.bonds.push(Bond::with_type(0, 0, BondType::Single));
        s.bonds.push(Bond::with_type(0, 1, BondType::Single));
        let topo = MdTopology::framework(&s, FrameworkMode::Flexible).unwrap();
        // The 4-ring still has exactly 4 unique bonds.
        assert_eq!(topo.molecules[0].bonds.len(), 4);
    }
}
