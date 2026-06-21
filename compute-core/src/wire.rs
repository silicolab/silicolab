//! The transport-agnostic engine-job contract.
//!
//! An [`EngineRequest`] names an engine, carries its typed input, and pins a core
//! count; [`run_job`] runs it under a chosen [`Executor`] — on a worker thread in
//! this process, or in a self-exec'd subprocess — and returns a [`Running`] handle
//! that streams progress, cancels, and yields the [`EngineOutcome`]. The
//! subprocess path uses the same `request.json`/`outcome.json` files an
//! out-of-process worker reads and writes, so [`exec`] is the one engine entry
//! every out-of-process path shares.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::engines::qm::{QmJob, QmOutcome};
use crate::workflows::qm::{QmCalculationProgress, run_qm_calculation};

/// A complete engine job, independent of where it runs. `cores` travels with the
/// request so an in-process pool, a subprocess, and a remote worker all size the
/// engine's thread pool the same way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRequest {
    pub engine: Engine,
    #[serde(default)]
    pub cores: Option<usize>,
}

impl EngineRequest {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            cores: None,
        }
    }

    pub fn with_cores(engine: Engine, cores: Option<usize>) -> Self {
        Self { engine, cores }
    }
}

/// The engine to run and its typed input. A new engine is a new variant here (with
/// the matching [`EngineOutcome`] variant) — never a new transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Engine {
    Qm(QmJob),
}

/// The typed result of an [`EngineRequest`], discriminated to match [`Engine`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineOutcome {
    Qm(QmOutcome),
}

/// Where a job runs.
pub enum Executor {
    /// A worker thread in this process — the default for built-ins, so tiny jobs
    /// stay instant.
    InProcess,
    /// A self-exec'd subprocess, giving an OS-level crash/out-of-memory boundary
    /// and a kill-based cancel.
    LocalSubprocess,
}

/// A coarse progress update or the terminal result, as it arrives from a job.
pub enum JobUpdate {
    Progress {
        stage: String,
    },
    /// Boxed: a finished outcome carries a full structure, far larger than the
    /// other variants, and most messages on the channel are `Progress`.
    Finished(Box<EngineOutcome>),
    Failed(String),
}

/// A live job. Poll [`Running::updates`] for progress and the final result;
/// [`Running::cancel`] stops it — cooperatively in-process, by killing the child
/// for a subprocess.
pub struct Running {
    cancel: CancelHandle,
    updates: Receiver<JobUpdate>,
}

enum CancelHandle {
    /// The cooperative flag the in-process engine polls.
    Flag(Arc<AtomicBool>),
    /// The subprocess to kill.
    Child(Arc<Mutex<Child>>),
}

impl Running {
    pub fn updates(&self) -> &Receiver<JobUpdate> {
        &self.updates
    }

    pub fn cancel(&self) {
        match &self.cancel {
            CancelHandle::Flag(flag) => flag.store(true, Ordering::SeqCst),
            CancelHandle::Child(child) => {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        }
    }

    /// The cooperative cancel flag, when the job runs in-process. Lets a caller
    /// that already speaks the flag protocol (the engine-specific UI handles) share
    /// one cancel signal with the worker. `None` for a subprocess job, which
    /// cancels by kill instead.
    pub fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        match &self.cancel {
            CancelHandle::Flag(flag) => Some(Arc::clone(flag)),
            CancelHandle::Child(_) => None,
        }
    }
}

/// Run `request` under `executor`, returning a handle to the live job.
pub fn run_job(request: EngineRequest, executor: Executor) -> Running {
    match executor {
        Executor::InProcess => run_in_process(request),
        Executor::LocalSubprocess => run_subprocess(request),
    }
}

fn run_in_process(request: EngineRequest) -> Running {
    let (sender, updates) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let result = run_request(request, cancel_for_worker, |stage| {
            let _ = sender.send(JobUpdate::Progress { stage });
        });
        let _ = sender.send(match result {
            Ok(outcome) => JobUpdate::Finished(Box::new(outcome)),
            Err(error) => JobUpdate::Failed(error.to_string()),
        });
    });
    Running {
        cancel: CancelHandle::Flag(cancel),
        updates,
    }
}

fn run_subprocess(request: EngineRequest) -> Running {
    let (sender, updates) = mpsc::channel();
    match stage_subprocess(&request) {
        Ok((child, run_dir)) => {
            let child = Arc::new(Mutex::new(child));
            let monitor = Arc::clone(&child);
            std::thread::spawn(move || {
                let outcome_path = run_dir.join("outcome.json");
                let result = wait_for_subprocess(&monitor, &outcome_path);
                let _ = std::fs::remove_dir_all(&run_dir);
                let _ = sender.send(match result {
                    Ok(outcome) => JobUpdate::Finished(Box::new(outcome)),
                    Err(error) => JobUpdate::Failed(error.to_string()),
                });
            });
            Running {
                cancel: CancelHandle::Child(child),
                updates,
            }
        }
        Err(error) => {
            let _ = sender.send(JobUpdate::Failed(error.to_string()));
            Running {
                cancel: CancelHandle::Flag(Arc::new(AtomicBool::new(false))),
                updates,
            }
        }
    }
}

/// Stage `request.json` into a fresh per-run directory and self-exec the running
/// executable to process it. `current_exe` is re-resolved here, per launch, so a
/// mid-session self-update that replaces the on-disk image is picked up.
fn stage_subprocess(request: &EngineRequest) -> Result<(Child, PathBuf)> {
    let run_dir = std::env::temp_dir().join(format!("silicolab-job-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("create run directory {}", run_dir.display()))?;
    let request_path = run_dir.join("request.json");
    let outcome_path = run_dir.join("outcome.json");
    let json = serde_json::to_vec(request).context("serialize engine request")?;
    std::fs::write(&request_path, json)
        .with_context(|| format!("write {}", request_path.display()))?;
    let exe = std::env::current_exe().context("resolve current executable")?;
    let child = Command::new(exe)
        .arg("exec")
        .arg(&request_path)
        .arg(&outcome_path)
        .spawn()
        .context("spawn engine subprocess")?;
    Ok((child, run_dir))
}

fn wait_for_subprocess(child: &Arc<Mutex<Child>>, outcome_path: &Path) -> Result<EngineOutcome> {
    let status = loop {
        {
            let mut guard = child
                .lock()
                .map_err(|_| anyhow::anyhow!("engine subprocess handle was poisoned"))?;
            if let Some(status) = guard.try_wait().context("poll engine subprocess")? {
                break status;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        bail!("engine subprocess exited without success ({status})");
    }
    let bytes =
        std::fs::read(outcome_path).with_context(|| format!("read {}", outcome_path.display()))?;
    serde_json::from_slice(&bytes).context("parse engine outcome")
}

/// Run an engine request in-process, reporting coarse stages through `progress`.
/// Shared by the in-process executor and the [`exec`] subcommand so local and
/// out-of-process runs go through identical engine code.
fn run_request(
    request: EngineRequest,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(String) + Send,
) -> Result<EngineOutcome> {
    let EngineRequest { engine, cores } = request;
    match engine {
        Engine::Qm(job) => {
            let result =
                run_qm_calculation(job, cores, cancel, |QmCalculationProgress { stage }| {
                    progress(stage)
                })?;
            Ok(EngineOutcome::Qm(result.outcome))
        }
    }
}

/// Process a staged `request.json` and write `outcome.json`. This is the engine
/// entry a subprocess (and, later, a remote worker) runs; malformed input fails
/// the parse and returns an error, so the process exits non-zero.
pub fn exec(request_path: &Path, outcome_path: &Path) -> Result<()> {
    let bytes =
        std::fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    let request: EngineRequest = serde_json::from_slice(&bytes).context("parse engine request")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let outcome = run_request(request, cancel, |stage| eprintln!("{stage}"))?;
    let json = serde_json::to_vec(&outcome).context("serialize engine outcome")?;
    std::fs::write(outcome_path, json)
        .with_context(|| format!("write {}", outcome_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::*;
    use crate::domain::{Atom, Structure};
    use crate::engines::qm::{QmKind, QmMethod, QmOptions, QmOutcome, QmRequest};

    fn h2_single_point() -> EngineRequest {
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
        EngineRequest::new(Engine::Qm(QmJob::Molecular(QmRequest {
            structure,
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
        })))
    }

    #[test]
    fn engine_request_round_trips() {
        let request = h2_single_point();
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        match back.engine {
            Engine::Qm(QmJob::Molecular(req)) => {
                assert_eq!(req.basis, "sto-3g");
                assert_eq!(req.structure.atoms.len(), 2);
            }
            _ => panic!("expected a molecular QM request"),
        }
    }

    #[test]
    fn engine_outcome_round_trips_with_optimized_structure() {
        // Exercises the Option<Structure> wire adapter's Some branch — an optimize
        // job returns relaxed geometry, where a single point returns None.
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
                    position: Point3::new(0.0, 0.0, 0.71),
                    charge: 0.0,
                },
            ],
        );
        let outcome = EngineOutcome::Qm(QmOutcome {
            energy_hartree: -1.117,
            converged: true,
            optimized_structure: Some(structure),
            summary: "relaxed".to_string(),
        });
        let json = serde_json::to_vec(&outcome).unwrap();
        let EngineOutcome::Qm(back) = serde_json::from_slice(&json).unwrap();
        let relaxed = back
            .optimized_structure
            .expect("optimized structure survives the wire");
        assert_eq!(relaxed.atoms.len(), 2);
        assert!((relaxed.atoms[1].position.z - 0.71).abs() < 1e-6);
        assert!(back.converged);
    }

    #[test]
    fn periodic_request_round_trips_with_cell() {
        use crate::domain::{Structure, UnitCell};
        use crate::engines::qm::PeriodicQmRequest;

        let structure = Structure::with_cell(
            "si",
            vec![Atom {
                element: "Si".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            UnitCell::from_parameters(5.43, 5.43, 5.43, 90.0, 90.0, 90.0),
        );
        let request = EngineRequest::new(Engine::Qm(QmJob::Periodic(PeriodicQmRequest::new(
            structure,
        ))));
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        match back.engine {
            Engine::Qm(QmJob::Periodic(req)) => {
                assert!(req.structure.cell.is_some());
            }
            _ => panic!("expected a periodic QM request"),
        }
    }

    #[test]
    fn in_process_runs_to_completion() {
        let running = run_job(h2_single_point(), Executor::InProcess);
        let outcome = loop {
            match running.updates().recv().expect("worker stays alive") {
                JobUpdate::Finished(outcome) => break outcome,
                JobUpdate::Failed(error) => panic!("in-process job failed: {error}"),
                JobUpdate::Progress { .. } => {}
            }
        };
        let EngineOutcome::Qm(outcome) = *outcome;
        assert!(outcome.converged);
    }

    #[test]
    fn in_process_and_exec_agree_within_tolerance() {
        // Parity is to convergence tolerance, not bit-for-bit: the same source runs
        // both paths, so a small SCF-level delta is the only allowed difference.
        let in_process = run_job(h2_single_point(), Executor::InProcess);
        let local = loop {
            match in_process.updates().recv().expect("worker stays alive") {
                JobUpdate::Finished(outcome) => {
                    let EngineOutcome::Qm(outcome) = *outcome;
                    break outcome;
                }
                JobUpdate::Failed(error) => panic!("in-process job failed: {error}"),
                JobUpdate::Progress { .. } => {}
            }
        };

        let dir = std::env::temp_dir().join("silicolab-exec-parity");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let request_path = dir.join("request.json");
        let outcome_path = dir.join("outcome.json");
        std::fs::write(
            &request_path,
            serde_json::to_vec(&h2_single_point()).unwrap(),
        )
        .unwrap();
        exec(&request_path, &outcome_path).expect("exec succeeds");
        let bytes = std::fs::read(&outcome_path).unwrap();
        let EngineOutcome::Qm(via_exec) = serde_json::from_slice(&bytes).unwrap();

        assert!(via_exec.converged);
        assert!(
            (local.energy_hartree - via_exec.energy_hartree).abs() < 1e-6,
            "in-process {} vs exec {} exceeded SCF tolerance",
            local.energy_hartree,
            via_exec.energy_hartree
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
