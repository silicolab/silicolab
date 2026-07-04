use std::sync::{Arc, atomic::AtomicBool, mpsc::Receiver};
use std::time::Duration;

use eframe::egui;

use crate::domain::Structure;
use crate::engines::forcefield::{OptimizationOptions, OptimizationReport};
use crate::workflows::optimization::{
    GeometryOptimizationProgress, GeometryOptimizationRequest, run_geometry_optimization,
};

pub const OPTIMIZATION_POLL_FRAME: Duration = Duration::from_millis(50);

pub struct RunningOptimization {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<OptimizationWorkerMessage>,
    pub latest_report: Option<OptimizationReport>,
    /// Live `[step, energy]` trace accumulated from progress reports; feeds
    /// the task panel's chart while the job runs and dies with the handle.
    pub energy_trace: Vec<[f64; 2]>,
}

pub enum OptimizationWorkerMessage {
    Progress {
        structure: Structure,
        report: OptimizationReport,
    },
    Finished {
        structure: Structure,
        report: OptimizationReport,
    },
    Failed(String),
}

pub fn spawn_optimization_job(
    structure: Structure,
    options: OptimizationOptions,
) -> anyhow::Result<RunningOptimization> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let result = run_geometry_optimization(
            GeometryOptimizationRequest { structure, options },
            cancel_for_worker,
            |GeometryOptimizationProgress { structure, report }| {
                sender
                    .send(OptimizationWorkerMessage::Progress { structure, report })
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(OptimizationWorkerMessage::Finished {
                    structure: result.structure,
                    report: result.report,
                });
            }
            Err(error) => {
                let _ = sender.send(OptimizationWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    Ok(RunningOptimization {
        cancel,
        receiver,
        latest_report: None,
        energy_trace: Vec::new(),
    })
}

pub fn optimization_finished_message(report: OptimizationReport) -> String {
    if report.timed_out {
        return format!(
            "forcefield optimization timed out: energy {:.3} -> {:.3} in {} steps",
            report.initial_energy, report.final_energy, report.steps
        );
    }
    if report.stopped {
        return format!(
            "forcefield optimization stopped: energy {:.3} -> {:.3} in {} steps",
            report.initial_energy, report.final_energy, report.steps
        );
    }

    format!(
        "forcefield optimized: energy {:.3} -> {:.3} in {} steps{}",
        report.initial_energy,
        report.final_energy,
        report.steps,
        if report.converged { " (converged)" } else { "" }
    )
}

pub fn request_next_optimization_poll(ctx: &egui::Context) {
    ctx.request_repaint_after(OPTIMIZATION_POLL_FRAME);
}
