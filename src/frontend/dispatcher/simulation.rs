use super::*;

use crate::backend::config::ComputeTarget;
use crate::frontend::state::{EngineDraft, EngineProbeState};

mod md_build;
mod remote;
#[cfg(test)]
mod tests;

pub(crate) use md_build::{FRAMEWORK_C_FLOOR_ANGSTROM, build_md_system};
pub(crate) use remote::{
    add_remote_host, cancel_add_remote_host, check_remote_host, commit_remote_host_draft,
    detect_remote_slurm, fetch_remote_hardware, refresh_slurm_capabilities, remove_remote_host,
    save_remote_host, set_monitor_source, setup_remote_host_key, test_remote_slurm,
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
    // A remote target relays the whole pipeline to a deployed worker, which runs
    // the `gmx` the submission resolved against that host; the local arm runs
    // `gmx` here.
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
                resources: crate::launch::ComputeResources {
                    cores: prompt.prefs.cores_per_subtask,
                    gpu: prompt.prefs.gpu.count(),
                },
            },
        );
        state.optimization_origin = None;
        state.ui.pending_md_run = None;
        let resources = prompt.prefs.job_resources();
        relay_gromacs_job(state, host, prompt.engine.label(), job, resources);
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
    compute.resources = crate::launch::ComputeResources {
        cores: prompt.prefs.cores_per_subtask,
        gpu: prompt.prefs.gpu.count(),
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
        begin_local_job(
            state,
            crate::frontend::jobs::LocalJobSlot::Engine,
            task_run_id,
        );
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

/// The `gmx` launch for a local MD job: the same override → probe → error rule a
/// remote job resolves against its host, here against this machine. A detected
/// launch is cached as an override so later builds reuse it rather than
/// cold-starting WSL to re-probe every time.
pub(crate) fn resolve_md_engine_launch(
    state: &mut AppState,
    engine: crate::frontend::state::MdEngineChoice,
) -> anyhow::Result<crate::engines::registry::EngineLaunch> {
    use crate::backend::engine_launch::{LaunchTarget, resolve_engine_launch};
    let id = match engine {
        crate::frontend::state::MdEngineChoice::Gromacs => EngineId::GROMACS,
    };
    let resolved = resolve_engine_launch(LaunchTarget::Local(&state.config.engine_overrides), id)?;
    if resolved.detected {
        persist_detected_engine_launch(
            state,
            id,
            resolved.launch.clone(),
            resolved.version.clone(),
        );
    }
    Ok(resolved.launch)
}

/// The local [`Compute`] for an MD job: the resolved `gmx` launch. A remote MD job
/// resolves its own launch against the target host in [`spawn_remote_submit`] and
/// ships it in `request.json`, so this only ever yields a local launch.
///
/// [`spawn_remote_submit`]: crate::frontend::remote_jobs::spawn_remote_submit
pub(crate) fn resolve_md_compute(
    state: &mut AppState,
    engine: crate::frontend::state::MdEngineChoice,
) -> anyhow::Result<crate::launch::Compute> {
    Ok(crate::launch::Compute::local(resolve_md_engine_launch(
        state, engine,
    )?))
}

/// Cache an auto-detected engine launch into `engine_overrides` and save the
/// config, so later builds reuse it without re-probing. No-op when a launch is
/// already configured (by the user or a prior detection), which is never clobbered.
///
/// A probe verifies what it finds, so `version` is a proof of *this* launch and is
/// stored with it — the panel can show it without re-running the engine.
pub(crate) fn persist_detected_engine_launch(
    state: &mut AppState,
    id: EngineId,
    launch: crate::engines::registry::EngineLaunch,
    version: Option<String>,
) {
    if state
        .config
        .engine_overrides
        .cache_detected(id, launch, version)
    {
        // Keep the Settings panel draft in sync so it reflects the cached launch.
        state.ui.settings.engine_drafts.remove(id.as_str());
        persist_engine_config(state, "GROMACS launch detected and saved");
        reprobe_engines(state);
    }
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

/// Cheap resolve of what is *configured* (no subprocess). Used to populate the
/// panel on first open and after edits, without paying the WSL `--version` cost.
pub(crate) fn reprobe_engines(state: &mut AppState) {
    state.ui.settings.engine_registry = Some(EngineRegistry::probe(&state.config.engine_overrides));
}

/// Verify `engine` on `target`: commit whatever the user has typed, then run it on
/// a worker thread. Committing first is what makes the button and the field reach
/// each other — the launch that gets verified is the one on screen.
///
/// An empty program is committed as "not configured", which makes the verification
/// a probe of the target's candidates; its result is written back into the field.
pub(crate) fn verify_engine(state: &mut AppState, target: ComputeTarget, engine: EngineId) {
    use crate::frontend::jobs::VerifyTarget;

    if state.jobs.engine_verify.is_some() {
        state.set_message("An engine check is already running".to_string());
        return;
    }
    let launches = match &target {
        ComputeTarget::Local => {
            commit_local_engine_draft(state, engine);
            VerifyTarget::Local(state.config.engine_overrides.clone())
        }
        ComputeTarget::Remote(id) => {
            if let Err(error) = crate::engines::remote::ensure_ssh_available() {
                state.set_message(error.to_string());
                return;
            }
            match super::commit_remote_host_draft(state, id) {
                Ok(host) => VerifyTarget::Remote(Box::new(host)),
                Err(error) => {
                    state.set_message(error.to_string());
                    return;
                }
            }
        }
    };
    state.ui.settings.engine_probe.insert(
        (target.clone(), engine.as_str()),
        EngineProbeState::Verifying,
    );
    state.jobs.engine_verify = Some(crate::frontend::jobs::spawn_engine_verify(
        target, launches, engine,
    ));
}

/// Write this machine's engine draft into `engine_overrides`. An empty program
/// clears the launch, which is how the user asks for auto-detection back.
fn commit_local_engine_draft(state: &mut AppState, id: EngineId) {
    let draft = state
        .ui
        .settings
        .engine_drafts
        .entry(id.as_str().to_string())
        .or_default();
    match draft.to_launch() {
        Some(launch) => state.config.engine_overrides.insert(id, launch),
        None => state.config.engine_overrides.remove(id),
    }
    persist_engine_config(state, "engine launch updated");
    reprobe_engines(state);
}

/// Record what a verification learned. A success is persisted onto the launch it
/// proves; a failure stays in session state, beside the field that caused it.
pub(crate) fn apply_verify_outcome(
    state: &mut AppState,
    target: ComputeTarget,
    engine: EngineId,
    checked_launch: Option<crate::engines::registry::EngineLaunch>,
    outcome: crate::backend::engine_launch::VerifyOutcome,
) {
    use crate::backend::engine_launch::VerifyOutcome;

    let key = (target.clone(), engine.as_str());
    state.ui.settings.engine_probe.remove(&key);
    if current_engine_draft_launch(state, &target, engine) != checked_launch {
        state.set_message("Engine check ignored because the launch was edited".to_string());
        return;
    }
    let message = match outcome {
        VerifyOutcome::Verified { launch, version } => {
            let command = launch.display_command();
            set_engine_launch(state, &target, engine, Some((launch, version.clone())));
            format!("{} {version} verified at {command}", engine.as_str())
        }
        VerifyOutcome::Failed { launch, reason } => {
            state.ui.settings.engine_probe.insert(
                key,
                EngineProbeState::Failed {
                    launch: Some(launch),
                    reason,
                },
            );
            return;
        }
        VerifyOutcome::NotFound { reason } => {
            state.ui.settings.engine_probe.insert(
                key,
                EngineProbeState::Failed {
                    launch: None,
                    reason,
                },
            );
            return;
        }
    };
    state.set_message(message);
}

fn current_engine_draft_launch(
    state: &AppState,
    target: &ComputeTarget,
    engine: EngineId,
) -> Option<crate::engines::registry::EngineLaunch> {
    match target {
        ComputeTarget::Local => state
            .ui
            .settings
            .engine_drafts
            .get(engine.as_str())
            .and_then(EngineDraft::to_launch),
        ComputeTarget::Remote(id) => {
            if let Some(draft) = state.ui.settings.remote_host_drafts.get(id) {
                draft
                    .engines
                    .get(engine.as_str())
                    .and_then(EngineDraft::to_launch)
            } else {
                state
                    .config
                    .remote_hosts
                    .get(id)
                    .and_then(|host| host.engines.get(engine))
                    .cloned()
            }
        }
    }
}

/// Store (or clear) `engine`'s launch on `target`, refreshing the draft the panel
/// edits so the user sees what was written — a probe fills in the program it found.
fn set_engine_launch(
    state: &mut AppState,
    target: &ComputeTarget,
    engine: EngineId,
    resolved: Option<(crate::engines::registry::EngineLaunch, String)>,
) {
    let launches = match target {
        ComputeTarget::Local => Some(&mut state.config.engine_overrides),
        ComputeTarget::Remote(id) => state
            .config
            .remote_hosts
            .get_mut(id)
            .map(|host| &mut host.engines),
    };
    let Some(launches) = launches else {
        return;
    };
    match &resolved {
        Some((launch, version)) => launches.insert_verified(engine, launch.clone(), version),
        None => launches.remove(engine),
    }

    let draft = resolved
        .as_ref()
        .map(|(launch, _)| EngineDraft::from_launch(launch));
    match target {
        ComputeTarget::Local => match draft {
            Some(draft) => {
                state
                    .ui
                    .settings
                    .engine_drafts
                    .insert(engine.as_str().to_string(), draft);
            }
            None => {
                state.ui.settings.engine_drafts.remove(engine.as_str());
            }
        },
        ComputeTarget::Remote(id) => {
            if let Some(host_draft) = state.ui.settings.remote_host_drafts.get_mut(id) {
                match draft {
                    Some(draft) => {
                        host_draft
                            .engines
                            .insert(engine.as_str().to_string(), draft);
                    }
                    None => {
                        host_draft.engines.remove(engine.as_str());
                    }
                }
            }
        }
    }
    persist_engine_config(state, "engine launch updated");
    reprobe_engines(state);
}

/// Forget `engine`'s configured launch on `target`, returning it to auto-detection.
pub(crate) fn clear_engine_launch(state: &mut AppState, target: ComputeTarget, engine: EngineId) {
    state
        .ui
        .settings
        .engine_probe
        .remove(&(target.clone(), engine.as_str()));
    set_engine_launch(state, &target, engine, None);
    state.set_message("Engine launch cleared; using auto-detection".to_string());
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
