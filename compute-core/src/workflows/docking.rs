//! Composed molecular docking calculation.
//!
//! A thin wrapper over the [`crate::engines::docking`] engine that gives the
//! frontend a workflow-layer entry point and a progress type, keeping the
//! frontend → workflows → engines layering intact. The Vina search runs in one
//! opaque blocking call, so progress is a coarse stage label rather than a
//! per-step structure.

use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Result;

use crate::engines::docking::{DockingOutcome, DockingRequest, run_docking};

/// A coarse progress update (`"preparing ligand"`, `"searching …"`, …).
pub struct DockingProgress {
    pub stage: String,
}

/// The completed docking calculation.
pub struct DockingResult {
    pub outcome: DockingOutcome,
}

pub fn run_docking_calculation(
    request: DockingRequest,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(DockingProgress),
) -> Result<DockingResult> {
    let report = |stage: &str| {
        progress(DockingProgress {
            stage: stage.to_string(),
        })
    };
    let outcome = run_docking(request, cancel, report)?;
    Ok(DockingResult { outcome })
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::run_docking_calculation;
    use crate::{
        domain::{Atom, Bond, BondType, Structure},
        engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest},
    };

    fn carbon(x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    #[test]
    fn docking_workflow_scores_a_pose() {
        // A trivial receptor and ligand (butane skeletons); ScoreOnly is a single
        // point evaluation, so the workflow runs quickly end-to-end.
        let skeleton = || {
            Structure::with_bonds(
                "butane",
                vec![
                    carbon(0.0, 0.0, 0.0),
                    carbon(1.5, 0.0, 0.0),
                    carbon(2.2, 1.3, 0.0),
                    carbon(3.7, 1.3, 0.0),
                ],
                vec![
                    Bond::with_type(0, 1, BondType::Single),
                    Bond::with_type(1, 2, BondType::Single),
                    Bond::with_type(2, 3, BondType::Single),
                ],
            )
        };

        let request = DockingRequest {
            receptor: DockingInput::Structure(Box::new(skeleton())),
            ligand: DockingInput::Structure(Box::new(skeleton())),
            box_center: [1.8, 0.6, 0.0],
            box_size: [20.0, 20.0, 20.0],
            config: DockingConfig::default(),
            kind: DockingKind::ScoreOnly,
        };

        let mut stages = Vec::new();
        let result = run_docking_calculation(request, Default::default(), |p| stages.push(p.stage))
            .expect("docking workflow should succeed");

        assert_eq!(result.outcome.poses.len(), 1);
        assert!(result.outcome.poses[0].affinity.is_finite());
        assert!(!stages.is_empty());
        // The structure inputs were prepared heuristically, so a caveat is surfaced.
        assert!(
            result
                .outcome
                .notes
                .iter()
                .any(|n| n.contains("heuristically"))
        );
    }
}
