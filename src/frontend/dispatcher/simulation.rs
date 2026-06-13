use super::*;

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
    let compute = match resolve_md_compute(state, prompt.engine, &prompt.target) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return;
        }
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

/// Resolve a [`ComputeTarget`] into a [`Compute`] (launch + transport) for a
/// GROMACS job. This is the single place a target becomes runnable.
///
/// - `Local` → today's launch resolution (override / PATH / WSL auto-detect),
///   wrapped as a local transport.
/// - `Remote(host)` → the host's per-engine launch (remote `gmx` path) bound to a
///   [`RemoteTarget`] anchored at the active task's durable run UUID. A configured
///   host that lacks GROMACS yields a guidance error that blocks the run; a
///   dangling host id (deleted/renamed) resolves leniently back to `Local`.
///
/// Only **local** checks run here (this is the UI thread): the OS `ssh` presence
/// check is a cheap PATH lookup. All network work — reachability, the passwordless
/// check, the remote `--version` probe — happens later on the worker thread, so a
/// slow or dead host never freezes the UI.
pub(crate) fn resolve_md_compute(
    state: &mut AppState,
    engine: crate::frontend::state::MdEngineChoice,
    target: &crate::backend::config::ComputeTarget,
) -> anyhow::Result<crate::engines::remote::Compute> {
    use crate::backend::config::ComputeTarget;
    use crate::engines::remote::{Compute, RemoteTarget};

    match target {
        ComputeTarget::Local => Ok(Compute::local(resolve_md_engine_launch(state, engine)?)),
        ComputeTarget::Remote(host_id) => {
            let Some(host) = state.config.remote_hosts.get(host_id).cloned() else {
                // Lenient fallback: a deleted/renamed host routes to Local rather
                // than dangling (mirrors the registry's PATH fallback on a miss).
                return Ok(Compute::local(resolve_md_engine_launch(state, engine)?));
            };
            crate::engines::remote::ensure_ssh_available()?;
            let launch = host
                .engines
                .get(EngineId::GROMACS.as_str())
                .filter(|launch| !launch.is_empty())
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "Configure GROMACS for host {} in Settings → Remote Hosts before running there.",
                        host.label
                    )
                })?;
            let run_uuid = state
                .active_task_run
                .and_then(|id| state.tasks.task_run(id))
                .map(|task| task.run_uuid.clone())
                .ok_or_else(|| anyhow!("no active task run to anchor the remote run directory"))?;
            Ok(Compute::remote(
                launch,
                RemoteTarget::for_run(&host, &run_uuid),
            ))
        }
    }
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

// --- Remote Hosts (Settings → Engines → Remote Hosts) ----------------------

/// Build a validated [`RemoteHost`] from a settings draft. `prior_versions`
/// carries forward any cached `--version` strings on an edit.
fn host_from_draft(
    id: String,
    draft: &crate::frontend::state::RemoteHostDraft,
    prior_versions: std::collections::HashMap<String, String>,
) -> anyhow::Result<crate::backend::config::RemoteHost> {
    let hostname = draft.hostname.trim();
    let username = draft.username.trim();
    if hostname.is_empty() {
        anyhow::bail!("Hostname is required");
    }
    if username.is_empty() {
        anyhow::bail!("Username is required");
    }
    let port: u16 = if draft.port.trim().is_empty() {
        22
    } else {
        draft
            .port
            .trim()
            .parse()
            .map_err(|_| anyhow!("Port must be a number between 1 and 65535"))?
    };
    let work_root = if draft.work_root.trim().is_empty() {
        "~/.silicolab".to_string()
    } else {
        draft.work_root.trim().to_string()
    };
    crate::engines::remote::validate_work_root(&work_root)?;
    let prelude: Vec<String> = draft
        .prelude
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let mut engines = std::collections::HashMap::new();
    let gmx = draft.gmx_program.trim();
    if !gmx.is_empty() {
        engines.insert(
            EngineId::GROMACS.as_str().to_string(),
            crate::engines::registry::EngineLaunch::native(gmx),
        );
    }
    let label = {
        let label = draft.label.trim();
        if label.is_empty() {
            hostname.to_string()
        } else {
            label.to_string()
        }
    };
    Ok(crate::backend::config::RemoteHost {
        id,
        label,
        hostname: hostname.to_string(),
        username: username.to_string(),
        port,
        work_root,
        prelude,
        engines,
        engine_versions: prior_versions,
    })
}

pub(crate) fn add_remote_host(state: &mut AppState) {
    let draft = state.ui.settings.new_remote_host.clone();
    let id = uuid::Uuid::new_v4().simple().to_string();
    match host_from_draft(id.clone(), &draft, std::collections::HashMap::new()) {
        Ok(host) => {
            let label = host.label.clone();
            state.config.remote_hosts.insert(id, host);
            state.ui.settings.new_remote_host = Default::default();
            persist_engine_config(state, &format!("Added remote host {label}"));
        }
        Err(error) => state.set_message(error.to_string()),
    }
}

pub(crate) fn save_remote_host(state: &mut AppState, id: String) {
    let Some(draft) = state.ui.settings.remote_host_drafts.get(&id).cloned() else {
        return;
    };
    let prior_versions = state
        .config
        .remote_hosts
        .get(&id)
        .map(|host| host.engine_versions.clone())
        .unwrap_or_default();
    match host_from_draft(id.clone(), &draft, prior_versions) {
        Ok(host) => {
            let label = host.label.clone();
            state.config.remote_hosts.insert(id, host);
            persist_engine_config(state, &format!("Saved remote host {label}"));
        }
        Err(error) => state.set_message(error.to_string()),
    }
}

pub(crate) fn remove_remote_host(state: &mut AppState, id: String) {
    state.config.remote_hosts.remove(&id);
    state.ui.settings.remote_host_drafts.remove(&id);
    state.ui.settings.remote_status.remove(&id);
    if matches!(&state.ui.settings.remote_bootstrap, Some((bid, _)) if *bid == id) {
        state.ui.settings.remote_bootstrap = None;
    }
    persist_engine_config(state, "Removed remote host");
}

/// Shared guard: ssh must exist and only one probe runs at a time. Returns the
/// host clone on success.
fn begin_remote_probe(
    state: &mut AppState,
    id: &str,
) -> Option<crate::backend::config::RemoteHost> {
    if state.jobs.remote_probe.is_some() {
        state.set_message("A remote-host check is already running".to_string());
        return None;
    }
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.set_message(error.to_string());
        return None;
    }
    state.config.remote_hosts.get(id).cloned()
}

pub(crate) fn detect_remote_gromacs(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::DetectGromacs,
    ));
    state.set_message("Detecting GROMACS on the remote host…".to_string());
}

pub(crate) fn check_remote_host(state: &mut AppState, id: String) {
    // The BatchMode test uses the dedicated key, so make sure it exists first.
    if let Err(error) = crate::engines::remote::bootstrap::ensure_key() {
        state.set_message(format!("Could not prepare the SSH key: {error}"));
        return;
    }
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state
        .ui
        .settings
        .remote_status
        .insert(id, crate::frontend::state::RemoteHostStatus::Checking);
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::Passwordless,
    ));
    state.set_message("Testing connection to the remote host…".to_string());
}

pub(crate) fn setup_remote_host_key(state: &mut AppState, id: String) {
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.set_message(error.to_string());
        return;
    }
    if let Err(error) = crate::engines::remote::bootstrap::ensure_key() {
        state.set_message(format!("Could not prepare the SSH key: {error}"));
        return;
    }
    let pubkey = match crate::engines::remote::bootstrap::public_key() {
        Ok(key) => key,
        Err(error) => {
            state.set_message(format!("Could not read the public key: {error}"));
            return;
        }
    };
    let command = crate::engines::remote::bootstrap::install_command(&pubkey);
    state.ui.settings.remote_bootstrap = Some((id, command));
    state.set_message(
        "Run the shown command once on the remote host, then click Verify.".to_string(),
    );
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

/// Build the MD system with the engine the panel selected. Returns `true` once
/// the work is launched/finished and the panel may close; `false` leaves the
/// panel open with a reported reason so the user can adjust inputs and retry.
///
/// GROMACS is the default: it runs the full pdb2gmx pipeline on a worker thread
/// and produces a force-field topology a run can reuse. The built-in path is a
/// geometry-only fallback (box + solvation coordinates, no topology) that the
/// user can opt into explicitly.
pub(crate) fn build_md_system(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    use crate::frontend::state::MdBuildEngine;
    use crate::workflows::molecular_dynamics::is_framework_shape;
    match prompt.engine {
        MdBuildEngine::Gromacs => {
            // A covalent framework (celled, bonded, non-biopolymer) has no residue
            // template for pdb2gmx; generate its topology directly from the bonds
            // instead. The material build validates parameters and reports any
            // element the built-in tables and the custom force field don't cover.
            if is_framework_shape(state.structure()) {
                start_material_md_build(state, prompt)
            } else {
                start_gromacs_md_build(state, prompt)
            }
        }
        MdBuildEngine::BuiltIn => build_md_system_builtin(state, prompt),
    }
}

/// Out-of-plane cell length (A) the framework cell editor is seeded to when the
/// crystal's own gap is thinner. It clears a 1.0 nm cutoff plus the Verlet
/// buffer on both sides of the slab (`2·(1.0+0.1) nm + buffer`), so the default
/// box runs without the user having to widen `c` first.
pub(crate) const FRAMEWORK_C_FLOOR_ANGSTROM: f32 = 25.0;

/// Launch the framework (nanosheet) build: generate the topology from the
/// structure's bonds (rigid or flexible per the prompt), use the user-edited
/// crystal cell as the box, and optionally solvate. Writes `topol.top` and
/// `framework_run.json` into the build run directory for a later MD run.
pub(crate) fn start_material_md_build(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    use crate::engines::gromacs::MaterialBuildRequest;

    if state.jobs.engine_running() {
        state.set_message("another external engine job is already running".to_string());
        return false;
    }
    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    // Resolve the compute target after the run dir exists so the remote dir can be
    // anchored at the active task's run UUID.
    let compute = match resolve_md_compute(
        state,
        crate::frontend::state::MdEngineChoice::Gromacs,
        &prompt.target,
    ) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
        state
            .tasks
            .set_engine_label(task_run_id, Some("GROMACS".to_string()));
        sync_task_manifest(state, task_run_id);
    }

    // The box is the user-edited crystal cell, preserving its (e.g. hexagonal)
    // shape. Falling back to the structure's own cell keeps non-GUI callers
    // working. When an explicit cell is supplied, build_material_system uses it
    // verbatim instead of opening the out-of-plane axis itself.
    let cell_override = prompt.framework_cell.map(|[a, b, c, alpha, beta, gamma]| {
        crate::domain::UnitCell::from_parameters(a, b, c, alpha, beta, gamma)
    });

    state.ui.pending_optimization = None;
    let job = crate::frontend::jobs::spawn_material_build_job(MaterialBuildRequest {
        structure: state.structure().clone(),
        mode: prompt.framework_mode,
        working_dir: run_dir,
        compute,
        solvation: prompt.solvation_options(),
        cell_override,
        custom_force_field: prompt.custom_force_field_text.clone(),
        solvent_gap_angstrom: FRAMEWORK_C_FLOOR_ANGSTROM,
        cutoff_nm: crate::workflows::molecular_dynamics::DEFAULT_CUTOFF_NM,
        max_duration: Duration::from_secs(60 * 60),
    });
    state.jobs.set_engine(job);
    state.set_message("Building framework MD system; press Esc to stop".to_string());
    true
}

/// Launch the GROMACS pdb2gmx → editconf → solvate → genion pipeline as a
/// background engine job, writing its `topol.top` into the build run directory
/// so a later MD run can reuse it. On a setup error (engine missing, run
/// directory) it reports the reason and keeps the panel open.
pub(crate) fn start_gromacs_md_build(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    if state.jobs.engine_running() {
        state.set_message("another external engine job is already running".to_string());
        return false;
    }
    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    // GROMACS is required for this build; we never silently fall back to a
    // topology-less geometry build. Resolve after the run dir exists so a remote
    // run dir can be anchored at the active task's run UUID.
    let compute = match resolve_md_compute(
        state,
        crate::frontend::state::MdEngineChoice::Gromacs,
        &prompt.target,
    ) {
        Ok(compute) => compute,
        Err(error) => {
            state.set_message(error.to_string());
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
        state
            .tasks
            .set_engine_label(task_run_id, Some("GROMACS".to_string()));
        sync_task_manifest(state, task_run_id);
    }

    // Only attach an ion step when solvation is on and the user asked for ions;
    // genion needs the solvent it replaces.
    let ions = if prompt.solvate && (prompt.neutralize || prompt.add_salt) {
        Some(IonOptions {
            neutralize: prompt.neutralize,
            concentration_molar: prompt.add_salt.then_some(prompt.salt_concentration_molar),
            positive_ion: prompt.positive_ion.clone(),
            negative_ion: prompt.negative_ion.clone(),
        })
    } else {
        None
    };

    state.ui.pending_optimization = None;
    let job = spawn_gromacs_build_job(BuildRequest {
        structure: state.structure().clone(),
        working_dir: run_dir,
        compute,
        force_field: prompt.force_field.clone(),
        water: prompt.water,
        box_config: prompt.config(),
        solvate: prompt.solvate,
        ions,
        max_duration: Duration::from_secs(60 * 60),
    });
    state.jobs.set_engine(job);
    state.set_message("GROMACS building MD system; press Esc to stop".to_string());
    true
}

/// Returns `true` on success; on failure it reports the reason and leaves the
/// panel open so the user can adjust inputs and retry.
pub(crate) fn build_md_system_builtin(
    state: &mut AppState,
    prompt: &crate::frontend::state::MdSystemPrompt,
) -> bool {
    // The pre-box solute carries any residue metadata system-type detection reads.
    let solute = state.structure().clone();
    let config = prompt.config();
    let result = crate::workflows::molecular_dynamics::build_md_system(state.structure(), &config);
    let (boxed, report) = match result {
        Ok(value) => value,
        Err(error) => {
            state.set_message(format!("MD system build failed: {error}"));
            return false;
        }
    };

    // Optionally fill the freshly built box with water and ions — geometry only,
    // no force field (an engine parameterizes the system later). On failure keep
    // the panel open so the user can adjust the box or solvation settings.
    let solvated = match prompt.solvation_options() {
        Some(options) => match crate::workflows::molecular_dynamics::solvate(&boxed, &options) {
            Ok(out) => Some(out),
            Err(error) => {
                state.set_message(format!("MD system solvation failed: {error}"));
                return false;
            }
        },
        None => None,
    };

    let run_dir = match ensure_active_task_run_dir(
        state,
        TaskKind::BuildMdSystem,
        Some(prompt.run_name.as_str()),
    ) {
        Ok(path) => path,
        Err(error) => {
            state.set_message(format!("failed to create run directory: {error}"));
            complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Failed);
            return false;
        }
    };
    if let Some(task_run_id) = state.active_task_run {
        mark_task_status(state, task_run_id, TaskStatus::Running);
    }
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.ui.editor = None;
    state.ui.selection.clear();

    // The entry structure is the solvated system when solvation ran, else the
    // bare box.
    let (final_structure, solvation_note) = match solvated {
        Some((structure, report)) => (
            structure,
            format!(
                "; solvated +{} water, +{} {}, +{} {}",
                report.water_added,
                report.cations_added,
                prompt.positive_ion,
                report.anions_added,
                prompt.negative_ion,
            ),
        ),
        None => (boxed, String::new()),
    };
    let final_atom_count = final_structure.atoms.len();
    let save_path = structure_io::default_structure_save_path(&final_structure, None);
    let entry_id = add_and_show_entry(state, final_structure, None, save_path);
    if let Some(task_run_id) = state.active_task_run {
        record_task_result_entry(state, task_run_id, entry_id);
    }
    // A built-in build is geometry-only — no force field is applied — so the
    // context records the generic family (a later run uses the captured
    // engine-neutral topology / cutoff path, not a biomolecular nonbonded block).
    let water_token = prompt.solvate.then(|| prompt.water.db_token().to_string());
    write_md_system_context(
        &run_dir,
        &solute,
        final_atom_count,
        "builtin",
        water_token.as_deref(),
        false,
        0.0,
        false,
        Vec::new(),
    );

    let [a, b, c] = report.edges_angstrom;
    let replaced = if report.replaced_existing_cell {
        " (replaced existing cell)"
    } else {
        ""
    };
    state.set_message(format!(
        "Built MD system: {a:.1} x {b:.1} x {c:.1} A box, {} atoms{replaced}{solvation_note}",
        state.structure().atoms.len()
    ));
    complete_active_task(state, TaskKind::BuildMdSystem, TaskStatus::Completed);
    true
}
