use super::*;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::domain::{Atom, Structure};
use nalgebra::Point3;

fn h2() -> Structure {
    // H2 near its equilibrium bond length (~0.74 Å).
    Structure::new(
        "h2",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.74),
                charge: 0.0,
            },
        ],
    )
}

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// A bare single point, the simplest request.
fn request(structure: Structure, method: QmMethod, basis: &str, kind: QmKind) -> QmRequest {
    QmRequest {
        structure,
        method,
        basis: basis.to_string(),
        charge: 0,
        multiplicity: 1,
        kind,
        options: QmOptions::default(),
    }
}

#[test]
fn rhf_sto3g_h2_single_point_energy() {
    let outcome = run_qm(
        request(h2(), QmMethod::Rhf, "sto-3g", QmKind::SinglePoint),
        no_cancel(),
        |_| {},
    )
    .expect("RHF/STO-3G on H2 should succeed");

    assert!(outcome.converged);
    // RHF/STO-3G H2 at 0.74 Å is about -1.117 Eh.
    assert!(
        (outcome.energy_hartree - (-1.117)).abs() < 0.02,
        "unexpected H2 energy: {}",
        outcome.energy_hartree
    );
    assert!(outcome.optimized_structure.is_none());
}

#[test]
fn method_parse_splits_dispersion_suffix() {
    assert_eq!(
        QmMethod::parse("pbe-d4"),
        (QmMethod::Dft("pbe".to_string()), Some(QmDispersion::D4))
    );
    assert_eq!(
        QmMethod::parse("b3lyp-d3"),
        (QmMethod::Dft("b3lyp".to_string()), Some(QmDispersion::D3Bj))
    );
    // A composite keyword is recognized and carries no separate dispersion.
    assert_eq!(
        QmMethod::parse("r2scan-3c"),
        (QmMethod::Composite("r2scan-3c".to_string()), None)
    );
    assert_eq!(QmMethod::parse("ccsd(t)"), (QmMethod::CcsdT, None));
}

#[test]
fn structure_to_molecule_preserves_atoms() {
    let mol = molecule_from_structure(&h2(), 0, 1).expect("valid molecule");
    assert_eq!(mol.len(), 2);
}

#[test]
fn invalid_spin_is_rejected() {
    // Two electrons (H2, neutral) cannot be a doublet.
    let err = molecule_from_structure(&h2(), 0, 2);
    assert!(err.is_err());
}

#[test]
fn invalid_element_is_rejected_with_atom_index() {
    // A hand-drawn atom whose element was mistyped (e.g. a stray bracket).
    let mut structure = h2();
    structure.atoms[1].element = "（".to_string();
    let message = molecule_from_structure(&structure, 0, 1)
        .expect_err("invalid element should be rejected")
        .to_string();
    assert!(
        message.contains("atom 2") && message.contains("（"),
        "error should name the offending atom and symbol: {message}"
    );
}

#[test]
fn blank_element_is_rejected() {
    let mut structure = h2();
    structure.atoms[0].element = "  ".to_string();
    assert!(molecule_from_structure(&structure, 0, 1).is_err());
}

#[test]
fn optimize_h2_returns_relaxed_structure() {
    let outcome = run_qm(
        request(h2(), QmMethod::Rhf, "sto-3g", QmKind::Optimize),
        no_cancel(),
        |_| {},
    )
    .expect("RHF/STO-3G H2 optimization should succeed");

    let relaxed = outcome
        .optimized_structure
        .expect("optimize should return a structure");
    assert_eq!(relaxed.atoms.len(), 2);
    // Optimized H-H bond length should be a sane, positive value in Å.
    let d = (relaxed.atoms[1].position - relaxed.atoms[0].position).norm();
    assert!(
        (0.5..1.2).contains(&d),
        "optimized H-H distance out of range: {d} Å"
    );
}

#[test]
fn dispersion_reported_in_summary() {
    let mut req = request(
        h2(),
        QmMethod::Dft("pbe".to_string()),
        "sto-3g",
        QmKind::SinglePoint,
    );
    req.options.dispersion = Some(QmDispersion::D3Bj);
    let outcome = run_qm(req, no_cancel(), |_| {}).expect("PBE-D3/STO-3G H2 should succeed");
    assert!(
        outcome.summary.contains("dispersion"),
        "summary should report the dispersion correction: {}",
        outcome.summary
    );
}
