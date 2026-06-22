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

use crate::engines::docking::{DockingInput, DockingOutcome, DockingRequest};
use crate::engines::gromacs::GromacsProgress;
use crate::engines::qm::{QmJob, QmOutcome};
use crate::engines::remote::launcher::{self, Liveness, RemoteExecution};
use crate::workflows::docking::{DockingProgress, run_docking_calculation};
use crate::workflows::gromacs::{GromacsJob, GromacsOutcome, run_gromacs_calculation};
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
// The QM variant embeds a full inline `Structure`, so it dwarfs the others; this
// envelope is built once per job and immediately serialized or moved, never held
// in bulk, so boxing every variant would only add indirection to a cold path (and
// break the nested `Engine::Qm(QmJob::Molecular(..))` matching the call sites use).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Engine {
    Qm(QmJob),
    Docking(DockingRequest),
    Gromacs(GromacsJob),
}

/// The typed result of an [`EngineRequest`], discriminated to match [`Engine`].
// Same rationale as [`Engine`]: the QM outcome carries an optional optimized
// `Structure`, so it is far larger than the docking outcome; this is a per-job
// value, not a bulk-stored one.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineOutcome {
    Qm(QmOutcome),
    Docking(DockingOutcome),
    Gromacs(GromacsOutcome),
}

/// Where a job runs.
pub enum Executor {
    /// A worker thread in this process — the default for built-ins, so tiny jobs
    /// stay instant.
    InProcess,
    /// A self-exec'd subprocess, giving an OS-level crash/out-of-memory boundary
    /// and a kill-based cancel.
    LocalSubprocess,
    /// A pre-deployed headless worker on a remote host, driven over SSH behind a
    /// pluggable launcher. Boxed because a [`RemoteExecution`] is far larger than
    /// the unit variants. This is the attached run-and-wait path; the GUI's
    /// detached, durable status model drives the same launcher primitives
    /// directly instead.
    Remote(Box<RemoteExecution>),
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
        Executor::Remote(execution) => run_remote(request, execution),
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
    // Only the success path's monitor thread removes `run_dir`; clean it up here
    // if staging fails (most likely a failed `spawn`) so the dir is not leaked.
    match spawn_subprocess(request, &run_dir) {
        Ok(child) => Ok((child, run_dir)),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&run_dir);
            Err(error)
        }
    }
}

fn spawn_subprocess(request: &EngineRequest, run_dir: &Path) -> Result<Child> {
    let request_path = run_dir.join("request.json");
    let outcome_path = run_dir.join("outcome.json");
    let json = serde_json::to_vec(request).context("serialize engine request")?;
    std::fs::write(&request_path, json)
        .with_context(|| format!("write {}", request_path.display()))?;
    let exe = std::env::current_exe().context("resolve current executable")?;
    Command::new(exe)
        .arg("exec")
        .arg(&request_path)
        .arg(&outcome_path)
        .spawn()
        .context("spawn engine subprocess")
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

/// Poll cadence for the attached remote path. Modest: this is an explicit
/// run-and-wait call, not the GUI's on-demand refresh model.
const REMOTE_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Granularity at which the poll wait wakes to check the cancel flag.
const REMOTE_CANCEL_TICK: Duration = Duration::from_millis(250);

/// Drive a job on a remote worker to completion, streaming the remote console as
/// progress. Cancel kills the remote process group. This attached path keeps the
/// uniform `run_job` contract; the GUI uses the launcher's detached primitives
/// directly so a run survives an app restart.
fn run_remote(request: EngineRequest, execution: Box<RemoteExecution>) -> Running {
    let (sender, updates) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_worker = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let result = run_remote_blocking(request, &execution, &cancel_for_worker, |stage| {
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

fn run_remote_blocking(
    request: EngineRequest,
    execution: &RemoteExecution,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(String),
) -> Result<EngineOutcome> {
    let RemoteExecution {
        target,
        launcher,
        working_dir,
        worker_path,
    } = execution;

    progress("staging the remote job".to_string());
    std::fs::create_dir_all(working_dir)
        .with_context(|| format!("create remote run directory {}", working_dir.display()))?;
    let json = serde_json::to_vec(&request).context("serialize engine request")?;
    std::fs::write(working_dir.join(launcher::REQUEST_FILE), json).context("write request.json")?;

    progress("submitting to the remote host".to_string());
    let handle = launcher.submit(target, working_dir, worker_path)?;

    let mut forwarded = 0usize;
    loop {
        // Cancel-responsive wait between polls.
        let mut slept = Duration::ZERO;
        while slept < REMOTE_POLL_INTERVAL {
            if cancel.load(Ordering::Relaxed) {
                let _ = launcher.cancel(target, &handle);
                bail!("remote job cancelled");
            }
            std::thread::sleep(REMOTE_CANCEL_TICK);
            slept += REMOTE_CANCEL_TICK;
        }
        let (liveness, console) = launcher.poll(target, &handle)?;
        forward_console(&console, &mut forwarded, &mut progress);
        match liveness {
            Liveness::Alive => {}
            Liveness::Lost => {
                bail!("remote job was lost (no exit code — node crash, OOM, or external kill)")
            }
            Liveness::Done(0) => {
                progress("retrieving the outcome".to_string());
                let bytes = launcher::retrieve_outcome(target, working_dir)?;
                return serde_json::from_slice(&bytes).context("parse engine outcome");
            }
            Liveness::Done(code) => bail!("remote worker exited with status {code}"),
        }
    }
}

/// Forward console lines that appeared since the last forward (best-effort live
/// streaming; the authoritative outcome arrives in `outcome.json`).
fn forward_console(console: &str, forwarded: &mut usize, progress: &mut impl FnMut(String)) {
    let lines: Vec<&str> = console.lines().collect();
    for line in lines.iter().skip(*forwarded) {
        progress((*line).to_string());
    }
    *forwarded = lines.len().max(*forwarded);
}

/// Reject a payload the worker should not run: no atoms, or a non-finite
/// (NaN/inf) coordinate. The engine would also reject these, but checking up
/// front turns malformed remote input into a clear, immediate non-zero exit
/// rather than a deeper engine error.
fn validate_request(request: &EngineRequest) -> Result<()> {
    match &request.engine {
        Engine::Qm(job) => validate_qm_job(job),
        Engine::Docking(docking) => validate_docking_request(docking),
        Engine::Gromacs(job) => validate_gromacs_job(job),
    }
}

fn validate_gromacs_job(job: &GromacsJob) -> Result<()> {
    let structure = match job {
        GromacsJob::Run(req) => &req.structure,
        GromacsJob::Build(req) => &req.structure,
        GromacsJob::BuildMaterial(req) => &req.structure,
    };
    if structure.atoms.is_empty() {
        bail!("the GROMACS request structure has no atoms");
    }
    if let Some((index, _)) = structure
        .atoms
        .iter()
        .enumerate()
        .find(|(_, atom)| !atom.position.coords.iter().all(|c| c.is_finite()))
    {
        bail!("atom {index} has a non-finite coordinate");
    }
    Ok(())
}

fn validate_qm_job(job: &QmJob) -> Result<()> {
    let structure = match job {
        QmJob::Molecular(req) => &req.structure,
        QmJob::Periodic(req) => &req.structure,
    };
    if structure.atoms.is_empty() {
        bail!("engine request carries no atoms");
    }
    if let Some((index, _)) = structure
        .atoms
        .iter()
        .enumerate()
        .find(|(_, atom)| !atom.position.coords.iter().all(|c| c.is_finite()))
    {
        bail!("atom {index} has a non-finite coordinate");
    }
    // A periodic job carries a lattice; a non-finite component would corrupt the
    // reciprocal-space setup, so reject it up front the way atom coordinates are.
    if let QmJob::Periodic(_) = job
        && let Some(cell) = &structure.cell
    {
        let lattice_finite = [cell.a, cell.b, cell.c, cell.alpha, cell.beta, cell.gamma]
            .into_iter()
            .all(f32::is_finite)
            && cell.vectors.iter().all(|v| v.iter().all(|c| c.is_finite()));
        if !lattice_finite {
            bail!("periodic request has a non-finite lattice component");
        }
    }
    Ok(())
}

/// Reject a docking payload before the search runs: an input with no atoms (or
/// empty PDBQT), a non-finite coordinate, or a non-positive search box.
fn validate_docking_request(request: &DockingRequest) -> Result<()> {
    validate_docking_input(&request.receptor, "receptor")?;
    validate_docking_input(&request.ligand, "ligand")?;
    if !request.box_center.iter().all(|c| c.is_finite()) {
        bail!("the docking search box center has a non-finite coordinate");
    }
    if !request
        .box_size
        .iter()
        .all(|size| size.is_finite() && *size > 0.0)
    {
        bail!("the docking search box must have a positive, finite size on every axis");
    }
    Ok(())
}

fn validate_docking_input(input: &DockingInput, role: &str) -> Result<()> {
    match input {
        DockingInput::Pdbqt(text) => {
            if text.trim().is_empty() {
                bail!("the {role} PDBQT input is empty");
            }
            Ok(())
        }
        DockingInput::Structure(structure) => {
            if structure.atoms.is_empty() {
                bail!("the {role} structure has no atoms");
            }
            if let Some((index, _)) = structure
                .atoms
                .iter()
                .enumerate()
                .find(|(_, atom)| !atom.position.coords.iter().all(|c| c.is_finite()))
            {
                bail!("{role} atom {index} has a non-finite coordinate");
            }
            Ok(())
        }
    }
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
        Engine::Docking(request) => {
            // The docking engine is single-threaded (no rayon pool), so the
            // requested core count does not size a thread pool here as it does for QM.
            let result = run_docking_calculation(request, cancel, |DockingProgress { stage }| {
                progress(stage)
            })?;
            Ok(EngineOutcome::Docking(result.outcome))
        }
        Engine::Gromacs(job) => {
            // GROMACS drives the external `gmx` (single-threaded via `mdrun -nt 1`),
            // so the requested core count does not size a thread pool here either;
            // both progress variants collapse onto the one console channel.
            let outcome = run_gromacs_calculation(job, cancel, |event| {
                progress(match event {
                    GromacsProgress::Stage(text) | GromacsProgress::Log(text) => text,
                })
            })?;
            Ok(EngineOutcome::Gromacs(outcome))
        }
    }
}

/// Process a staged `request.json` and write `outcome.json`. This is the engine
/// entry a subprocess (and a remote worker) runs; malformed input fails the parse
/// or [`validate_request`] and returns an error, so the process exits non-zero.
pub fn exec(request_path: &Path, outcome_path: &Path) -> Result<()> {
    let bytes =
        std::fs::read(request_path).with_context(|| format!("read {}", request_path.display()))?;
    let request: EngineRequest = serde_json::from_slice(&bytes).context("parse engine request")?;
    validate_request(&request)?;
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
    fn validate_request_rejects_empty_nan_and_accepts_h2() {
        // Empty atoms → rejected.
        let empty = EngineRequest::new(Engine::Qm(QmJob::Molecular(QmRequest {
            structure: Structure::new("empty", Vec::new()),
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
        })));
        assert!(validate_request(&empty).is_err());

        // A non-finite coordinate → rejected, message names the atom index. The
        // structure is built clean (bond inference rejects non-finite input), then
        // a coordinate is poked to infinity to exercise the validator.
        let mut nan_structure = Structure::new(
            "nan",
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
        nan_structure.atoms[1].position.y = f32::INFINITY;
        let nan = EngineRequest::new(Engine::Qm(QmJob::Molecular(QmRequest {
            structure: nan_structure,
            method: QmMethod::Rhf,
            basis: "sto-3g".to_string(),
            charge: 0,
            multiplicity: 1,
            kind: QmKind::SinglePoint,
            options: QmOptions::default(),
        })));
        let error = validate_request(&nan).unwrap_err().to_string();
        assert!(
            error.contains("atom 1"),
            "message should name atom 1: {error}"
        );

        // A clean H2 → accepted.
        assert!(validate_request(&h2_single_point()).is_ok());
    }

    #[test]
    fn validate_request_rejects_a_non_finite_lattice() {
        use crate::domain::UnitCell;
        use crate::engines::qm::PeriodicQmRequest;

        let mut cell = UnitCell::from_parameters(5.43, 5.43, 5.43, 90.0, 90.0, 90.0);
        cell.vectors[0].x = f32::NAN;
        let structure = Structure::with_cell(
            "si",
            vec![Atom {
                element: "Si".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            cell,
        );
        let request = EngineRequest::new(Engine::Qm(QmJob::Periodic(PeriodicQmRequest::new(
            structure,
        ))));
        let error = validate_request(&request).unwrap_err().to_string();
        assert!(
            error.contains("lattice"),
            "message should name the lattice: {error}"
        );
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
        let EngineOutcome::Qm(back) = serde_json::from_slice(&json).unwrap() else {
            panic!("expected a QM outcome");
        };
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
        let EngineOutcome::Qm(outcome) = *outcome else {
            panic!("expected a QM outcome");
        };
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
                    let EngineOutcome::Qm(outcome) = *outcome else {
                        panic!("expected a QM outcome");
                    };
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
        let EngineOutcome::Qm(via_exec) = serde_json::from_slice(&bytes).unwrap() else {
            panic!("expected a QM outcome");
        };

        assert!(via_exec.converged);
        assert!(
            (local.energy_hartree - via_exec.energy_hartree).abs() < 1e-6,
            "in-process {} vs exec {} exceeded SCF tolerance",
            local.energy_hartree,
            via_exec.energy_hartree
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small docking request whose receptor and ligand are butane skeletons,
    /// prepared heuristically from structures (exercising the payload bridge on the
    /// `DockingInput::Structure` variant). `ScoreOnly` keeps it a single, fast
    /// evaluation.
    fn butane_score_request() -> EngineRequest {
        use crate::domain::{Bond, BondType};
        use crate::engines::docking::{DockingConfig, DockingInput, DockingKind, DockingRequest};

        let carbon = |x: f32, y: f32, z: f32| Atom {
            element: "C".to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        };
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
        EngineRequest::new(Engine::Docking(DockingRequest {
            receptor: DockingInput::Structure(Box::new(skeleton())),
            ligand: DockingInput::Structure(Box::new(skeleton())),
            box_center: [1.8, 0.6, 0.0],
            box_size: [20.0, 20.0, 20.0],
            config: DockingConfig::default(),
            kind: DockingKind::ScoreOnly,
        }))
    }

    #[test]
    fn docking_request_round_trips_through_the_payload_bridge() {
        use crate::engines::docking::DockingInput;

        let request = butane_score_request();
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        match back.engine {
            Engine::Docking(docking) => {
                let DockingInput::Structure(receptor) = &docking.receptor else {
                    panic!("expected a structure receptor");
                };
                assert_eq!(receptor.atoms.len(), 4);
                assert_eq!(receptor.bonds.len(), 3);
                assert_eq!(docking.box_size, [20.0, 20.0, 20.0]);
            }
            _ => panic!("expected a docking request"),
        }
    }

    #[test]
    fn docking_outcome_round_trips() {
        let outcome = EngineOutcome::Docking(crate::engines::docking::DockingOutcome {
            poses: vec![crate::engines::docking::DockedPose {
                affinity: -5.5,
                intermolecular: -6.0,
                internal: 0.5,
                torsional: 0.0,
                structure: Structure::new(
                    "pose 1",
                    vec![Atom {
                        element: "C".to_string(),
                        position: Point3::new(0.0, 0.0, 0.0),
                        charge: 0.0,
                    }],
                ),
                pdbqt: "ATOM      1  C   LIG A   1       0.000   0.000   0.000\n".to_string(),
            }],
            notes: vec!["prepared heuristically".to_string()],
            summary: "Score only:".to_string(),
        });
        let json = serde_json::to_vec(&outcome).unwrap();
        let EngineOutcome::Docking(back) = serde_json::from_slice(&json).unwrap() else {
            panic!("expected a docking outcome");
        };
        assert_eq!(back.poses.len(), 1);
        assert!((back.poses[0].affinity + 5.5).abs() < 1e-9);
        assert_eq!(back.poses[0].structure.atoms.len(), 1);
    }

    #[test]
    fn in_process_docking_scores_a_pose() {
        let running = run_job(butane_score_request(), Executor::InProcess);
        let outcome = loop {
            match running.updates().recv().expect("worker stays alive") {
                JobUpdate::Finished(outcome) => break outcome,
                JobUpdate::Failed(error) => panic!("in-process docking failed: {error}"),
                JobUpdate::Progress { .. } => {}
            }
        };
        let EngineOutcome::Docking(outcome) = *outcome else {
            panic!("expected a docking outcome");
        };
        assert_eq!(outcome.poses.len(), 1);
        assert!(outcome.poses[0].affinity.is_finite());
    }

    #[test]
    fn gromacs_run_request_round_trips_through_the_payload_bridge() {
        use crate::domain::UnitCell;
        use crate::engines::gromacs::{FreezeSelection, MdpSettings, StageLinks, StageSpec};
        use crate::workflows::gromacs::{GromacsJob, GromacsRunRequest, WireTopology};

        let structure = Structure::with_cell(
            "argon",
            vec![
                Atom {
                    element: "Ar".to_string(),
                    position: Point3::new(1.0, 1.0, 1.0),
                    charge: 0.0,
                },
                Atom {
                    element: "Ar".to_string(),
                    position: Point3::new(2.0, 1.0, 1.0),
                    charge: 0.0,
                },
            ],
            UnitCell::from_parameters(20.0, 20.0, 20.0, 90.0, 90.0, 90.0),
        );
        let request = EngineRequest::new(Engine::Gromacs(GromacsJob::Run(GromacsRunRequest {
            structure,
            topology: WireTopology {
                top: "; topol\n".to_string(),
                includes: vec![("posre.itp".to_string(), "; restraints\n".to_string())],
            },
            stages: vec![
                StageSpec {
                    stage_name: "em".to_string(),
                    settings: MdpSettings::energy_minimization(),
                    links: StageLinks::from_prepared(),
                },
                StageSpec {
                    stage_name: "nvt".to_string(),
                    settings: MdpSettings::nvt(300.0),
                    links: StageLinks::from_prepared(),
                },
            ],
            max_duration_per_stage: Duration::from_secs(3600),
            freeze: Some(FreezeSelection {
                group: "Framework".to_string(),
                atom_indices: vec![0, 1],
            }),
        })));
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        let Engine::Gromacs(GromacsJob::Run(req)) = back.engine else {
            panic!("expected a GROMACS run job");
        };
        assert_eq!(req.structure.atoms.len(), 2);
        assert!(req.structure.cell.is_some());
        assert_eq!(req.stages.len(), 2);
        assert_eq!(req.topology.includes.len(), 1);
        assert_eq!(
            req.freeze.expect("freeze survives").atom_indices,
            vec![0, 1]
        );
    }

    #[test]
    fn gromacs_build_request_round_trips() {
        use crate::engines::gromacs::IonOptions;
        use crate::workflows::gromacs::{GromacsBuildRequest, GromacsJob};
        use crate::workflows::molecular_dynamics::{BoxShape, MdSystemConfig, WaterModel};

        let structure = Structure::new(
            "solute",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        );
        let request = EngineRequest::new(Engine::Gromacs(GromacsJob::Build(GromacsBuildRequest {
            structure,
            force_field: "amber99sb-ildn".to_string(),
            water: WaterModel::Tip3p,
            box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
            solvate: true,
            ions: Some(IonOptions {
                neutralize: true,
                concentration_molar: Some(0.15),
                positive_ion: "NA".to_string(),
                negative_ion: "CL".to_string(),
            }),
            max_duration: Duration::from_secs(3600),
        })));
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        let Engine::Gromacs(GromacsJob::Build(req)) = back.engine else {
            panic!("expected a GROMACS build job");
        };
        assert_eq!(req.force_field, "amber99sb-ildn");
        assert_eq!(req.water, WaterModel::Tip3p);
        assert!(req.solvate);
        let ions = req.ions.expect("ions survive");
        assert!(ions.neutralize);
        assert_eq!(ions.positive_ion, "NA");
    }

    #[test]
    fn gromacs_material_request_round_trips_with_cell_override() {
        use crate::domain::UnitCell;
        use crate::workflows::gromacs::{GromacsJob, GromacsMaterialRequest};
        use crate::workflows::molecular_dynamics::FrameworkMode;

        let structure = Structure::new(
            "sheet",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
        );
        // A hexagonal (gamma = 120°) cell: confirms the lattice VECTORS survive the
        // cell payload bridge, not just the six scalar parameters.
        let cell = UnitCell::from_parameters(2.46, 2.46, 12.0, 90.0, 90.0, 120.0);
        let original_vectors = cell.vectors;
        let request = EngineRequest::new(Engine::Gromacs(GromacsJob::BuildMaterial(
            GromacsMaterialRequest {
                structure,
                mode: FrameworkMode::Rigid,
                solvation: None,
                custom_force_field: Some("[ atomtypes ]\n".to_string()),
                cell_override: Some(cell),
                solvent_gap_angstrom: 25.0,
                cutoff_nm: 1.0,
                max_duration: Duration::from_secs(3600),
            },
        )));
        let json = serde_json::to_vec(&request).unwrap();
        let back: EngineRequest = serde_json::from_slice(&json).unwrap();
        let Engine::Gromacs(GromacsJob::BuildMaterial(req)) = back.engine else {
            panic!("expected a GROMACS material job");
        };
        assert_eq!(req.mode, FrameworkMode::Rigid);
        assert!(req.custom_force_field.is_some());
        let restored = req.cell_override.expect("cell survives the bridge");
        for (original, restored) in original_vectors.iter().zip(restored.vectors.iter()) {
            assert!((original.x - restored.x).abs() < 1e-6);
            assert!((original.y - restored.y).abs() < 1e-6);
            assert!((original.z - restored.z).abs() < 1e-6);
        }
    }

    #[test]
    fn gromacs_outcome_round_trips_with_trajectory() {
        use crate::workflows::gromacs::{GromacsOutcome, GromacsStageReport, GromacsTrajectory};

        let outcome = EngineOutcome::Gromacs(GromacsOutcome {
            structure: Structure::new(
                "final",
                vec![Atom {
                    element: "Ar".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                }],
            ),
            summary: "GROMACS MD complete".to_string(),
            stages: vec![GromacsStageReport {
                stage_name: "em".to_string(),
                final_potential_energy: Some(-12.3),
                wall_time: Duration::from_millis(500),
            }],
            trajectory: Some(GromacsTrajectory {
                file_name: "prod.xtc".to_string(),
                bytes: vec![1, 2, 3, 4],
            }),
            topology: None,
            system_context: None,
            material: None,
        });
        let json = serde_json::to_vec(&outcome).unwrap();
        let EngineOutcome::Gromacs(back) = serde_json::from_slice(&json).unwrap() else {
            panic!("expected a GROMACS outcome");
        };
        assert_eq!(back.structure.atoms.len(), 1);
        assert_eq!(back.stages.len(), 1);
        let trajectory = back.trajectory.expect("trajectory survives");
        assert_eq!(trajectory.file_name, "prod.xtc");
        assert_eq!(trajectory.bytes, vec![1, 2, 3, 4]);
    }

    #[test]
    fn validate_gromacs_rejects_an_empty_structure() {
        use crate::workflows::gromacs::{GromacsBuildRequest, GromacsJob};
        use crate::workflows::molecular_dynamics::{BoxShape, MdSystemConfig, WaterModel};

        let request = EngineRequest::new(Engine::Gromacs(GromacsJob::Build(GromacsBuildRequest {
            structure: Structure::new("empty", Vec::new()),
            force_field: "amber99sb-ildn".to_string(),
            water: WaterModel::Spc,
            box_config: MdSystemConfig::with_uniform_padding(10.0, BoxShape::Cubic),
            solvate: false,
            ions: None,
            max_duration: Duration::from_secs(60),
        })));
        assert!(validate_request(&request).is_err());
    }
}
