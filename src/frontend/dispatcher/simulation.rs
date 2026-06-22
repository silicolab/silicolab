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
    // A remote target relays the whole pipeline to a deployed worker, which
    // resolves `gmx` and runs it on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_md_remote_host(state, &prompt.target) {
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
            },
        );
        state.optimization_origin = None;
        state.ui.pending_md_run = None;
        relay_gromacs_job(state, host, prompt.engine.label(), job);
        return;
    }
    let compute = match resolve_md_compute(state, prompt.engine) {
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

/// The remote host an MD job targets, or `None` for local execution. A `Remote`
/// target whose host id is no longer configured resolves leniently to local
/// (mirroring the registry's fallback on a miss), so a deleted host never dangles.
/// Pure and local — the network work happens later on the submit worker thread.
pub(crate) fn resolve_md_remote_host(
    state: &AppState,
    target: &crate::backend::config::ComputeTarget,
) -> Option<crate::backend::config::RemoteHost> {
    use crate::backend::config::ComputeTarget;
    match target {
        ComputeTarget::Local => None,
        ComputeTarget::Remote(host_id) => state.config.remote_hosts.get(host_id).cloned(),
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

// --- Remote Hosts (Settings → Engines → Remote Hosts) ----------------------

/// Build a validated [`RemoteHost`] from a settings draft. `prior_versions` and
/// `prior_resources` carry forward any cached `--version` strings and the per-host
/// resource defaults on an edit, neither of which the settings draft exposes.
fn host_from_draft(
    id: String,
    draft: &crate::frontend::state::RemoteHostDraft,
    prior_versions: std::collections::HashMap<String, String>,
    prior_resources: crate::backend::config::ResourceSpec,
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
        resources: prior_resources,
    })
}

pub(crate) fn add_remote_host(state: &mut AppState) {
    let draft = state.ui.settings.new_remote_host.clone();
    let id = uuid::Uuid::new_v4().simple().to_string();
    match host_from_draft(
        id.clone(),
        &draft,
        std::collections::HashMap::new(),
        Default::default(),
    ) {
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
    let prior = state.config.remote_hosts.get(&id);
    let prior_versions = prior
        .map(|host| host.engine_versions.clone())
        .unwrap_or_default();
    let prior_resources = prior.map(|host| host.resources.clone()).unwrap_or_default();
    match host_from_draft(id.clone(), &draft, prior_versions, prior_resources) {
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
    // Don't keep sampling a host that no longer exists: if the monitor was pointed
    // at it, stop the sampler and fall back to Local.
    let monitoring_removed = state
        .jobs
        .remote_gpu_monitor
        .as_ref()
        .is_some_and(|m| m.host_id == id)
        || state.ui.layout.monitor_source == crate::frontend::state::MonitorSource::Remote(id);
    if monitoring_removed {
        set_monitor_source(state, crate::frontend::state::MonitorSource::Local);
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

/// Fetch the static hardware inventory of a remote host over SSH (CPU/memory/GPU)
/// on a worker thread, for the Hardware ▸ Remote settings panel.
pub(crate) fn fetch_remote_hardware(state: &mut AppState, id: String) {
    if state.jobs.remote_hardware.is_some() {
        state.set_message("Already fetching remote hardware…".to_string());
        return;
    }
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.set_message(error.to_string());
        return;
    }
    let Some(host) = state.config.remote_hosts.get(&id).cloned() else {
        return;
    };
    state.ui.settings.remote_hardware_host = Some(id);
    state.jobs.remote_hardware = Some(crate::frontend::jobs::spawn_remote_hardware_fetch(host));
    state.set_message("Fetching remote hardware…".to_string());
}

/// Point the sidebar system monitor at `src` (Local or a remote host), reconciling
/// the live remote-GPU SSH sampler so exactly the selected host is being polled. At
/// most one sampler runs at a time; re-selecting the host already running is a no-op
/// (it keeps the sparkline history rather than restarting from empty).
pub(crate) fn set_monitor_source(state: &mut AppState, src: crate::frontend::state::MonitorSource) {
    use crate::frontend::state::MonitorSource;

    let desired_host = match &src {
        MonitorSource::Local => None,
        MonitorSource::Remote(id) => Some(id.clone()),
    };
    let running_host = state
        .jobs
        .remote_gpu_monitor
        .as_ref()
        .map(|m| m.host_id.clone());

    if running_host == desired_host {
        state.ui.layout.monitor_source = src;
        return;
    }

    if let Some(monitor) = state.jobs.remote_gpu_monitor.take() {
        monitor.cancel();
    }
    state.ui.settings.remote_gpu_live = None;
    state.ui.layout.monitor_source = src;

    let Some(id) = desired_host else {
        return;
    };
    // ssh missing: keep the source on this host and surface the error in the dock
    // (the panel renders `last_error`), rather than silently snapping back to Local.
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.ui.settings.remote_gpu_live = Some(crate::frontend::state::RemoteGpuLive {
            host_id: id,
            gpus: Vec::new(),
            last_error: Some(error.to_string()),
        });
        return;
    }
    let Some(host) = state.config.remote_hosts.get(&id).cloned() else {
        return; // host vanished from config between selection and dispatch.
    };
    state.ui.settings.remote_gpu_live = Some(crate::frontend::state::RemoteGpuLive {
        host_id: id,
        gpus: Vec::new(),
        last_error: None,
    });
    state.jobs.remote_gpu_monitor = Some(crate::frontend::jobs::spawn_remote_gpu_monitor(
        host,
        std::time::Duration::from_secs(15),
    ));
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
    // The box is the user-edited crystal cell, preserving its (e.g. hexagonal)
    // shape. Falling back to the structure's own cell keeps non-GUI callers
    // working. When an explicit cell is supplied, build_material_system uses it
    // verbatim instead of opening the out-of-plane axis itself.
    let cell_override = prompt.framework_cell.map(|[a, b, c, alpha, beta, gamma]| {
        crate::domain::UnitCell::from_parameters(a, b, c, alpha, beta, gamma)
    });

    // A remote target relays the build to a deployed worker, which resolves `gmx`
    // on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_md_remote_host(state, &prompt.target) {
        state.ui.pending_optimization = None;
        let job = crate::workflows::gromacs::GromacsJob::BuildMaterial(
            crate::workflows::gromacs::GromacsMaterialRequest {
                structure: state.structure().clone(),
                mode: prompt.framework_mode,
                solvation: prompt.solvation_options(),
                custom_force_field: prompt.custom_force_field_text.clone(),
                cell_override,
                solvent_gap_angstrom: FRAMEWORK_C_FLOOR_ANGSTROM,
                cutoff_nm: crate::workflows::molecular_dynamics::DEFAULT_CUTOFF_NM,
                max_duration: Duration::from_secs(60 * 60),
            },
        );
        relay_gromacs_job(state, host, "GROMACS", job);
        return true;
    }

    let compute = match resolve_md_compute(state, crate::frontend::state::MdEngineChoice::Gromacs) {
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

    // A remote target relays the build to a deployed worker, which resolves `gmx`
    // on the node; the local arm runs `gmx` here.
    if let Some(host) = resolve_md_remote_host(state, &prompt.target) {
        state.ui.pending_optimization = None;
        let job = crate::workflows::gromacs::GromacsJob::Build(
            crate::workflows::gromacs::GromacsBuildRequest {
                structure: state.structure().clone(),
                force_field: prompt.force_field.clone(),
                water: prompt.water,
                box_config: prompt.config(),
                solvate: prompt.solvate,
                ions,
                max_duration: Duration::from_secs(60 * 60),
            },
        );
        relay_gromacs_job(state, host, "GROMACS", job);
        return true;
    }

    // GROMACS is required for this build; we never silently fall back to a
    // topology-less geometry build.
    let compute = match resolve_md_compute(state, crate::frontend::state::MdEngineChoice::Gromacs) {
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

#[cfg(test)]
mod monitor_source_tests {
    use super::*;
    use crate::frontend::jobs::RunningRemoteGpuMonitor;
    use crate::frontend::state::{MonitorSource, RemoteGpuLive};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A running-monitor handle bound to `host_id`. We never read the receiver in
    /// these tests, so a dropped sender is harmless; the returned `cancel` flag lets
    /// the caller assert whether the sampler was told to stop.
    fn running_monitor(host_id: &str) -> (RunningRemoteGpuMonitor, Arc<AtomicBool>) {
        let (_tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            RunningRemoteGpuMonitor {
                host_id: host_id.into(),
                receiver: rx,
                cancel: cancel.clone(),
            },
            cancel,
        )
    }

    fn live(host_id: &str) -> RemoteGpuLive {
        RemoteGpuLive {
            host_id: host_id.into(),
            gpus: Vec::new(),
            last_error: None,
        }
    }

    #[test]
    fn switching_to_local_stops_the_running_monitor() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let (monitor, cancel) = running_monitor("a");
        state.jobs.remote_gpu_monitor = Some(monitor);
        state.ui.settings.remote_gpu_live = Some(live("a"));
        state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

        set_monitor_source(&mut state, MonitorSource::Local);

        assert!(
            cancel.load(Ordering::Relaxed),
            "sampler should be cancelled"
        );
        assert!(state.jobs.remote_gpu_monitor.is_none());
        assert!(state.ui.settings.remote_gpu_live.is_none());
        assert_eq!(state.ui.layout.monitor_source, MonitorSource::Local);
    }

    #[test]
    fn reselecting_the_same_host_is_idempotent() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let (monitor, cancel) = running_monitor("a");
        state.jobs.remote_gpu_monitor = Some(monitor);
        state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

        set_monitor_source(&mut state, MonitorSource::Remote("a".into()));

        assert!(
            !cancel.load(Ordering::Relaxed),
            "an already-running host must not be restarted"
        );
        assert_eq!(
            state
                .jobs
                .remote_gpu_monitor
                .as_ref()
                .map(|m| m.host_id.as_str()),
            Some("a")
        );
        assert_eq!(
            state.ui.layout.monitor_source,
            MonitorSource::Remote("a".into())
        );
    }

    #[test]
    fn switching_to_another_host_stops_the_previous_one() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let (monitor, cancel) = running_monitor("a");
        state.jobs.remote_gpu_monitor = Some(monitor);
        state.ui.settings.remote_gpu_live = Some(live("a"));
        state.ui.layout.monitor_source = MonitorSource::Remote("a".into());

        // Host "b" isn't in config, so no new sampler spawns regardless of whether
        // ssh is available in the test environment — but the previous "a" sampler
        // must always be stopped and the source must move to "b".
        set_monitor_source(&mut state, MonitorSource::Remote("b".into()));

        assert!(cancel.load(Ordering::Relaxed), "previous sampler cancelled");
        assert_ne!(
            state
                .jobs
                .remote_gpu_monitor
                .as_ref()
                .map(|m| m.host_id.clone()),
            Some("a".to_string()),
            "the old host's sampler handle must be gone"
        );
        assert_eq!(
            state.ui.layout.monitor_source,
            MonitorSource::Remote("b".into())
        );
    }
}
