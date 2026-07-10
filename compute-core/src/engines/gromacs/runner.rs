//! Orchestration of GROMACS jobs around the `engines::process` subprocess
//! runner.
//!
//! GROMACS workflows naturally split into two parts:
//!
//! 1. **System preparation** — writing the coordinate file (`.gro`) and the
//!    topology (`.top`) into a working directory, done once per structure and
//!    reused for any number of subsequent stages.
//! 2. **Stage execution** — writing a `.mdp` and invoking `gmx grompp` +
//!    `gmx mdrun`. Energy minimization, NVT/NPT equilibration, and
//!    production MD are all stages that differ only in their [`MdpSettings`].
//!
//! Callers use [`prepare_system`] once, then [`run_stage`] or
//! [`run_pipeline`] to execute one or more stages.

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::{
    domain::Structure,
    engines::{
        gromacs::{
            input::{self, MdpSettings},
            output,
            topology::TopologySource,
        },
        process,
    },
    launch::{Compute, ComputeResources},
};

#[cfg(test)]
use crate::engines::registry::EngineLaunch;

mod prepare;
mod subprocess;

pub use prepare::prepare_system;
pub(crate) use subprocess::{run_subprocess, subprocess_failure};
// Consumed only by the `tests` module's `use super::*`; absent from the
// non-test build, where they would otherwise read as unused re-exports.
#[cfg(test)]
use prepare::{copy_topology_includes, render_index_file};
#[cfg(test)]
use subprocess::extract_fatal_error;
use subprocess::{is_cancelled, remaining_budget};

/// A streamed progress message emitted while a GROMACS workflow is running.
#[derive(Debug, Clone)]
pub enum GromacsProgress {
    /// Pre-flight stage (writing input files, invoking grompp).
    Stage(String),
    /// A line of subprocess stdout/stderr that the UI can append to a log.
    Log(String),
}

/// A named index group of atoms to freeze, written to an index file so a
/// stage's `freezegrps` can reference it. `atom_indices` are 0-based into the
/// prepared structure; a `System` group covering every atom is written
/// alongside it (the thermostat references `System`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreezeSelection {
    pub group: String,
    pub atom_indices: Vec<usize>,
}

/// Input parameters for [`prepare_system`].
#[derive(Debug, Clone)]
pub struct PrepareSystemRequest {
    pub structure: Structure,
    pub topology: TopologySource,
    pub working_dir: PathBuf,
    /// When set, an index file naming this group (plus `System`) is written and
    /// passed to `grompp -n`, so a rigid framework can be frozen by name.
    pub freeze: Option<FreezeSelection>,
}

/// Result of [`prepare_system`]: a working directory pre-populated with the
/// coordinate and topology files, ready to be fed to one or more
/// [`run_stage`] invocations.
#[derive(Debug, Clone)]
pub struct PreparedSystem {
    pub working_dir: PathBuf,
    pub conf_file: PathBuf,
    pub topology_file: PathBuf,
    /// Index file (`index.ndx`) passed to `grompp -n`, when a freeze group was
    /// requested. `None` for an ordinary system.
    pub index_file: Option<PathBuf>,
    /// The original (un-minimized) structure, kept so that bond topology and
    /// element labels can be re-grafted onto coordinate files that GROMACS
    /// emits without that metadata.
    pub original_structure: Structure,
}

/// Input parameters for [`run_stage`].
#[derive(Debug, Clone)]
pub struct StageRequest {
    pub system: PreparedSystem,
    /// Short identifier used as the basename for stage-specific files
    /// (`{stage}.mdp`, `{stage}.tpr`, `{stage}_out.gro`, ...). Lower-case
    /// alphanumerics + underscore are recommended.
    pub stage_name: String,
    pub settings: MdpSettings,
    /// Coordinate file (`grompp -c`). Defaults to the prepared `conf.gro`, but a
    /// chained stage points this at a prior stage's output `.gro` so each stage
    /// starts from the previous one's relaxed/equilibrated coordinates.
    pub coordinate_input: PathBuf,
    /// Continuation checkpoint (`grompp -t`). NPT/production resume from the
    /// prior stage's `.cpt` so velocities and box state carry over.
    pub checkpoint_input: Option<PathBuf>,
    /// How to launch `gmx` (native, WSL prefix, …) and where it runs (local or a
    /// remote host over SSH).
    pub compute: Compute,
    /// Wall-clock budget shared by `grompp` and `mdrun` together.
    pub max_duration: Duration,
}

/// Outcome of a successful GROMACS stage.
#[derive(Debug, Clone)]
pub struct StageResult {
    pub structure: Structure,
    pub final_potential_energy: Option<f64>,
    pub grompp_stdout: String,
    pub grompp_stderr: String,
    pub mdrun_stdout: String,
    pub mdrun_stderr: String,
    pub wall_time: Duration,
    pub working_dir: PathBuf,
    pub stage_name: String,
    /// Final coordinate file this stage wrote (`{stage}_out.gro`). The input to
    /// the next stage's coordinates.
    pub output_gro: PathBuf,
    /// Continuation checkpoint (`{stage}.cpt`) if `mdrun` produced one.
    pub checkpoint: Option<PathBuf>,
    /// Compressed trajectory (`{stage}.xtc`) if the stage requested one.
    pub trajectory: Option<PathBuf>,
    /// Energy file (`{stage}.edr`) for downstream analysis (`gmx energy`).
    pub edr: PathBuf,
    /// Run log (`{stage}.log`).
    pub log: PathBuf,
}

/// Which produced file a stage link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageFileRole {
    /// The stage's final coordinates (`{stage}_out.gro`).
    OutputGro,
    /// The stage's continuation checkpoint (`{stage}.cpt`).
    Checkpoint,
    /// The stage's compressed trajectory (`{stage}.xtc`).
    Trajectory,
}

/// A reference to a file feeding a stage's `grompp` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileRef {
    /// The `conf.gro` written by [`prepare_system`].
    PreparedConf,
    /// A file produced by a previously run stage, identified by name and role.
    Stage { stage: String, role: StageFileRole },
}

/// Declares where a stage's coordinate (`-c`) and checkpoint (`-t`) inputs come
/// from. Resolved against the prepared system and prior stage outputs by
/// [`run_pipeline`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageLinks {
    pub coordinates: FileRef,
    pub checkpoint: Option<FileRef>,
}

impl StageLinks {
    /// Start fresh from the prepared coordinates with no continuation (the
    /// energy-minimization arrangement).
    pub fn from_prepared() -> Self {
        Self {
            coordinates: FileRef::PreparedConf,
            checkpoint: None,
        }
    }
}

/// One stage in a [`run_pipeline`] chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageSpec {
    pub stage_name: String,
    pub settings: MdpSettings,
    pub links: StageLinks,
}

/// The files a completed stage exposes to later stages. A lightweight view of
/// [`StageResult`] used to resolve [`FileRef`]s without cloning structures.
#[derive(Debug, Clone)]
struct StageOutputs {
    output_gro: PathBuf,
    checkpoint: Option<PathBuf>,
    trajectory: Option<PathBuf>,
}

impl From<&StageResult> for StageOutputs {
    fn from(result: &StageResult) -> Self {
        Self {
            output_gro: result.output_gro.clone(),
            checkpoint: result.checkpoint.clone(),
            trajectory: result.trajectory.clone(),
        }
    }
}

/// Run a single stage (grompp + mdrun) against an already-prepared system.
pub fn run_stage<F>(
    request: StageRequest,
    cancel: Arc<AtomicBool>,
    mut report: F,
) -> Result<StageResult>
where
    F: FnMut(GromacsProgress),
{
    let started_at = Instant::now();
    let stage = sanitize_stage_name(&request.stage_name);
    let working_dir = request.system.working_dir.clone();

    let mdp_name = format!("{stage}.mdp");
    let tpr_name = format!("{stage}.tpr");
    let out_name = format!("{stage}_out.gro");

    report(GromacsProgress::Stage(format!(
        "writing run parameters ({mdp_name})"
    )));
    let mdp_path = working_dir.join(&mdp_name);
    fs::write(&mdp_path, input::render_mdp(&request.settings))
        .with_context(|| format!("failed to write {}", mdp_path.display()))?;

    if is_cancelled(&cancel) {
        bail!("GROMACS run cancelled before grompp launched");
    }

    let coord_name = file_name_or(&request.coordinate_input, "conf.gro");
    let topology_file_name = file_name_or(&request.system.topology_file, "topol.top");
    let checkpoint_name = request
        .checkpoint_input
        .as_ref()
        .map(|p| file_name_or(p, "state.cpt"));
    let index_name = request
        .system
        .index_file
        .as_ref()
        .map(|p| file_name_or(p, "index.ndx"));
    // When this stage applies position restraints (`-DPOSRES`), GROMACS >= 2018
    // requires the restraint *reference* coordinates via `grompp -r`. Per the
    // GROMACS guidance, reuse the same file given to `-c`. Omitted otherwise, so
    // an unrestrained (or framework/argon) stage's argument list is unchanged.
    let restraint_ref = request
        .settings
        .define
        .as_deref()
        .filter(|define| define.contains("POSRES"))
        .map(|_| coord_name.as_str());

    let remaining = remaining_budget(request.max_duration, started_at)?;
    report(GromacsProgress::Stage(format!(
        "running gmx grompp ({stage})"
    )));
    let grompp = run_subprocess(
        request.compute.launch.to_process_config(
            working_dir.clone(),
            build_grompp_args(
                &mdp_name,
                &coord_name,
                checkpoint_name.as_deref(),
                restraint_ref,
                &topology_file_name,
                &tpr_name,
                index_name.as_deref(),
            ),
            Some(remaining),
        ),
        Arc::clone(&cancel),
        &mut report,
    )?;

    if !grompp.result.success() {
        return Err(subprocess_failure("grompp", &grompp));
    }

    if is_cancelled(&cancel) {
        bail!("GROMACS run cancelled before mdrun launched");
    }

    let remaining = remaining_budget(request.max_duration, started_at)?;
    report(GromacsProgress::Stage(format!(
        "running gmx mdrun ({stage})"
    )));
    let mdrun = run_subprocess(
        request.compute.launch.to_process_config(
            working_dir.clone(),
            mdrun_args(
                &stage,
                &out_name,
                request.compute.resources,
                request.settings.integrator,
            ),
            Some(remaining),
        ),
        Arc::clone(&cancel),
        &mut report,
    )?;

    if !mdrun.result.success() {
        return Err(subprocess_failure("mdrun", &mdrun));
    }

    let output_gro = working_dir.join(&out_name);
    let structure =
        output::load_minimized_structure(&output_gro, &request.system.original_structure)?;
    let log = working_dir.join(format!("{stage}.log"));
    let final_energy = output::parse_final_potential_energy(&mdrun.combined_log).or_else(|| {
        fs::read_to_string(&log)
            .ok()
            .as_deref()
            .and_then(output::parse_final_potential_energy)
    });

    Ok(StageResult {
        structure,
        final_potential_energy: final_energy,
        grompp_stdout: grompp.result.stdout,
        grompp_stderr: grompp.result.stderr,
        mdrun_stdout: mdrun.result.stdout,
        mdrun_stderr: mdrun.result.stderr,
        wall_time: started_at.elapsed(),
        working_dir: working_dir.clone(),
        checkpoint: optional_existing(&working_dir, &format!("{stage}.cpt")),
        trajectory: optional_existing(&working_dir, &format!("{stage}.xtc")),
        edr: working_dir.join(format!("{stage}.edr")),
        log,
        output_gro,
        stage_name: stage,
    })
}

/// Run a chain of stages against one prepared system, threading each stage's
/// coordinate/restraint/checkpoint inputs from earlier stages per its
/// [`StageLinks`]. This is the EM→NVT→NPT→production engine: prepare once, run
/// many. Stops at the first stage that fails and returns the results gathered so
/// far via the error context.
pub fn run_pipeline<F>(
    system: PreparedSystem,
    stages: Vec<StageSpec>,
    compute: Compute,
    max_duration_per_stage: Duration,
    cancel: Arc<AtomicBool>,
    mut report: F,
) -> Result<Vec<StageResult>>
where
    F: FnMut(GromacsProgress),
{
    use std::collections::HashMap;

    let mut outputs: HashMap<String, StageOutputs> = HashMap::new();
    let mut results: Vec<StageResult> = Vec::with_capacity(stages.len());

    for spec in stages {
        let coordinate_input = resolve_file_ref(&spec.links.coordinates, &system, &outputs)
            .with_context(|| format!("resolving coordinates for stage '{}'", spec.stage_name))?;
        let checkpoint_input = match &spec.links.checkpoint {
            Some(file_ref) => Some(resolve_file_ref(file_ref, &system, &outputs).with_context(
                || format!("resolving checkpoint for stage '{}'", spec.stage_name),
            )?),
            None => None,
        };

        let result = run_stage(
            StageRequest {
                system: system.clone(),
                stage_name: spec.stage_name.clone(),
                settings: spec.settings,
                coordinate_input,
                checkpoint_input,
                compute: compute.clone(),
                max_duration: max_duration_per_stage,
            },
            Arc::clone(&cancel),
            &mut report,
        )
        .with_context(|| format!("GROMACS stage '{}' failed", spec.stage_name))?;

        outputs.insert(result.stage_name.clone(), StageOutputs::from(&result));
        results.push(result);
    }

    Ok(results)
}

/// Assemble the `gmx mdrun` argument vector from the requested resources. With no
/// explicit request (`cores`/`gpu` both 0) it emits only the I/O flags and lets gmx
/// pick its own thread/GPU defaults (its all-cores, auto-GPU behaviour). A GPU
/// request runs one thread-MPI rank per GPU. Dynamical stages explicitly offload
/// nonbonded, PME, bonded, and update work; minimization only forces nonbonded
/// work onto the GPU because GPU PME does not support non-dynamical integrators.
/// A CPU-only core request maps to `-nt`; under a GPU rank cores map to `-ntomp`.
fn mdrun_args(
    stage: &str,
    out_name: &str,
    resources: ComputeResources,
    integrator: input::Integrator,
) -> Vec<String> {
    let mut args = vec![
        "mdrun".to_string(),
        "-deffnm".to_string(),
        stage.to_string(),
        "-c".to_string(),
        out_name.to_string(),
    ];
    if resources.gpu >= 1 {
        args.extend(["-ntmpi".to_string(), resources.gpu.to_string()]);
        args.extend(["-nb".to_string(), "gpu".to_string()]);
        if !integrator.is_minimization() {
            args.extend(["-pme", "gpu", "-bonded", "gpu", "-update", "gpu"].map(String::from));
        }
        if !integrator.is_minimization() && resources.gpu > 1 {
            // GPU PME across multiple ranks needs one dedicated PME rank; gmx maps
            // the ranks onto the available GPUs itself. We deliberately do not emit
            // `-gpu_id` — its single-digit-per-device form can't express ids >= 10.
            args.extend(["-npme".to_string(), "1".to_string()]);
        }
        if resources.cores > 0 {
            args.extend(["-ntomp".to_string(), resources.cores.to_string()]);
        }
    } else if resources.cores > 0 {
        args.extend(["-nt".to_string(), resources.cores.to_string()]);
    }
    args
}

/// Assemble the `gmx grompp` argument vector. The optional `-t` (continuation
/// checkpoint) is emitted only when present, so a single-stage minimization
/// produces exactly the original argument list.
pub(crate) fn build_grompp_args(
    mdp: &str,
    coordinates: &str,
    checkpoint: Option<&str>,
    restraint_ref: Option<&str>,
    topology: &str,
    tpr: &str,
    index: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "grompp".to_string(),
        "-f".to_string(),
        mdp.to_string(),
        "-c".to_string(),
        coordinates.to_string(),
    ];
    if let Some(restraint_ref) = restraint_ref {
        // Position-restraint reference coordinates (`-DPOSRES` stages only).
        args.push("-r".to_string());
        args.push(restraint_ref.to_string());
    }
    if let Some(checkpoint) = checkpoint {
        args.push("-t".to_string());
        args.push(checkpoint.to_string());
    }
    if let Some(index) = index {
        // An index file naming the freeze group; grompp resolves `freezegrps`
        // against it.
        args.push("-n".to_string());
        args.push(index.to_string());
    }
    args.extend([
        "-p".to_string(),
        topology.to_string(),
        "-o".to_string(),
        tpr.to_string(),
        "-maxwarn".to_string(),
        "5".to_string(),
    ]);
    args
}

/// Resolve a [`FileRef`] to a concrete path against the prepared system and the
/// outputs of stages that have already run.
fn resolve_file_ref(
    file_ref: &FileRef,
    system: &PreparedSystem,
    outputs: &std::collections::HashMap<String, StageOutputs>,
) -> Result<PathBuf> {
    match file_ref {
        FileRef::PreparedConf => Ok(system.conf_file.clone()),
        FileRef::Stage { stage, role } => {
            let key = sanitize_stage_name(stage);
            let out = outputs.get(&key).ok_or_else(|| {
                anyhow!("stage '{stage}' is referenced before it has produced output")
            })?;
            match role {
                StageFileRole::OutputGro => Ok(out.output_gro.clone()),
                StageFileRole::Checkpoint => out
                    .checkpoint
                    .clone()
                    .ok_or_else(|| anyhow!("stage '{stage}' produced no checkpoint (.cpt)")),
                StageFileRole::Trajectory => out
                    .trajectory
                    .clone()
                    .ok_or_else(|| anyhow!("stage '{stage}' produced no trajectory (.xtc)")),
            }
        }
    }
}

fn optional_existing(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let candidate = dir.join(name);
    candidate.exists().then_some(candidate)
}

fn sanitize_stage_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "stage".to_string()
    } else {
        cleaned
    }
}

fn file_name_or(path: &std::path::Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Name of the cumulative console log every `gmx` invocation appends to inside
/// its working (run) directory. GROMACS prints almost everything to the
/// terminal and nothing durable on failure; capturing it here is what makes a
/// failed run debuggable after the fact.
pub(crate) const GROMACS_LOG_FILE: &str = "gromacs.log";

pub(crate) struct SubprocessOutcome {
    pub(crate) result: process::ProcessResult,
    pub(crate) combined_log: String,
    /// The on-disk log this invocation's output was appended to.
    pub(crate) log_path: PathBuf,
}

#[cfg(test)]
mod tests;
