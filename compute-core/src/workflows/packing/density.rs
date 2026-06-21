//! Convert a target mass density (g/cm³) or molar concentration (mol/L) into a
//! number of copies to pack into a region, given the region's volume.
//!
//! These let the UI/console offer "fill this box with water at 1.0 g/cm³" or
//! "0.15 mol/L NaCl" instead of forcing the user to count molecules. The counts
//! are exact for the closed-form region volume; the packer then reports honestly
//! if that many copies do not actually fit.

use crate::domain::{Structure, chemistry};

use super::region::Region;

/// Avogadro's number (mol⁻¹).
const AVOGADRO: f64 = 6.022_140_76e23;
/// Grams per unified atomic mass unit (1 u = 1/N_A g/mol).
const GRAMS_PER_U: f64 = 1.660_539_066_60e-24;

/// The molecule's molar mass in u, summing standard atomic weights. Unknown
/// elements contribute nothing (so a fully unknown molecule yields `0.0`).
pub fn molar_mass_u(molecule: &Structure) -> f32 {
    molecule
        .atoms
        .iter()
        .map(|atom| chemistry::atomic_mass(&atom.element).unwrap_or(0.0))
        .sum()
}

/// How many copies of `molecule` fill `region` at the target mass `density`
/// (g/cm³). Returns `0` for a non-positive density or an unknown-mass molecule.
pub fn count_for_density_g_per_cm3(molecule: &Structure, density: f32, region: &Region) -> usize {
    let mass_per_molecule_u = molar_mass_u(molecule) as f64;
    if density <= 0.0 || mass_per_molecule_u <= 0.0 {
        return 0;
    }
    // Å³ → cm³ (1 Å = 1e-8 cm ⇒ 1 Å³ = 1e-24 cm³).
    let volume_cm3 = region.volume_angstrom3() as f64 * 1.0e-24;
    let mass_per_molecule_g = mass_per_molecule_u * GRAMS_PER_U;
    let total_mass_g = density as f64 * volume_cm3;
    (total_mass_g / mass_per_molecule_g).round().max(0.0) as usize
}

/// How many copies of a solute give molar concentration `molar` (mol/L) in
/// `region`. The molecule's identity does not affect the count — only its
/// presence — so this also serves bare ions. Returns `0` for non-positive input.
pub fn count_for_concentration_molar(_molecule: &Structure, molar: f32, region: &Region) -> usize {
    if molar <= 0.0 {
        return 0;
    }
    // Å³ → L (1 Å³ = 1e-27 L).
    let volume_l = region.volume_angstrom3() as f64 * 1.0e-27;
    (molar as f64 * volume_l * AVOGADRO).round().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Structure};
    use nalgebra::Point3;

    fn water() -> Structure {
        Structure::with_bonds(
            "water",
            vec![
                Atom {
                    element: "O".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.96, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(-0.24, 0.93, 0.0),
                    charge: 0.0,
                },
            ],
            Vec::new(),
        )
    }

    #[test]
    fn water_molar_mass_is_about_18() {
        assert!((molar_mass_u(&water()) - 18.015).abs() < 0.01);
    }

    #[test]
    fn water_at_unit_density_matches_bulk_number_density() {
        // Bulk water is ~33.4 molecules/nm³ ⇒ ~901 in a 30 Å cube (27 nm³).
        let region = Region::Box {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(30.0, 30.0, 30.0),
        };
        let count = count_for_density_g_per_cm3(&water(), 1.0, &region);
        assert!(
            (880..=920).contains(&count),
            "expected ~901 waters, got {count}"
        );
    }

    #[test]
    fn concentration_scales_with_volume() {
        // 1 mol/L in a (100 Å)³ box (1e-21 L) ⇒ ~602 molecules.
        let region = Region::Box {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(100.0, 100.0, 100.0),
        };
        let count = count_for_concentration_molar(&water(), 1.0, &region);
        assert!((595..=610).contains(&count), "expected ~602, got {count}");
    }

    #[test]
    fn non_positive_inputs_yield_zero() {
        let region = Region::Sphere {
            center: Point3::origin(),
            radius: 10.0,
        };
        assert_eq!(count_for_density_g_per_cm3(&water(), 0.0, &region), 0);
        assert_eq!(count_for_concentration_molar(&water(), -1.0, &region), 0);
    }
}
