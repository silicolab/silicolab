use super::*;

mod md_build;
mod remote;
#[cfg(test)]
mod tests;

pub(crate) use md_build::{FRAMEWORK_C_FLOOR_ANGSTROM, build_md_system};
pub(crate) use remote::{
    add_remote_host, check_remote_host, detect_remote_gromacs, fetch_remote_hardware,
    remove_remote_host, save_remote_host, set_monitor_source, setup_remote_host_key,
};

pub(crate) fn start_pending_md_run(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::MdRunPrompt);
    let Some(prompt) = state.ui.pending_md_run.clone() else {
        return;
    };
    if state.jobs.engine_running() {
        state.set_message("another external engine job is already running".to_string());
        return;
    }
    if state.structure().cell.is_none() {
        state.set_message("MD runs need a structure with a simulation box".to_string());
        return;
    }
    // Validate the neutral stage sequence against the effective context; errors
    // block the run with an explanatory message.
    if let Some(eff) = prompt.effective() {
        let issues = crate::workflows::molecular_dynamics::run::validate(&prompt.stages, &eff);
        if crate::workflows::molecular_dynamics::run::has_errors(&issues) {
            let first = issues
                .iter()
                .find(|issue| {
                    issue.severity
                        == crate::workflows::molecular_dynamics::run::IssueSeverity::Error
                })
                .map(|issue| issue.message.clone())
                .unwrap_or_default();
            state.set_message(format!("Cannot run: {first}"));
            return;
        }
    }
    if prompt.stages.is_empty() {
        state.set_message("Add at least one MD stage".to_string());
        return;
    }
    let topology = match resolve_md_topology_source(state, &prompt) {
        Ok(topology) => topology,
        Err(error) => {
            state.set_message(error.to_string());
            return;
        }
    };
    // Realize the neutral stages into GROMACS stage specs (modern engine assumed;
    // no version probing on the UI thread).
    let mut stages = crate::engines::gromacs::stage_specs_from_md_stages(
        &prompt.stages,
        prompt.force_field_family(),
        None,
    );
    // A framework (nanosheet) system carries run hints from its build: keep the
    // molecule periodic (flexible) and/or freeze the sheet (rigid). Apply them to
    // every stage and capture the freeze selection for prepare_system.
    let framework_freeze = state
        .entries
        .active_entry_id()
        .and_then(|id| crate::frontend::md_support::load_framework_metadata_for_entry(state, id))
        .and_then(|meta| {
            for spec in &mut stages {
                meta.apply_to(&mut spec.settings);
            }
            meta.freeze_selection()
        });
    // A remote target relays the whole pipeline to a deployed worker, which
    // resolves `gmx` and runs it on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_remote_host(state, &prompt.prefs.target) {
        let topology = match crate::workflows::gromacs::WireTopology::from_source(&topology) {
            Ok(topology) => topology,
            Err(error) => {
                state.set_message(format!("could not read the run topology: {error}"));
                return;
            }
        };
        let job = crate::workflows::gromacs::GromacsJob::Run(
            crate::workflows::gromacs::GromacsRunRequest {
                structure: state.structure().clone(),
                topology,
                stages,
                max_duration_per_stage: Duration::from_secs(60 * 60),
                freeze: framework_freeze,
                resources: crate::engines::remote::ComputeResources {
                    cores: prompt.prefs.cores_per_subtask,
                    gpu: prompt.prefs.gpu_count,
                },
            },
        );
        state.optimization_origin = None;
        state.ui.pending_md_run = None;
        relay_gromacs_job(state, host, prompt.engine.label(), job);
        return;
    }
    let mut compute = match resolve_md_compute(state, prompt.engine) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return;
        }
    };
    // Local run: apply the panel's CPU/GPU request to the mdrun stages.
    compute.resources = crate::engines::remote::ComputeResources {
        cores: prompt.prefs.cores_per_subtask,
        gpu: prompt.prefs.gpu_count,
    };

    let working_dir =
        match ensure_active_task_run_dir(state, TaskKind::RunMd, Some(prompt.run_name.as_str())) {
            Ok(path) => path,
            Err(error) => {
                state.set_message(format!("failed to create run directory: {error}"));
                complete_active_task(state, TaskKind::RunMd, TaskStatus::Failed);
                return;
            }
        };
    if let Some(task_run_id) = state.active_task_run {
        state
            .tasks
            .set_engine_label(task_run_id, Some(prompt.engine.label().to_string()));
        sync_task_manifest(state, task_run_id);
    }
    let job = spawn_gromacs_pipeline_job(GromacsPipelineRequest {
        structure: state.structure().clone(),
        topology,
        stages,
        working_dir,
        compute,
        max_duration_per_stage: Duration::from_secs(60 * 60),
        freeze: framework_freeze,
    });
    state.optimization_origin = None;
    state.ui.pending_md_run = None;
    state.jobs.set_engine(job);
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
    }
    state.set_message(format!(
        "{} MD running; press Esc to stop",
        prompt.engine.label()
    ));
}

pub(crate) fn cancel_pending_md_run_request(state: &mut AppState) {
    bind_active_panel_task(state, TaskPanelKind::MdRunPrompt);
    if state.jobs.engine_running() {
        let _ = crate::frontend::jobs::cancel_controlled_job(
            state,
            &crate::frontend::jobs::JobControlId::Local(
                crate::frontend::jobs::LocalJobSlot::Engine,
            ),
        );
    }
    state.ui.pending_md_run = None;
    state.set_message("MD run canceled".to_string());
    complete_active_task(state, TaskKind::RunMd, TaskStatus::Failed);
    close_active_task_panel(state);
}

pub(crate) fn resolve_md_topology_source(
    state: &AppState,
    prompt: &crate::frontend::state::MdRunPrompt,
) -> anyhow::Result<TopologySource> {
    if let Some(path) = prompt.topology_override_path.clone() {
        return Ok(TopologySource::File(path));
    }

    if let Some(entry_id) = state.entries.active_entry_id() {
        // Prefer a force-field topology produced by a GROMACS build; it is the
        // real `topol.top` (with FF/water/ion includes) the run reuses directly.
        if let Some(path) = gromacs_topology_path_for_entry(state, entry_id) {
            return Ok(TopologySource::File(path));
        }
        // Otherwise fall back to a captured engine-neutral topology (e.g. from
        // the `md build` console command for a monatomic system).
        if let Some(topology) = load_md_topology_for_entry(state, entry_id) {
            return Ok(TopologySource::Inline(render_top(&topology)));
        }
    }

    let topology = crate::workflows::molecular_dynamics::MdTopology::from_structure(
        state.structure(),
    )
    .map_err(|_| {
        anyhow!(
            "No automatic MD topology is available for this structure. Build an MD system first or choose a custom topology in Advanced."
        )
    })?;
    Ok(TopologySource::Inline(render_top(&topology)))
}

pub(crate) fn resolve_md_engine_launch(
    state: &mut AppState,
    engine: crate::frontend::state::MdEngineChoice,
) -> anyhow::Result<crate::engines::registry::EngineLaunch> {
    let registry = EngineRegistry::probe(&state.config.engine_overrides);
    match engine {
        crate::frontend::state::MdEngineChoice::Gromacs => {
            // A configured override or a native PATH install wins (cheap, already
            // resolved by probe). Otherwise, on Windows GROMACS conventionally
            // lives in WSL: auto-detect it (cold-starts WSL once) so the common
            // setup works with no manual configuration. Only when there is no
            // WSL, or WSL has no gmx, do we surface the not-found guidance.
            if let Some(launch) = registry.launch(EngineId::GROMACS).cloned() {
                return Ok(launch);
            }
            let launch = crate::engines::registry::detect_wsl_gromacs_launch().ok_or_else(|| {
                anyhow!(
                    "Could not find GROMACS. Install it and ensure `gmx` is on PATH, set up WSL with GROMACS installed, or configure its launch in Settings -> Engines."
                )
            })?;
            // Persist the detected launch as an override so later builds reuse it
            // directly instead of cold-starting WSL to re-probe every time (slow),
            // and so it shows up in Settings -> Engines.
            persist_detected_engine_launch(state, EngineId::GROMACS, launch.clone());
            Ok(launch)
        }
    }
}

/// The local [`Compute`] for an MD job: the resolved `gmx` launch (override / PATH
/// / WSL auto-detect). A remote MD job does not build a `Compute` — it relays
/// through [`start_remote_engine`] and the worker resolves `gmx` on the node — so
/// this only ever yields a local launch.
pub(crate) fn resolve_md_compute(
    state: &mut AppState,
    engine: crate::frontend::state::MdEngineChoice,
) -> anyhow::Result<crate::engines::remote::Compute> {
    Ok(crate::engines::remote::Compute::local(
        resolve_md_engine_launch(state, engine)?,
    ))
}

/// Cache an auto-detected engine launch into `engine_overrides` and save the
/// config, so later builds reuse it without re-probing. No-op when an override
/// already exists (set by the user or a prior detection) so a configured launch
/// is never clobbered.
pub(crate) fn persist_detected_engine_launch(
    state: &mut AppState,
    id: EngineId,
    launch: crate::engines::registry::EngineLaunch,
) {
    if cache_engine_override(&mut state.config.engine_overrides, id, launch) {
        // Keep the Settings panel draft in sync so it reflects the cached launch.
        state.ui.settings.engine_drafts.remove(id.as_str());
        persist_engine_config(state, "GROMACS launch detected and saved");
        // Refresh the Settings registry so the engine's status indicator flips to
        // available (green) immediately — the detection just succeeded, so the
        // user shouldn't have to click "Detect" to see it. Cheap re-probe (reads
        // the override; no `--version` WSL cold-start).
        reprobe_engines(state);
    }
}

/// Insert `launch` as the override for `id` only when none is configured.
/// Returns `true` when newly inserted (the caller should then persist), `false`
/// when an existing override was left untouched.
pub(crate) fn cache_engine_override(
    overrides: &mut std::collections::HashMap<String, crate::engines::registry::EngineLaunch>,
    id: EngineId,
    launch: crate::engines::registry::EngineLaunch,
) -> bool {
    let key = id.as_str().to_string();
    if overrides.contains_key(&key) {
        return false;
    }
    overrides.insert(key, launch);
    true
}

/// Load the MD system context recorded by the active entry's build, or derive a
/// minimal one from the active structure (generic family) when none was recorded.
pub(crate) fn load_or_derive_md_context(
    state: &AppState,
) -> crate::workflows::molecular_dynamics::MdSystemContext {
    use crate::workflows::molecular_dynamics::{MdSystemContext, is_framework_shape};
    if let Some(id) = state.entries.active_entry_id()
        && let Some(context) =
            crate::frontend::md_support::load_md_system_context_for_entry(state, id)
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

/// Edit the Run MD prompt in place (the dispatcher is the only mutator of state).
pub(crate) fn with_md_run_prompt(
    state: &mut AppState,
    edit: impl FnOnce(&mut crate::frontend::state::MdRunPrompt),
) {
    if let Some(prompt) = state.ui.pending_md_run.as_mut() {
        edit(prompt);
    }
}

/// Change the Run MD preset and rebuild the stage sequence for the context.
pub(crate) fn set_md_run_preset(
    state: &mut AppState,
    preset: crate::workflows::molecular_dynamics::PresetId,
) {
    with_md_run_prompt(state, |prompt| {
        prompt.preset = preset;
        prompt.rebuild_stages();
    });
}

/// Set a system-type override and rebuild the stages. Edits the per-run overrides
/// only — the persisted detection context is never touched.
pub(crate) fn set_md_run_override(
    state: &mut AppState,
    axis: crate::frontend::state::MdSystemAxis,
    value: Option<bool>,
) {
    use crate::frontend::state::MdSystemAxis;
    with_md_run_prompt(state, |prompt| {
        match axis {
            MdSystemAxis::Membrane => prompt.overrides.membrane = value,
            MdSystemAxis::Ligand => prompt.overrides.ligand = value,
            MdSystemAxis::Nucleic => prompt.overrides.nucleic = value,
        }
        prompt.rebuild_stages();
    });
}

/// Cheap availability resolve (no subprocess). Used to populate the panel on
/// first open and after edits, without paying the WSL `--version` cost.
pub(crate) fn reprobe_engines(state: &mut AppState) {
    state.ui.settings.engine_registry = Some(EngineRegistry::probe(&state.config.engine_overrides));
}

/// Slow, user-initiated: resolve availability *and* run each engine's
/// `--version`, then record the time so the panel can show how fresh the
/// version strings are.
pub(crate) fn detect_engine_versions(state: &mut AppState) {
    state.ui.settings.engine_registry = Some(EngineRegistry::probe_with_versions(
        &state.config.engine_overrides,
    ));
    state.ui.settings.engine_versions_checked_at = Some(std::time::SystemTime::now());
    state.set_message("Detected engine versions".to_string());
}

pub(crate) fn apply_engine_override(state: &mut AppState, id: EngineId) {
    let key = id.as_str().to_string();
    let draft = state
        .ui
        .settings
        .engine_drafts
        .entry(key.clone())
        .or_default();
    match draft.to_launch() {
        Some(launch) => {
            state.config.engine_overrides.insert(key, launch);
        }
        None => {
            state.config.engine_overrides.remove(&key);
        }
    }
    // "Apply & Detect" is an explicit user action, so paying the version probe
    // cost here is expected.
    detect_engine_versions(state);
    persist_engine_config(state, "engine launch updated");
}

pub(crate) fn clear_engine_override(state: &mut AppState, id: EngineId) {
    let key = id.as_str().to_string();
    state.config.engine_overrides.remove(&key);
    state.ui.settings.engine_drafts.remove(&key);
    persist_engine_config(state, "engine override cleared; using auto-detection");
    reprobe_engines(state);
}

pub(crate) fn browse_engine_program(state: &mut AppState, id: EngineId) {
    let Some(path) = rfd::FileDialog::new()
        .set_directory(&state.config.default_project_dir)
        .pick_file()
    else {
        return;
    };
    let key = id.as_str().to_string();
    let draft = state.ui.settings.engine_drafts.entry(key).or_default();
    draft.program = path.to_string_lossy().into_owned();
}

pub(crate) fn persist_engine_config(state: &mut AppState, message: &str) {
    match save_config(&state.config) {
        Ok(()) => state.set_message(message.to_string()),
        Err(error) => state.set_message(format!("failed to save engine settings: {error}")),
    }
}

pub(crate) fn add_hydrogens(state: &mut AppState) {
    let before = state.capture_edit_snapshot();
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    let added = state.structure_mut().add_missing_hydrogens();
    state.mark_structure_changed();
    state.set_source_path(None);
    state.ui.editor = None;
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    state.history.push_undo(before);
    state.set_message(format!("Added {added} hydrogens"));
}

pub(crate) fn recompute_bonds(state: &mut AppState) {
    let before = state.capture_edit_snapshot();
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.structure_mut().recompute_bonds();
    state.mark_structure_changed();
    state.set_source_path(None);
    state.ui.editor = None;
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    state.history.push_undo(before);
    state.set_message(format!(
        "Recomputed bonds: {} bonds detected",
        state.structure().bonds.len()
    ));
}

pub(crate) fn translate_atoms_into_first_unit_cell(state: &mut AppState) {
    if let Err(error) = require_periodic_structure(
        state.structure(),
        "translating atoms into the first unit cell requires a periodic structure",
    ) {
        state.set_message(error.to_string());
        return;
    }

    let before = state.capture_edit_snapshot();
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.set_source_path(None);
    state.ui.editor = None;
    state
        .structure_mut()
        .wrap_atoms_into_cell_preserving_bonds();
    state.mark_structure_changed();
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    state.history.push_undo(before);
    state.set_message("Translated atoms into the first unit cell".to_string());
}

pub(crate) fn expand_supercell(state: &mut AppState, repeats: [u32; 3]) {
    let before = state.capture_edit_snapshot();
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.set_source_path(None);
    state.ui.editor = None;
    state.structure_mut().make_supercell(repeats);
    state.mark_structure_changed();
    state.ui.selection.clear();
    state.history.push_undo(before);
    state.set_message(format!(
        "Expanded to {}x{}x{} supercell ({} atoms, {} bonds)",
        repeats[0],
        repeats[1],
        repeats[2],
        state.structure().atoms.len(),
        state.structure().bonds.len()
    ));
    complete_active_task(state, TaskKind::ExpandSupercell, TaskStatus::Completed);
}
