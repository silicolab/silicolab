//! Composed quantum-chemistry calculation.
//!
//! A thin wrapper over the [`crate::engines::qm`] engine that gives the frontend
//! a workflow-layer entry point and a progress type, keeping the
//! frontend → workflows → engines layering intact. It dispatches a [`QmJob`] to
//! the molecular or periodic engine path; both return a [`QmOutcome`], so the
//! caller's plumbing is agnostic to which ran. chemx runs the whole calculation
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

pub fn run_qm_calculation(
    job: QmJob,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(QmCalculationProgress),
) -> Result<QmCalculationResult> {
    let report = |stage: &str| {
        progress(QmCalculationProgress {
            stage: stage.to_string(),
        })
    };
    let outcome = match job {
        QmJob::Molecular(request) => run_qm(request, cancel, report)?,
        QmJob::Periodic(request) => run_periodic_qm(request, cancel, report)?,
    };
    Ok(QmCalculationResult { outcome })
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
            }),
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
