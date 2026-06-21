use std::sync::{Arc, atomic::AtomicBool};

use anyhow::Result;

use crate::{
    domain::Structure,
    engines::forcefield::{
        GeometryOptimizer, OptimizationControl, OptimizationOptions, OptimizationReport,
    },
};

pub struct GeometryOptimizationRequest {
    pub structure: Structure,
    pub options: OptimizationOptions,
}

pub struct GeometryOptimizationProgress {
    pub structure: Structure,
    pub report: OptimizationReport,
}

pub struct GeometryOptimizationResult {
    pub structure: Structure,
    pub report: OptimizationReport,
}

pub fn run_geometry_optimization(
    request: GeometryOptimizationRequest,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(GeometryOptimizationProgress) -> Result<()>,
) -> Result<GeometryOptimizationResult> {
    let GeometryOptimizationRequest {
        mut structure,
        options,
    } = request;
    let max_duration = options.max_duration;
    let mut optimizer = GeometryOptimizer::new(&structure, options)?;
    let control = OptimizationControl::new(cancel, max_duration);

    loop {
        let done = optimizer.step_with_control(&mut structure, Some(&control))?;
        let report = optimizer.report();
        if done {
            return Ok(GeometryOptimizationResult { structure, report });
        }

        progress(GeometryOptimizationProgress {
            structure: structure.clone(),
            report,
        })?;
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use crate::{
        domain::{Atom, Bond, BondType, Structure},
        engines::forcefield::OptimizationOptions,
        workflows::optimization::{GeometryOptimizationRequest, run_geometry_optimization},
    };

    #[test]
    fn geometry_optimization_workflow_returns_relaxed_structure() {
        let structure = Structure::with_bonds(
            "stretched hydrogen",
            vec![
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(2.0, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
        );
        let initial_distance = (structure.atoms[0].position - structure.atoms[1].position).norm();

        let result = run_geometry_optimization(
            GeometryOptimizationRequest {
                structure,
                options: OptimizationOptions::default(),
            },
            Default::default(),
            |_| Ok(()),
        )
        .unwrap();
        let final_distance =
            (result.structure.atoms[0].position - result.structure.atoms[1].position).norm();

        assert!(final_distance < initial_distance);
        assert!(result.report.steps > 0);
    }
}
