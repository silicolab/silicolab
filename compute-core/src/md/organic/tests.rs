use nalgebra::Point3;

use super::*;
use crate::domain::{Atom, BondType, UnitCell};
use crate::md::typed::{AtomTypeParams, TypedAtom};

fn typed_atom(name: &str, type_name: &str, charge: f64, mass: f64) -> TypedAtom {
    TypedAtom {
        name: name.to_string(),
        type_name: type_name.to_string(),
        residue_name: "MOL".to_string(),
        residue_number: 1,
        charge,
        mass_u: mass,
    }
}

fn methane_template(label: &str) -> OrganicTemplate {
    let mut atoms = vec![typed_atom("C1", "c3", -0.4, 12.01)];
    for i in 1..=4 {
        atoms.push(typed_atom(&format!("H{i}"), "hc", 0.1, 1.008));
    }
    let bonds = (2..=5)
        .map(|h| BondedInteraction {
            atoms: vec![1, h],
            func: 1,
            params: vec![0.1092, 282252.6],
        })
        .collect();
    OrganicTemplate {
        label: label.to_string(),
        molecule: TypedMolecule {
            name: "MOL".to_string(),
            nrexcl: 3,
            source: "test".to_string(),
            atom_types: vec![
                AtomTypeParams {
                    name: "c3".to_string(),
                    mass_u: 0.0,
                    sigma_nm: 0.3397,
                    epsilon_kj_mol: 0.4577,
                },
                AtomTypeParams {
                    name: "hc".to_string(),
                    mass_u: 1.008,
                    sigma_nm: 0.2600,
                    epsilon_kj_mol: 0.0870,
                },
            ],
            atoms,
            bonds,
            pairs: Vec::new(),
            angles: vec![BondedInteraction {
                atoms: vec![2, 1, 3],
                func: 1,
                params: vec![108.35, 329.95],
            }],
            dihedrals: vec![BondedInteraction {
                atoms: vec![2, 1, 3, 4],
                func: 4,
                params: vec![180.0, 4.6, 2.0],
            }],
        },
    }
}

fn atom(element: &str, x: f32) -> Atom {
    Atom {
        element: element.to_string(),
        position: Point3::new(x, 0.0, 0.0),
        charge: 0.0,
    }
}

fn two_methanes() -> Structure {
    let mut atoms = Vec::new();
    for copy in 0..2 {
        let offset = copy as f32 * 10.0;
        atoms.push(atom("C", offset));
        for i in 0..4 {
            atoms.push(atom("H", offset + 1.0 + i as f32 * 0.1));
        }
    }
    let mut structure = Structure::with_cell(
        "two methanes",
        atoms,
        UnitCell::from_parameters(20.0, 20.0, 20.0, 90.0, 90.0, 90.0),
    );
    // `with_cell` guesses bonds from distances, which joins the packed hydrogens.
    structure.bonds.clear();
    for copy in 0..2 {
        let base = copy * 5;
        for h in 1..=4 {
            structure.add_bond(base, base + h, BondType::Single);
        }
    }
    structure
}

#[test]
fn packed_copies_become_one_molecule_type_with_a_count() {
    let (topology, report) = organic_topology(
        &two_methanes(),
        &[methane_template("methane")],
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(report.counts, vec![("methane".to_string(), 2)]);
    assert_eq!(report.atom_count, 10);
    assert_eq!(topology.atom_count(), 10);
    assert_eq!(topology.molecules.len(), 1);
    assert_eq!(topology.composition.len(), 1);
    assert_eq!(topology.composition[0].count, 2);
    assert_eq!(topology.defaults, Some(GAFF_DEFAULTS));
    let c3 = topology.species.iter().find(|s| s.element == "c3").unwrap();
    assert_eq!(c3.atomic_number, 6);
    assert!((c3.mass_u - 12.01).abs() < 1e-3);
    assert!((c3.sigma_angstrom - 3.397).abs() < 1e-3);
    let molecule = &topology.molecules[0];
    assert_eq!(molecule.name, "methane");
    assert_eq!(molecule.atoms[0].residue_name.as_deref(), Some("MET"));
    assert_eq!(molecule.bonds.len(), 4);
    assert_eq!(molecule.bonds[0].params, vec![0.1092, 282252.6]);
    assert_eq!(molecule.dihedrals.len(), 0);
    assert_eq!(molecule.impropers.len(), 1);
    assert!((molecule.net_charge()).abs() < 1e-6);
}

#[test]
fn a_molecule_without_a_template_names_itself() {
    let mut structure = two_methanes();
    structure.atoms[6].element = "F".to_string();
    let err = organic_topology(&structure, &[methane_template("methane")], &BTreeMap::new())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("atom 6") && err.contains("forcefield type"),
        "{err}"
    );
}

#[test]
fn interleaved_molecules_are_rejected() {
    let mut structure = two_methanes();
    structure.bonds.clear();
    for h in 1..=4 {
        structure.add_bond(0, 5 + h, BondType::Single);
        structure.add_bond(5, h, BondType::Single);
    }
    let err = organic_topology(&structure, &[methane_template("methane")], &BTreeMap::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("contiguous"), "{err}");
}

#[test]
fn ambiguous_templates_need_a_residue_name_or_a_preference() {
    let templates = [methane_template("meth-a"), methane_template("meth-b")];
    let err = organic_topology(&two_methanes(), &templates, &BTreeMap::new())
        .unwrap_err()
        .to_string();
    assert!(err.contains("--typed"), "{err}");

    let mut structure = two_methanes();
    structure.biopolymer = Some(crate::domain::Biopolymer {
        residues: (0..2)
            .map(|copy| crate::domain::ResidueRecord {
                id: crate::domain::ResidueId {
                    chain_id: 'X',
                    sequence_number: copy as i32 + 1,
                    insertion_code: ' ',
                },
                residue_name: "MET".to_string(),
                atom_indices: (copy * 5..copy * 5 + 5).collect(),
                alpha_carbon: None,
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
                is_standard_amino_acid: false,
            })
            .collect(),
        chains: Vec::new(),
        secondary_structures: Vec::new(),
        residue_for_atom: (0..10).map(|i| Some(i / 5)).collect(),
        atom_name_for_atom: vec![None; 10],
    });
    let mut preferred = BTreeMap::new();
    preferred.insert("MET".to_string(), "meth-b".to_string());
    let (topology, _) = organic_topology(&structure, &templates, &preferred).unwrap();
    assert_eq!(topology.molecules[0].name, "meth-b");
    assert_eq!(
        topology.molecules[0].atoms[0].residue_name.as_deref(),
        Some("MET")
    );

    preferred.insert("MET".to_string(), "nothing".to_string());
    let err = organic_topology(&structure, &templates, &preferred)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no typed molecule named"), "{err}");
}

#[test]
fn conflicting_atom_type_parameters_are_rejected() {
    let mut other = methane_template("other");
    other.molecule.atom_types[0].sigma_nm = 0.35;
    let mut structure = two_methanes();
    structure.atoms[9].element = "F".to_string();
    other.molecule.atoms[4] = typed_atom("F5", "f", -0.2, 18.998);
    let err = organic_topology(
        &structure,
        &[methane_template("methane"), other],
        &BTreeMap::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("different parameters"), "{err}");
}

/// Same atoms in the same order as methane, but H4 hangs off H3 instead of C.
fn rewired_methane_template(label: &str) -> OrganicTemplate {
    let mut template = methane_template(label);
    template.molecule.bonds[3].atoms = vec![4, 5];
    template
}

#[test]
fn a_template_with_the_same_atoms_but_other_bonds_is_rejected() {
    let err = organic_topology(
        &two_methanes(),
        &[rewired_methane_template("rewired")],
        &BTreeMap::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("different bonds") && err.contains("rewired"),
        "{err}"
    );

    let templates = [
        rewired_methane_template("rewired"),
        methane_template("methane"),
    ];
    let (topology, _) = organic_topology(&two_methanes(), &templates, &BTreeMap::new()).unwrap();
    assert_eq!(topology.molecules[0].name, "methane");
}
