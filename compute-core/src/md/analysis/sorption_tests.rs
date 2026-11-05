use nalgebra::Point3;

use super::*;
use crate::domain::{Atom, UnitCell};
use crate::md::topology::{MdTopology, MoleculeAtom, MoleculeRun, MoleculeType, Species};

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

fn lj(sigma: f64, epsilon: f64) -> NonbondedAtom {
    NonbondedAtom {
        sigma,
        epsilon,
        charge: 0.0,
    }
}

fn spec(probe: NonbondedAtom, insertions: usize) -> WidomSpec {
    WidomSpec {
        probe_positions: vec![Point3::origin()],
        probe: vec![probe],
        temperature_k: 300.0,
        insertions,
        seed: 1,
        cutoff: 8.0,
    }
}

#[test]
fn an_ideal_gas_probe_has_no_excess_chemical_potential() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 5.0, 5.0, 5.0)], cubic(20.0));
    let host = [lj(3.4, 1.0)];
    let report = widom_insertion(&structure, &host, &spec(lj(0.0, 0.0), 500)).unwrap();
    assert!((report.boltzmann_mean - 1.0).abs() < 1e-12);
    assert!(report.mu_excess_kj_mol.abs() < 1e-9);
    assert!(report.boltzmann_error.abs() < 1e-12);
    // S = 22414e-6 · 1333.22 / (8.314 · 300) cm³(STP)/(cm³·cmHg).
    assert!((report.solubility_cm3stp_per_cm3_cmhg - 0.01198).abs() < 1e-4);
    assert_eq!(report.insertions, 500);
}

#[test]
fn a_crowded_box_excludes_the_probe() {
    let mut atoms = Vec::new();
    for i in 0..8 {
        for j in 0..8 {
            for k in 0..8 {
                atoms.push(atom("C", i as f32 * 2.5, j as f32 * 2.5, k as f32 * 2.5));
            }
        }
    }
    let structure = Structure::with_cell("t", atoms, cubic(20.0));
    let host = vec![lj(3.4, 0.4); 512];
    let report = widom_insertion(&structure, &host, &spec(lj(3.4, 0.4), 2000)).unwrap();
    assert!(report.boltzmann_mean < 1e-3, "{}", report.boltzmann_mean);
    assert!(
        report.mu_excess_kj_mol > 15.0,
        "{}",
        report.mu_excess_kj_mol
    );
    assert!(report.favourable_fraction < 0.01);
}

#[test]
fn an_attractive_host_raises_the_boltzmann_factor_and_is_deterministic() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 10.0, 10.0, 10.0)], cubic(20.0));
    let host = [lj(3.4, 5.0)];
    let probe = spec(lj(3.4, 5.0), 40_000);
    let report = widom_insertion(&structure, &host, &probe).unwrap();
    assert!(report.boltzmann_mean > 1.0, "{}", report.boltzmann_mean);
    assert!(report.mu_excess_kj_mol < 0.0);
    assert!(report.favourable_fraction > 0.01);
    let again = widom_insertion(&structure, &host, &probe).unwrap();
    assert_eq!(report.boltzmann_mean, again.boltzmann_mean);
    let mut other = probe.clone();
    other.seed = 2;
    let other = widom_insertion(&structure, &host, &other).unwrap();
    assert!((other.boltzmann_mean - report.boltzmann_mean).abs() < 5.0 * report.boltzmann_error);
}

#[test]
fn a_thin_cell_is_refused_for_the_cutoff() {
    let structure = Structure::with_cell("t", vec![atom("Ar", 5.0, 5.0, 5.0)], cubic(12.0));
    let err = widom_insertion(&structure, &[lj(3.4, 1.0)], &spec(lj(3.4, 1.0), 10))
        .unwrap_err()
        .to_string();
    assert!(err.contains("cutoff"), "{err}");
}

#[test]
fn host_parameters_expand_the_composition_in_order() {
    let topology = MdTopology {
        title: "t".to_string(),
        species: vec![
            Species {
                element: "c3".to_string(),
                atomic_number: 6,
                mass_u: 12.0,
                charge: 0.0,
                sigma_angstrom: 3.4,
                epsilon_kj_mol: 0.45,
            },
            Species {
                element: "hc".to_string(),
                atomic_number: 1,
                mass_u: 1.0,
                charge: 0.0,
                sigma_angstrom: 2.6,
                epsilon_kj_mol: 0.09,
            },
        ],
        molecules: vec![MoleculeType {
            name: "CH".to_string(),
            nrexcl: 3,
            atoms: vec![
                MoleculeAtom::new("c3", "C1", -0.1),
                MoleculeAtom::new("hc", "H1", 0.1),
            ],
            settle: None,
            bonds: Vec::new(),
            pairs: Vec::new(),
            angles: Vec::new(),
            dihedrals: Vec::new(),
            impropers: Vec::new(),
            exclusions: Vec::new(),
        }],
        composition: vec![MoleculeRun {
            molecule: "CH".to_string(),
            count: 2,
        }],
        defaults: None,
        bonded_params: Vec::new(),
        inline_force_field: None,
    };
    let host = host_parameters(&topology, 4).unwrap();
    assert_eq!(host.len(), 4);
    assert!((host[0].sigma - 3.4).abs() < 1e-6);
    assert!((host[0].epsilon - 0.45).abs() < 1e-6);
    assert!((host[0].charge + 0.1).abs() < 1e-6);
    assert!((host[3].charge - 0.1).abs() < 1e-6);
    let err = host_parameters(&topology, 5).unwrap_err().to_string();
    assert!(err.contains("describes 4 atoms"), "{err}");
}

#[test]
fn simple_cubic_lattice_has_six_first_neighbours() {
    let mut atoms = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                atoms.push(atom("Ar", i as f32 * 5.0, j as f32 * 5.0, k as f32 * 5.0));
            }
        }
    }
    let structure = Structure::with_cell("t", atoms, cubic(20.0));
    let all: Vec<usize> = (0..64).collect();
    let spec = RdfSpec {
        first: all.clone(),
        second: all,
        r_max: 9.0,
        bin_width: 0.5,
    };
    let report = radial_distribution(&structure, &spec).unwrap();
    assert_eq!(report.frames, 1);
    assert_eq!(report.pairs_per_frame, 64 * 64);
    let bin = |r: f64| (r / 0.5) as usize;
    assert_eq!(report.g[bin(2.0)], 0.0);
    // 6 neighbours in the 5.0–5.5 Å shell: 6 / (0.008 Å⁻³ · 173.4 Å³) ≈ 4.3.
    assert!(
        (report.g[bin(5.1)] - 4.33).abs() < 0.05,
        "{}",
        report.g[bin(5.1)]
    );
    // Six nearest neighbours at 5 Å, twelve more at 7.07 Å.
    assert!((report.coordination[bin(6.0)] - 6.0).abs() < 1e-9);
    assert!((report.coordination[bin(8.0)] - 18.0).abs() < 1e-9);
    let csv = rdf_csv(&report);
    assert!(csv.starts_with("r_angstrom,g,coordination\n0.250,"));

    let err = radial_distribution(
        &structure,
        &RdfSpec {
            r_max: 11.0,
            ..spec
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("half the cell"), "{err}");
}

#[test]
fn rdf_between_two_selections_excludes_shared_atoms() {
    let structure = Structure::with_cell(
        "t",
        vec![atom("O", 0.0, 0.0, 0.0), atom("H", 1.0, 0.0, 0.0)],
        cubic(20.0),
    );
    let report = radial_distribution(
        &structure,
        &RdfSpec {
            first: vec![0, 1],
            second: vec![0, 1],
            r_max: 5.0,
            bin_width: 0.5,
        },
    )
    .unwrap();
    assert_eq!(report.coordination.last().copied(), Some(1.0));
}
