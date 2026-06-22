//! The GROMACS relay: the serializable job a client submits to a headless worker
//! and the outcome it returns, plus the worker-side orchestration that runs the
//! whole `gmx` pipeline locally on the executing host.
//!
//! `gmx` is an external engine, so rather than ship a library result the worker
//! relays an ENTIRE multi-stage pipeline in one allocation. A [`GromacsJob`]
//! carries everything needed to run a run / build / material-build pipeline on the
//! node; [`run_gromacs_calculation`] resolves a local `gmx`, reconstructs the
//! engine request, and reuses the same [`run_pipeline`]/[`build_system`]/
//! [`build_material_system`] the local path uses. A [`GromacsOutcome`] carries the
//! produced structure, the final trajectory bytes, and — for a build — the
//! topology and system context the client persists for later runs. Every embedded
//! [`Structure`]/[`UnitCell`] crosses through the payload bridge, never a serde
//! derive on the domain type.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::domain::{Structure, UnitCell};
use crate::engines::gromacs::{
    BuildRequest, FrameworkRunHints, FreezeSelection, GromacsProgress, IonOptions,
    MaterialBuildRequest, PrepareSystemRequest, StageResult, StageSpec, TopologySource,
    build_material_system, build_system, prepare_system, run_pipeline,
};
use crate::engines::registry::detect_local_gromacs;
use crate::engines::remote::{Compute, GMX_REMOTE_CANDIDATES};
use crate::workflows::molecular_dynamics::{
    FrameworkMode, MdSystemConfig, MdSystemContext, SolvationOptions, WaterModel,
};

/// The `force_field_token` recorded for a framework build's system context — a
/// framework has no biomolecular force-field convention. Mirrors the local path.
const FRAMEWORK_FORCE_FIELD_TOKEN: &str = "framework";

/// One of the three remote `gmx` pipelines, with the portable inputs the worker
/// needs to run it. The non-portable bits a local pipeline carries (the `Compute`
/// launch, absolute working-dir paths) are supplied by the worker on the node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GromacsJob {
    Run(GromacsRunRequest),
    Build(GromacsBuildRequest),
    BuildMaterial(GromacsMaterialRequest),
}

/// A multi-stage MD run: a prepared system (structure + topology + optional freeze
/// group) and the stage chain to run against it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsRunRequest {
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    pub topology: WireTopology,
    pub stages: Vec<StageSpec>,
    pub max_duration_per_stage: Duration,
    pub freeze: Option<FreezeSelection>,
}

/// A biomolecular system build (`pdb2gmx` → `editconf` → `solvate`/`genion`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsBuildRequest {
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    pub force_field: String,
    pub water: WaterModel,
    pub box_config: MdSystemConfig,
    pub solvate: bool,
    pub ions: Option<IonOptions>,
    pub max_duration: Duration,
}

/// A covalent-framework (nanosheet) build: topology from the bonds, optional
/// solvation, optional explicit cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsMaterialRequest {
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    pub mode: FrameworkMode,
    pub solvation: Option<SolvationOptions>,
    pub custom_force_field: Option<String>,
    #[serde(with = "crate::payload::cell_serde_opt")]
    pub cell_override: Option<UnitCell>,
    pub solvent_gap_angstrom: f32,
    pub cutoff_nm: f32,
    pub max_duration: Duration,
}

/// A GROMACS topology in a transport-portable form: the `.top` body plus every
/// `.itp` it `#include`s, carried by name. A client builds it from a local
/// [`TopologySource`] (reading sibling `.itp` files for a `File` source); the
/// worker writes the includes into the run dir then prepares from the inline top.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTopology {
    pub top: String,
    pub includes: Vec<(String, String)>,
}

/// The result of a relayed GROMACS pipeline. `trajectory` rides as bytes inside
/// the outcome (the detached client only retrieves `outcome.json`); `topology` and
/// `system_context` are `Some` only for a build, and `material` only for a
/// framework build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsOutcome {
    #[serde(with = "crate::payload::structure_serde")]
    pub structure: Structure,
    pub summary: String,
    pub stages: Vec<GromacsStageReport>,
    pub trajectory: Option<GromacsTrajectory>,
    pub topology: Option<WireTopology>,
    pub system_context: Option<MdSystemContext>,
    pub material: Option<GromacsMaterialReport>,
}

/// Per-stage summary a run reports back (mirrors the fields a finished local run
/// surfaces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsStageReport {
    pub stage_name: String,
    pub final_potential_energy: Option<f64>,
    pub wall_time: Duration,
}

/// A trajectory file carried inline in the outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsTrajectory {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// The extra metadata a framework build records so a later run applies the right
/// periodic-molecules / freeze settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GromacsMaterialReport {
    pub framework_atom_count: usize,
    pub hints: FrameworkRunHints,
}

impl WireTopology {
    /// Build the wire form from a client-side topology source, reading sibling
    /// `.itp` includes off disk for a `File` source (the relay analogue of
    /// `copy_topology_includes`).
    pub fn from_source(source: &TopologySource) -> Result<Self> {
        match source {
            TopologySource::Inline(top) => Ok(Self {
                top: top.clone(),
                includes: Vec::new(),
            }),
            TopologySource::File(path) => {
                let top = std::fs::read_to_string(path)
                    .with_context(|| format!("reading topology {}", path.display()))?;
                let mut includes = Vec::new();
                if let Some(dir) = path.parent()
                    && let Ok(entries) = std::fs::read_dir(dir)
                {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some("itp")
                            && let Some(name) = p.file_name().and_then(|n| n.to_str())
                        {
                            let contents = std::fs::read_to_string(&p)
                                .with_context(|| format!("reading topology include {name}"))?;
                            includes.push((name.to_string(), contents));
                        }
                    }
                }
                Ok(Self { top, includes })
            }
        }
    }

    /// Write the includes into `dir` and return an inline topology source the
    /// worker prepares from. The `#include`d files land beside the `.top` so
    /// `grompp` resolves them.
    fn materialize(self, dir: &Path) -> Result<TopologySource> {
        for (name, contents) in &self.includes {
            std::fs::write(dir.join(name), contents)
                .with_context(|| format!("writing topology include {name}"))?;
        }
        Ok(TopologySource::Inline(self.top))
    }
}

/// Run a relayed GROMACS job on the executing host: resolve a local `gmx`, then
/// reconstruct and run the requested pipeline, packaging its result for retrieval.
pub fn run_gromacs_calculation(
    job: GromacsJob,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(GromacsProgress),
) -> Result<GromacsOutcome> {
    let launch = detect_local_gromacs().ok_or_else(|| {
        anyhow!("no working `gmx` found on this host (tried {GMX_REMOTE_CANDIDATES:?}); check the host prelude")
    })?;
    let compute = Compute::local(launch);
    let working_dir = std::env::current_dir().context("resolving the run working directory")?;
    match job {
        GromacsJob::Run(request) => run_relay(request, compute, working_dir, cancel, progress),
        GromacsJob::Build(request) => build_relay(request, compute, working_dir, cancel, progress),
        GromacsJob::BuildMaterial(request) => {
            material_relay(request, compute, working_dir, cancel, progress)
        }
    }
}

fn run_relay(
    request: GromacsRunRequest,
    compute: Compute,
    working_dir: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(GromacsProgress),
) -> Result<GromacsOutcome> {
    let topology = request.topology.materialize(&working_dir)?;
    let system = prepare_system(PrepareSystemRequest {
        structure: request.structure,
        topology,
        working_dir,
        freeze: request.freeze,
    })?;
    let results = run_pipeline(
        system,
        request.stages,
        compute,
        request.max_duration_per_stage,
        cancel,
        progress,
    )?;
    run_outcome(results)
}

fn build_relay(
    request: GromacsBuildRequest,
    compute: Compute,
    working_dir: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(GromacsProgress),
) -> Result<GromacsOutcome> {
    // The solute (not the solvated output) carries the residue metadata the
    // system-type detection reads.
    let solute = request.structure.clone();
    let force_field = request.force_field.clone();
    let water_token = request
        .solvate
        .then(|| request.water.db_token().to_string());
    let outcome = build_system(
        BuildRequest {
            structure: request.structure,
            working_dir: working_dir.clone(),
            compute,
            force_field: request.force_field,
            water: request.water,
            box_config: request.box_config,
            solvate: request.solvate,
            ions: request.ions,
            max_duration: request.max_duration,
        },
        cancel,
        progress,
    )?;

    // pdb2gmx writes posre.itp, giving the run a "solute" restraint group.
    let restraint_groups = if working_dir.join("posre.itp").exists() {
        vec!["solute".to_string()]
    } else {
        Vec::new()
    };
    let context = built_context(
        &solute,
        &outcome.structure,
        &force_field,
        water_token.as_deref(),
        false,
        restraint_groups,
    );
    let topology = WireTopology::from_source(&TopologySource::File(outcome.topology_file))?;
    Ok(GromacsOutcome {
        structure: outcome.structure,
        summary: outcome.summary,
        stages: Vec::new(),
        trajectory: None,
        topology: Some(topology),
        system_context: Some(context),
        material: None,
    })
}

fn material_relay(
    request: GromacsMaterialRequest,
    compute: Compute,
    working_dir: PathBuf,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(GromacsProgress),
) -> Result<GromacsOutcome> {
    let solute = request.structure.clone();
    let water_token = request
        .solvation
        .as_ref()
        .map(|solvation| solvation.water.db_token().to_string());
    let outcome = build_material_system(
        MaterialBuildRequest {
            structure: request.structure,
            mode: request.mode,
            working_dir: working_dir.clone(),
            compute,
            solvation: request.solvation,
            custom_force_field: request.custom_force_field,
            cell_override: request.cell_override,
            solvent_gap_angstrom: request.solvent_gap_angstrom,
            cutoff_nm: request.cutoff_nm,
            max_duration: request.max_duration,
        },
        cancel,
        progress,
    )?;

    let context = built_context(
        &solute,
        &outcome.structure,
        FRAMEWORK_FORCE_FIELD_TOKEN,
        water_token.as_deref(),
        true,
        Vec::new(),
    );
    let topology = WireTopology::from_source(&TopologySource::File(outcome.topology_file))?;
    Ok(GromacsOutcome {
        structure: outcome.structure,
        summary: outcome.summary,
        stages: Vec::new(),
        trajectory: None,
        topology: Some(topology),
        system_context: Some(context),
        material: Some(GromacsMaterialReport {
            framework_atom_count: outcome.framework_atom_count,
            hints: outcome.hints,
        }),
    })
}

/// Build the system-context detection record for a finished build, mirroring the
/// local `write_md_system_context` path (`net_charge`/`hmr` are not parsed back
/// from the topology, matching the local default).
fn built_context(
    solute: &Structure,
    built: &Structure,
    force_field_token: &str,
    water_token: Option<&str>,
    is_framework: bool,
    restraint_groups: Vec<String>,
) -> MdSystemContext {
    let mut context = MdSystemContext::from_built(
        solute,
        force_field_token,
        water_token,
        is_framework,
        0.0,
        false,
        restraint_groups,
    );
    context.atom_count = built.atoms.len();
    context
}

fn run_outcome(results: Vec<StageResult>) -> Result<GromacsOutcome> {
    let final_result = results
        .last()
        .ok_or_else(|| anyhow!("the GROMACS pipeline produced no stages"))?;
    let stage_count = results.len();
    let stage = &final_result.stage_name;
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
    let structure = final_result.structure.clone();
    let stages = results
        .iter()
        .map(|stage| GromacsStageReport {
            stage_name: stage.stage_name.clone(),
            final_potential_energy: stage.final_potential_energy,
            wall_time: stage.wall_time,
        })
        .collect();
    // The production stage writes the compressed `.xtc`; take the last stage that
    // produced one so playback follows the actual MD trajectory.
    let trajectory = results
        .iter()
        .rev()
        .find_map(|stage| stage.trajectory.as_ref())
        .map(|path| read_trajectory(path))
        .transpose()?;
    Ok(GromacsOutcome {
        structure,
        summary,
        stages,
        trajectory,
        topology: None,
        system_context: None,
        material: None,
    })
}

/// Upper bound on an embedded trajectory. The `.xtc` bytes travel inside the JSON
/// outcome — and for a remote run are encoded as a number array and read back
/// into RAM on the client — so an unbounded read would let a long production run
/// OOM the client. Above this, fail with an actionable message rather than
/// producing an unusable multi-gigabyte outcome.
const MAX_TRAJECTORY_BYTES: u64 = 512 * 1024 * 1024;

fn read_trajectory(path: &Path) -> Result<GromacsTrajectory> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trajectory.xtc")
        .to_string();
    let len = std::fs::metadata(path)
        .with_context(|| format!("reading trajectory {}", path.display()))?
        .len();
    if len > MAX_TRAJECTORY_BYTES {
        bail!(
            "trajectory {} is {} MiB, over the {} MiB limit for inline playback; \
             reduce the run length or trajectory output frequency (nstxout-compressed)",
            path.display(),
            len / (1024 * 1024),
            MAX_TRAJECTORY_BYTES / (1024 * 1024)
        );
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("reading trajectory {}", path.display()))?;
    Ok(GromacsTrajectory { file_name, bytes })
}
