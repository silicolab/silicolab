use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::Duration,
};

use eframe::egui;

use crate::{
    domain::Structure,
    engines::{
        forcefield::{OptimizationOptions, OptimizationReport},
        gromacs::{
            BuildRequest, GromacsProgress, MaterialBuildRequest, StageResult, StageSpec,
            TopologySource, build_material_system, build_system, prepare_system, run_pipeline,
        },
        qm::{QmOutcome, QmRequest},
        registry::EngineLaunch,
    },
    frontend::md_support::{FrameworkRunMetadata, MD_FRAMEWORK_FILE, write_md_system_context},
    workflows::{
        optimization::{
            GeometryOptimizationProgress, GeometryOptimizationRequest, run_geometry_optimization,
        },
        qm::{QmCalculationProgress, run_qm_calculation},
    },
};

pub const OPTIMIZATION_POLL_FRAME: Duration = Duration::from_millis(50);

pub struct RunningOptimization {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<OptimizationWorkerMessage>,
    pub latest_report: Option<OptimizationReport>,
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

/// A background quantum-chemistry (chemx) job the UI is polling.
pub struct RunningQmJob {
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<QmWorkerMessage>,
}

pub enum QmWorkerMessage {
    Progress { stage: String },
    Finished(Box<QmOutcome>),
    Failed(String),
}

/// Streaming messages produced by an external-engine worker.
pub enum EngineWorkerMessage {
    Stage(String),
    Log(String),
    Finished(Box<EngineSuccess>),
    Failed(String),
}

/// Aggregated information about a successful engine run that should be
/// surfaced to the UI / project state.
#[allow(dead_code)]
pub struct EngineSuccess {
    pub engine: &'static str,
    pub job_kind: &'static str,
    pub structure: Structure,
    pub summary: String,
    pub working_dir: PathBuf,
    /// Trajectory file produced by the run (the production stage's `.xtc`), if
    /// any. Used to mark the resulting entry as an MD-run output that can be
    /// played back; `None` for build jobs.
    pub trajectory: Option<PathBuf>,
}

pub struct GromacsPipelineRequest {
    pub structure: Structure,
    pub topology: TopologySource,
    pub stages: Vec<StageSpec>,
    pub working_dir: PathBuf,
    pub gmx_launch: EngineLaunch,
    pub max_duration_per_stage: Duration,
    /// Atoms to freeze (a rigid framework's sheet); `None` for an ordinary run.
    pub freeze: Option<crate::engines::gromacs::FreezeSelection>,
}

/// A background engine job that the UI is currently polling.
pub struct RunningEngineJob {
    pub engine: &'static str,
    pub job_kind: &'static str,
    pub cancel: Arc<AtomicBool>,
    pub receiver: Receiver<EngineWorkerMessage>,
    pub latest_stage: Option<String>,
    pub log_tail: Vec<String>,
}

impl RunningEngineJob {
    pub fn append_log(&mut self, line: String) {
        self.log_tail.push(line);
        if self.log_tail.len() > 200 {
            let drop = self.log_tail.len() - 200;
            self.log_tail.drain(0..drop);
        }
    }
}

/// The once-per-launch background query of GitHub Releases. No cancel flag:
/// the single HTTP request either answers or times out on its own, and the
/// result is ignored if the handle was dropped.
pub struct RunningUpdateCheck {
    pub receiver: Receiver<anyhow::Result<Option<crate::io::update_check::AvailableUpdate>>>,
}

/// Spawn the update check on a worker thread and return the polling handle.
pub fn spawn_update_check() -> RunningUpdateCheck {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(crate::io::update_check::check_for_update());
    });
    RunningUpdateCheck { receiver }
}

#[derive(Default)]
pub struct JobManager {
    pub optimizer: Option<RunningOptimization>,
    pub qm: Option<RunningQmJob>,
    pub engine: Option<RunningEngineJob>,
    /// In-flight background decode of an entry's trajectory file for playback.
    pub trajectory_load: Option<crate::frontend::trajectory::RunningTrajectoryLoad>,
    /// In-flight check of GitHub Releases for a newer version (startup, or the
    /// moment the setting is switched on).
    pub update_check: Option<RunningUpdateCheck>,
}

impl JobManager {
    pub fn optimization_running(&self) -> bool {
        self.optimizer.is_some()
    }

    pub fn take_optimizer(&mut self) -> Option<RunningOptimization> {
        self.optimizer.take()
    }

    pub fn set_optimizer(&mut self, optimizer: RunningOptimization) {
        self.optimizer = Some(optimizer);
    }

    pub fn cancel_optimization(&mut self) {
        if let Some(running) = self.optimizer.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn qm_running(&self) -> bool {
        self.qm.is_some()
    }

    pub fn take_qm(&mut self) -> Option<RunningQmJob> {
        self.qm.take()
    }

    pub fn set_qm(&mut self, qm: RunningQmJob) {
        self.qm = Some(qm);
    }

    pub fn cancel_qm(&mut self) {
        if let Some(running) = self.qm.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn engine_running(&self) -> bool {
        self.engine.is_some()
    }

    pub fn take_engine(&mut self) -> Option<RunningEngineJob> {
        self.engine.take()
    }

    pub fn set_engine(&mut self, engine: RunningEngineJob) {
        self.engine = Some(engine);
    }

    pub fn cancel_engine(&mut self) {
        if let Some(running) = self.engine.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
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
    })
}

/// Spawn a quantum-chemistry calculation on a worker thread and return the live
/// handle. The worker streams coarse stage updates, then a `Finished` outcome or
/// `Failed` error. Caller stores the handle in [`JobManager`].
pub fn spawn_qm_job(request: QmRequest) -> RunningQmJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let progress_sender = sender.clone();
        let result = run_qm_calculation(
            request,
            cancel_for_worker,
            move |QmCalculationProgress { stage }| {
                let _ = progress_sender.send(QmWorkerMessage::Progress { stage });
            },
        );
        match result {
            Ok(result) => {
                let _ = sender.send(QmWorkerMessage::Finished(Box::new(result.outcome)));
            }
            Err(error) => {
                let _ = sender.send(QmWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningQmJob { cancel, receiver }
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

/// Spawn a multi-step GROMACS pipeline as a background engine job and return
/// the live handle. Caller is responsible for storing it in [`JobManager`].
pub fn spawn_gromacs_pipeline_job(request: GromacsPipelineRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        let system = prepare_system(crate::engines::gromacs::PrepareSystemRequest {
            structure: request.structure,
            topology: request.topology,
            working_dir: request.working_dir,
            freeze: request.freeze,
        });
        let outcome = system.and_then(|system| {
            run_pipeline(
                system,
                request.stages,
                request.gmx_launch,
                request.max_duration_per_stage,
                cancel_for_worker,
                move |progress| match progress {
                    GromacsProgress::Stage(stage) => {
                        let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
                    }
                    GromacsProgress::Log(line) => {
                        let _ = report_sender.send(EngineWorkerMessage::Log(line));
                    }
                },
            )
        });

        match outcome {
            Ok(results) => {
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(
                    engine_success_from_gromacs_pipeline(results),
                )));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "run-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

/// Spawn the GROMACS system-build pipeline (pdb2gmx → editconf → solvate →
/// genion) as a background engine job. The build writes `topol.top` into
/// `request.working_dir` (the build task's run directory), which a later MD run
/// reuses as its force-field topology. Caller stores the handle in
/// [`JobManager`].
pub fn spawn_gromacs_build_job(request: BuildRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    // Capture the build inputs the run-MD recommendation later inherits, before
    // `request` is consumed by the build. The solute (not the solvated output)
    // carries the residue metadata system-type detection reads.
    let force_field_token = request.force_field.clone();
    let water_token = request
        .solvate
        .then(|| request.water.db_token().to_string());
    let solute = request.structure.clone();

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        let outcome = build_system(request, cancel_for_worker, move |progress| match progress {
            GromacsProgress::Stage(stage) => {
                let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
            }
            GromacsProgress::Log(line) => {
                let _ = report_sender.send(EngineWorkerMessage::Log(line));
            }
        });

        match outcome {
            Ok(outcome) => {
                // pdb2gmx writes posre.itp, giving the run a "solute" restraint
                // group; record it so restrained equilibration validates.
                let restraint_groups = if outcome.working_dir.join("posre.itp").exists() {
                    vec!["solute".to_string()]
                } else {
                    Vec::new()
                };
                // A successful build with genion neutralization is net-neutral; the
                // exact charge is not parsed back from topol.top here.
                write_md_system_context(
                    &outcome.working_dir,
                    &solute,
                    outcome.structure.atoms.len(),
                    &force_field_token,
                    water_token.as_deref(),
                    false,
                    0.0,
                    false,
                    restraint_groups,
                );
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(EngineSuccess {
                    engine: "gromacs",
                    job_kind: "build-md",
                    structure: outcome.structure,
                    summary: outcome.summary,
                    working_dir: outcome.working_dir,
                    trajectory: None,
                })));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "build-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

/// Spawn the framework (nanosheet) build as a background engine job: it
/// generates the topology directly from the structure's bonds and optionally
/// solvates, writing `topol.top` and `framework_run.json` into
/// `request.working_dir` so a later MD run reuses both. Reported as a `build-md`
/// success, so the same completion handling adds the boxed entry.
pub fn spawn_material_build_job(request: MaterialBuildRequest) -> RunningEngineJob {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);

    // A framework is not a biomolecule: it has no biomolecular force-field
    // convention (token classifies to the generic family) and uses freeze groups
    // rather than position restraints. Capture the solvent model and solute before
    // the request is consumed.
    let water_token = request
        .solvation
        .as_ref()
        .map(|solvation| solvation.water.db_token().to_string());
    let solute = request.structure.clone();

    std::thread::spawn(move || {
        let report_sender = sender.clone();
        let outcome =
            build_material_system(request, cancel_for_worker, move |progress| match progress {
                GromacsProgress::Stage(stage) => {
                    let _ = report_sender.send(EngineWorkerMessage::Stage(stage));
                }
                GromacsProgress::Log(line) => {
                    let _ = report_sender.send(EngineWorkerMessage::Log(line));
                }
            });

        match outcome {
            Ok(outcome) => {
                // Record the run hints so the MD run applies periodic-molecules
                // / freeze settings; a write failure is non-fatal (the run falls
                // back to plain settings).
                let meta = FrameworkRunMetadata {
                    periodic_molecules: outcome.hints.periodic_molecules,
                    freeze_group: outcome.hints.freeze_group.clone(),
                    framework_atom_count: outcome.framework_atom_count,
                };
                let _ = meta.save(&outcome.working_dir.join(MD_FRAMEWORK_FILE));
                write_md_system_context(
                    &outcome.working_dir,
                    &solute,
                    outcome.structure.atoms.len(),
                    "framework",
                    water_token.as_deref(),
                    true,
                    0.0,
                    false,
                    Vec::new(),
                );
                let _ = sender.send(EngineWorkerMessage::Finished(Box::new(EngineSuccess {
                    engine: "gromacs",
                    job_kind: "build-md",
                    structure: outcome.structure,
                    summary: outcome.summary,
                    working_dir: outcome.working_dir,
                    trajectory: None,
                })));
            }
            Err(error) => {
                let _ = sender.send(EngineWorkerMessage::Failed(error.to_string()));
            }
        }
    });

    RunningEngineJob {
        engine: "gromacs",
        job_kind: "build-md",
        cancel,
        receiver,
        latest_stage: None,
        log_tail: Vec::new(),
    }
}

fn engine_success_from_gromacs_pipeline(results: Vec<StageResult>) -> EngineSuccess {
    let stage_count = results.len();
    let final_result = results
        .last()
        .expect("successful GROMACS pipeline must yield at least one stage");
    let stage = final_result.stage_name.clone();
    let summary = match final_result.final_potential_energy {
        Some(energy) => format!(
            "GROMACS MD complete: {stage_count} steps, final stage {stage}, E = {energy:.3} kJ/mol in {:.2?}",
            final_result.wall_time
        ),
        None => format!(
            "GROMACS MD complete: {stage_count} steps, final stage {stage} in {:.2?}",
            final_result.wall_time
        ),
    };
    // The production stage writes the compressed `.xtc`; take the last stage
    // that produced one so playback follows the actual MD trajectory.
    let trajectory = results
        .iter()
        .rev()
        .find_map(|stage| stage.trajectory.clone());
    EngineSuccess {
        engine: "gromacs",
        job_kind: "run-md",
        structure: final_result.structure.clone(),
        summary,
        working_dir: final_result.working_dir.clone(),
        trajectory,
    }
}

pub fn engine_poll_frame() -> Duration {
    OPTIMIZATION_POLL_FRAME
}
