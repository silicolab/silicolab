use super::super::*;

/// Build a validated [`RemoteHost`] from a settings draft. `prior_versions` and
/// `prior_resources` carry forward any cached `--version` strings and the per-host
/// resource defaults on an edit, neither of which the settings draft exposes.
fn host_from_draft(
    id: String,
    draft: &crate::frontend::state::RemoteHostDraft,
    prior_versions: std::collections::HashMap<String, String>,
    _prior_resources: crate::backend::config::ResourceSpec,
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
    let parse_optional = |label: &str, value: &str| -> anyhow::Result<Option<u64>> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let parsed = value
            .parse::<u64>()
            .map_err(|_| anyhow!("{label} must be a positive number"))?;
        if parsed == 0 {
            anyhow::bail!("{label} must be greater than zero");
        }
        Ok(Some(parsed))
    };
    let default_gpu_count = || -> anyhow::Result<u32> {
        parse_optional("Default GPU count", &draft.default_gpu_count)?
            .unwrap_or(1)
            .try_into()
            .map_err(|_| anyhow!("Default GPU count is too large"))
    };
    let default_gpu = match draft.default_gpu_kind.as_str() {
        "any" => crate::backend::config::GpuRequest::Any {
            count: default_gpu_count()?,
        },
        "typed" => crate::backend::config::GpuRequest::Typed {
            gpu_type: draft.default_gpu_type.trim().to_string(),
            count: default_gpu_count()?,
        },
        _ => crate::backend::config::GpuRequest::None,
    };
    default_gpu.validate()?;
    let resources = crate::backend::config::JobResources {
        cpus_per_task: parse_optional("Default CPUs", &draft.default_cpus)?
            .map(|value| u32::try_from(value).map_err(|_| anyhow!("Default CPUs is too large")))
            .transpose()?,
        memory_mib: parse_optional("Default memory", &draft.default_memory_mib)?,
        walltime_seconds: parse_optional("Default walltime", &draft.default_walltime_minutes)?
            .map(|minutes| minutes.saturating_mul(60)),
        gpu: default_gpu,
        gpu_explicit: true,
    };
    let optional = |value: &str| (!value.trim().is_empty()).then(|| value.trim().to_string());
    let scheduler = if draft.slurm {
        let gpu_syntax = match draft.gpu_syntax.as_str() {
            "gpus" => crate::backend::config::SlurmGpuSyntax::Gpus,
            "custom" => crate::backend::config::SlurmGpuSyntax::CustomTemplate {
                argument: draft.custom_gpu_argument.trim().to_string(),
            },
            _ => crate::backend::config::SlurmGpuSyntax::Gres {
                resource_name: if draft.gres_name.trim().is_empty() {
                    "gpu".to_string()
                } else {
                    draft.gres_name.trim().to_string()
                },
            },
        };
        let profile = crate::backend::config::SlurmProfile {
            scheduler_prelude: draft
                .scheduler_prelude
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            partition: optional(&draft.partition),
            account: optional(&draft.account),
            qos: optional(&draft.qos),
            reservation: optional(&draft.reservation),
            constraint: optional(&draft.constraint),
            gpu_syntax,
            extra_args: draft
                .extra_args
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
        };
        profile.validate()?;
        crate::backend::config::SchedulerConfig::Slurm(profile)
    } else {
        crate::backend::config::SchedulerConfig::Direct
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
        resources,
        scheduler,
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
    let has_active_jobs = (|| -> anyhow::Result<bool> {
        let conn = crate::backend::storage::jobs::open()?;
        Ok(crate::backend::storage::jobs::list_non_terminal(&conn)?
            .iter()
            .any(|job| job.host_id == id))
    })()
    .unwrap_or(true);
    if has_active_jobs {
        state.set_message("This host has active remote jobs and cannot be removed".to_string());
        return;
    }
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

pub(crate) fn detect_remote_slurm(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::DetectSlurm,
    ));
    state.set_message("Detecting Slurm…".to_string());
}

pub(crate) fn refresh_slurm_capabilities(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::SlurmCapabilities,
    ));
    state.set_message("Refreshing Slurm capabilities…".to_string());
}

pub(crate) fn test_remote_slurm(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::TestSlurm,
    ));
    state.set_message("Submitting a Slurm test job…".to_string());
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
    if matches!(
        host.scheduler,
        crate::backend::config::SchedulerConfig::Slurm(_)
    ) {
        state.ui.settings.remote_gpu_live = Some(crate::frontend::state::RemoteGpuLive {
            host_id: id,
            gpus: Vec::new(),
            last_error: Some(
                "Slurm targets show allocation state in the task monitor; login-node utilization is not cluster utilization"
                    .to_string(),
            ),
        });
        return;
    }
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
