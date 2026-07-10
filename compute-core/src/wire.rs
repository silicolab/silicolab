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
use crate::engines::remote::launcher::{self, RemoteExecution, RemoteJobPhase};
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

#[derive(Clone)]
enum CancelHandle {
    /// The cooperative flag the in-process engine polls.
    Flag(Arc<AtomicBool>),
    /// The subprocess to kill.
    Child(Arc<Mutex<Child>>),
}

#[derive(Clone)]
pub struct JobCancelHandle {
    inner: CancelHandle,
}

impl JobCancelHandle {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            inner: CancelHandle::Flag(flag),
        }
    }

    pub fn cancel(&self) {
        self.inner.cancel();
    }

    pub fn flag(&self) -> Option<Arc<AtomicBool>> {
        match &self.inner {
            CancelHandle::Flag(flag) => Some(Arc::clone(flag)),
            CancelHandle::Child(_) => None,
        }
    }
}

impl CancelHandle {
    fn cancel(&self) {
        match self {
            CancelHandle::Flag(flag) => flag.store(true, Ordering::SeqCst),
            CancelHandle::Child(child) => {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        }
    }
}

impl Running {
    pub fn updates(&self) -> &Receiver<JobUpdate> {
        &self.updates
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancel_handle(&self) -> JobCancelHandle {
        JobCancelHandle {
            inner: self.cancel.clone(),
        }
    }

    /// The cooperative cancel flag, when the job runs in-process. Lets a caller
    /// that already speaks the flag protocol (the engine-specific UI handles) share
    /// one cancel signal with the worker. `None` for a subprocess job, which
    /// cancels by kill instead.
    pub fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        self.cancel_handle().flag()
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
/// How long a scheduler may keep reporting `Unknown` before the job counts as
/// lost. Bounded because a finished Slurm job ages out of `squeue` and, without
/// accounting, out of `scontrol` too — otherwise the wait never terminates.
const REMOTE_UNKNOWN_GRACE: Duration = Duration::from_secs(60);

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
        resources,
        slurm_profile,
    } = execution;

    progress("staging the remote job".to_string());
    std::fs::create_dir_all(working_dir)
        .with_context(|| format!("create remote run directory {}", working_dir.display()))?;
    let json = serde_json::to_vec(&request).context("serialize engine request")?;
    std::fs::write(working_dir.join(launcher::REQUEST_FILE), json).context("write request.json")?;

    progress("submitting to the remote host".to_string());
    let handle = launcher.submit(
        target,
        working_dir,
        worker_path,
        resources,
        slurm_profile.as_ref(),
    )?;

    let mut console_offset = 0;
    let mut unknown_since: Option<std::time::Instant> = None;
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
        let observation = launcher.poll(target, &handle, console_offset, false)?;
        console_offset = observation.console.next_offset;
        for line in observation.console.text.lines() {
            progress(line.to_string());
        }
        match observation.phase {
            RemoteJobPhase::Unknown => {
                let since = *unknown_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() >= REMOTE_UNKNOWN_GRACE {
                    bail!(
                        "remote job state is unknown: the scheduler stopped reporting it and no exit marker appeared"
                    )
                }
            }
            RemoteJobPhase::Queued
            | RemoteJobPhase::Starting
            | RemoteJobPhase::Running
            | RemoteJobPhase::Completing
            | RemoteJobPhase::Cancelling => unknown_since = None,
            RemoteJobPhase::Lost => {
                bail!("remote job was lost (no exit code — node crash, OOM, or external kill)")
            }
            RemoteJobPhase::Succeeded => {
                progress("retrieving the outcome".to_string());
                let bytes = launcher::retrieve_outcome(target, working_dir)?;
                return serde_json::from_slice(&bytes).context("parse engine outcome");
            }
            RemoteJobPhase::Cancelled => bail!("remote job was cancelled"),
            RemoteJobPhase::Failed => bail!(
                "remote worker failed{}",
                observation
                    .exit_code
                    .map(|code| format!(" with status {code}"))
                    .unwrap_or_default()
            ),
        }
    }
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

/// Reject a structure with no atoms or a non-finite coordinate, naming its role
/// (`"engine request"`, `"transition-state product"`) in the diagnostic.
fn validate_structure_atoms(structure: &crate::domain::Structure, role: &str) -> Result<()> {
    if structure.atoms.is_empty() {
        bail!("{role} carries no atoms");
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

fn validate_qm_job(job: &QmJob) -> Result<()> {
    let structure = match job {
        QmJob::Molecular(req) => &req.structure,
        QmJob::Periodic(req) => &req.structure,
    };
    validate_structure_atoms(structure, "engine request")?;
    // A two-endpoint transition-state search carries a second (product) structure
    // over the same untrusted boundary; validate it the way the reactant is.
    if let QmJob::Molecular(req) = job
        && let Some(ts) = &req.ts
        && let crate::engines::qm::QmTsGuess::TwoEndpoint(endpoints) = &ts.guess
    {
        validate_structure_atoms(&endpoints.product, "transition-state product")?;
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
mod tests;
