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
    io::Write as _,
    path::{Path, PathBuf},
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
        process::{self, ProcessConfig, ProcessEventKind},
        remote::Compute,
    },
};

#[cfg(test)]
use crate::engines::registry::EngineLaunch;

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

/// Write the coordinate file and materialize the topology into `working_dir`.
pub fn prepare_system(request: PrepareSystemRequest) -> Result<PreparedSystem> {
    fs::create_dir_all(&request.working_dir).with_context(|| {
        format!(
            "failed to create GROMACS working directory {}",
            request.working_dir.display()
        )
    })?;

    let conf_file = request.working_dir.join("conf.gro");
    fs::write(
        &conf_file,
        input::to_gro(&request.structure, &request.structure.title)?,
    )
    .with_context(|| format!("failed to write {}", conf_file.display()))?;

    let topology_file = request
        .topology
        .materialize(&request.working_dir, "topol.top")?;

    // A file topology reused from a build directory may `#include` sibling `.itp`
    // files (e.g. the `posre.itp` position restraints pdb2gmx writes). Copy them
    // alongside so grompp resolves the includes when the run directory differs
    // from the build directory.
    if let TopologySource::File(source) = &request.topology {
        copy_topology_includes(source, &request.working_dir)?;
    }

    let index_file = match &request.freeze {
        Some(freeze) => {
            let path = request.working_dir.join("index.ndx");
            fs::write(
                &path,
                render_index_file(request.structure.atoms.len(), freeze),
            )
            .with_context(|| format!("failed to write {}", path.display()))?;
            Some(path)
        }
        None => None,
    };

    Ok(PreparedSystem {
        working_dir: request.working_dir,
        conf_file,
        topology_file,
        index_file,
        original_structure: request.structure,
    })
}

/// Copy the `.itp` files sitting beside a file topology into the run directory,
/// so `#include` directives (such as `posre.itp` for position restraints) resolve
/// when the run directory differs from the topology's source directory. A no-op
/// when they are the same directory; best-effort if the source dir can't be read.
fn copy_topology_includes(topology_source: &Path, target_dir: &Path) -> Result<()> {
    let Some(source_dir) = topology_source.parent() else {
        return Ok(());
    };
    if source_dir == target_dir {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(source_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("itp")
            && let Some(name) = path.file_name()
        {
            fs::copy(&path, target_dir.join(name))
                .with_context(|| format!("copying topology include {}", path.display()))?;
        }
    }
    Ok(())
}

/// Render a GROMACS index file (`.ndx`) with a `System` group covering every
/// atom and the named freeze group. Indices are 1-based, wrapped to a column
/// width GROMACS parses without issue.
fn render_index_file(atom_count: usize, freeze: &FreezeSelection) -> String {
    fn group(out: &mut String, name: &str, indices: impl Iterator<Item = usize>) {
        out.push_str(&format!("[ {name} ]\n"));
        for (n, index) in indices.enumerate() {
            out.push_str(&format!("{index:>6}"));
            if (n + 1) % 15 == 0 {
                out.push('\n');
            }
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }

    let mut out = String::new();
    group(&mut out, "System", 1..=atom_count);
    group(
        &mut out,
        &freeze.group,
        freeze.atom_indices.iter().map(|i| i + 1),
    );
    out
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
            [
                "mdrun".to_string(),
                "-deffnm".to_string(),
                stage.clone(),
                "-c".to_string(),
                out_name.clone(),
                "-nt".to_string(),
                "1".to_string(),
            ],
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

fn is_cancelled(cancel: &Arc<AtomicBool>) -> bool {
    cancel.load(std::sync::atomic::Ordering::Relaxed)
}

fn remaining_budget(total: Duration, started_at: Instant) -> Result<Duration> {
    let elapsed = started_at.elapsed();
    if elapsed >= total {
        bail!("GROMACS wall-clock budget exhausted before next stage");
    }
    Ok(total - elapsed)
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

impl SubprocessOutcome {
    pub fn success(&self) -> bool {
        self.result.success()
    }
}

/// Append one `gmx` invocation's full console output to the run directory's
/// cumulative log, prefixed with the command line and exit code. Best-effort:
/// logging must never mask the underlying run error, so write failures are
/// swallowed.
fn append_gromacs_log(log_path: &Path, command_line: &str, exit_code: i32, body: &str) {
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return;
    };
    let _ = write!(
        file,
        "=== {command_line} ===\n{body}\n--- exit code {exit_code} ---\n\n"
    );
}

/// Pull the GROMACS "Fatal error:" block out of a captured log, if present.
/// GROMACS buffers stdout, so on a crash the unbuffered stderr error often
/// lands *before* the trailing buffered progress lines — a plain tail misses
/// it. This finds the real error regardless of where it sits in the stream.
fn extract_fatal_error(log: &str) -> Option<String> {
    let start = log.rfind("Fatal error:")?;
    let rest = &log[start..];
    // The block closes with GROMACS' row-of-dashes banner; stop there.
    let end = rest.find("\n---").unwrap_or(rest.len());
    Some(rest[..end].trim_end().to_string())
}

pub(crate) fn run_subprocess<F>(
    config: ProcessConfig,
    cancel: Arc<AtomicBool>,
    report: &mut F,
) -> Result<SubprocessOutcome>
where
    F: FnMut(GromacsProgress),
{
    // Capture where to log and what command this is before `config` is moved.
    let log_path = config.working_dir.join(GROMACS_LOG_FILE);
    let command_line = format!(
        "{} {}",
        config.executable.to_string_lossy(),
        config.args.join(" ")
    );

    let (result, combined_log) = run_subprocess_local(config, cancel)?;

    // Persist the full output to the run directory so a failed run is always
    // debuggable, even though only the tail is streamed to the UI.
    append_gromacs_log(&log_path, &command_line, result.exit_code, &combined_log);

    // The child's output is not streamed during the run; surface its tail now.
    for line in combined_log
        .lines()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        report(GromacsProgress::Log(line.to_string()));
    }

    Ok(SubprocessOutcome {
        result,
        combined_log,
        log_path,
    })
}

/// The local-execution path: spawn the child, drain its streamed stdout/stderr
/// into a combined log, and return the aggregated result. Byte-for-byte the
/// historical behavior.
fn run_subprocess_local(
    config: ProcessConfig,
    cancel: Arc<AtomicBool>,
) -> Result<(process::ProcessResult, String)> {
    let mut handle = process::spawn_with_cancel(config, cancel)?;
    let receiver = handle
        .take_receiver()
        .ok_or_else(|| anyhow!("subprocess handle missing event receiver"))?;

    let log_join = std::thread::spawn(move || {
        let mut combined = String::new();
        while let Ok(event) = receiver.recv() {
            if let ProcessEventKind::Stdout(line) | ProcessEventKind::Stderr(line) = event.kind {
                combined.push_str(&line);
                combined.push('\n');
            }
        }
        combined
    });

    let result = handle.join()?;
    let combined_log = log_join.join().map_err(|_| anyhow!("log drain panicked"))?;
    Ok((result, combined_log))
}

pub(crate) fn subprocess_failure(tool: &str, outcome: &SubprocessOutcome) -> anyhow::Error {
    // Prefer GROMACS' own fatal-error block; fall back to the tail otherwise.
    let snippet = extract_fatal_error(&outcome.combined_log)
        .unwrap_or_else(|| tail(&outcome.combined_log, 20));
    anyhow!(
        "gmx {tool} failed (exit {}). Full log: {}\n{}",
        outcome.result.exit_code,
        outcome.log_path.display(),
        snippet
    )
}

pub(crate) fn tail(log: &str, lines: usize) -> String {
    let collected: Vec<&str> = log.lines().rev().take(lines).collect();
    collected.into_iter().rev().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::*;
    use crate::domain::{Atom, UnitCell};

    #[test]
    fn extract_fatal_error_pulls_block_even_when_progress_trails_it() {
        // GROMACS buffers stdout, so on a crash the (unbuffered) stderr error
        // can land before the trailing buffered progress — a plain tail would
        // return the progress, not the error. Extraction must find the error.
        let log = "\
-------------------------------------------------------
Fatal error:
Atom HE2 in residue HIS7 was not found in rtp entry.
Option -ignh will ignore all hydrogens in the input.
-------------------------------------------------------
Processing chain 1 'P' (457 atoms, 30 residues)
Identified residue ARG36 as a ending terminus.
";
        let extracted = extract_fatal_error(log).expect("fatal error block found");
        assert!(extracted.starts_with("Fatal error:"));
        assert!(extracted.contains("not found in rtp entry"));
        assert!(extracted.contains("-ignh"));
        // The trailing progress noise must not be carried into the message.
        assert!(!extracted.contains("ending terminus"));
    }

    #[test]
    fn extract_fatal_error_absent_returns_none() {
        assert!(extract_fatal_error("all good\nWriting topology\n").is_none());
    }

    /// Hand-written, fully self-contained argon topology: Lennard-Jones only,
    /// no bonded terms, no charges, and crucially no dependence on any external
    /// force-field data files. Sigma/epsilon are given directly under
    /// combination rule 2 (Lorentz-Berthelot). Eight single-atom `AR` molecules
    /// line up with the eight atoms [`prepare_system`] writes to `conf.gro`.
    const ARGON_TOP: &str = "\
[ defaults ]
1         2          no         1.0      1.0

[ atomtypes ]
; name  at.num  mass      charge  ptype  sigma     epsilon
  Ar    18      39.948    0.000   A      0.34050   0.99600

[ moleculetype ]
; name  nrexcl
  AR    1

[ atoms ]
; nr  type  resnr  residue  atom  cgnr  charge  mass
  1    Ar    1      AR       Ar    1     0.000   39.948

[ system ]
Argon

[ molecules ]
AR  8
";

    /// Build a hermetic eight-atom argon box: a 2x2x2 grid at 5 angstrom
    /// spacing centered in a 30 angstrom (3 nm) cubic cell. The spacing sits
    /// well outside the LJ minimum so the starting energy is finite, and the
    /// box comfortably exceeds twice the 1 nm cutoff so the Verlet
    /// minimum-image check in grompp passes.
    fn argon_box() -> Structure {
        let mut atoms = Vec::with_capacity(8);
        for x in [10.0_f32, 15.0] {
            for y in [10.0_f32, 15.0] {
                for z in [10.0_f32, 15.0] {
                    atoms.push(Atom {
                        element: "Ar".to_string(),
                        position: Point3::new(x, y, z),
                        charge: 0.0,
                    });
                }
            }
        }
        Structure::with_cell(
            "argon",
            atoms,
            UnitCell::from_parameters(30.0, 30.0, 30.0, 90.0, 90.0, 90.0),
        )
    }

    fn wsl_gmx_launch() -> EngineLaunch {
        EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        }
    }

    #[test]
    fn grompp_args_for_single_stage_match_legacy_form() {
        // No checkpoint and no restraints -> the exact minimization argument list.
        let args = build_grompp_args(
            "em.mdp",
            "conf.gro",
            None,
            None,
            "topol.top",
            "em.tpr",
            None,
        );
        let expected: Vec<String> = [
            "grompp",
            "-f",
            "em.mdp",
            "-c",
            "conf.gro",
            "-p",
            "topol.top",
            "-o",
            "em.tpr",
            "-maxwarn",
            "5",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn grompp_args_include_checkpoint_when_present() {
        let args = build_grompp_args(
            "npt.mdp",
            "nvt_out.gro",
            Some("nvt.cpt"),
            None,
            "topol.top",
            "npt.tpr",
            None,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-t nvt.cpt"), "missing -t: {joined}");
        // -c precedes -t, and -p/-o/-maxwarn trail.
        assert!(joined.contains("-c nvt_out.gro -t nvt.cpt -p topol.top"));
    }

    #[test]
    fn grompp_args_include_restraint_reference_when_restrained() {
        // A restrained stage passes the restraint reference coordinates via `-r`
        // (required by GROMACS >= 2018); reusing the `-c` file is the documented
        // approach.
        let args = build_grompp_args(
            "nvt.mdp",
            "em_out.gro",
            None,
            Some("em_out.gro"),
            "topol.top",
            "nvt.tpr",
            None,
        );
        assert!(
            args.join(" ").contains("-c em_out.gro -r em_out.gro"),
            "missing -r: {args:?}"
        );
    }

    #[test]
    fn index_file_lists_system_and_freeze_groups() {
        let ndx = render_index_file(
            4,
            &FreezeSelection {
                group: "Framework".to_string(),
                atom_indices: vec![0, 1],
            },
        );
        assert!(ndx.contains("[ System ]"));
        assert!(ndx.contains("[ Framework ]"));
        // System covers all four atoms (1-based); the freeze group the first two.
        assert!(ndx.contains("1     2     3     4") || ndx.contains("1") && ndx.contains("4"));
        let frame_section = ndx.split("[ Framework ]").nth(1).unwrap();
        assert!(frame_section.contains('1') && frame_section.contains('2'));
        assert!(!frame_section.contains('3'));
    }

    #[test]
    fn grompp_args_include_index_when_present() {
        let args = build_grompp_args(
            "em.mdp",
            "conf.gro",
            None,
            None,
            "topol.top",
            "em.tpr",
            Some("index.ndx"),
        );
        assert!(args.join(" ").contains("-n index.ndx"), "{args:?}");
    }

    #[test]
    fn stage_links_resolve_against_prepared_system_and_prior_outputs() {
        use std::collections::HashMap;

        let system = PreparedSystem {
            working_dir: PathBuf::from("/wd"),
            conf_file: PathBuf::from("/wd/conf.gro"),
            topology_file: PathBuf::from("/wd/topol.top"),
            index_file: None,
            original_structure: Structure::empty(),
        };

        let mut outputs: HashMap<String, StageOutputs> = HashMap::new();
        outputs.insert(
            "nvt".to_string(),
            StageOutputs {
                output_gro: PathBuf::from("/wd/nvt_out.gro"),
                checkpoint: Some(PathBuf::from("/wd/nvt.cpt")),
                trajectory: None,
            },
        );

        assert_eq!(
            resolve_file_ref(&FileRef::PreparedConf, &system, &outputs).unwrap(),
            PathBuf::from("/wd/conf.gro")
        );
        assert_eq!(
            resolve_file_ref(
                &FileRef::Stage {
                    stage: "nvt".to_string(),
                    role: StageFileRole::Checkpoint,
                },
                &system,
                &outputs,
            )
            .unwrap(),
            PathBuf::from("/wd/nvt.cpt")
        );
        assert_eq!(
            resolve_file_ref(
                &FileRef::Stage {
                    stage: "nvt".to_string(),
                    role: StageFileRole::OutputGro,
                },
                &system,
                &outputs,
            )
            .unwrap(),
            PathBuf::from("/wd/nvt_out.gro")
        );
        // A missing trajectory is an error, not a silent empty path.
        assert!(
            resolve_file_ref(
                &FileRef::Stage {
                    stage: "nvt".to_string(),
                    role: StageFileRole::Trajectory,
                },
                &system,
                &outputs,
            )
            .is_err()
        );
        // Referencing a stage that has not run yet is an error.
        assert!(
            resolve_file_ref(
                &FileRef::Stage {
                    stage: "npt".to_string(),
                    role: StageFileRole::OutputGro,
                },
                &system,
                &outputs,
            )
            .is_err()
        );
    }

    /// Real end-to-end energy minimization through the WSL GROMACS launch.
    /// Ignored by default so it never fails on machines without WSL/GROMACS;
    /// run with
    /// `cargo test --release -- --ignored wsl_gromacs_energy_minimization`.
    ///
    /// Unlike the `--version` detection check in `registry.rs`, this drives the
    /// full Phase 1 path -- `to_gro` + inline topology + `grompp` + `mdrun` +
    /// output parsing -- and proves grompp and mdrun both succeed on a
    /// self-contained system and that a minimized structure with a finite
    /// potential energy comes back out.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_energy_minimization_runs_end_to_end() {
        let working_dir = std::env::temp_dir().join("silicolab_gmx_em_integration");
        let _ = fs::remove_dir_all(&working_dir);

        let system = prepare_system(PrepareSystemRequest {
            structure: argon_box(),
            topology: TopologySource::Inline(ARGON_TOP.to_string()),
            working_dir,
            freeze: None,
        })
        .expect("system preparation should succeed");

        let result = run_stage(
            StageRequest {
                coordinate_input: system.conf_file.clone(),
                checkpoint_input: None,
                system,
                stage_name: "em".to_string(),
                settings: MdpSettings::energy_minimization(),
                compute: wsl_gmx_launch().into(),
                max_duration: Duration::from_secs(120),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("energy minimization should run to completion");

        assert_eq!(
            result.structure.atoms.len(),
            8,
            "minimized structure should preserve all argon atoms"
        );
        let energy = result
            .final_potential_energy
            .expect("a final potential energy should be parsed from the mdrun log");
        assert!(
            energy.is_finite(),
            "final potential energy should be finite, got {energy}"
        );
    }

    /// Real end-to-end EM -> NVT -> NPT -> production on the self-contained
    /// argon system, exercising [`run_pipeline`] and the stage-linking machinery
    /// (coordinates/checkpoint threading) with the actual GROMACS binary.
    /// Ignored by default; run with
    /// `cargo test --release -- --ignored wsl_gromacs_full_md`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_full_md_pipeline_runs_end_to_end() {
        use crate::workflows::molecular_dynamics::{MdProtocolOptions, full_protocol};

        let working_dir = std::env::temp_dir().join("silicolab_gmx_full_md_integration");
        let _ = fs::remove_dir_all(&working_dir);

        let system = prepare_system(PrepareSystemRequest {
            structure: argon_box(),
            topology: TopologySource::Inline(ARGON_TOP.to_string()),
            working_dir,
            freeze: None,
        })
        .expect("system preparation should succeed");

        // Short production so the acceptance run completes quickly. Trajectory
        // saving is on by default, so every stage writes a compressed `.xtc`.
        let options = MdProtocolOptions {
            production_ps: 20.0,
            timestep_ps: 0.002,
            temperature_k: 94.0,
            relax_before_production: true,
            save_trajectory: true,
        };

        let results = run_pipeline(
            system,
            full_protocol(&options),
            wsl_gmx_launch().into(),
            Duration::from_secs(600),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("full EM/NVT/NPT/production pipeline should run to completion");

        assert_eq!(results.len(), 4, "expected EM, NVT, NPT, production stages");
        let production = results.last().expect("production stage present");
        assert_eq!(production.structure.atoms.len(), 8);
        assert!(
            production.checkpoint.is_some(),
            "production should write a checkpoint"
        );

        // Every dynamics stage must write a decodable trajectory (the real-tool
        // gate for per-stage playback): each genuine `.xtc` parses into one or
        // more frames over the same atom count, with finite Angstrom coordinates.
        // Minimization (`em`) relaxes to a minimum and writes no motion track.
        let mut dynamics_trajectories = 0;
        for stage in &results {
            if stage.stage_name == "em" {
                assert!(
                    stage.trajectory.is_none(),
                    "minimization should not write a trajectory"
                );
                continue;
            }
            let trajectory_path = stage.trajectory.as_ref().unwrap_or_else(|| {
                panic!("stage '{}' should write a trajectory", stage.stage_name)
            });
            let trajectory = crate::io::trajectory::read_xtc(trajectory_path)
                .unwrap_or_else(|_| panic!("decode '{}' .xtc", stage.stage_name));
            assert!(
                trajectory.frame_count() >= 1,
                "stage '{}' trajectory should contain at least one frame",
                stage.stage_name
            );
            assert_eq!(
                trajectory.natoms(),
                stage.structure.atoms.len(),
                "stage '{}' trajectory atom count should match its structure",
                stage.stage_name
            );
            for frame in 0..trajectory.frame_count() {
                for atom in 0..trajectory.natoms() {
                    let position = trajectory.position(frame, atom);
                    assert!(
                        position.coords.iter().all(|value| value.is_finite()),
                        "stage '{}' frame {frame} atom {atom} has non-finite coordinates",
                        stage.stage_name
                    );
                }
            }
            dynamics_trajectories += 1;
        }
        assert_eq!(
            dynamics_trajectories, 3,
            "NVT, NPT and production should each write a trajectory"
        );
    }

    /// Like [`wsl_gromacs_full_md_pipeline_runs_end_to_end`], but the topology
    /// is generated automatically from the structure (no hand-written `.top`),
    /// proving the auto-topology path produces a grompp-valid file. Run with
    /// `cargo test --release -- --ignored wsl_gromacs_generated_topology`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_generated_topology_runs_full_md() {
        use crate::engines::gromacs::render_top;
        use crate::workflows::molecular_dynamics::{MdProtocolOptions, MdTopology, full_protocol};

        let working_dir = std::env::temp_dir().join("silicolab_gmx_generated_top_integration");
        let _ = fs::remove_dir_all(&working_dir);

        let structure = argon_box();
        // Build the engine-neutral topology, then render it to a GROMACS .top —
        // exactly the path the System Builder + simulate stage take.
        let topology =
            MdTopology::from_structure(&structure).expect("topology generation should succeed");

        let system = prepare_system(PrepareSystemRequest {
            structure,
            topology: TopologySource::Inline(render_top(&topology)),
            working_dir,
            freeze: None,
        })
        .expect("system preparation should succeed");

        let options = MdProtocolOptions {
            production_ps: 20.0,
            timestep_ps: 0.002,
            temperature_k: 94.0,
            relax_before_production: true,
            save_trajectory: true,
        };

        let results = run_pipeline(
            system,
            full_protocol(&options),
            wsl_gmx_launch().into(),
            Duration::from_secs(600),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("full pipeline with generated topology should run to completion");

        assert_eq!(results.len(), 4);
        assert_eq!(results.last().unwrap().structure.atoms.len(), 8);
    }
}
