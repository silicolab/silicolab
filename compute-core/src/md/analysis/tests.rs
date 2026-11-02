use nalgebra::{Point3, Vector3};

use super::*;
use crate::domain::{Atom, BondType, Trajectory, UnitCell};
use crate::md::solvation::splitmix64;

fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
    Atom {
        element: element.to_string(),
        position: Point3::new(x, y, z),
        charge: 0.0,
    }
}

fn cubic(length: f32) -> UnitCell {
    UnitCell::from_parameters(length, length, length, 90.0, 90.0, 90.0)
}

fn trajectory_from(frames: &[Vec<Point3<f32>>], dt: f32, cell: Option<&UnitCell>) -> Trajectory {
    let natoms = frames[0].len();
    let coords: Vec<f32> = frames
        .iter()
        .flat_map(|frame| frame.iter().flat_map(|p| [p.x, p.y, p.z]))
        .collect();
    let times: Vec<f32> = (0..frames.len()).map(|i| i as f32 * dt).collect();
    let trajectory = Trajectory::from_parts(natoms, coords, times, 1, frames.len());
    match cell {
        Some(cell) => trajectory.with_boxes(vec![cell.vectors; frames.len()]),
        None => trajectory,
    }
}

#[test]
fn selection_by_element_residue_and_all() {
    let mut structure = Structure::new(
        "t",
        vec![
            atom("C", 0.0, 0.0, 0.0),
            atom("H", 1.0, 0.0, 0.0),
            atom("O", 2.0, 0.0, 0.0),
        ],
    );
    assert_eq!(select_atoms(&structure, "all").unwrap(), vec![0, 1, 2]);
    assert_eq!(select_atoms(&structure, "h").unwrap(), vec![1]);
    let err = select_atoms(&structure, "BEN").unwrap_err().to_string();
    assert!(err.contains("no residues"), "{err}");

    structure.biopolymer = Some(crate::domain::Biopolymer {
        residues: vec![crate::domain::ResidueRecord {
            id: crate::domain::ResidueId {
                chain_id: 'A',
                sequence_number: 1,
                insertion_code: ' ',
            },
            residue_name: "BEN".to_string(),
            atom_indices: vec![0, 1],
            alpha_carbon: None,
            backbone_nitrogen: None,
            backbone_carbon: None,
            backbone_oxygen: None,
            is_standard_amino_acid: false,
        }],
        chains: Vec::new(),
        secondary_structures: Vec::new(),
        residue_for_atom: vec![Some(0), Some(0), None],
        atom_name_for_atom: vec![None, None, None],
    });
    assert_eq!(select_atoms(&structure, "ben").unwrap(), vec![0, 1]);
    let err = select_atoms(&structure, "CHX").unwrap_err().to_string();
    assert!(err.contains("BEN"), "{err}");
}

#[test]
fn molecules_follow_bonds_within_the_selection() {
    let mut structure = Structure::new(
        "t",
        vec![
            atom("C", 0.0, 0.0, 0.0),
            atom("C", 1.5, 0.0, 0.0),
            atom("O", 5.0, 0.0, 0.0),
            atom("H", 6.0, 0.0, 0.0),
        ],
    );
    structure.add_bond(0, 1, BondType::Single);
    structure.add_bond(2, 3, BondType::Single);
    assert_eq!(
        molecules_of(&structure, &[0, 1, 2, 3]),
        vec![vec![0, 1], vec![2, 3]]
    );
    assert_eq!(molecules_of(&structure, &[0, 2]), vec![vec![0], vec![2]]);
}

#[test]
fn ballistic_motion_has_exponent_two() {
    let structure = Structure::new("t", vec![atom("Ar", 0.0, 0.0, 0.0)]);
    let frames: Vec<Vec<Point3<f32>>> = (0..40)
        .map(|i| vec![Point3::new(0.5 * i as f32, 0.0, 0.0)])
        .collect();
    let trajectory = trajectory_from(&frames, 2.0, None);
    let report = mean_squared_displacement(
        &trajectory,
        &structure,
        &MsdSpec {
            molecules: vec![vec![0]],
            max_lag_frames: None,
            fit_window: (0.2, 0.8),
        },
    )
    .unwrap();
    assert!(!report.unwrapped);
    assert_eq!(report.lag_ps.len(), 20);
    assert!((report.lag_ps[0] - 2.0).abs() < 1e-6);
    // One frame of lag moves 0.5 Å: MSD(2 ps) = 0.25 Å².
    assert!((report.msd[0] - 0.25).abs() < 1e-6, "{}", report.msd[0]);
    assert!((report.exponent - 2.0).abs() < 1e-3, "{}", report.exponent);
}

#[test]
fn random_walk_recovers_its_diffusion_coefficient() {
    // 200 walkers taking ±1 Å steps along each axis every 1 ps:
    // MSD = 3 Å² per ps, so D = 0.5 Å²/ps = 5e-5 cm²/s.
    let walkers = 200;
    let atoms: Vec<Atom> = (0..walkers).map(|_| atom("He", 0.0, 0.0, 0.0)).collect();
    let structure = Structure::new("t", atoms);
    let mut positions = vec![Point3::origin(); walkers];
    let mut frames = vec![positions.clone()];
    let mut seed = 1u64;
    for _ in 0..300 {
        for position in positions.iter_mut() {
            for axis in 0..3 {
                seed = splitmix64(seed);
                position[axis] += if seed & 1 == 0 { 1.0 } else { -1.0 };
            }
        }
        frames.push(positions.clone());
    }
    let trajectory = trajectory_from(&frames, 1.0, None);
    let report = mean_squared_displacement(
        &trajectory,
        &structure,
        &MsdSpec {
            molecules: (0..walkers).map(|i| vec![i]).collect(),
            max_lag_frames: Some(60),
            fit_window: (0.2, 0.8),
        },
    )
    .unwrap();
    let expected = 5.0e-5;
    assert!(
        (report.diffusion_cm2_s - expected).abs() < 0.15 * expected,
        "D = {}",
        report.diffusion_cm2_s
    );
    assert!((report.exponent - 1.0).abs() < 0.1, "{}", report.exponent);
}

#[test]
fn wrapped_coordinates_are_unwrapped_through_the_box() {
    let cell = cubic(10.0);
    let structure = Structure::with_cell("t", vec![atom("Ar", 0.0, 0.0, 0.0)], cell.clone());
    // Drifts 0.8 Å per frame along x, wrapped into [0, 10).
    let frames: Vec<Vec<Point3<f32>>> = (0..30)
        .map(|i| vec![Point3::new((0.8 * i as f32).rem_euclid(10.0), 0.0, 0.0)])
        .collect();
    let trajectory = trajectory_from(&frames, 1.0, Some(&cell));
    let spec = MsdSpec {
        molecules: vec![vec![0]],
        max_lag_frames: Some(10),
        fit_window: (0.2, 0.8),
    };
    let report = mean_squared_displacement(&trajectory, &structure, &spec).unwrap();
    assert!(report.unwrapped);
    assert!((report.msd[0] - 0.64).abs() < 1e-3, "{}", report.msd[0]);
    assert!((report.msd[9] - 64.0).abs() < 1e-2, "{}", report.msd[9]);

    // The structure's cell serves when the trajectory has no boxes.
    let no_boxes = trajectory_from(&frames, 1.0, None);
    let report = mean_squared_displacement(&no_boxes, &structure, &spec).unwrap();
    assert!(report.unwrapped);
    assert!((report.msd[9] - 64.0).abs() < 1e-2, "{}", report.msd[9]);
}

#[test]
fn centre_of_mass_tracks_a_rigid_molecule() {
    let mut structure = Structure::with_cell(
        "t",
        vec![atom("C", 0.0, 0.0, 0.0), atom("O", 1.2, 0.0, 0.0)],
        cubic(20.0),
    );
    structure.add_bond(0, 1, BondType::Single);
    let frames: Vec<Vec<Point3<f32>>> = (0..12)
        .map(|i| {
            let shift = Vector3::new(0.0, 0.3 * i as f32, 0.0);
            vec![Point3::from(shift), Point3::new(1.2, 0.0, 0.0) + shift]
        })
        .collect();
    let trajectory = trajectory_from(&frames, 1.0, None);
    let report = mean_squared_displacement(
        &trajectory,
        &structure,
        &MsdSpec {
            molecules: molecules_of(&structure, &[0, 1]),
            max_lag_frames: None,
            fit_window: (0.2, 0.8),
        },
    )
    .unwrap();
    assert_eq!(report.molecules, 1);
    assert!((report.msd[0] - 0.09).abs() < 1e-4, "{}", report.msd[0]);
}

#[test]
fn free_volume_of_one_atom_in_a_box() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 5.0, 5.0, 5.0)], cubic(10.0));
    let spec = FreeVolumeSpec {
        probe_radius: 0.0,
        grid_spacing: 0.25,
    };
    let report = free_volume(&structure, spec).unwrap();
    let sphere = 4.0 / 3.0 * std::f64::consts::PI * 1.88f64.powi(3);
    let expected = 1.0 - sphere / 1000.0;
    assert!((report.cell_volume_a3 - 1000.0).abs() < 1e-3);
    assert!(
        (report.fraction - expected).abs() < 0.01,
        "{} vs {expected}",
        report.fraction
    );
    assert_eq!(report.grid_points, 40 * 40 * 40);
    assert!((report.density_g_cm3 - 39.948 * 1.660539 / 1000.0).abs() < 1e-3);

    // A probe shrinks the free fraction, and a sphere on the face is wrapped.
    let probe = free_volume(
        &structure,
        FreeVolumeSpec {
            probe_radius: 1.0,
            grid_spacing: 0.25,
        },
    )
    .unwrap();
    assert!(probe.fraction < report.fraction);
    let on_face = Structure::with_cell("t", vec![atom("Ar", 0.0, 5.0, 5.0)], cubic(10.0));
    let wrapped = free_volume(&on_face, spec).unwrap();
    assert!((wrapped.fraction - report.fraction).abs() < 0.005);
}

#[test]
fn free_volume_needs_a_real_cell() {
    let structure = Structure::new("t", vec![atom("Ar", 0.0, 0.0, 0.0)]);
    let err = free_volume(&structure, FreeVolumeSpec::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("periodic cell"), "{err}");
}

#[test]
fn free_volume_averages_over_trajectory_frames_with_their_boxes() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 5.0, 5.0, 5.0)], cubic(10.0));
    let frames = vec![
        vec![Point3::new(5.0, 5.0, 5.0)],
        vec![Point3::new(2.0, 2.0, 2.0)],
    ];
    let small = cubic(8.0);
    let trajectory = trajectory_from(&frames, 1.0, Some(&small));
    let spec = FreeVolumeSpec {
        probe_radius: 0.0,
        grid_spacing: 0.25,
    };
    let report = free_volume_over_frames(&structure, &trajectory, &[0, 1], spec).unwrap();
    assert_eq!(report.frames, 2);
    assert!((report.cell_volume_a3 - 512.0).abs() < 1e-3);
    let sphere = 4.0 / 3.0 * std::f64::consts::PI * 1.88f64.powi(3);
    assert!((report.fraction - (1.0 - sphere / 512.0)).abs() < 0.01);
    let err = free_volume_over_frames(&structure, &trajectory, &[5], spec)
        .unwrap_err()
        .to_string();
    assert!(err.contains("out of range"), "{err}");
}

fn moving_atom_with_times(times: &[f32]) -> (Structure, Trajectory) {
    let structure = Structure::new("t", vec![atom("Ar", 0.0, 0.0, 0.0)]);
    let coords: Vec<f32> = (0..times.len())
        .flat_map(|i| [0.5 * i as f32, 0.0, 0.0])
        .collect();
    let trajectory = Trajectory::from_parts(1, coords, times.to_vec(), 1, times.len());
    (structure, trajectory)
}

fn msd_of(structure: &Structure, trajectory: &Trajectory) -> anyhow::Result<DiffusionReport> {
    mean_squared_displacement(
        trajectory,
        structure,
        &MsdSpec {
            molecules: vec![vec![0]],
            max_lag_frames: None,
            fit_window: (0.2, 0.8),
        },
    )
}

#[test]
fn msd_refuses_irregular_frame_spacing() {
    let mut times: Vec<f32> = (0..20).map(|i| i as f32).collect();
    for time in &mut times[10..] {
        *time += 9.0;
    }
    let (structure, trajectory) = moving_atom_with_times(&times);
    let err = msd_of(&structure, &trajectory).unwrap_err().to_string();
    assert!(err.contains("evenly spaced"), "{err}");
}

#[test]
fn msd_refuses_repeated_or_missing_times() {
    let mut times: Vec<f32> = (0..20).map(|i| i as f32).collect();
    times[10] = times[9];
    let (structure, trajectory) = moving_atom_with_times(&times);
    let err = msd_of(&structure, &trajectory).unwrap_err().to_string();
    assert!(err.contains("do not increase"), "{err}");

    let (structure, trajectory) = moving_atom_with_times(&[0.0; 20]);
    assert!(msd_of(&structure, &trajectory).is_err());

    times = (0..20).map(|i| i as f32).collect();
    times[5] = f32::NAN;
    let (structure, trajectory) = moving_atom_with_times(&times);
    let err = msd_of(&structure, &trajectory).unwrap_err().to_string();
    assert!(err.contains("finite"), "{err}");
}

#[test]
fn msd_accepts_late_times_that_f32_rounds() {
    // At 1 µs an f32 time is only good to ~0.06 ps; a 1 ps spacing still holds.
    let times: Vec<f32> = (0..20).map(|i| 1.0e6 + i as f32 * 1.0).collect();
    let (structure, trajectory) = moving_atom_with_times(&times);
    let report = msd_of(&structure, &trajectory).unwrap();
    assert!((report.lag_ps[0] - 1.0).abs() < 0.1, "{}", report.lag_ps[0]);
}

#[test]
fn free_volume_refuses_a_grid_too_fine_to_count() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 0.0, 0.0, 0.0)], cubic(10.0));
    let spec = FreeVolumeSpec {
        probe_radius: 0.0,
        grid_spacing: 1.0e-6,
    };
    let err = free_volume(&structure, spec).unwrap_err().to_string();
    assert!(err.contains("coarser --grid"), "{err}");
}
