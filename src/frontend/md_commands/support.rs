//! Shared helpers for the `md` commands: engine launch resolution, topology /
//! context loading, the boxed-structure guard, and the post-run energy analysis.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};

use crate::engines::gromacs::topology::TopologySource;
use crate::{
    domain::Structure,
    engines::{
        gromacs::{AnalysisContext, render_top},
        registry::{EngineId, EngineLaunch, EngineRegistry},
    },
    frontend::{
        md_support::{
            gromacs_topology_path_for_entry, load_md_system_context_for_entry,
            md_topology_path_for_entry,
        },
        state::AppState,
    },
    workflows::molecular_dynamics::{MdTopology, is_framework_shape, run::MdSystemContext},
};

pub const STAGE_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
pub const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn resolve_launch(state: &AppState) -> Result<EngineLaunch> {
    let registry = EngineRegistry::probe(&state.config.engine_overrides);
    registry.launch(EngineId::GROMACS).cloned().ok_or_else(|| {
        anyhow!(
            "Could not find GROMACS. Install it and ensure `gmx` is on PATH, or configure its \
             launch (including WSL) in Settings -> Engines."
        )
    })
}

/// Load the MD system context recorded by the active entry's build, or derive a
/// minimal one from the active structure when no build recorded it (e.g. a
/// directly-opened coordinate file). The minimal context classifies to the
/// generic force-field family, so the run uses the legacy cut-off path.
pub fn load_or_derive_context(state: &AppState) -> MdSystemContext {
    if let Some(id) = state.entries.active_entry_id()
        && let Some(context) = load_md_system_context_for_entry(state, id)
    {
        return context;
    }
    let structure = state.structure();
    MdSystemContext::from_built(
        structure,
        "builtin",
        None,
        is_framework_shape(structure),
        0.0,
        false,
        Vec::new(),
    )
}

/// The topology source for a run: the GROMACS `topol.top` from the entry's build
/// when present (the real force-field topology), else the captured engine-neutral
/// topology rendered inline.
pub fn resolve_run_topology(state: &AppState, entry_id: Option<u64>) -> Result<TopologySource> {
    if let Some(id) = entry_id
        && let Some(path) = gromacs_topology_path_for_entry(state, id)
    {
        return Ok(TopologySource::File(path));
    }
    let topology = load_active_or_derive_md_topology(state)?;
    Ok(TopologySource::Inline(render_top(&topology)))
}

/// Extract thermodynamic terms (Temperature, Potential) from the production
/// energy file. Analysis failures are reported but do not fail the whole command
/// (the trajectory is the primary deliverable).
pub fn run_analysis(
    work_dir: &Path,
    launch: &EngineLaunch,
    production: &crate::engines::gromacs::StageResult,
    cancel: Arc<AtomicBool>,
) -> String {
    let ctx = AnalysisContext {
        working_dir: work_dir.to_path_buf(),
        gmx_launch: launch.clone(),
        max_duration: ANALYSIS_TIMEOUT,
    };
    match crate::engines::gromacs::gmx_energy(
        &ctx,
        &production.edr,
        "energy.xvg",
        &["Temperature", "Potential"],
        cancel,
        |_| {},
    ) {
        Ok(_) => "; analysis: energy.xvg (Temperature, Potential)".to_string(),
        Err(_) => String::new(),
    }
}

/// Clone the active structure after checking it is a usable MD system: non-empty
/// and carrying a simulation box (as produced by `md build` / the System
/// Builder, or by opening a `.gro` with box vectors).
pub fn require_boxed_structure(state: &AppState) -> Result<Structure> {
    let structure = state.structure();
    if structure.atoms.is_empty() {
        bail!("no active structure; open or build a system before `md simulate`");
    }
    if structure.cell.is_none() {
        bail!(
            "the active structure has no simulation box; run `md build` (or the MD System \
             Builder) first"
        );
    }
    Ok(structure.clone())
}

fn active_entry_md_topology_path(state: &AppState) -> Option<PathBuf> {
    let entry_id = state.entries.active_entry_id()?;
    md_topology_path_for_entry(state, entry_id)
}

pub fn load_active_or_derive_md_topology(state: &AppState) -> Result<MdTopology> {
    if let Some(path) = active_entry_md_topology_path(state) {
        return MdTopology::load(&path);
    }
    MdTopology::from_structure(state.structure())
}
