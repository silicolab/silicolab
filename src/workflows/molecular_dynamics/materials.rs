//! Engine-neutral force-field data for periodic covalent framework materials
//! (2D nanosheets such as graphene, hexagonal boron nitride, silicene,
//! transition-metal dichalcogenides and graphitic carbon nitride).
//!
//! The MD System Builder already parameterizes a noble-gas system from
//! [`crate::domain::nonbonded`] and a biomolecular system from a full force
//! field. Neither covers a covalent framework: a nanosheet has no residue
//! template, and its atoms are not noble gases. This module supplies the
//! missing chemistry so a framework topology can be generated directly from the
//! structure's own bonds:
//!
//! * [`atom_type`] gives every supported element a Lennard-Jones atom type. This
//!   is all a **rigid** (frozen-framework) model needs — the sheet contributes
//!   only nonbonded interactions.
//! * [`flexible_force_field`] gives the bonded parameters (bonds, angles,
//!   Ryckaert-Bellemans dihedrals) for a **flexible** model. These are only
//!   well-grounded for the aromatic-carbon family, so it returns `None` for any
//!   material it cannot parameterize, letting the caller fall back to the rigid
//!   model with a clear message.
//!
//! Carbon/hydrogen use the OPLS-AA aromatic parameters; the remaining elements
//! use Universal Force Field (UFF) Lennard-Jones values. Parameter quality
//! varies, so each atom type carries a [`Coverage`] flag the UI surfaces as a
//! warning.

use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::domain::Structure;
use crate::domain::chemistry::normalized_symbol;

use super::solvation::WaterModel;
use super::topology::{
    BondedParam, MoleculeAtom, MoleculeType, SettleGeometry, Species, TopologyDefaults,
};

/// How trustworthy a material's force-field parameters are, so the UI can warn
/// before a user runs a simulation on weakly-parameterized chemistry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Coverage {
    /// Published, validated parameters (aromatic carbon/hydrogen).
    Good,
    /// Generic transferable parameters (UFF) — usable but unvalidated for this
    /// material.
    Approximate,
    /// Generic parameters for chemistry these force fields were not designed for
    /// (transition-metal dichalcogenides); treat results as qualitative.
    Poor,
}

impl Coverage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Good => "Good",
            Self::Approximate => "Approximate",
            Self::Poor => "Poor",
        }
    }

    /// The lower (more cautionary) of two coverages, for summarizing a whole
    /// structure from its per-element values.
    pub fn worst(self, other: Self) -> Self {
        self.max(other)
    }
}

/// A Lennard-Jones atom type for one framework element: its atom-type name
/// (kept distinct from the bare element symbol so bonded-parameter patterns
/// match unambiguously, e.g. aromatic carbon is `CJ`), nonbonded parameters in
/// `domain` units (angstrom / kJ·mol⁻¹), and a coverage flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialAtomType {
    pub type_name: &'static str,
    pub atomic_number: u32,
    pub mass_u: f32,
    pub sigma_angstrom: f32,
    pub epsilon_kj_mol: f32,
    pub coverage: Coverage,
}

/// kcal·mol⁻¹ → kJ·mol⁻¹.
const KCAL_TO_KJ: f32 = 4.184;
/// UFF tabulates the nonbonded distance as the well-minimum separation x; the
/// Lennard-Jones collision diameter is sigma = x / 2^(1/6).
const UFF_X_TO_SIGMA: f32 = 0.890_898_7; // 1 / 2^(1/6)

/// Build an atom type from UFF nonbonded data (well-minimum distance `x` in
/// angstrom, well depth `depth` in kcal·mol⁻¹).
const fn uff(
    type_name: &'static str,
    atomic_number: u32,
    mass_u: f32,
    x_angstrom: f32,
    depth_kcal: f32,
    coverage: Coverage,
) -> MaterialAtomType {
    MaterialAtomType {
        type_name,
        atomic_number,
        mass_u,
        sigma_angstrom: x_angstrom * UFF_X_TO_SIGMA,
        epsilon_kj_mol: depth_kcal * KCAL_TO_KJ,
        coverage,
    }
}

/// The Lennard-Jones atom type for `element`, or `None` if the element has no
/// framework parameters. Case-insensitive in the element symbol.
pub fn atom_type(element: &str) -> Option<MaterialAtomType> {
    Some(match normalized_symbol(element).as_str() {
        // OPLS-AA aromatic carbon/hydrogen (validated for graphene and carbon
        // nanostructures): sigma 0.355/0.242 nm, epsilon 0.29288/0.12552 kJ/mol.
        "C" => MaterialAtomType {
            type_name: "CJ",
            atomic_number: 6,
            mass_u: 12.011,
            sigma_angstrom: 3.55,
            epsilon_kj_mol: 0.292_88,
            coverage: Coverage::Good,
        },
        "H" => MaterialAtomType {
            type_name: "HJ",
            atomic_number: 1,
            mass_u: 1.008,
            sigma_angstrom: 2.42,
            epsilon_kj_mol: 0.125_52,
            coverage: Coverage::Good,
        },
        // UFF Lennard-Jones for the remaining framework elements.
        "B" => uff("B", 5, 10.811, 4.083, 0.180, Coverage::Approximate),
        "N" => uff("N", 7, 14.007, 3.660, 0.069, Coverage::Approximate),
        "Si" => uff("Si", 14, 28.085, 4.295, 0.402, Coverage::Approximate),
        "S" => uff("S", 16, 32.06, 4.035, 0.274, Coverage::Poor),
        "Se" => uff("Se", 34, 78.971, 4.205, 0.291, Coverage::Poor),
        "Te" => uff("Te", 52, 127.60, 4.470, 0.398, Coverage::Poor),
        "Mo" => uff("Mo", 42, 95.95, 3.052, 0.056, Coverage::Poor),
        "W" => uff("W", 74, 183.84, 3.069, 0.067, Coverage::Poor),
        _ => return None,
    })
}

/// Elements with framework Lennard-Jones parameters, in periodic-table order.
pub fn supported_elements() -> &'static [&'static str] {
    &["B", "C", "N", "H", "Si", "S", "Se", "Te", "Mo", "W"]
}

/// The atom-type names a user-supplied force field defines, kept engine-neutral
/// (the engine-specific force-field parsing lives in the engine layer). An element is
/// covered by the custom force field when its symbol — or, for an override, its
/// built-in type name — appears here. By convention a custom atom type is named
/// after its element symbol (e.g. `Pt` parameterizes platinum).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomTypes {
    pub names: BTreeSet<String>,
}

impl CustomTypes {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

/// How an element is parameterized for a framework build, given any custom types.
#[derive(Debug, Clone, PartialEq)]
pub enum ElementParameterization {
    /// A built-in type. Its Lennard-Jones [`Species`] is emitted unless
    /// `overridden` — the custom force field redefines the same type name, so the
    /// `#include` provides it and emitting it too would duplicate the atom type.
    BuiltIn {
        atom_type: MaterialAtomType,
        overridden: bool,
    },
    /// Covered only by the custom force field: the type name is the element
    /// symbol and no built-in `Species` is emitted (the `#include` provides it).
    Custom { type_name: String },
}

impl ElementParameterization {
    /// The atom-type name to reference in the molecule's `[atoms]`.
    pub fn type_name(&self) -> &str {
        match self {
            Self::BuiltIn { atom_type, .. } => atom_type.type_name,
            Self::Custom { type_name } => type_name,
        }
    }
}

/// Resolve how `element` is parameterized, preferring the built-in tables and
/// falling back to a user-supplied custom type named after the element symbol.
/// Returns `None` when neither covers the element.
pub fn parameterize_element(
    element: &str,
    custom: &CustomTypes,
) -> Option<ElementParameterization> {
    let symbol = normalized_symbol(element);
    if let Some(atom_type) = atom_type(&symbol) {
        let overridden = custom.contains(atom_type.type_name);
        return Some(ElementParameterization::BuiltIn {
            atom_type,
            overridden,
        });
    }
    if custom.contains(&symbol) {
        return Some(ElementParameterization::Custom { type_name: symbol });
    }
    None
}

/// Whether `structure` has the shape of a covalent framework — a periodic cell,
/// bonds, and no biopolymer metadata — regardless of whether its chemistry is
/// parameterized. This is the structural gate; coverage is checked separately so
/// a user can supply missing parameters.
pub fn is_framework_shape(structure: &Structure) -> bool {
    structure.cell.is_some()
        && !structure.bonds.is_empty()
        && structure.biopolymer.is_none()
        && !structure.atoms.is_empty()
}

/// Whether `structure` is a framework whose every element is parameterized by the
/// built-in tables plus `custom`. This routes a structure to the framework
/// (nanosheet) MD path rather than the biomolecular `pdb2gmx` path.
pub fn is_framework_with_custom(structure: &Structure, custom: &CustomTypes) -> bool {
    is_framework_shape(structure)
        && structure
            .atoms
            .iter()
            .all(|a| parameterize_element(&a.element, custom).is_some())
}

/// Whether `structure` is a framework fully covered by the built-in tables alone.
pub fn is_framework(structure: &Structure) -> bool {
    is_framework_with_custom(structure, &CustomTypes::default())
}

/// Distinct element symbols in `structure` that have no built-in parameters but
/// are supplied by `custom`, in first-seen order. The UI surfaces these so the
/// user knows which chemistry rests entirely on their own force field.
pub fn user_provided_elements(structure: &Structure, custom: &CustomTypes) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for atom in &structure.atoms {
        if atom_type(&atom.element).is_none() {
            let symbol = normalized_symbol(&atom.element);
            if custom.contains(&symbol) && !out.contains(&symbol) {
                out.push(symbol);
            }
        }
    }
    out
}

/// Distinct element symbols in `structure` that neither the built-in tables nor
/// `custom` parameterize — the chemistry a user must still supply before the
/// structure can be built. Empty when the structure is fully covered.
pub fn unparameterized_elements(structure: &Structure, custom: &CustomTypes) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for atom in &structure.atoms {
        if parameterize_element(&atom.element, custom).is_none() {
            let symbol = normalized_symbol(&atom.element);
            if !out.contains(&symbol) {
                out.push(symbol);
            }
        }
    }
    out
}

/// The worst (most cautionary) parameter coverage across a structure's elements,
/// or `None` if any element has no framework parameters. The UI surfaces this as
/// a warning before a user runs MD on weakly-parameterized chemistry.
pub fn framework_coverage(structure: &Structure) -> Option<Coverage> {
    if structure.atoms.is_empty() {
        return None;
    }
    structure.atoms.iter().try_fold(Coverage::Good, |worst, a| {
        atom_type(&a.element).map(|t| worst.worst(t.coverage))
    })
}

/// Whether a flexible (bonded) model is available for `structure` — i.e. every
/// element is in the aromatic-carbon family. When false, only the rigid model
/// applies.
pub fn supports_flexible(structure: &Structure) -> bool {
    let mut types: Vec<&str> = Vec::new();
    for atom in &structure.atoms {
        match atom_type(&atom.element) {
            Some(t) => {
                if !types.contains(&t.type_name) {
                    types.push(t.type_name);
                }
            }
            None => return false,
        }
    }
    !types.is_empty() && flexible_force_field(&types).is_some()
}

/// A complete flexible (bonded) force field for a framework: the nonbonded
/// defaults plus the bonded parameter table grompp resolves the index-only
/// bonded terms against.
#[derive(Debug, Clone, PartialEq)]
pub struct FlexibleForceField {
    pub defaults: TopologyDefaults,
    pub bonded_params: Vec<BondedParam>,
}

/// The OPLS-AA nonbonded defaults the carbon force field uses (combination rule
/// 3, generated 1-4 pairs scaled by 0.5).
fn opls_defaults() -> TopologyDefaults {
    TopologyDefaults {
        comb_rule: 3,
        gen_pairs: true,
        fudge_lj: 0.5,
        fudge_qq: 0.5,
    }
}

fn param(kind: &str, atoms: &str, func: i64, params: &[f64]) -> BondedParam {
    BondedParam {
        kind: kind.to_string(),
        atoms: atoms.to_string(),
        func: Some(func),
        params: params.to_vec(),
    }
}

/// The flexible bonded force field for a framework whose atom types are
/// `present_types`, or `None` if any present type lacks bonded parameters (i.e.
/// the material can only be modeled rigidly). Only the aromatic-carbon family
/// (`CJ`/`HJ`) is supported, matching the OPLS-AA carbon-nanostructure
/// parameter set; everything else returns `None`.
pub fn flexible_force_field(present_types: &[&str]) -> Option<FlexibleForceField> {
    if !present_types.iter().all(|t| matches!(*t, "CJ" | "HJ")) {
        return None;
    }

    // OPLS-AA aromatic carbon: harmonic bonds/angles (func 1) and a
    // Ryckaert-Bellemans ring dihedral (func 3). Bond force constants are in
    // kJ·mol⁻¹·nm⁻², angle constants in kJ·mol⁻¹·rad⁻², lengths in nm.
    let mut bonded_params = vec![
        param("bondtypes", "CJ CJ", 1, &[0.140, 392_459.2]),
        param("angletypes", "CJ CJ CJ", 1, &[120.0, 527.184]),
        // A single wildcard ring dihedral covers every aromatic CJ–CJ torsion.
        param(
            "dihedraltypes",
            "X CJ CJ X",
            3,
            &[30.334, 0.0, -30.334, 0.0, 0.0, 0.0],
        ),
    ];
    if present_types.contains(&"HJ") {
        bonded_params.push(param("bondtypes", "CJ HJ", 1, &[0.108, 307_105.6]));
        bonded_params.push(param("angletypes", "CJ CJ HJ", 1, &[120.0, 292.88]));
        bonded_params.push(param("angletypes", "HJ CJ HJ", 1, &[117.0, 292.88]));
    }

    Some(FlexibleForceField {
        defaults: opls_defaults(),
        bonded_params,
    })
}

/// The atom types and molecule types defining a solvent (water + ions) to merge
/// into a self-contained framework topology, so a renderer can emit `SOL`/ion
/// definitions and `gmx solvate`/`genion` can reference them by name.
#[derive(Debug, Clone, PartialEq)]
pub struct SolventDefinitions {
    pub species: Vec<Species>,
    pub molecules: Vec<MoleculeType>,
}

/// Build the water + ion definitions for a self-contained framework topology.
///
/// Only the three-point water models (SPC, SPC/E, TIP3P) are supported, because
/// the geometric solvation fills with a three-point solvent box; four/five-point
/// models error. Only the common monatomic ions (Na⁺, K⁺, Cl⁻) are
/// parameterized; another ion name errors with a clear message.
pub fn solvent_definitions(
    water: WaterModel,
    cation: &str,
    anion: &str,
) -> Result<SolventDefinitions> {
    let w = water_params(water).ok_or_else(|| {
        anyhow::anyhow!(
            "material solvation supports only three-point water (SPC, SPC/E, TIP3P); {} is not \
             a three-point model",
            water.label()
        )
    })?;

    let mut species = vec![
        Species {
            element: "OW".to_string(),
            atomic_number: 8,
            mass_u: 15.999_4,
            charge: 0.0,
            sigma_angstrom: w.ow_sigma_angstrom,
            epsilon_kj_mol: w.ow_epsilon_kj_mol,
        },
        Species {
            element: "HW".to_string(),
            atomic_number: 1,
            mass_u: 1.008,
            charge: 0.0,
            sigma_angstrom: 0.0,
            epsilon_kj_mol: 0.0,
        },
    ];

    let mut molecules = vec![MoleculeType {
        name: "SOL".to_string(),
        nrexcl: 2,
        atoms: vec![
            MoleculeAtom::new("OW", "OW", w.ow_charge),
            MoleculeAtom::new("HW", "HW1", w.hw_charge),
            MoleculeAtom::new("HW", "HW2", w.hw_charge),
        ],
        settle: Some(SettleGeometry {
            doh_angstrom: w.doh_angstrom,
            dhh_angstrom: w.dhh_angstrom,
        }),
        bonds: Vec::new(),
        pairs: Vec::new(),
        angles: Vec::new(),
        dihedrals: Vec::new(),
        impropers: Vec::new(),
        exclusions: Vec::new(),
    }];

    for name in [cation, anion] {
        let ion = ion_params(name)?;
        species.push(Species {
            element: ion.type_name.to_string(),
            atomic_number: ion.atomic_number,
            mass_u: ion.mass_u,
            charge: 0.0,
            sigma_angstrom: ion.sigma_angstrom,
            epsilon_kj_mol: ion.epsilon_kj_mol,
        });
        molecules.push(MoleculeType::monatomic(
            ion.type_name,
            ion.type_name,
            ion.charge,
        ));
    }

    Ok(SolventDefinitions { species, molecules })
}

/// Three-point water parameters in `domain` units (angstrom / kJ·mol⁻¹).
struct WaterParams {
    ow_sigma_angstrom: f32,
    ow_epsilon_kj_mol: f32,
    ow_charge: f32,
    hw_charge: f32,
    doh_angstrom: f32,
    dhh_angstrom: f32,
}

fn water_params(model: WaterModel) -> Option<WaterParams> {
    Some(match model {
        WaterModel::Spc => WaterParams {
            ow_sigma_angstrom: 3.165_57,
            ow_epsilon_kj_mol: 0.650_17,
            ow_charge: -0.82,
            hw_charge: 0.41,
            doh_angstrom: 1.0,
            dhh_angstrom: 1.633,
        },
        WaterModel::SpcE => WaterParams {
            ow_sigma_angstrom: 3.165_57,
            ow_epsilon_kj_mol: 0.650_17,
            ow_charge: -0.847_6,
            hw_charge: 0.423_8,
            doh_angstrom: 1.0,
            dhh_angstrom: 1.633,
        },
        WaterModel::Tip3p => WaterParams {
            ow_sigma_angstrom: 3.150_61,
            ow_epsilon_kj_mol: 0.636_4,
            ow_charge: -0.834,
            hw_charge: 0.417,
            doh_angstrom: 0.957_2,
            dhh_angstrom: 1.513_9,
        },
        // Four/five-point models are not three-point and have no SPC-box solvent.
        WaterModel::Tip4p | WaterModel::Tip4pEw | WaterModel::Tip5p | WaterModel::Tip5pEwald => {
            return None;
        }
    })
}

/// A monatomic ion's atom type and charge.
struct IonParams {
    type_name: &'static str,
    atomic_number: u32,
    mass_u: f32,
    sigma_angstrom: f32,
    epsilon_kj_mol: f32,
    charge: f32,
}

/// Parameters for a monatomic ion by residue name (e.g. `NA`, `CL`, `K`). The
/// Lennard-Jones values are the standard OPLS aqueous-ion set.
fn ion_params(name: &str) -> Result<IonParams> {
    Ok(match name.trim().to_ascii_uppercase().as_str() {
        "NA" => IonParams {
            type_name: "NA",
            atomic_number: 11,
            mass_u: 22.99,
            sigma_angstrom: 3.328_4,
            epsilon_kj_mol: 0.011_59,
            charge: 1.0,
        },
        "K" => IonParams {
            type_name: "K",
            atomic_number: 19,
            mass_u: 39.098,
            sigma_angstrom: 4.934_6,
            epsilon_kj_mol: 0.001_37,
            charge: 1.0,
        },
        "CL" => IonParams {
            type_name: "CL",
            atomic_number: 17,
            mass_u: 35.45,
            sigma_angstrom: 4.417_2,
            epsilon_kj_mol: 0.492_8,
            charge: -1.0,
        },
        other => {
            bail!("ion `{other}` is not parameterized for material solvation; use NA, K or CL")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carbon_uses_validated_aromatic_parameters() {
        let c = atom_type("C").expect("carbon supported");
        assert_eq!(c.type_name, "CJ");
        assert_eq!(c.atomic_number, 6);
        assert!((c.sigma_angstrom - 3.55).abs() < 1e-4);
        assert!((c.epsilon_kj_mol - 0.292_88).abs() < 1e-5);
        assert_eq!(c.coverage, Coverage::Good);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(atom_type("mo").map(|t| t.type_name), Some("Mo"));
        assert_eq!(atom_type("MO").map(|t| t.type_name), Some("Mo"));
    }

    #[test]
    fn uff_distance_is_converted_to_a_collision_diameter() {
        // UFF x for sulfur is 4.035 A; sigma = x / 2^(1/6) is smaller.
        let s = atom_type("S").expect("sulfur supported");
        assert!(s.sigma_angstrom < 4.035);
        assert!((s.sigma_angstrom - 4.035 * UFF_X_TO_SIGMA).abs() < 1e-4);
        // Depth 0.274 kcal/mol converts to kJ/mol.
        assert!((s.epsilon_kj_mol - 0.274 * KCAL_TO_KJ).abs() < 1e-4);
    }

    #[test]
    fn unsupported_element_returns_none() {
        assert!(atom_type("Au").is_none());
    }

    #[test]
    fn every_supported_element_resolves() {
        for element in supported_elements() {
            assert!(atom_type(element).is_some(), "missing {element}");
        }
    }

    #[test]
    fn carbon_family_has_a_flexible_force_field() {
        let ff = flexible_force_field(&["CJ"]).expect("carbon is flexible");
        assert_eq!(ff.defaults.comb_rule, 3);
        assert!(ff.defaults.gen_pairs);
        assert!(
            ff.bonded_params
                .iter()
                .any(|p| p.kind == "bondtypes" && p.atoms == "CJ CJ")
        );
        assert!(
            ff.bonded_params
                .iter()
                .any(|p| p.kind == "dihedraltypes" && p.atoms == "X CJ CJ X")
        );
    }

    #[test]
    fn hydrogen_edges_add_their_bonded_terms() {
        let ff = flexible_force_field(&["CJ", "HJ"]).expect("carbon+hydrogen is flexible");
        assert!(
            ff.bonded_params
                .iter()
                .any(|p| p.kind == "bondtypes" && p.atoms == "CJ HJ")
        );
    }

    #[test]
    fn non_carbon_has_no_flexible_force_field() {
        // Transition-metal dichalcogenides and the like can only be modeled
        // rigidly: no bonded parameters are returned.
        assert!(flexible_force_field(&["Mo", "S"]).is_none());
        assert!(flexible_force_field(&["B", "N"]).is_none());
    }

    #[test]
    fn solvent_definitions_provide_water_and_ions() {
        let defs = solvent_definitions(WaterModel::Spc, "NA", "CL").expect("SPC + NaCl");
        // OW, HW, NA, CL atom types.
        let names: Vec<&str> = defs.species.iter().map(|s| s.element.as_str()).collect();
        assert!(names.contains(&"OW") && names.contains(&"HW"));
        assert!(names.contains(&"NA") && names.contains(&"CL"));
        // SOL is a rigid three-site water; ions are single-atom molecules.
        let sol = defs.molecules.iter().find(|m| m.name == "SOL").unwrap();
        assert!(sol.settle.is_some());
        assert_eq!(sol.atoms.len(), 3);
        assert!((sol.net_charge()).abs() < 1e-4, "water is neutral");
    }

    #[test]
    fn four_point_water_and_unknown_ions_are_rejected() {
        assert!(solvent_definitions(WaterModel::Tip4p, "NA", "CL").is_err());
        assert!(solvent_definitions(WaterModel::Spc, "ZN", "CL").is_err());
    }

    #[test]
    fn framework_detection_and_coverage() {
        use crate::domain::{Atom, Bond, BondType, Structure, UnitCell};
        use nalgebra::Point3;

        let cell = UnitCell::from_parameters(10.0, 10.0, 20.0, 90.0, 90.0, 90.0);
        let carbon = Structure::with_cell_and_bonds(
            "c",
            vec![
                Atom {
                    element: "C".into(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".into(),
                    position: Point3::new(1.4, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell.clone(),
        );
        assert!(is_framework(&carbon));
        assert_eq!(framework_coverage(&carbon), Some(Coverage::Good));
        assert!(supports_flexible(&carbon));

        // A TMD is a framework but only rigidly modelable, with poor coverage.
        let mos2 = Structure::with_cell_and_bonds(
            "mos2",
            vec![
                Atom {
                    element: "Mo".into(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "S".into(),
                    position: Point3::new(1.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
            cell,
        );
        assert!(is_framework(&mos2));
        assert_eq!(framework_coverage(&mos2), Some(Coverage::Poor));
        assert!(!supports_flexible(&mos2));
    }

    #[test]
    fn non_framework_structures_are_rejected() {
        use crate::domain::{Atom, Structure};
        use nalgebra::Point3;
        // No cell, no bonds.
        let loose = Structure::new(
            "x",
            vec![Atom {
                element: "C".into(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        );
        assert!(!is_framework(&loose));
    }

    #[test]
    fn coverage_worst_is_the_more_cautionary() {
        assert_eq!(Coverage::Good.worst(Coverage::Poor), Coverage::Poor);
        assert_eq!(
            Coverage::Approximate.worst(Coverage::Good),
            Coverage::Approximate
        );
    }
}
