use nalgebra::Point3;

use super::{Atom, Bond, BondType, Structure, UnitCell};
use crate::domain::{AtomCategory, PdbAtomAnnotation, build_biopolymer};

fn atom(element: &str) -> Atom {
    Atom {
        element: element.to_string(),
        position: Point3::origin(),
        charge: 0.0,
    }
}

fn annotation(atom_name: &str, residue_name: &str, seq: i32) -> PdbAtomAnnotation {
    PdbAtomAnnotation {
        atom_name: atom_name.to_string(),
        residue_name: residue_name.to_string(),
        chain_id: 'A',
        residue_seq: seq,
        insertion_code: ' ',
    }
}

#[test]
fn atom_category_uses_residue_metadata() {
    // One alanine atom (protein), one ligand hetero atom, one water oxygen.
    let annotations = vec![
        annotation("CA", "ALA", 1),
        annotation("C1", "LIG", 2),
        annotation("OW", "SOL", 3),
    ];
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
    let structure = Structure {
        title: "t".to_string(),
        atoms: vec![atom("C"), atom("C"), atom("O")],
        bonds: Vec::new(),
        cell: None,
        biopolymer: Some(biopolymer),
    };
    assert_eq!(structure.atom_category(0), AtomCategory::Protein);
    assert_eq!(structure.atom_category(1), AtomCategory::Ligand);
    assert_eq!(structure.atom_category(2), AtomCategory::Solvent);
}

#[test]
fn atom_category_classifies_carbohydrate_residues() {
    let annotations = vec![annotation("C1", "NAG", 1), annotation("C1", "MAN", 2)];
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
    let structure = Structure {
        title: "glycan".to_string(),
        atoms: vec![atom("C"), atom("C")],
        bonds: Vec::new(),
        cell: None,
        biopolymer: Some(biopolymer),
    };
    assert_eq!(structure.atom_category(0), AtomCategory::Carbohydrate);
    assert_eq!(structure.atom_category(1), AtomCategory::Carbohydrate);
}

#[test]
fn atom_category_falls_back_to_element_for_lone_ions() {
    let structure = Structure {
        title: "ions".to_string(),
        atoms: vec![atom("Na"), atom("C")],
        bonds: Vec::new(),
        cell: None,
        biopolymer: None,
    };
    assert_eq!(structure.atom_category(0), AtomCategory::Ion);
    assert_eq!(structure.atom_category(1), AtomCategory::Other);
}

#[test]
fn wraps_periodic_atoms_into_unit_cell() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell(
        "wrapped",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(12.0, -1.0, 5.0),
            charge: 0.0,
        }],
        cell,
    );

    structure.wrap_atoms_into_cell();
    let frac = structure
        .cell
        .as_ref()
        .expect("cell")
        .cartesian_to_fractional(structure.atoms[0].position);

    assert!((frac.x - 0.2).abs() < 0.0001);
    assert!((frac.y - 0.9).abs() < 0.0001);
    assert!((frac.z - 0.5).abs() < 0.0001);
}

#[test]
fn wraps_periodic_atoms_without_recomputing_bond_types() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "wrapped",
        vec![
            Atom {
                element: "C".to_string(),
                position: Point3::new(12.0, -1.0, 5.0),
                charge: 0.0,
            },
            Atom {
                element: "C".to_string(),
                position: Point3::new(-3.0, 11.0, -6.0),
                charge: 0.0,
            },
        ],
        vec![Bond::with_type(0, 1, BondType::Aromatic)],
        cell,
    );

    structure.wrap_atoms_into_cell_preserving_bonds();

    let cell = structure.cell.as_ref().expect("cell");
    for atom in &structure.atoms {
        let frac = cell.cartesian_to_fractional(atom.position);
        assert!((0.0..1.0).contains(&frac.x));
        assert!((0.0..1.0).contains(&frac.y));
        assert!((0.0..1.0).contains(&frac.z));
    }
    assert_eq!(structure.bonds.len(), 1);
    assert_eq!(structure.bonds[0].bond_type, BondType::Aromatic);
}

#[test]
fn make_supercell_expands_atoms_and_cell() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell(
        "test",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(2.5, 2.5, 2.5),
            charge: 0.0,
        }],
        cell,
    );

    structure.make_supercell([2, 2, 2]);

    assert_eq!(structure.atoms.len(), 8);
    let expanded_cell = structure.cell.as_ref().expect("cell");
    assert!((expanded_cell.a - 20.0).abs() < 0.001);
    assert!((expanded_cell.b - 20.0).abs() < 0.001);
    assert!((expanded_cell.c - 20.0).abs() < 0.001);
}

#[test]
fn make_supercell_preserves_bond_types() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "test",
        vec![
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
        ],
        vec![Bond::with_type(0, 1, BondType::Double)],
        cell,
    );

    structure.make_supercell([2, 1, 1]);

    assert_eq!(structure.atoms.len(), 4);
    assert!(
        structure
            .bonds
            .iter()
            .any(|b| b.bond_type == BondType::Double)
    );
}

#[test]
fn make_supercell_no_op_for_identity() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell(
        "test",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(2.5, 2.5, 2.5),
            charge: 0.0,
        }],
        cell,
    );

    structure.make_supercell([1, 1, 1]);

    assert_eq!(structure.atoms.len(), 1);
    let expanded_cell = structure.cell.as_ref().expect("cell");
    assert!((expanded_cell.a - 10.0).abs() < 0.001);
}

#[test]
fn make_supercell_no_op_without_cell() {
    let mut structure = Structure::new(
        "test",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
    );

    structure.make_supercell([2, 2, 2]);

    assert_eq!(structure.atoms.len(), 1);
    assert!(structure.cell.is_none());
}

#[test]
fn make_supercell_handles_cross_boundary_bonds() {
    let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
    let mut structure = Structure::with_cell_and_bonds(
        "test",
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

    structure.make_supercell([2, 1, 1]);

    assert_eq!(structure.atoms.len(), 4);
    assert_eq!(structure.bonds.len(), 2);
    for bond in &structure.bonds {
        assert_eq!(bond.bond_type, BondType::Single);
    }
}
