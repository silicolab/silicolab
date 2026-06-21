use nalgebra::Point3;

use crate::domain::{Atom, BondType, Structure, UnitCell};

use super::{add_missing_hydrogens, infer_bonds_with_cell};

#[test]
fn infers_periodic_bond_across_cell_boundary() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let atoms = vec![
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
    ];

    assert_eq!(infer_bonds_with_cell(&atoms, Some(&cell)).len(), 1);
}

#[test]
fn monatomic_ions_are_never_bonded() {
    // A sodium ion 2.0 Å from a water oxygen sits well inside the naive
    // (1.66 + 0.66) * 1.25 ≈ 2.9 Å covalent cutoff, yet must not bond.
    let atoms = vec![
        Atom {
            element: "Na".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        },
        Atom {
            element: "O".to_string(),
            position: Point3::new(2.0, 0.0, 0.0),
            charge: 0.0,
        },
        Atom {
            element: "H".to_string(),
            position: Point3::new(2.0, 0.97, 0.0),
            charge: 0.0,
        },
    ];

    // Non-periodic and periodic paths both skip the ion; only the O–H bond
    // survives.
    let bonds = infer_bonds_with_cell(&atoms, None);
    assert_eq!(bonds.len(), 1);
    assert!(bonds.iter().all(|bond| bond.a != 0 && bond.b != 0));

    let cell = UnitCell::from_parameters(20.0, 20.0, 20.0, 90.0, 90.0, 90.0);
    let periodic = infer_bonds_with_cell(&atoms, Some(&cell));
    assert_eq!(periodic.len(), 1);
    assert!(periodic.iter().all(|bond| bond.a != 0 && bond.b != 0));
}

#[test]
fn infers_carbon_carbon_double_bond_from_distance() {
    let atoms = vec![
        Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(1.34, 0.0, 0.0),
            charge: 0.0,
        },
    ];
    let bonds = infer_bonds_with_cell(&atoms, None);

    assert_eq!(bonds[0].bond_type, BondType::Double);
}

#[test]
fn infers_aromatic_bonds_in_benzene_like_ring() {
    let atoms = (0..6)
        .map(|index| {
            let angle = index as f32 * std::f32::consts::TAU / 6.0;

            Atom {
                element: "C".to_string(),
                position: Point3::new(angle.cos() * 1.39, angle.sin() * 1.39, 0.0),
                charge: 0.0,
            }
        })
        .collect::<Vec<_>>();
    let bonds = infer_bonds_with_cell(&atoms, None);

    assert_eq!(bonds.len(), 6);
    assert!(
        bonds
            .iter()
            .all(|bond| bond.bond_type == BondType::Aromatic)
    );
}

#[test]
fn adds_missing_hydrogens_to_methane_carbon() {
    let mut structure = Structure::with_bonds(
        "carbon",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::origin(),
            charge: 0.0,
        }],
        Vec::new(),
    );
    let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);

    assert_eq!(added, 4);
    assert_eq!(structure.atoms.len(), 5);
    assert_eq!(structure.bonds.len(), 4);
}

#[test]
fn adds_aromatic_hydrogen_with_sp2_angle() {
    let atoms = toluene_heavy_atoms();
    let bonds = infer_bonds_with_cell(&atoms, None);
    let mut structure = Structure::with_bonds("toluene heavy atoms", atoms, bonds);
    let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);
    let phenyl_carbon = 2;
    let methyl_bearing_carbon = 1;
    let h_index = structure
        .bonds
        .iter()
        .find_map(|bond| {
            if bond.a == phenyl_carbon && structure.atoms[bond.b].element == "H" {
                Some(bond.b)
            } else if bond.b == phenyl_carbon && structure.atoms[bond.a].element == "H" {
                Some(bond.a)
            } else {
                None
            }
        })
        .expect("phenyl C-H");
    let ring_substituent =
        structure.atoms[methyl_bearing_carbon].position - structure.atoms[phenyl_carbon].position;
    let hydrogen = structure.atoms[h_index].position - structure.atoms[phenyl_carbon].position;
    let angle = (ring_substituent.dot(&hydrogen) / (ring_substituent.norm() * hydrogen.norm()))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();

    assert_eq!(added, 8);
    assert!((angle - 120.0).abs() < 5.0, "angle was {angle}");
}

#[test]
fn adds_toluene_methyl_hydrogens_with_tetrahedral_angles() {
    let atoms = toluene_heavy_atoms();
    let bonds = infer_bonds_with_cell(&atoms, None);
    let mut structure = Structure::with_bonds("toluene heavy atoms", atoms, bonds);
    let added = add_missing_hydrogens(&mut structure.atoms, &mut structure.bonds);
    let methyl_carbon = 0;
    let phenyl_carbon = 1;
    let methyl_hydrogens = bonded_hydrogens(&structure, methyl_carbon);

    assert_eq!(added, 8);
    assert_eq!(methyl_hydrogens.len(), 3);

    for hydrogen in &methyl_hydrogens {
        let angle = bond_angle(&structure, phenyl_carbon, methyl_carbon, *hydrogen);

        assert!((angle - 109.47).abs() < 5.0, "C(Ph)-C-H angle was {angle}");
    }

    for i in 0..methyl_hydrogens.len() {
        for j in (i + 1)..methyl_hydrogens.len() {
            let angle = bond_angle(
                &structure,
                methyl_hydrogens[i],
                methyl_carbon,
                methyl_hydrogens[j],
            );

            assert!((angle - 109.47).abs() < 5.0, "H-C-H angle was {angle}");
        }
    }
}

fn toluene_heavy_atoms() -> Vec<Atom> {
    vec![
        Atom {
            element: "C".to_string(),
            position: Point3::new(2.221095, 0.033319, -0.027866),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(0.721422, 0.009962, -0.039183),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(0.032134, -1.212_05, 0.024559),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(-1.366126, -1.230666, 0.051405),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(-2.083336, -0.031206, 0.027755),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(-1.403292, 1.189092, -0.017693),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(-0.005096, 1.211892, -0.044659),
            charge: 0.0,
        },
    ]
}

fn bonded_hydrogens(structure: &Structure, atom_index: usize) -> Vec<usize> {
    structure
        .bonds
        .iter()
        .filter_map(|bond| {
            if bond.a == atom_index && structure.atoms[bond.b].element == "H" {
                Some(bond.b)
            } else if bond.b == atom_index && structure.atoms[bond.a].element == "H" {
                Some(bond.a)
            } else {
                None
            }
        })
        .collect()
}

fn bond_angle(structure: &Structure, first: usize, center: usize, second: usize) -> f32 {
    let first = structure.atoms[first].position - structure.atoms[center].position;
    let second = structure.atoms[second].position - structure.atoms[center].position;

    (first.dot(&second) / (first.norm() * second.norm()))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}
