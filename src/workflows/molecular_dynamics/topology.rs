//! Engine-neutral description of an MD system's chemistry.
//!
//! An [`MdTopology`] captures *what the system is made of* — the distinct atom
//! types and their nonbonded parameters, the molecule types (each a list of
//! atoms plus any rigid-water constraint), and the molecule composition in
//! coordinate order — independent of any simulation engine. It is produced at
//! **system-build time** (the MD System Builder), persisted with the project, and
//! only translated into an engine-specific topology (each engine provides its own
//! renderer) when a calculation is launched. Keeping this representation
//! engine-neutral means a second MD engine can reuse the same data rather than
//! re-deriving it.
//!
//! [`MdTopology::from_structure`] describes monatomic Lennard-Jones systems from
//! [`crate::domain::nonbonded`] alone (e.g. argon); richer systems are
//! parameterized by an external engine from its own force field.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::{Structure, chemistry::normalized_symbol, nonbonded};

/// One distinct atom type, with its Lennard-Jones nonbonded parameters. Names are
/// type names (e.g. `Ar`, `OW`, `HW`, `Na`, `Cl`), not necessarily element
/// symbols. Per-atom partial charge lives on [`MoleculeAtom`]; `charge` here is
/// the atom type's default (usually 0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Species {
    pub element: String,
    pub atomic_number: u32,
    pub mass_u: f32,
    pub charge: f32,
    pub sigma_angstrom: f32,
    pub epsilon_kj_mol: f32,
}

/// One atom within a [`MoleculeType`], referencing an atom type by name and
/// carrying its partial charge and label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoleculeAtom {
    /// References [`Species::element`].
    pub species: String,
    /// Atom label within the molecule (e.g. `OW`, `HW1`, `Ar`, `NA`).
    pub atom_name: String,
    pub charge: f32,
    /// Residue name for this atom (e.g. `ALA`, `ACE`). `None` for single-residue
    /// molecule types (argon, water, ions), which fall back to the molecule
    /// name. Set for multi-residue molecule types such as a whole protein.
    #[serde(default)]
    pub residue_name: Option<String>,
    /// 1-based residue number within the molecule. `None` falls back to `1`.
    #[serde(default)]
    pub residue_number: Option<i32>,
}

impl MoleculeAtom {
    /// A single-residue molecule atom (argon/water/ion), with no per-atom
    /// residue labelling.
    pub fn new(species: impl Into<String>, atom_name: impl Into<String>, charge: f32) -> Self {
        Self {
            species: species.into(),
            atom_name: atom_name.into(),
            charge,
            residue_name: None,
            residue_number: None,
        }
    }
}

/// Rigid three-site water geometry (a SETTLE constraint): the O–H and H–H
/// distances. Present only on water molecule types.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SettleGeometry {
    pub doh_angstrom: f32,
    pub dhh_angstrom: f32,
}

/// One bonded interaction term within a [`MoleculeType`]: the 1-based local atom
/// indices it connects and the functional-form code of the potential. Parameters
/// are **not** stored here — they are looked up by the simulation engine's
/// topology preprocessor from the force-field parameter tables
/// ([`MdTopology::bonded_params`]), keyed by atom type, the way a standard
/// force-field topology references its parameter set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BondedTerm {
    /// 1-based atom indices local to the molecule type.
    pub atoms: Vec<u32>,
    /// Functional-form code for the interaction potential, following the common
    /// force-field topology convention (1 = harmonic bond/angle and 1-4 pair,
    /// 9 = multiple proper dihedral, 4 = improper dihedral).
    pub func: i32,
}

/// A molecule type: an ordered list of atoms plus optional bonded terms and an
/// optional rigid-water constraint. A monatomic species (argon) and an ion are
/// single-atom molecule types; water is three atoms with a [`SettleGeometry`]; a
/// whole protein is one molecule type carrying bonds/angles/dihedrals/impropers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoleculeType {
    /// Molecule/residue name (e.g. `AR`, `SOL`, `NA`, `CL`).
    pub name: String,
    /// Number of bonded neighbors excluded from nonbonded interactions.
    pub nrexcl: u32,
    pub atoms: Vec<MoleculeAtom>,
    pub settle: Option<SettleGeometry>,
    #[serde(default)]
    pub bonds: Vec<BondedTerm>,
    /// 1-4 nonbonded pairs (the ends of proper dihedrals).
    #[serde(default)]
    pub pairs: Vec<BondedTerm>,
    #[serde(default)]
    pub angles: Vec<BondedTerm>,
    /// Proper (torsional) dihedrals.
    #[serde(default)]
    pub dihedrals: Vec<BondedTerm>,
    /// Improper dihedrals.
    #[serde(default)]
    pub impropers: Vec<BondedTerm>,
    /// Explicit per-atom nonbonded exclusions: `exclusions[i]` lists the 1-based
    /// local atom indices excluded from interacting with atom `i+1`. Used by a
    /// bond-free rigid framework (where there are no bonds for grompp to derive
    /// exclusions from); empty for molecule types that rely on `nrexcl`.
    #[serde(default)]
    pub exclusions: Vec<Vec<u32>>,
}

impl MoleculeType {
    /// A single-atom molecule (a monatomic species or an ion).
    pub fn monatomic(name: impl Into<String>, species: impl Into<String>, charge: f32) -> Self {
        let name = name.into();
        let species = species.into();
        Self {
            atoms: vec![MoleculeAtom::new(species, name.clone(), charge)],
            name,
            nrexcl: 1,
            settle: None,
            bonds: Vec::new(),
            pairs: Vec::new(),
            angles: Vec::new(),
            dihedrals: Vec::new(),
            impropers: Vec::new(),
            exclusions: Vec::new(),
        }
    }

    /// Whether this molecule type carries explicit bonded terms (a protein),
    /// versus a monatomic/rigid-water type that needs none.
    pub fn has_bonded_terms(&self) -> bool {
        !self.bonds.is_empty()
            || !self.angles.is_empty()
            || !self.dihedrals.is_empty()
            || !self.impropers.is_empty()
    }

    /// Net charge of one molecule of this type.
    pub fn net_charge(&self) -> f32 {
        self.atoms.iter().map(|a| a.charge).sum()
    }
}

/// A run of identical molecules, preserving coordinate order so an engine's
/// molecule list lines up with the coordinate file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoleculeRun {
    /// References [`MoleculeType::name`].
    pub molecule: String,
    pub count: usize,
}

/// Force-field nonbonded defaults: the combination rule plus how 1-4
/// interactions are handled. Present when a force field is involved (bonded
/// topology); `None` keeps the historical monatomic defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TopologyDefaults {
    /// Lennard-Jones combination rule (the standard 1/2/3 numbering).
    pub comb_rule: u8,
    /// Whether 1-4 LJ pairs are generated from the atom types (scaled by
    /// `fudge_lj`) rather than read from explicit pair parameters.
    pub gen_pairs: bool,
    pub fudge_lj: f32,
    pub fudge_qq: f32,
}

/// One row of a force-field parameter table (a bond, angle, dihedral, 1-4 pair,
/// or constraint parameter entry), kept engine-neutral so any engine's renderer
/// can emit it. The bonded [`BondedTerm`]s reference these by atom type at
/// preprocess time rather than carrying parameters inline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BondedParam {
    /// Parameter-table kind: `bondtypes` | `angletypes` | `dihedraltypes` |
    /// `pairtypes` | `constrainttypes`.
    pub kind: String,
    /// Space-joined atom-type pattern (may contain `X` wildcards).
    pub atoms: String,
    pub func: Option<i64>,
    pub params: Vec<f64>,
}

/// Engine-neutral topology: atom types, molecule types, and composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdTopology {
    pub title: String,
    /// Distinct atom types, in first-seen order.
    pub species: Vec<Species>,
    /// Molecule types referenced by the composition.
    pub molecules: Vec<MoleculeType>,
    /// Molecule runs in coordinate order.
    pub composition: Vec<MoleculeRun>,
    /// Force-field nonbonded defaults; `None` uses the monatomic-system defaults.
    #[serde(default)]
    pub defaults: Option<TopologyDefaults>,
    /// Force-field parameter tables needed to resolve the bonded terms. Empty
    /// for monatomic/water/ion systems.
    #[serde(default)]
    pub bonded_params: Vec<BondedParam>,
    /// Raw force-field text inserted verbatim after `[defaults]` (a
    /// user-supplied custom force field's `[atomtypes]`/`[bondtypes]`/…). Kept
    /// inline rather than as an `#include` so the rendered `.top` stays
    /// self-contained and portable across run directories. `None` for fully
    /// built-in systems; it must not contain its own `[defaults]`.
    #[serde(default)]
    pub inline_force_field: Option<String>,
}

impl MdTopology {
    /// Whether [`Self::from_structure`] can describe `structure` without an
    /// external force field: a non-empty, bond-free system whose every atom is a
    /// tabulated monatomic species.
    pub fn can_build(structure: &Structure) -> bool {
        !structure.atoms.is_empty()
            && structure.bonds.is_empty()
            && structure
                .atoms
                .iter()
                .all(|a| nonbonded::lennard_jones(&a.element).is_some())
    }

    /// Build the engine-neutral topology for a monatomic Lennard-Jones system.
    ///
    /// Errors (with an actionable message) when the structure is empty, has
    /// bonds (needs a real force field), or contains an element with no
    /// tabulated parameters.
    pub fn from_structure(structure: &Structure) -> Result<Self> {
        if structure.atoms.is_empty() {
            bail!("cannot build a topology for a structure with no atoms");
        }
        if !structure.bonds.is_empty() {
            bail!(
                "automatic topology generation supports only monatomic systems (the structure \
                 has bonds); supply a topology explicitly instead"
            );
        }

        let elements: Vec<String> = structure
            .atoms
            .iter()
            .map(|a| normalized_symbol(&a.element))
            .collect();

        let mut species: Vec<Species> = Vec::new();
        let mut molecules: Vec<MoleculeType> = Vec::new();
        for element in &elements {
            if species.iter().any(|s| &s.element == element) {
                continue;
            }
            let lj = nonbonded::lennard_jones(element).ok_or_else(|| {
                anyhow::anyhow!(
                    "no nonbonded parameters for element `{element}`; supported elements are \
                     {:?}. Supply a topology explicitly instead.",
                    nonbonded::supported_elements()
                )
            })?;
            species.push(Species {
                element: element.clone(),
                atomic_number: lj.atomic_number,
                mass_u: lj.mass_u,
                charge: 0.0,
                sigma_angstrom: lj.sigma_angstrom,
                epsilon_kj_mol: lj.epsilon_kj_mol,
            });
            molecules.push(MoleculeType::monatomic(
                molecule_name(element),
                element.clone(),
                0.0,
            ));
        }

        // Run-length encode consecutive identical elements in coordinate order.
        let mut composition: Vec<MoleculeRun> = Vec::new();
        let mut i = 0;
        while i < elements.len() {
            let element = &elements[i];
            let mut count = 1;
            while i + count < elements.len() && &elements[i + count] == element {
                count += 1;
            }
            composition.push(MoleculeRun {
                molecule: molecule_name(element),
                count,
            });
            i += count;
        }

        let title = structure.title.lines().next().unwrap_or("").trim();
        Ok(Self {
            title: if title.is_empty() {
                "SilicoLab system".to_string()
            } else {
                title.to_string()
            },
            species,
            molecules,
            composition,
            defaults: None,
            bonded_params: Vec::new(),
            inline_force_field: None,
        })
    }

    /// Look up a molecule type by name.
    pub fn molecule(&self, name: &str) -> Option<&MoleculeType> {
        self.molecules.iter().find(|m| m.name == name)
    }

    /// Total number of atoms described, which must match the coordinate file.
    pub fn atom_count(&self) -> usize {
        self.composition
            .iter()
            .map(|run| self.molecule(&run.molecule).map_or(0, |m| m.atoms.len()) * run.count)
            .sum()
    }

    /// Net charge of the whole system (sum over the composition).
    pub fn net_charge(&self) -> f32 {
        self.composition
            .iter()
            .map(|run| {
                self.molecule(&run.molecule).map_or(0.0, |m| m.net_charge()) * run.count as f32
            })
            .sum()
    }

    /// Ensure an atom type is present (idempotent by name), adding it if new.
    pub fn ensure_species(&mut self, species: Species) {
        if !self.species.iter().any(|s| s.element == species.element) {
            self.species.push(species);
        }
    }

    /// Ensure a molecule type is present (idempotent by name), adding it if new.
    pub fn ensure_molecule(&mut self, molecule: MoleculeType) {
        if !self.molecules.iter().any(|m| m.name == molecule.name) {
            self.molecules.push(molecule);
        }
    }

    /// Append (or extend) a run of `molecule` to the composition. Consecutive
    /// runs of the same molecule are merged.
    pub fn push_run(&mut self, molecule: &str, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(last) = self.composition.last_mut()
            && last.molecule == molecule
        {
            last.count += count;
            return;
        }
        self.composition.push(MoleculeRun {
            molecule: molecule.to_string(),
            count,
        });
    }

    /// Persist as JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("serializing MD topology")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Load a previously persisted topology from JSON.
    pub fn load(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
    }
}

/// Molecule/residue name for a monatomic element (uppercased symbol), e.g.
/// `Ar` -> `AR`.
pub fn molecule_name(element: &str) -> String {
    element.to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::*;
    use crate::domain::{Atom, Bond, BondType, UnitCell};

    fn argon(n: usize) -> Structure {
        let atoms = (0..n)
            .map(|i| Atom {
                element: "Ar".to_string(),
                position: Point3::new(i as f32 * 5.0, 0.0, 0.0),
                charge: 0.0,
            })
            .collect();
        Structure::with_cell(
            "argon",
            atoms,
            UnitCell::from_parameters(100.0, 100.0, 100.0, 90.0, 90.0, 90.0),
        )
    }

    #[test]
    fn argon_topology_has_one_species_one_molecule_and_one_run() {
        let topo = MdTopology::from_structure(&argon(8)).unwrap();
        assert_eq!(topo.species.len(), 1);
        assert_eq!(topo.species[0].element, "Ar");
        assert_eq!(topo.molecules.len(), 1);
        assert_eq!(topo.molecules[0].name, "AR");
        assert_eq!(topo.molecules[0].atoms.len(), 1);
        assert_eq!(
            topo.composition,
            vec![MoleculeRun {
                molecule: "AR".to_string(),
                count: 8,
            }]
        );
        assert_eq!(topo.atom_count(), 8);
        assert!(topo.net_charge().abs() < 1e-6);
    }

    #[test]
    fn mixed_species_runs_preserve_order() {
        let atoms = vec![
            Atom {
                element: "Ar".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "Ar".to_string(),
                position: Point3::new(5.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "Ne".to_string(),
                position: Point3::new(10.0, 0.0, 0.0),
                charge: 0.0,
            },
        ];
        let structure = Structure::with_cell(
            "mix",
            atoms,
            UnitCell::from_parameters(100.0, 100.0, 100.0, 90.0, 90.0, 90.0),
        );
        let topo = MdTopology::from_structure(&structure).unwrap();
        assert_eq!(topo.species.len(), 2);
        assert_eq!(topo.molecules.len(), 2);
        assert_eq!(
            topo.composition,
            vec![
                MoleculeRun {
                    molecule: "AR".to_string(),
                    count: 2,
                },
                MoleculeRun {
                    molecule: "NE".to_string(),
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn bonds_are_rejected() {
        let mut structure = argon(2);
        structure.bonds = vec![Bond::with_type(0, 1, BondType::Single)];
        assert!(!MdTopology::can_build(&structure));
        let err = MdTopology::from_structure(&structure)
            .unwrap_err()
            .to_string();
        assert!(err.contains("monatomic"), "unexpected error: {err}");
    }

    #[test]
    fn unsupported_element_is_rejected() {
        let structure = Structure::with_cell(
            "carbon",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            UnitCell::from_parameters(100.0, 100.0, 100.0, 90.0, 90.0, 90.0),
        );
        let err = MdTopology::from_structure(&structure)
            .unwrap_err()
            .to_string();
        assert!(err.contains("element `C`"), "unexpected error: {err}");
    }

    #[test]
    fn push_run_merges_consecutive_and_counts_atoms() {
        let mut topo = MdTopology::from_structure(&argon(4)).unwrap();
        // A three-atom water molecule type.
        topo.ensure_molecule(MoleculeType {
            name: "SOL".to_string(),
            nrexcl: 2,
            atoms: vec![
                MoleculeAtom::new("OW", "OW", -0.82),
                MoleculeAtom::new("HW", "HW1", 0.41),
                MoleculeAtom::new("HW", "HW2", 0.41),
            ],
            settle: Some(SettleGeometry {
                doh_angstrom: 1.0,
                dhh_angstrom: 1.633,
            }),
            bonds: Vec::new(),
            pairs: Vec::new(),
            angles: Vec::new(),
            dihedrals: Vec::new(),
            impropers: Vec::new(),
            exclusions: Vec::new(),
        });
        topo.push_run("SOL", 10);
        topo.push_run("SOL", 5);
        assert_eq!(topo.composition.last().unwrap().count, 15);
        // 4 argon + 15 waters * 3 atoms = 49.
        assert_eq!(topo.atom_count(), 4 + 15 * 3);
        assert!(topo.net_charge().abs() < 1e-4);
    }

    #[test]
    fn legacy_json_without_bonded_fields_still_loads() {
        // A topology serialized before B3 (no defaults/bonded_params, molecule
        // atoms without residue labels, molecule types without bonded terms)
        // must still deserialize, with the new fields defaulting.
        let legacy = r#"{
            "title": "argon",
            "species": [
                {"element":"Ar","atomic_number":18,"mass_u":39.95,"charge":0.0,
                 "sigma_angstrom":3.405,"epsilon_kj_mol":0.996}
            ],
            "molecules": [
                {"name":"AR","nrexcl":1,
                 "atoms":[{"species":"Ar","atom_name":"AR","charge":0.0}],
                 "settle":null}
            ],
            "composition": [{"molecule":"AR","count":4}]
        }"#;
        let topo: MdTopology = serde_json::from_str(legacy).expect("legacy json loads");
        assert!(topo.defaults.is_none());
        assert!(topo.bonded_params.is_empty());
        let mol = &topo.molecules[0];
        assert!(!mol.has_bonded_terms());
        assert!(mol.atoms[0].residue_name.is_none());
        assert!(mol.atoms[0].residue_number.is_none());
        assert_eq!(topo.atom_count(), 4);
    }

    #[test]
    fn round_trips_through_json() {
        let topo = MdTopology::from_structure(&argon(4)).unwrap();
        let dir = std::env::temp_dir().join("silicolab_md_topology_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("system_topology.json");
        topo.save(&path).unwrap();
        let loaded = MdTopology::load(&path).unwrap();
        assert_eq!(topo, loaded);
    }
}
