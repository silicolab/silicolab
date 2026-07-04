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
        ts: None,
    }
}

/// Drift guard: every basis the molecular QM panel offers must still be an
/// orbital set hartree can load. A hartree bump that renames or drops one trips
/// here rather than at run time, in front of a user who picked it.
#[test]
fn panel_bases_are_all_loadable() {
    for name in QM_BASIS_SETS {
        assert!(
            hartree::BasisSet::load(name).is_ok(),
            "panel basis `{name}` is not a loadable hartree orbital set"
        );
    }
}

/// Drift guard: every DFT functional preset must still parse in hartree. Mirrors
/// the basis guard above for the method dropdown.
#[test]
fn panel_functionals_are_all_recognized() {
    for method in QmMethod::presets() {
        if let QmMethod::Dft(name) = method {
            assert!(
                hartree::dft::FunctionalSpec::parse(&name).is_ok(),
                "panel functional `{name}` is not recognized by hartree"
            );
        }
    }
}

/// Locks which method/dispersion pairs hartree actually parametrizes, so the
/// panel only ever offers a buildable combination (and a hartree bump that shifts
/// coverage is caught here). D3(BJ) covers a small functional set; D4 adds the
/// double hybrids; composites and post-HF carry/allow none.
#[test]
fn supports_dispersion_matches_hartree_coverage() {
    use crate::engines::qm::supports_dispersion;
    let d3 = QmDispersion::D3Bj;
    let d4 = QmDispersion::D4;

    // b3lyp: both. The default panel method, so its D3(BJ) default must build.
    assert!(supports_dispersion(&QmMethod::Dft("b3lyp".into()), d3));
    assert!(supports_dispersion(&QmMethod::Dft("b3lyp".into()), d4));
    // m06-2x and the VV10 family: neither — the panel must drop a stale D3(BJ).
    assert!(!supports_dispersion(&QmMethod::Dft("m06-2x".into()), d3));
    assert!(!supports_dispersion(&QmMethod::Dft("m06-2x".into()), d4));
    assert!(!supports_dispersion(&QmMethod::Dft("wb97x-v".into()), d3));
    // b2plyp: D4 only.
    assert!(supports_dispersion(&QmMethod::Dft("b2plyp".into()), d4));
    assert!(!supports_dispersion(&QmMethod::Dft("b2plyp".into()), d3));
    // HF carries dispersion; composites and post-HF do not.
    assert!(supports_dispersion(&QmMethod::Rhf, d3));
    assert!(!supports_dispersion(
        &QmMethod::Composite("r2scan-3c".into()),
        d3
    ));
    assert!(!supports_dispersion(&QmMethod::Mp2, d3));
}

/// The new estimate-memory path returns a sane figure for a small in-core job and
/// labels its backend.
#[test]
fn estimate_request_memory_reports_incore_water() {
    let req = request(h2(), QmMethod::Rhf, "def2-svp", QmKind::SinglePoint);
    let report = crate::engines::qm::estimate_request_memory(&req, u64::MAX)
        .expect("in-core RHF/def2-svp should estimate");
    assert!(report.peak_bytes > 0);
    assert_eq!(report.backend_label, "in-core");
    assert!(report.fits(), "u64::MAX budget should always fit");
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

/// A linear H3 (doublet) near its symmetric exchange saddle — the canonical cheap
/// transition state, used to exercise the TS-option plumbing.
fn h3_linear() -> Structure {
    Structure::new(
        "h3",
        vec![
            Atom {
                element: "H".into(),
                position: Point3::new(0.0, 0.0, -0.93),
                charge: 0.0,
            },
            Atom {
                element: "H".into(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".into(),
                position: Point3::new(0.0, 0.0, 0.93),
                charge: 0.0,
            },
        ],
    )
}

fn build_ts_job(structure: &Structure, ts: Option<&QmTsConfig>) -> super::build::ResolvedJob {
    build_job(
        structure,
        &QmMethod::Uhf,
        "sto-3g",
        0,
        2,
        QmKind::TransitionState,
        &QmOptions::default(),
        ts,
    )
    .expect("TS job should build")
}

/// The TS kind sets hartree's `transition_state` flag (and not `optimize_geometry`);
/// the single-guess route leaves both guess inputs empty.
#[test]
fn ts_single_guess_sets_transition_state_flag() {
    let resolved = build_ts_job(&h3_linear(), Some(&QmTsConfig::default()));
    assert!(resolved.job.options.transition_state);
    assert!(!resolved.job.options.optimize_geometry);
    assert!(resolved.job.options.ts_guess.is_none());
    assert!(resolved.job.options.ts_coord_scan.is_none());
    // A TS request with no config behaves like the single-guess default.
    let none = build_ts_job(&h3_linear(), None);
    assert!(none.job.options.transition_state);
    assert!(none.job.options.ts_guess.is_none());
}

/// The search algorithm and IRC toggle map onto hartree's `TsOptions`.
#[test]
fn ts_options_map_algorithm_and_irc() {
    use crate::engines::qm::{QmTsAlgorithm, QmTsCoordinates};
    let ts = QmTsConfig {
        guess: QmTsGuess::Single,
        algorithm: QmTsAlgorithm::Dimer,
        coordinates: QmTsCoordinates::Internal,
        confirm_irc: true,
    };
    let resolved = build_ts_job(&h3_linear(), Some(&ts));
    let opts = &resolved.job.options.ts_options;
    assert_eq!(opts.algorithm, hartree::opt::ts::TsAlgorithm::Dimer);
    assert_eq!(opts.coordinates, hartree::opt::ts::Coordinates::Internal);
    assert!(opts.confirm_irc);
}

/// A distinguished-coordinate scan converts 1-based UI atom indices to 0-based
/// hartree internals and the range from Ångström to Bohr for a bond.
#[test]
fn ts_coord_scan_converts_indices_and_units() {
    use crate::engines::qm::{QmInternalCoordinate, QmTsCoordinateScan};
    let ts = QmTsConfig {
        guess: QmTsGuess::CoordinateScan(QmTsCoordinateScan {
            coordinate: QmInternalCoordinate::Bond(1, 3),
            start: 1.0,
            end: 2.0,
            n_points: 5,
        }),
        ..QmTsConfig::default()
    };
    let resolved = build_ts_job(&h3_linear(), Some(&ts));
    let spec = resolved
        .job
        .options
        .ts_coord_scan
        .expect("coord-scan spec set");
    assert_eq!(
        spec.coordinate,
        hartree::opt::internals::Internal::Bond(0, 2)
    );
    // 1.0 Å → ~1.889 Bohr.
    assert!((spec.start - 1.0 / BOHR_TO_ANGSTROM).abs() < 1e-9);
    assert_eq!(spec.n_points, 5);
}

/// A coordinate scan rejects an out-of-range atom index and a too-coarse grid.
#[test]
fn ts_coord_scan_validates_inputs() {
    use crate::engines::qm::{QmInternalCoordinate, QmTsCoordinateScan};
    let bad_atom = QmTsConfig {
        guess: QmTsGuess::CoordinateScan(QmTsCoordinateScan {
            // h3 has 3 atoms; atom 9 is out of range.
            coordinate: QmInternalCoordinate::Bond(1, 9),
            start: 1.0,
            end: 2.0,
            n_points: 5,
        }),
        ..QmTsConfig::default()
    };
    assert!(
        build_job(
            &h3_linear(),
            &QmMethod::Uhf,
            "sto-3g",
            0,
            2,
            QmKind::TransitionState,
            &QmOptions::default(),
            Some(&bad_atom),
        )
        .is_err()
    );

    // A coordinate over a repeated atom (Bond from an atom to itself) is degenerate.
    let repeated = QmTsConfig {
        guess: QmTsGuess::CoordinateScan(QmTsCoordinateScan {
            coordinate: QmInternalCoordinate::Bond(2, 2),
            start: 1.0,
            end: 2.0,
            n_points: 5,
        }),
        ..QmTsConfig::default()
    };
    assert!(
        build_job(
            &h3_linear(),
            &QmMethod::Uhf,
            "sto-3g",
            0,
            2,
            QmKind::TransitionState,
            &QmOptions::default(),
            Some(&repeated),
        )
        .is_err()
    );
}

/// Transition-state search rejects the options hartree cannot run a saddle search
/// with (post-HF, non-in-core backends, solvation), before the job is assembled.
#[test]
fn ts_rejects_incompatible_options() {
    let ts = QmTsConfig::default();
    let build = |method: QmMethod, options: QmOptions| {
        build_job(
            &h3_linear(),
            &method,
            "sto-3g",
            0,
            2,
            QmKind::TransitionState,
            &options,
            Some(&ts),
        )
    };
    // Post-HF: no analytic gradient path for the saddle search.
    assert!(build(QmMethod::Mp2, QmOptions::default()).is_err());
    // Integral-direct backend.
    let direct = QmOptions {
        scf_backend: crate::engines::qm::QmScfBackend::Direct,
        ..QmOptions::default()
    };
    assert!(build(QmMethod::Uhf, direct).is_err());
    // Implicit solvation.
    let solvated = QmOptions {
        solvation: Some(crate::engines::qm::QmSolvation::Smd("water".into())),
        ..QmOptions::default()
    };
    assert!(build(QmMethod::Uhf, solvated).is_err());
}

/// End-to-end saddle search on linear H3. Marked `#[ignore]`: a full P-RFO climb
/// with finite-difference Hessians is slow and its convergence is not a contract
/// the build guards, so it is run on demand rather than in the default suite.
#[test]
#[ignore = "slow: full transition-state search (run on demand)"]
fn ts_h3_finds_a_saddle() {
    let mut req = request(
        h3_linear(),
        QmMethod::Uhf,
        "sto-3g",
        QmKind::TransitionState,
    );
    req.multiplicity = 2;
    req.ts = Some(QmTsConfig::default());
    let outcome = run_qm(req, no_cancel(), |_| {}).expect("TS search should run");
    // The best saddle geometry is surfaced even if the search did not fully
    // converge, and the summary reports the transition-state section.
    assert!(outcome.optimized_structure.is_some());
    assert!(outcome.summary.contains("transition state"));
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

/// SinglePoint surfaces the run's SCF energies per iteration; the moving-job
/// and frequency vectors stay empty (spec's QmKind table).
#[test]
fn single_point_surfaces_scf_trace() {
    let outcome = run_qm(
        request(h2(), QmMethod::Rhf, "sto-3g", QmKind::SinglePoint),
        no_cancel(),
        |_| {},
    )
    .expect("RHF/STO-3G on H2 should succeed");
    assert!(!outcome.scf_trace.is_empty());
    let last = *outcome.scf_trace.last().unwrap();
    assert!(
        (last - outcome.energy_hartree).abs() < 1e-6,
        "trace should end at the converged energy: {last} vs {}",
        outcome.energy_hartree
    );
    assert!(outcome.opt_trace.is_empty());
    assert!(outcome.frequencies.is_empty());
}

/// Optimize surfaces the energy per optimizer step plus the final geometry's
/// SCF history (hartree keeps only the last step's SCF).
#[test]
fn optimize_h2_surfaces_energy_traces() {
    let outcome = run_qm(
        request(h2(), QmMethod::Rhf, "sto-3g", QmKind::Optimize),
        no_cancel(),
        |_| {},
    )
    .expect("RHF/STO-3G H2 optimization should succeed");
    assert!(!outcome.opt_trace.is_empty());
    assert!(
        outcome.opt_trace.last().unwrap() <= &(outcome.opt_trace[0] + 1e-6),
        "relaxation should not raise the energy: {:?}",
        outcome.opt_trace
    );
    assert!(!outcome.scf_trace.is_empty());
    assert!(outcome.frequencies.is_empty());
}

#[test]
fn frequencies_h2_surfaces_wavenumbers() {
    let outcome = run_qm(
        request(h2(), QmMethod::Rhf, "sto-3g", QmKind::Frequencies),
        no_cancel(),
        |_| {},
    )
    .expect("RHF/STO-3G H2 frequencies should succeed");
    assert!(!outcome.frequencies.is_empty());
    let max = outcome.frequencies.iter().cloned().fold(f64::MIN, f64::max);
    assert!(
        max > 1000.0,
        "H2 stretch should appear: {:?}",
        outcome.frequencies
    );
    assert!(!outcome.scf_trace.is_empty());
    assert!(outcome.opt_trace.is_empty());
}
