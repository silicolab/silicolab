//! Composed quantum-chemistry calculation.
//!
//! A thin wrapper over the [`crate::engines::qm`] engine that gives the frontend
//! a workflow-layer entry point and a progress type, keeping the
//! frontend → workflows → engines layering intact. It dispatches a [`QmJob`] to
//! the molecular or periodic engine path; both return a [`QmOutcome`], so the
//! caller's plumbing is agnostic to which ran. hartree runs the whole calculation
//! in one opaque call, so progress is a coarse stage label rather than a
//! per-step structure.

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Result;

use crate::engines::qm::{QmJob, QmOutcome, run_periodic_qm, run_qm};

/// A coarse progress update (`"running scf"`, `"collecting results"`, …).
pub struct QmCalculationProgress {
    pub stage: String,
}

/// The completed calculation.
pub struct QmCalculationResult {
    pub outcome: QmOutcome,
}

/// Run a QM calculation, optionally capping parallelism to `threads` cores.
///
/// hartree parallelizes via the global rayon pool. Wrapping the calculation in
/// `ThreadPool::install` makes hartree's internal `par_iter` adopt the current
/// thread's pool, capping it to `n` threads per job without restarting the
/// process. The fallback to the global pool ensures this never panics on a bad
/// thread count.
pub fn run_qm_calculation(
    job: QmJob,
    threads: Option<usize>,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(QmCalculationProgress) + Send,
) -> Result<QmCalculationResult> {
    let run = move || -> Result<QmCalculationResult> {
        let mut report = |stage: &str| {
            progress(QmCalculationProgress {
                stage: stage.to_string(),
            });
        };
        let outcome = match job {
            QmJob::Molecular(request) => run_qm(request, cancel, &mut report)?,
            QmJob::Periodic(request) => run_periodic_qm(request, cancel, &mut report)?,
        };
        Ok(QmCalculationResult { outcome })
    };
    match threads {
        // hartree's internal global-pool par_iter adopts the current thread's
        // pool, so running inside `install` caps it to n threads. Never panic on
        // a bad core count — fall back to the global pool.
        Some(n) if n >= 1 => match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
            Ok(pool) => pool.install(run),
            Err(_) => run(),
        },
        _ => run(),
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::run_qm_calculation;
    use crate::{
        domain::{Atom, Structure},
        engines::qm::{QmJob, QmKind, QmMethod, QmRequest},
    };

    #[test]
    fn qm_workflow_runs_with_capped_threads() {
        let structure = Structure::new(
            "h2",
            vec![
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".into(),
                    position: Point3::new(0.0, 0.0, 0.74),
                    charge: 0.0,
                },
            ],
        );
        let result = run_qm_calculation(
            QmJob::Molecular(QmRequest {
                structure,
                method: QmMethod::Rhf,
                basis: "sto-3g".into(),
                charge: 0,
                multiplicity: 1,
                kind: QmKind::SinglePoint,
                options: Default::default(),
                ts: None,
            }),
            Some(2),
            Default::default(),
            |_progress| {},
        )
        .expect("capped-thread workflow should succeed");
        assert!(result.outcome.converged);
    }

    #[test]
    fn qm_workflow_runs_single_point() {
        let structure = Structure::new(
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
        );

        let mut stages = Vec::new();
        let result = run_qm_calculation(
            QmJob::Molecular(QmRequest {
                structure,
                method: QmMethod::Rhf,
                basis: "sto-3g".to_string(),
                charge: 0,
                multiplicity: 1,
                kind: QmKind::SinglePoint,
                options: crate::engines::qm::QmOptions {
                    compute_properties: true,
                    ..Default::default()
                },
                ts: None,
            }),
            None,
            Default::default(),
            |progress| stages.push(progress.stage),
        )
        .expect("workflow should succeed");

        assert!(result.outcome.converged);
        // `compute_properties` was requested, so the report includes the dipole.
        assert!(result.outcome.summary.contains("dipole"));
        assert!(!stages.is_empty(), "expected at least one progress stage");
    }
}
