use nalgebra::Point3;

use crate::{
    domain::{Atom, Bond, BondType, Structure, UnitCell},
    engines::forcefield::{
        AtomOptimizationScope, CellOptimizationOptions, OptimizationOptions, optimize_geometry,
    },
};

use super::energy;

#[test]
fn uff_optimization_shortens_stretched_hydrogen_bond() {
    let mut structure = Structure::with_bonds(
        "stretched hydrogen",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(1.5, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Single)],
    );
    let initial_distance =
        nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);
    let report = optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
    let final_distance =
        nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

    assert!(report.final_energy < report.initial_energy);
    assert!(report.steps > 0);
    assert!(report.converged || report.final_energy < report.initial_energy);
    assert!(final_distance < initial_distance);
    assert!((final_distance - 0.708).abs() < 0.05);
}

#[test]
fn uff_optimization_opens_water_angle_toward_tetrahedral_oxygen() {
    let mut structure = Structure::with_bonds(
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
                position: Point3::new(0.0, 0.96, 0.0),
                charge: 0.0,
            },
        ],
        vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(0, 2, BondType::Single),
        ],
    );
    let initial_error = (h_o_h_angle(&structure) - 104.51).abs();

    optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
    let final_error = (h_o_h_angle(&structure) - 104.51).abs();

    assert!(final_error < initial_error);
}

#[test]
fn uff_vdw_repulsion_separates_unbonded_hydrogens() {
    let mut structure = Structure::with_bonds(
        "unbonded hydrogens",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(-0.25, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.25, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        Vec::new(),
    );
    let initial_energy = energy(&structure).unwrap();
    let initial_distance =
        nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

    optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
    let final_energy = energy(&structure).unwrap();
    let final_distance =
        nalgebra::distance(&structure.atoms[0].position, &structure.atoms[1].position);

    assert!(final_energy < initial_energy);
    assert!(final_distance > initial_distance);
}

#[test]
fn uff_partial_optimization_freezes_unselected_atoms() {
    let mut structure = Structure::with_bonds(
        "partial hydrogen stretch",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(1.5, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Single)],
    );
    let fixed_before = structure.atoms[0].position;
    let movable_before = structure.atoms[1].position;

    let report = optimize_geometry(
        &mut structure,
        OptimizationOptions {
            atoms: AtomOptimizationScope::Selected(vec![1]),
            ..OptimizationOptions::default()
        },
    )
    .unwrap();

    assert!(report.final_energy < report.initial_energy);
    assert_eq!(structure.atoms[0].position, fixed_before);
    assert_ne!(structure.atoms[1].position, movable_before);
}

#[test]
fn uff_optimization_uses_periodic_bond_across_cell_boundary() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "periodic hydrogen bond",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.1, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(9.9, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Single)],
        cell,
    );
    let initial_distance = periodic_distance(&structure, 0, 1);

    let report = optimize_geometry(&mut structure, OptimizationOptions::default()).unwrap();
    let final_distance = periodic_distance(&structure, 0, 1);

    assert!(report.final_energy < report.initial_energy);
    assert!(final_distance > initial_distance);
    assert!((final_distance - 0.708).abs() < 0.05);
}

#[test]
fn uff_partial_periodic_optimization_freezes_unselected_atoms() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "partial periodic hydrogen bond",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.1, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(9.9, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Single)],
        cell,
    );
    let fixed_before = structure.atoms[0].position;
    let initial_distance = periodic_distance(&structure, 0, 1);

    let report = optimize_geometry(
        &mut structure,
        OptimizationOptions {
            atoms: AtomOptimizationScope::Selected(vec![1]),
            ..OptimizationOptions::default()
        },
    )
    .unwrap();

    assert!(report.final_energy < report.initial_energy);
    assert_eq!(structure.atoms[0].position, fixed_before);
    assert!(periodic_distance(&structure, 0, 1) > initial_distance);
}

#[test]
fn uff_optimization_can_adjust_periodic_cell_length() {
    let cell = UnitCell::from_parameters(1.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "cell length relaxation",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.5, 0.0, 0.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Single)],
        cell,
    );
    let initial_cell_a = structure.cell.as_ref().expect("cell").a;
    let initial_distance = periodic_distance(&structure, 0, 1);

    let report = optimize_geometry(
        &mut structure,
        OptimizationOptions {
            cell: CellOptimizationOptions {
                a: true,
                ..CellOptimizationOptions::default()
            },
            max_atom_step: 0.0,
            ..OptimizationOptions::default()
        },
    )
    .unwrap();
    let final_cell_a = structure.cell.as_ref().expect("cell").a;
    let final_distance = periodic_distance(&structure, 0, 1);

    assert!(report.final_energy < report.initial_energy);
    assert!(final_cell_a > initial_cell_a);
    assert!(final_distance > initial_distance);
    assert!((final_distance - 0.708).abs() < 0.05);
}

fn h_o_h_angle(structure: &Structure) -> f32 {
    let first = structure.atoms[1].position - structure.atoms[0].position;
    let second = structure.atoms[2].position - structure.atoms[0].position;

    (first.dot(&second) / (first.norm() * second.norm()))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

fn periodic_distance(structure: &Structure, first: usize, second: usize) -> f32 {
    let cell = structure.cell.as_ref().expect("cell");
    let mut delta = cell.cartesian_to_fractional(structure.atoms[second].position)
        - cell.cartesian_to_fractional(structure.atoms[first].position);
    delta.x -= delta.x.round();
    delta.y -= delta.y.round();
    delta.z -= delta.z.round();

    (cell.vectors[0] * delta.x + cell.vectors[1] * delta.y + cell.vectors[2] * delta.z).norm()
}
