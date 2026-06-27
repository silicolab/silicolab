use nalgebra::Point3;

use super::*;
use crate::domain::{Atom, Structure, UnitCell};

fn ethane_cell() -> Structure {
    let atoms = vec![
        Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        },
        Atom {
            element: "C".to_string(),
            position: Point3::new(1.54, 0.0, 0.0),
            charge: 0.0,
        },
    ];
    Structure::with_cell(
        "ethane",
        atoms,
        UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0),
    )
}

#[test]
fn rectangular_box_uses_three_field_form() {
    let structure = ethane_cell();
    let gro = to_gro(&structure, "ethane").expect("serialized");

    let last_line = gro.lines().last().expect("box line");
    let fields = last_line.split_whitespace().count();
    assert_eq!(fields, 3);
    assert!(gro.contains("ethane"));
    assert!(gro.contains("MOL"));
}

#[test]
fn positions_are_converted_to_nanometers() {
    let structure = ethane_cell();
    let gro = to_gro(&structure, "ethane").expect("serialized");

    let second_atom_line = gro.lines().nth(3).expect("second atom line");
    assert!(second_atom_line.contains("0.154"));
}

#[test]
fn em_mdp_contains_user_visible_parameters() {
    let settings = MdpSettings {
        nsteps: 250,
        emtol: 500.0,
        ..MdpSettings::energy_minimization()
    };
    let mdp = render_mdp(&settings);

    assert!(mdp.contains("integrator               = steep"));
    assert!(mdp.contains("emtol                    = 500.000"));
    assert!(mdp.contains("nsteps                   = 250"));
}

#[test]
fn md_mdp_uses_timestep_instead_of_emtol() {
    let settings = MdpSettings {
        integrator: Integrator::Leapfrog,
        nsteps: 5_000,
        timestep_ps: 0.002,
        ..MdpSettings::energy_minimization()
    };
    let mdp = render_mdp(&settings);

    assert!(mdp.contains("integrator               = md"));
    assert!(mdp.contains("dt                       = 0.00200"));
    assert!(!mdp.contains("emtol"));
}

#[test]
fn triclinic_box_round_trips_through_parser() {
    use crate::io::formats::gro::parse_gro;

    let structure = Structure::with_cell(
        "triclinic",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(1.0, 2.0, 3.0),
            charge: 0.0,
        }],
        UnitCell::from_parameters(10.0, 12.0, 15.0, 70.0, 80.0, 100.0),
    );

    let gro = to_gro(&structure, "triclinic").expect("serialized");
    let box_fields = gro
        .lines()
        .last()
        .expect("box line")
        .split_whitespace()
        .count();
    assert_eq!(
        box_fields, 9,
        "non-orthogonal cell must use the nine-field box form"
    );

    let reparsed = parse_gro(&gro).expect("round-trip parse");
    let original = structure.cell.as_ref().expect("cell").vectors;
    let restored = reparsed.cell.as_ref().expect("cell").vectors;
    for (o, r) in original.iter().zip(restored.iter()) {
        assert!((o.x - r.x).abs() < 1.0e-3, "x mismatch {o:?} vs {r:?}");
        assert!((o.y - r.y).abs() < 1.0e-3, "y mismatch {o:?} vs {r:?}");
        assert!((o.z - r.z).abs() < 1.0e-3, "z mismatch {o:?} vs {r:?}");
    }
}

#[test]
fn box_reduction_brings_overskewed_cells_into_range() {
    use nalgebra::Vector3;
    // v2x = 0.9*a is more skewed than the half limit; reduction shifts it in.
    let a = 10.0;
    let reduced = reduce_box([
        Vector3::new(a, 0.0, 0.0),
        Vector3::new(0.9 * a, a, 0.0),
        Vector3::new(0.0, 0.0, a),
    ]);
    assert!(
        reduced[1].x.abs() <= 0.5 * a + 1e-4,
        "v2x not reduced: {}",
        reduced[1].x
    );
}

#[test]
fn box_reduction_preserves_a_canonical_hexagonal_cell() {
    use nalgebra::Vector3;
    // The nanosheet hexagonal cell sits exactly at the half boundary, which
    // GROMACS accepts; reduction must leave it untouched.
    let a = 2.46;
    let v2 = Vector3::new(a * 0.5, a * 0.866_025_4, 0.0);
    let reduced = reduce_box([Vector3::new(a, 0.0, 0.0), v2, Vector3::new(0.0, 0.0, 12.0)]);
    assert!((reduced[1].x - v2.x).abs() < 1e-6);
    assert!((reduced[1].y - v2.y).abs() < 1e-6);
}

#[test]
fn energy_minimization_mdp_is_byte_stable() {
    // Guards backward compatibility with the committed EM integration: this
    // is the exact historical output.
    let mdp = render_mdp(&MdpSettings::energy_minimization());
    let expected = "\
; SilicoLab-generated GROMACS run parameters
integrator               = steep
nsteps                   = 5000
emtol                    = 1000.000
emstep                   = 0.01000
nstlist                  = 10
cutoff-scheme            = Verlet
ns_type                  = grid
coulombtype              = cutoff
rcoulomb                 = 1.0000
rvdw                     = 1.0000
pbc                      = xyz
constraints              = none
";
    assert_eq!(mdp, expected);
}

#[test]
fn periodic_molecules_and_freeze_render_only_when_set() {
    // Off by default: no framework directives leak into an ordinary run.
    let plain = render_mdp(&MdpSettings::nvt(300.0));
    assert!(!plain.contains("periodic-molecules"));
    assert!(!plain.contains("freezegrps"));

    // A rigid framework freezes its group; a flexible one marks the molecule
    // periodic.
    let mut settings = MdpSettings::nvt(300.0);
    settings.periodic_molecules = true;
    settings.freeze = Some(FreezeGroup {
        group: "Framework".to_string(),
    });
    let mdp = render_mdp(&settings);
    assert!(mdp.contains("periodic-molecules       = yes"));
    assert!(mdp.contains("freezegrps               = Framework"));
    assert!(mdp.contains("freezedim                = Y Y Y"));
}

#[test]
fn nvt_mdp_has_thermostat_and_genvel_but_no_pressure() {
    let mdp = render_mdp(&MdpSettings::nvt(94.0));
    assert!(mdp.contains("integrator               = md"));
    assert!(mdp.contains("coulombtype              = cutoff"));
    assert!(mdp.contains("constraints              = h-bonds"));
    assert!(mdp.contains("constraint-algorithm     = lincs"));
    assert!(mdp.contains("tcoupl                   = V-rescale"));
    assert!(mdp.contains("tc-grps                  = System"));
    assert!(mdp.contains("ref-t                    = 94"));
    assert!(mdp.contains("gen_vel                  = yes"));
    assert!(mdp.contains("pcoupl                   = no"));
}

#[test]
fn npt_mdp_adds_barostat_and_continuation() {
    let mdp = render_mdp(&MdpSettings::npt(94.0));
    assert!(mdp.contains("pcoupl                   = C-rescale"));
    assert!(mdp.contains("continuation             = yes"));
    assert!(mdp.contains("gen_vel                  = no"));
}

#[test]
fn production_mdp_writes_compressed_trajectory() {
    let mdp = render_mdp(&MdpSettings::production(10_000, 94.0));
    assert!(mdp.contains("nstxout-compressed       = 5000"));
    assert!(mdp.contains("coulombtype              = cutoff"));
    assert!(mdp.contains("pcoupl                   = C-rescale"));
}

#[test]
fn constraints_render_only_when_set() {
    // Minimization leaves bonds flexible and emits no algorithm line.
    let em = render_mdp(&MdpSettings::energy_minimization());
    assert!(em.contains("constraints              = none"));
    assert!(!em.contains("constraint-algorithm"));

    // An explicit h-bonds setting renders both lines.
    let settings = MdpSettings {
        constraints: Some(ConstraintKind::HBonds),
        constraint_algorithm: ConstraintAlgorithm::Lincs,
        ..MdpSettings::energy_minimization()
    };
    let mdp = render_mdp(&settings);
    assert!(mdp.contains("constraints              = h-bonds"));
    assert!(mdp.contains("constraint-algorithm     = lincs"));
}

#[test]
fn requires_periodic_cell() {
    let structure = Structure::new(
        "no-cell",
        vec![Atom {
            element: "C".to_string(),
            position: Point3::new(0.0, 0.0, 0.0),
            charge: 0.0,
        }],
    );

    let error = to_gro(&structure, "no-cell").expect_err("should fail");
    assert!(error.to_string().contains("simulation box"));
}
