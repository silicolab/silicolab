use super::*;
use crate::domain::{Atom, Structure};
use crate::workflows::packing::{PackLimits, PackSpecies};
use nalgebra::Point3;

use super::super::region::RegionSense;

fn argon() -> Structure {
    Structure::new(
        "Ar",
        vec![Atom {
            element: "Ar".to_string(),
            position: Point3::origin(),
            charge: 0.0,
        }],
    )
}

fn box_region(edge: f32) -> Region {
    Region::Box {
        min: Point3::origin(),
        max: Point3::new(edge, edge, edge),
    }
}

fn base_request(species: Vec<PackSpecies>, region: Region) -> PackRequest {
    PackRequest {
        species,
        region,
        sense: RegionSense::Inside,
        tolerance: 2.0,
        periodic: false,
        seed: 1,
        fixed: None,
        output_cell: None,
        limits: PackLimits {
            max_duration: Duration::from_secs(5),
            ..PackLimits::default()
        },
    }
}

fn no_progress(_: PackProgress) -> Result<()> {
    Ok(())
}

fn min_pair_distance(structure: &Structure, residue_size: usize) -> f32 {
    let mut min = f32::INFINITY;
    let atoms = &structure.atoms;
    for i in 0..atoms.len() {
        for j in (i + 1)..atoms.len() {
            // Skip intra-molecule pairs (same residue block).
            if i / residue_size == j / residue_size {
                continue;
            }
            let d = (atoms[i].position - atoms[j].position).norm();
            min = min.min(d);
        }
    }
    min
}

#[test]
fn overlapping_single_atoms_separate_to_tolerance() {
    let request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 8,
        }],
        box_region(12.0),
    );
    let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    assert_eq!(result.structure.atoms.len(), 8);
    let min = min_pair_distance(&result.structure, 1);
    assert!(
        min >= 2.0 - PLACE_TOL,
        "atoms still overlap: min distance {min:.3}"
    );
}

#[test]
fn same_seed_is_bit_for_bit_reproducible() {
    let make = || {
        base_request(
            vec![PackSpecies {
                molecule: argon(),
                count: 10,
            }],
            box_region(15.0),
        )
    };
    let a = run(make(), Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    let b = run(make(), Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    assert_eq!(a.structure.atoms.len(), b.structure.atoms.len());
    for (x, y) in a.structure.atoms.iter().zip(&b.structure.atoms) {
        assert_eq!(x.position, y.position, "packing is not deterministic");
    }
}

#[test]
fn different_seed_gives_a_different_packing() {
    let mut request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 10,
        }],
        box_region(15.0),
    );
    let a = run(
        request.clone(),
        Arc::new(AtomicBool::new(false)),
        no_progress,
    )
    .unwrap();
    request.seed = 2;
    let b = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    let differs = a
        .structure
        .atoms
        .iter()
        .zip(&b.structure.atoms)
        .any(|(x, y)| x.position != y.position);
    assert!(differs, "different seeds produced identical packings");
}

#[test]
fn all_atoms_land_inside_the_region() {
    let request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 12,
        }],
        box_region(14.0),
    );
    let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    for atom in &result.structure.atoms {
        let p = atom.position;
        assert!(
            (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.x)
                && (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.y)
                && (-PLACE_TOL..=14.0 + PLACE_TOL).contains(&p.z),
            "atom escaped the box: {p:?}"
        );
    }
}

#[test]
fn tiny_region_is_rejected() {
    let request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 1,
        }],
        box_region(0.5),
    );
    let err = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap_err();
    assert!(err.to_string().contains("too small"));
}

#[test]
fn periodic_box_result_carries_a_cell_without_output_cell() {
    let mut request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 6,
        }],
        box_region(16.0),
    );
    request.periodic = true;
    request.output_cell = None;
    let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    // A periodic box must stamp its cell, or the min-image spacing reads as
    // cross-edge clashes downstream.
    assert!(result.structure.cell.is_some());
}

#[test]
fn outside_a_box_is_rejected() {
    let mut request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 4,
        }],
        box_region(20.0),
    );
    request.sense = RegionSense::Outside;
    let err = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap_err();
    assert!(err.to_string().contains("sphere or cylinder"), "got: {err}");
}

#[test]
fn zero_count_is_a_no_op() {
    let request = base_request(
        vec![PackSpecies {
            molecule: argon(),
            count: 0,
        }],
        box_region(10.0),
    );
    let result = run(request, Arc::new(AtomicBool::new(false)), no_progress).unwrap();
    assert!(result.structure.atoms.is_empty());
    assert!(result.report.converged);
}
