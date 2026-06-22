//! Geometric, force-field-free solvation: fill a built MD box with water and add
//! ions as bare element-labelled coordinates.
//!
//! This is a lightweight, in-process box filler. Rigid three-point water
//! molecules are placed on a regular lattice at bulk number density, oriented
//! per-site to avoid systematic alignment, and any that overlap the solute are
//! discarded. Ions then replace a spread-out subset of waters to neutralize the
//! system and reach a target salt concentration. It assigns **no force-field
//! parameters and builds no topology** — only coordinates with element labels;
//! the simulation engine parameterizes the system from its own force field.

use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Vector3};
use serde::{Deserialize, Serialize};

use crate::domain::{AppendedResidue, Atom, Structure, extend_biopolymer_coverage};

/// Rigid three-point water geometry used for placement (SPC-like), in angstrom.
const WATER_DOH_ANGSTROM: f32 = 1.0;
const WATER_DHH_ANGSTROM: f32 = 1.633;
/// Lennard-Jones diameter (angstrom) of a water oxygen, for the clash test.
const WATER_OXYGEN_SIGMA_ANGSTROM: f32 = 3.166;
/// Generic solute-atom contact diameter (angstrom) for the clash test. No
/// per-atom force-field radii are available in this layer, so one value is used
/// for every solute atom.
const SOLUTE_CONTACT_SIGMA_ANGSTROM: f32 = 3.5;

/// Bulk water number density at ambient conditions (molecules per nm^3).
const WATER_NUMBER_DENSITY_PER_NM3: f32 = 33.4;
/// Avogadro's number, for converting a molar concentration to an ion count.
const AVOGADRO: f64 = 6.022_140_76e23;

/// The solvent water model token to fill the box with. Placement uses a generic
/// rigid three-point water geometry regardless of the choice; the model is
/// metadata a simulation engine uses when it later parameterizes the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WaterModel {
    /// TIP 4-point.
    Tip4p,
    /// TIP 4-point with Ewald.
    Tip4pEw,
    /// TIP 3-point.
    Tip3p,
    /// TIP 5-point.
    Tip5p,
    /// TIP 5-point improved for Ewald sums.
    Tip5pEwald,
    /// Simple Point Charge.
    #[default]
    Spc,
    /// Extended Simple Point Charge.
    SpcE,
}

impl WaterModel {
    /// All water models selectable in the UI, in menu order.
    pub fn all() -> &'static [WaterModel] {
        &[
            Self::Tip4p,
            Self::Tip4pEw,
            Self::Tip3p,
            Self::Tip5p,
            Self::Tip5pEwald,
            Self::Spc,
            Self::SpcE,
        ]
    }

    /// Short label for UI menus (title only).
    pub fn label(self) -> &'static str {
        match self {
            Self::Tip4p => "TIP4P",
            Self::Tip4pEw => "TIP4PEW",
            Self::Tip3p => "TIP3P",
            Self::Tip5p => "TIP5P",
            Self::Tip5pEwald => "TIP5P (Ewald)",
            Self::Spc => "SPC",
            Self::SpcE => "SPC/E",
        }
    }

    /// The engine selector token for this model (e.g. a value passed to a
    /// topology generator).
    pub fn db_token(self) -> &'static str {
        match self {
            Self::Tip4p => "tip4p",
            Self::Tip4pEw => "tip4pew",
            Self::Tip3p => "tip3p",
            Self::Tip5p => "tip5p",
            Self::Tip5pEwald => "tip5pe",
            Self::Spc => "spc",
            Self::SpcE => "spce",
        }
    }
}

/// How to solvate and ionize a built MD box. Ion names (`positive_ion`,
/// `negative_ion`) become the element labels of the placed ions (e.g. `NA`,
/// `CL`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolvationOptions {
    pub water: WaterModel,
    pub positive_ion: String,
    pub negative_ion: String,
    /// Add the minimum ions needed to make the system net-neutral.
    pub neutralize: bool,
    /// Target salt concentration in mol/L; `None` adds only neutralizing ions.
    pub concentration_molar: Option<f32>,
}

impl Default for SolvationOptions {
    /// SPC water neutralized with NaCl.
    fn default() -> Self {
        Self {
            water: WaterModel::Spc,
            positive_ion: "NA".to_string(),
            negative_ion: "CL".to_string(),
            neutralize: true,
            concentration_molar: None,
        }
    }
}

impl SolvationOptions {
    /// SPC water with a physiological 0.15 mol/L NaCl bath, neutralized.
    pub fn physiological_saline() -> Self {
        Self {
            concentration_molar: Some(0.15),
            ..Self::default()
        }
    }
}

/// What [`solvate`] added to the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolvationReport {
    pub water_added: usize,
    pub cations_added: usize,
    pub anions_added: usize,
}

/// A non-destructive preview of how many water molecules and ions [`solvate`]
/// would add for the given solute and options, without assembling coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolvationEstimate {
    pub water: usize,
    pub cations: usize,
    pub anions: usize,
}

/// Solvate a built MD box: wrap `solute` (which must already have a periodic
/// cell from the MD System Builder) in water and ions per `options`.
///
/// Returns the solvated structure (solute atoms first, then water, then cations,
/// then anions) and a report of what was added. Added atoms carry only element
/// labels and coordinates — no charges or parameters.
pub fn solvate(
    solute: &Structure,
    options: &SolvationOptions,
) -> Result<(Structure, SolvationReport)> {
    let plan = plan_solvation(solute, options)?;

    let water_canonical = canonical_water(WATER_DOH_ANGSTROM, WATER_DHH_ANGSTROM);
    let solute_atom_count = solute.atoms.len();
    let mut atoms: Vec<Atom> = solute.atoms.clone();
    let mut water_count = 0usize;
    let mut cations: Vec<Point3<f32>> = Vec::with_capacity(plan.n_cat);
    let mut anions: Vec<Point3<f32>> = Vec::with_capacity(plan.n_an);
    // Residue/atom-name metadata for the appended solvent, so the solvated
    // structure's biopolymer keeps covering every atom (water → SOL, ions →
    // their own residues). Without this, residue lookups (water detection,
    // category quick-select, cartoon) break on solvated systems.
    let mut appended_residues: Vec<AppendedResidue> = Vec::new();

    // Waters first (so they form one contiguous block), in site order.
    for (index, center) in plan.sites.iter().enumerate() {
        match plan.ion_kinds[index] {
            Some(IonKind::Cation) => cations.push(*center),
            Some(IonKind::Anion) => anions.push(*center),
            None => {
                let rot = site_rotation(index);
                let base = atoms.len();
                for (offset, &element) in water_canonical.iter().zip(["O", "H", "H"].iter()) {
                    atoms.push(Atom {
                        element: element.to_string(),
                        position: center + rot * offset,
                        charge: 0.0,
                    });
                }
                water_count += 1;
                appended_residues.push(AppendedResidue {
                    residue_name: "SOL".to_string(),
                    chain_id: 'W',
                    sequence_number: water_count as i32,
                    atoms: vec![
                        (base, "OW".to_string()),
                        (base + 1, "HW1".to_string()),
                        (base + 2, "HW2".to_string()),
                    ],
                });
            }
        }
    }

    let mut ion_sequence = 0i32;
    let cation_element = ion_element(&options.positive_ion);
    let cation_residue = options.positive_ion.trim().to_ascii_uppercase();
    for center in &cations {
        let index = atoms.len();
        atoms.push(Atom {
            element: cation_element.clone(),
            position: *center,
            charge: 0.0,
        });
        ion_sequence += 1;
        appended_residues.push(AppendedResidue {
            residue_name: cation_residue.clone(),
            chain_id: 'I',
            sequence_number: ion_sequence,
            atoms: vec![(index, cation_residue.clone())],
        });
    }
    let anion_element = ion_element(&options.negative_ion);
    let anion_residue = options.negative_ion.trim().to_ascii_uppercase();
    for center in &anions {
        let index = atoms.len();
        atoms.push(Atom {
            element: anion_element.clone(),
            position: *center,
            charge: 0.0,
        });
        ion_sequence += 1;
        appended_residues.push(AppendedResidue {
            residue_name: anion_residue.clone(),
            chain_id: 'I',
            sequence_number: ion_sequence,
            atoms: vec![(index, anion_residue.clone())],
        });
    }

    let total_atom_count = atoms.len();
    let biopolymer = extend_biopolymer_coverage(
        solute.biopolymer.as_ref(),
        solute_atom_count,
        total_atom_count,
        &appended_residues,
    );

    let solvated = Structure {
        title: solute.title.clone(),
        atoms,
        bonds: Vec::new(),
        cell: Some(plan.cell.clone()),
        biopolymer,
    };

    let report = SolvationReport {
        water_added: water_count,
        cations_added: plan.n_cat,
        anions_added: plan.n_an,
    };
    Ok((solvated, report))
}

/// Preview how many water molecules and ions [`solvate`] would add, without
/// assembling coordinates. Shares the planning logic with [`solvate`], so the
/// numbers are exact.
pub fn estimate(solute: &Structure, options: &SolvationOptions) -> Result<SolvationEstimate> {
    let plan = plan_solvation(solute, options)?;
    Ok(SolvationEstimate {
        water: plan.sites.len() - plan.n_cat - plan.n_an,
        cations: plan.n_cat,
        anions: plan.n_an,
    })
}

/// The resolved placement plan shared by [`solvate`] and [`estimate`]: the kept
/// grid sites and the per-site ion assignment. Computing this is the expensive
/// part (grid fill + solute-clash test); assembling coordinates from it is cheap.
struct SolvationPlan {
    cell: crate::domain::UnitCell,
    sites: Vec<Point3<f32>>,
    ion_kinds: Vec<Option<IonKind>>,
    n_cat: usize,
    n_an: usize,
}

fn plan_solvation(solute: &Structure, options: &SolvationOptions) -> Result<SolvationPlan> {
    let cell = solute.cell.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot solvate a structure with no periodic cell; build the MD system first"
        )
    })?;

    // --- 1. Grid-fill candidate water sites -----------------------------------
    let vectors = cell.vectors;
    let spacing_a = (1.0 / (WATER_NUMBER_DENSITY_PER_NM3 * 1e-3)).cbrt(); // per-A^3
    let counts = [
        (vectors[0].norm() / spacing_a).round().max(1.0) as usize,
        (vectors[1].norm() / spacing_a).round().max(1.0) as usize,
        (vectors[2].norm() / spacing_a).round().max(1.0) as usize,
    ];

    let clearance_sq = {
        let d = 0.5 * (SOLUTE_CONTACT_SIGMA_ANGSTROM + WATER_OXYGEN_SIGMA_ANGSTROM);
        d * d
    };

    let mut sites: Vec<Point3<f32>> = Vec::new();
    for i in 0..counts[0] {
        for j in 0..counts[1] {
            for k in 0..counts[2] {
                let frac = [
                    (i as f32 + 0.5) / counts[0] as f32,
                    (j as f32 + 0.5) / counts[1] as f32,
                    (k as f32 + 0.5) / counts[2] as f32,
                ];
                let r = vectors[0] * frac[0] + vectors[1] * frac[1] + vectors[2] * frac[2];
                let center = Point3::from(r);
                if !overlaps_solute(&center, solute, cell, clearance_sq) {
                    sites.push(center);
                }
            }
        }
    }

    // --- 2. Decide ion counts -------------------------------------------------
    let volume_a3 = vectors[0].dot(&vectors[1].cross(&vectors[2])).abs() as f64;
    let volume_l = volume_a3 * 1e-27; // A^3 -> L (1 A^3 = 1e-27 L)
    let salt_pairs = options
        .concentration_molar
        .map(|c| (c as f64 * volume_l * AVOGADRO).round() as usize)
        .unwrap_or(0);

    // Net solute charge from the per-atom partial charges (no force field needed).
    let solute_charge = solute.atoms.iter().map(|a| a.charge).sum::<f32>().round() as i64;
    let mut n_cat = salt_pairs;
    let mut n_an = salt_pairs;
    if options.neutralize {
        if solute_charge > 0 {
            n_an += solute_charge as usize;
        } else if solute_charge < 0 {
            n_cat += (-solute_charge) as usize;
        }
    }

    let n_ion = n_cat + n_an;
    if n_ion > sites.len() {
        bail!(
            "not enough room: solvation produced {} water sites but {} ions were requested; \
             use a larger box or lower concentration",
            sites.len(),
            n_ion
        );
    }

    let ion_kinds = assign_ion_sites(sites.len(), n_cat, n_an);

    Ok(SolvationPlan {
        cell: cell.clone(),
        sites,
        ion_kinds,
        n_cat,
        n_an,
    })
}

#[derive(Clone, Copy)]
enum IonKind {
    Cation,
    Anion,
}

/// Whether a candidate water-oxygen site sits within contact distance of any
/// solute atom (a single generic clearance is used, as no per-atom radii exist
/// in this layer). The distance is the minimum image under the cell's
/// periodicity, so a site near a cell boundary is tested against the nearest
/// periodic image of each solute atom — essential for a tight periodic slab (a
/// nanosheet), whose in-plane lattice is only a few angstroms wide.
fn overlaps_solute(
    center: &Point3<f32>,
    solute: &Structure,
    cell: &crate::domain::UnitCell,
    clearance_sq: f32,
) -> bool {
    solute
        .atoms
        .iter()
        .any(|atom| min_image_delta(cell, center - atom.position).norm_squared() < clearance_sq)
}

/// The minimum-image displacement of `delta` under the cell's periodicity: each
/// fractional component is wrapped into [-0.5, 0.5] before converting back to a
/// Cartesian vector.
fn min_image_delta(cell: &crate::domain::UnitCell, delta: Vector3<f32>) -> Vector3<f32> {
    let frac = cell.cartesian_to_fractional(Point3::from(delta));
    let wrap = |f: f32| f - f.round();
    cell.vectors[0] * wrap(frac.x) + cell.vectors[1] * wrap(frac.y) + cell.vectors[2] * wrap(frac.z)
}

/// Element label for a placed ion, from its (force-field) residue name. Maps the
/// common monatomic ions and otherwise title-cases the name.
fn ion_element(name: &str) -> String {
    match name.trim().to_ascii_uppercase().as_str() {
        "NA" => "Na".to_string(),
        "CL" => "Cl".to_string(),
        "K" => "K".to_string(),
        "MG" => "Mg".to_string(),
        "CA" => "Ca".to_string(),
        "LI" => "Li".to_string(),
        "ZN" => "Zn".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => "X".to_string(),
            }
        }
    }
}

/// Pick `n_cat + n_an` of `n_sites` sites, spread evenly, alternating cation and
/// anion assignment so opposite charges are interleaved.
fn assign_ion_sites(n_sites: usize, n_cat: usize, n_an: usize) -> Vec<Option<IonKind>> {
    let mut kinds = vec![None; n_sites];
    let n_ion = n_cat + n_an;
    if n_ion == 0 || n_ion > n_sites {
        return kinds;
    }
    let mut sequence = Vec::with_capacity(n_ion);
    let (mut c, mut a) = (n_cat, n_an);
    let mut want_cat = true;
    while c + a > 0 {
        if want_cat && c > 0 {
            sequence.push(IonKind::Cation);
            c -= 1;
        } else if a > 0 {
            sequence.push(IonKind::Anion);
            a -= 1;
        } else {
            sequence.push(IonKind::Cation);
            c -= 1;
        }
        want_cat = !want_cat;
    }
    for (k, kind) in sequence.into_iter().enumerate() {
        let idx = ((k * n_sites + n_sites / 2) / n_ion).min(n_sites - 1);
        kinds[idx] = Some(kind);
    }
    kinds
}

/// Canonical water coordinates (angstrom) with the oxygen at the origin and the
/// two hydrogens in the xy-plane, from the O–H and H–H distances.
fn canonical_water(doh_a: f32, dhh_a: f32) -> [Vector3<f32>; 3] {
    let sin_half = (dhh_a / (2.0 * doh_a)).clamp(-1.0, 1.0);
    let cos_half = (1.0 - sin_half * sin_half).sqrt();
    [
        Vector3::new(0.0, 0.0, 0.0),
        Vector3::new(doh_a * cos_half, doh_a * sin_half, 0.0),
        Vector3::new(doh_a * cos_half, -doh_a * sin_half, 0.0),
    ]
}

/// A deterministic per-site orientation, so a regular grid of water does not
/// share one alignment.
fn site_rotation(index: usize) -> Rotation3<f32> {
    use std::f32::consts::TAU;
    let h = splitmix64(index as u64);
    let angle = |shift: u32| ((h >> shift) & 0xffff) as f32 / 65535.0 * TAU;
    Rotation3::from_euler_angles(angle(0), angle(16), angle(32))
}

pub fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Atom;
    use crate::md::{MdSystemConfig, build_md_system, system::BoxShape};
    use nalgebra::Point3;

    fn argon_box(edge_a: f32) -> Structure {
        let atoms = vec![
            Atom {
                element: "Ar".into(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "Ar".into(),
                position: Point3::new(4.0, 0.0, 0.0),
                charge: 0.0,
            },
        ];
        let raw = Structure::new("argon", atoms);
        let config = MdSystemConfig::with_absolute_edges([edge_a; 3], BoxShape::Cubic);
        build_md_system(&raw, &config).unwrap().0
    }

    #[test]
    fn min_image_delta_wraps_across_a_cell_boundary() {
        use crate::domain::UnitCell;
        let cell = UnitCell::from_vectors([
            Vector3::new(8.0, 0.0, 0.0),
            Vector3::new(0.0, 8.0, 0.0),
            Vector3::new(0.0, 0.0, 8.0),
        ]);
        // A near-full-cell displacement is small under the minimum image.
        let wrapped = min_image_delta(&cell, Vector3::new(7.9, 0.0, 0.0));
        assert!(wrapped.norm() < 0.2, "did not wrap: {}", wrapped.norm());
    }

    #[test]
    fn clash_test_sees_a_solute_atom_across_the_periodic_boundary() {
        use crate::domain::UnitCell;
        let cell = UnitCell::from_vectors([
            Vector3::new(8.0, 0.0, 0.0),
            Vector3::new(0.0, 8.0, 0.0),
            Vector3::new(0.0, 0.0, 8.0),
        ]);
        // Solute atom hugs the x=0 face; a candidate site hugs the x=L face. In
        // raw Cartesian they are ~7.8 A apart (no clash), but their minimum-image
        // separation is ~0.4 A, which must register as an overlap.
        let solute = Structure::with_cell(
            "edge",
            vec![Atom {
                element: "C".into(),
                position: Point3::new(0.1, 4.0, 4.0),
                charge: 0.0,
            }],
            cell.clone(),
        );
        let site = Point3::new(7.9, 4.0, 4.0);
        let clearance_sq = 9.0; // 3 A contact
        assert!(overlaps_solute(&site, &solute, &cell, clearance_sq));
    }

    #[test]
    fn solvates_argon_with_neutral_water_box() {
        let solute = argon_box(25.0);
        let opts = SolvationOptions {
            neutralize: true,
            concentration_molar: None,
            ..SolvationOptions::default()
        };
        let (solvated, report) = solvate(&solute, &opts).unwrap();

        assert!(
            report.water_added > 100,
            "got {} waters",
            report.water_added
        );
        assert_eq!(report.cations_added, 0);
        assert_eq!(report.anions_added, 0);
        // Solute argon atoms come first and are unchanged.
        assert_eq!(&solvated.atoms[0].element, "Ar");
        assert_eq!(&solvated.atoms[1].element, "Ar");
        // Each added water is three atoms (O, H, H), so the count is consistent.
        assert_eq!(
            solvated.atoms.len(),
            solute.atoms.len() + report.water_added * 3
        );
    }

    #[test]
    fn solvation_keeps_biopolymer_covering_all_atoms() {
        use crate::domain::AtomCategory;
        let solute = argon_box(30.0);
        let opts = SolvationOptions::physiological_saline();
        let (solvated, _) = solvate(&solute, &opts).unwrap();

        // The biopolymer now spans every atom, so residue lookups work again.
        let biopolymer = solvated.biopolymer.as_ref().expect("solvated biopolymer");
        assert!(biopolymer.is_compatible_with_atom_count(solvated.atoms.len()));

        // The first appended atom is a water oxygen (waters precede ions).
        let first_added = solute.atoms.len();
        assert_eq!(solvated.atom_category(first_added), AtomCategory::Solvent);

        // Added Na/Cl classify as ions; the original argon stays unclassified.
        let ion_index = solvated
            .atoms
            .iter()
            .position(|a| a.element == "Na" || a.element == "Cl")
            .expect("an ion was added");
        assert_eq!(solvated.atom_category(ion_index), AtomCategory::Ion);
        assert_eq!(solvated.atom_category(0), AtomCategory::Other);
    }

    #[test]
    fn no_water_clashes_with_solute() {
        let solute = argon_box(25.0);
        let (solvated, _) = solvate(&solute, &SolvationOptions::default()).unwrap();
        let solute_n = solute.atoms.len();
        for added in &solvated.atoms[solute_n..] {
            for ar in &solute.atoms {
                let d = (added.position - ar.position).norm();
                assert!(d > 2.0, "solvent atom only {d:.2} A from solute");
            }
        }
    }

    #[test]
    fn adds_nacl_for_concentration() {
        let solute = argon_box(30.0);
        let opts = SolvationOptions::physiological_saline();
        let (solvated, report) = solvate(&solute, &opts).unwrap();

        // 0.15 M in ~27 nm^3 adds a few NaCl pairs; neutral argon keeps them equal.
        assert!(report.cations_added >= 1, "no cations added");
        assert_eq!(report.cations_added, report.anions_added);
        // Ions are placed as Na/Cl element labels at the end.
        assert!(solvated.atoms.iter().any(|a| a.element == "Na"));
        assert!(solvated.atoms.iter().any(|a| a.element == "Cl"));
    }

    #[test]
    fn neutralizes_charged_solute() {
        // A solute carrying net +2 should draw two extra anions.
        let mut solute = argon_box(30.0);
        solute.atoms[0].charge = 2.0;
        let base = estimate(&solute, &SolvationOptions::default()).unwrap();
        assert_eq!(base.anions, 2);
        assert_eq!(base.cations, 0);
    }

    #[test]
    fn estimate_matches_solvate_counts() {
        let solute = argon_box(30.0);
        let opts = SolvationOptions::physiological_saline();
        let preview = estimate(&solute, &opts).unwrap();
        let (_, report) = solvate(&solute, &opts).unwrap();
        assert_eq!(preview.water, report.water_added);
        assert_eq!(preview.cations, report.cations_added);
        assert_eq!(preview.anions, report.anions_added);
    }
}
