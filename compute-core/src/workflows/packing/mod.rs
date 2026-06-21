//! Build Disordered System — clash-free molecular packing.
//!
//! Packs `N` rigid copies of one or more molecule templates into a geometric
//! [`Region`] (box / periodic cell / sphere / cylinder, inside or outside) with
//! no atomic clashes, producing one combined [`Structure`]. The packing is
//! deterministic for a given seed, streams progress, and is cancelable — shaped
//! like [`crate::workflows::optimization::run_geometry_optimization`].
//!
//! The packer minimizes a smooth overlap-penalty objective by rigid-body
//! gradient descent with random restarts, using only the crate's existing seeded
//! `splitmix64` PRNG and a hand-rolled numeric-gradient optimizer — no external
//! dependencies.

mod assemble;
pub mod density;
mod engine;
pub mod region;

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::Result;

use crate::domain::{Structure, UnitCell};

pub use density::{count_for_concentration_molar, count_for_density_g_per_cm3};
pub use region::{Region, RegionSense};

/// One molecule type to pack: a rigid template and how many copies of it.
#[derive(Debug, Clone)]
pub struct PackSpecies {
    /// The molecule placed as a rigid body. Its bonds are replicated per copy.
    pub molecule: Structure,
    /// How many copies to pack.
    pub count: usize,
}

/// Stopping/step controls for the packer. The defaults suit interactive use:
/// generous restarts, a 30 s wall-clock budget, and conservative per-step caps.
#[derive(Debug, Clone)]
pub struct PackLimits {
    /// Maximum number of worst-copy re-seed restarts before giving up.
    pub max_restarts: usize,
    /// Maximum total gradient-descent steps across all restarts.
    pub max_steps: usize,
    /// Wall-clock budget; the run stops and reports a partial result past this.
    pub max_duration: Duration,
    /// Converged once the total penalty drops below this (≈ clash-free).
    pub penalty_tolerance: f32,
    /// Per-copy translation cap per accepted step (angstrom).
    pub max_translation_step: f32,
    /// Per-copy rotation cap per accepted step (radians).
    pub max_rotation_step: f32,
}

impl Default for PackLimits {
    fn default() -> Self {
        Self {
            max_restarts: 20,
            max_steps: 2000,
            max_duration: Duration::from_secs(30),
            penalty_tolerance: 1.0e-3,
            max_translation_step: 1.0,
            max_rotation_step: 0.3,
        }
    }
}

/// A complete packing request: the molecules, where to pack them, and how.
#[derive(Debug, Clone)]
pub struct PackRequest {
    /// The molecule types and their copy counts.
    pub species: Vec<PackSpecies>,
    /// The geometric region to fill.
    pub region: Region,
    /// Which side of the region to pack on.
    pub sense: RegionSense,
    /// Minimum allowed inter-molecular atom distance (angstrom).
    pub tolerance: f32,
    /// Use the minimum image for overlaps (seamless packing across box edges).
    pub periodic: bool,
    /// RNG seed; identical seeds give bit-for-bit identical packings.
    pub seed: u64,
    /// An immovable obstacle to pack around (its atoms are never moved).
    pub fixed: Option<Structure>,
    /// A unit cell to stamp on the result (e.g. when the region is the sim box).
    pub output_cell: Option<UnitCell>,
    /// Stopping/step controls.
    pub limits: PackLimits,
}

/// What a packing run achieved, surfaced honestly even when partial.
#[derive(Debug, Clone)]
pub struct PackReport {
    /// Copies of each species placed clash-free and in-region (parallel to
    /// `requested`).
    pub placed: Vec<usize>,
    /// Copies of each species requested (parallel to `placed`).
    pub requested: Vec<usize>,
    /// Worst-copy re-seed restarts used.
    pub restarts_used: usize,
    /// The final objective value (≈ 0 when fully packed).
    pub final_penalty: f32,
    /// The deepest remaining inter-molecular overlap (angstrom); 0 when clean.
    pub max_overlap: f32,
    /// Whether the packing reached the clash-free tolerance.
    pub converged: bool,
    /// Whether the run stopped on the wall-clock budget.
    pub timed_out: bool,
    /// Total gradient-descent steps taken.
    pub steps: usize,
}

impl PackReport {
    /// Total copies placed across all species.
    pub fn total_placed(&self) -> usize {
        self.placed.iter().sum()
    }

    /// Total copies requested across all species.
    pub fn total_requested(&self) -> usize {
        self.requested.iter().sum()
    }
}

/// A streamed intermediate packing state (the structure so far + its report).
#[derive(Debug)]
pub struct PackProgress {
    pub structure: Structure,
    pub report: PackReport,
}

/// The final packing result.
#[derive(Debug)]
pub struct PackResult {
    pub structure: Structure,
    pub report: PackReport,
}

/// Pack rigid copies of one or more molecules into a region without clashes.
///
/// Streams intermediate structures through `progress` and honors `cancel` and
/// the duration budget in `request.limits`. Deterministic for a given
/// `request.seed`. Shaped like
/// [`crate::workflows::optimization::run_geometry_optimization`].
pub fn pack(
    request: PackRequest,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(PackProgress) -> Result<()>,
) -> Result<PackResult> {
    engine::run(request, cancel, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType, Structure, UnitCell};
    use nalgebra::Point3;
    use std::sync::atomic::AtomicBool;

    /// Methane: a carbon with four tetrahedral hydrogens at ~1.09 Å.
    fn methane() -> Structure {
        let h = 1.09 / 3.0_f32.sqrt();
        Structure::with_bonds(
            "methane",
            vec![
                atom("C", 0.0, 0.0, 0.0),
                atom("H", h, h, h),
                atom("H", h, -h, -h),
                atom("H", -h, h, -h),
                atom("H", -h, -h, h),
            ],
            vec![
                Bond::with_type(0, 1, BondType::Single),
                Bond::with_type(0, 2, BondType::Single),
                Bond::with_type(0, 3, BondType::Single),
                Bond::with_type(0, 4, BondType::Single),
            ],
        )
    }

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn no_progress(_: PackProgress) -> Result<()> {
        Ok(())
    }

    /// The minimum distance between atoms of *different* copies (residue blocks
    /// of `residue_size`), optionally under the minimum image of `cell`.
    fn min_intermolecular(
        structure: &Structure,
        residue_size: usize,
        cell: Option<&UnitCell>,
    ) -> f32 {
        let atoms = &structure.atoms;
        let mut min = f32::INFINITY;
        for i in 0..atoms.len() {
            for j in (i + 1)..atoms.len() {
                if i / residue_size == j / residue_size {
                    continue;
                }
                let d = match cell {
                    Some(cell) => crate::domain::chemistry::nearest_periodic_delta(
                        cell,
                        atoms[i].position,
                        atoms[j].position,
                    )
                    .norm(),
                    None => (atoms[i].position - atoms[j].position).norm(),
                };
                min = min.min(d);
            }
        }
        min
    }

    #[test]
    fn packs_fifty_methane_into_a_cube_without_clashes() {
        let request = PackRequest {
            species: vec![PackSpecies {
                molecule: methane(),
                count: 50,
            }],
            region: Region::Box {
                min: Point3::origin(),
                max: Point3::new(40.0, 40.0, 40.0),
            },
            sense: RegionSense::Inside,
            tolerance: 2.0,
            periodic: false,
            seed: 7,
            fixed: None,
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(20),
                ..PackLimits::default()
            },
        };
        let result = pack(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert_eq!(result.structure.atoms.len(), 50 * 5);
        // Bonds are replicated per copy.
        assert_eq!(result.structure.bonds.len(), 50 * 4);
        let min = min_intermolecular(&result.structure, 5, None);
        assert!(
            min >= 2.0 - 0.25,
            "methane copies clash: min inter-molecular distance {min:.3} Å"
        );
        assert!(
            result.report.total_placed() >= 48,
            "only placed {}/50",
            result.report.total_placed()
        );
    }

    #[test]
    fn packs_a_two_component_mixture() {
        let request = PackRequest {
            species: vec![
                PackSpecies {
                    molecule: methane(),
                    count: 12,
                },
                PackSpecies {
                    molecule: Structure::new("Ar", vec![atom("Ar", 0.0, 0.0, 0.0)]),
                    count: 12,
                },
            ],
            region: Region::Box {
                min: Point3::origin(),
                max: Point3::new(30.0, 30.0, 30.0),
            },
            sense: RegionSense::Inside,
            tolerance: 2.0,
            periodic: false,
            seed: 3,
            fixed: None,
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(20),
                ..PackLimits::default()
            },
        };
        let result = pack(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert_eq!(result.structure.atoms.len(), 12 * 5 + 12);
        assert_eq!(result.report.placed.len(), 2);
    }

    #[test]
    fn packs_a_spherical_droplet() {
        let request = PackRequest {
            species: vec![PackSpecies {
                molecule: methane(),
                count: 20,
            }],
            region: Region::Sphere {
                center: Point3::new(0.0, 0.0, 0.0),
                radius: 14.0,
            },
            sense: RegionSense::Inside,
            tolerance: 2.0,
            periodic: false,
            seed: 5,
            fixed: None,
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(20),
                ..PackLimits::default()
            },
        };
        let result = pack(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        // Every atom lands within the droplet (plus the placement slack).
        for atom in &result.structure.atoms {
            assert!(
                atom.position.coords.norm() <= 14.0 + 1.5,
                "atom escaped the droplet: {:?}",
                atom.position
            );
        }
    }

    #[test]
    fn packs_periodically_without_clashes_across_edges() {
        let cell = UnitCell::from_parameters(25.0, 25.0, 25.0, 90.0, 90.0, 90.0);
        let request = PackRequest {
            species: vec![PackSpecies {
                molecule: Structure::new("Ar", vec![atom("Ar", 0.0, 0.0, 0.0)]),
                count: 40,
            }],
            region: Region::Cell(cell.clone()),
            sense: RegionSense::Inside,
            tolerance: 2.5,
            periodic: true,
            seed: 11,
            fixed: None,
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(20),
                ..PackLimits::default()
            },
        };
        let result = pack(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        assert_eq!(result.structure.atoms.len(), 40);
        // The result carries the periodic cell.
        assert!(result.structure.cell.is_some());
        // No clashes even under the minimum image (across box edges).
        let min = min_intermolecular(&result.structure, 1, Some(&cell));
        assert!(
            min >= 2.5 - 0.4,
            "periodic packing clashes across an edge: min image distance {min:.3} Å"
        );
    }

    #[test]
    fn packs_around_a_fixed_obstacle() {
        let obstacle = Structure::new("core", vec![atom("Fe", 15.0, 15.0, 15.0)]);
        let request = PackRequest {
            species: vec![PackSpecies {
                molecule: Structure::new("Ar", vec![atom("Ar", 0.0, 0.0, 0.0)]),
                count: 20,
            }],
            region: Region::Box {
                min: Point3::origin(),
                max: Point3::new(30.0, 30.0, 30.0),
            },
            sense: RegionSense::Inside,
            tolerance: 3.0,
            periodic: false,
            seed: 9,
            fixed: Some(obstacle),
            output_cell: None,
            limits: PackLimits {
                max_duration: Duration::from_secs(20),
                ..PackLimits::default()
            },
        };
        let result = pack(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
        // Obstacle first, then the packed argon.
        assert_eq!(result.structure.atoms.len(), 21);
        assert_eq!(result.structure.atoms[0].element, "Fe");
        let obstacle_pos = Point3::new(15.0, 15.0, 15.0);
        for atom in &result.structure.atoms[1..] {
            let d = (atom.position - obstacle_pos).norm();
            assert!(d >= 3.0 - 0.4, "a copy clashes with the obstacle: {d:.3} Å");
        }
    }
}
