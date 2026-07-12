use super::super::*;

use crate::frontend::state::SystemSubsystem;

/// Build a validated [`RemoteHost`] from a settings draft. `prior` is the host as
/// stored, carrying the state the draft does not expose: the worker deployment
/// identity, and each engine's verification.
///
/// A verification is carried across only when the draft still spells the *same*
/// launch. It is not a special case — `EngineLaunches::insert` discards the proof,
/// and this restores it exactly where it still applies.
fn host_from_draft(
    id: String,
    draft: &crate::frontend::state::RemoteHostDraft,
    prior: Option<&crate::backend::config::RemoteHost>,
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
    let mut engines = crate::engines::registry::EngineLaunches::new();
    for engine in crate::engines::registry::external_engine_ids() {
        let Some(launch) = draft
            .engines
            .get(engine.as_str())
            .and_then(crate::frontend::state::EngineDraft::to_launch)
        else {
            continue;
        };
        match prior
            .and_then(|host| host.engines.entry(engine))
            .filter(|entry| entry.launch == launch)
            .and_then(|entry| entry.verified.as_ref())
        {
            Some(verified) => engines.insert_verification(engine, launch, verified.clone()),
            None => engines.insert(engine, launch),
        }
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
        worker_deployment: prior.and_then(|host| host.worker_deployment.clone()),
        resources,
        scheduler,
    })
}

pub(crate) fn add_remote_host(state: &mut AppState) {
    let draft = state.ui.settings.new_remote_host.clone();
    let id = uuid::Uuid::new_v4().simple().to_string();
    match host_from_draft(id.clone(), &draft, None) {
        Ok(host) => {
            let label = host.label.clone();
            state.config.remote_hosts.insert(id, host);
            state.ui.settings.new_remote_host = Default::default();
            state.ui.settings.adding_host = false;
            persist_engine_config(state, &format!("Added remote host {label}"));
        }
        // The form stays open with the draft intact, beside the field to fix.
        Err(error) => state.status_neutral(error.to_string()),
    }
}

/// Abandon the add-host form and its draft.
pub(crate) fn cancel_add_remote_host(state: &mut AppState) {
    state.ui.settings.new_remote_host = Default::default();
    state.ui.settings.adding_host = false;
}

/// Validate the host's draft, store it, and hand back the committed host.
///
/// Every action that reaches out to a host goes through here first, so the host it
/// contacts is the one on screen. Reading `config` directly would test the values
/// as they were before the user's edits — a hostname typed but not saved would be
/// silently ignored by the very button meant to check it.
pub(crate) fn commit_remote_host_draft(
    state: &mut AppState,
    id: &str,
) -> anyhow::Result<crate::backend::config::RemoteHost> {
    let Some(draft) = state.ui.settings.remote_host_drafts.get(id).cloned() else {
        // No draft open: the stored host is already what the user sees.
        return state
            .config
            .remote_hosts
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("This host no longer exists"));
    };
    let host = host_from_draft(id.to_string(), &draft, state.config.remote_hosts.get(id))?;
    state
        .config
        .remote_hosts
        .insert(id.to_string(), host.clone());
    if let Err(error) = save_config(&state.config) {
        state.report_system_error(
            SystemSubsystem::Settings,
            format!("failed to save engine settings: {error}"),
        );
    }
    Ok(host)
}

pub(crate) fn save_remote_host(state: &mut AppState, id: String) {
    match commit_remote_host_draft(state, &id) {
        Ok(host) => state.status_success(format!("Saved remote host {}", host.label)),
        Err(error) => state.status_neutral(error.to_string()),
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
        state.status_neutral("This host has active remote jobs and cannot be removed");
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

/// Shared guard: ssh must exist, only one probe runs at a time, and the draft the
/// user is looking at is committed before we contact the host.
fn begin_remote_probe(
    state: &mut AppState,
    id: &str,
) -> Option<crate::backend::config::RemoteHost> {
    if state.jobs.remote_probe.is_some() {
        state.status_neutral("A remote-host check is already running");
        return None;
    }
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.status_neutral(error.to_string());
        return None;
    }
    match commit_remote_host_draft(state, id) {
        Ok(host) => Some(host),
        Err(error) => {
            state.status_neutral(error.to_string());
            None
        }
    }
}

pub(crate) fn detect_remote_slurm(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::DetectSlurm,
    ));
    state.status_neutral("Detecting Slurm…");
}

pub(crate) fn refresh_slurm_capabilities(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::SlurmCapabilities,
    ));
    state.status_neutral("Refreshing Slurm capabilities…");
}

pub(crate) fn test_remote_slurm(state: &mut AppState, id: String) {
    let Some(host) = begin_remote_probe(state, &id) else {
        return;
    };
    state.jobs.remote_probe = Some(crate::frontend::jobs::spawn_remote_probe(
        host,
        crate::frontend::jobs::RemoteProbeKind::TestSlurm,
    ));
    state.status_neutral("Submitting a Slurm test job…");
}

/// Fetch the static hardware inventory of a remote host over SSH (CPU/memory/GPU)
/// on a worker thread, for the Hardware ▸ Remote settings panel.
pub(crate) fn fetch_remote_hardware(state: &mut AppState, id: String) {
    if state.jobs.remote_hardware.is_some() {
        state.status_neutral("Already fetching remote hardware…");
        return;
    }
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.status_neutral(error.to_string());
        return;
    }
    let host = match commit_remote_host_draft(state, &id) {
        Ok(host) => host,
        Err(error) => {
            state.status_neutral(error.to_string());
            return;
        }
    };
    state.jobs.remote_hardware = Some(crate::frontend::jobs::spawn_remote_hardware_fetch(host));
    state.status_neutral("Fetching remote hardware…");
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
        state.status_error(format!("Could not prepare the SSH key: {error}"));
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
    state.status_neutral("Testing connection to the remote host…");
}

pub(crate) fn setup_remote_host_key(state: &mut AppState, id: String) {
    if let Err(error) = crate::engines::remote::ensure_ssh_available() {
        state.status_neutral(error.to_string());
        return;
    }
    if let Err(error) = crate::engines::remote::bootstrap::ensure_key() {
        state.status_error(format!("Could not prepare the SSH key: {error}"));
        return;
    }
    let pubkey = match crate::engines::remote::bootstrap::public_key() {
        Ok(key) => key,
        Err(error) => {
            state.status_error(format!("Could not read the public key: {error}"));
            return;
        }
    };
    let command = crate::engines::remote::bootstrap::install_command(&pubkey);
    state.ui.settings.remote_bootstrap = Some((id, command));
    state.status_neutral("Run the shown command once on the remote host, then click Verify.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::registry::EngineLaunch;
    use crate::frontend::state::{EngineDraft, RemoteHostDraft};
    use crate::launch::Verification;

    fn draft_with_gmx(program: &str) -> RemoteHostDraft {
        let mut draft = RemoteHostDraft {
            hostname: "login.example.edu".to_string(),
            username: "alice".to_string(),
            port: "22".to_string(),
            ..Default::default()
        };
        if !program.is_empty() {
            draft.engines.insert(
                EngineId::GROMACS.as_str().to_string(),
                EngineDraft {
                    command_prefix: String::new(),
                    program: program.to_string(),
                },
            );
        }
        draft
    }

    fn prior_host(program: &str) -> crate::backend::config::RemoteHost {
        let mut engines = crate::engines::registry::EngineLaunches::new();
        engines.insert_verification(
            EngineId::GROMACS,
            EngineLaunch::native(program),
            Verification {
                version: "2026.2".to_string(),
                checked_at: 123,
            },
        );
        crate::backend::config::RemoteHost {
            id: "h".to_string(),
            engines,
            worker_deployment: Some("dev:abc".to_string()),
            ..Default::default()
        }
    }

    /// A verification belongs to the launch it was taken against. Editing the path
    /// must not leave the old version beside the new binary — the job now runs
    /// whatever is configured here, so that pairing would be a lie.
    #[test]
    fn editing_the_gmx_path_drops_its_stale_version() {
        let prior = prior_host("/usr/local/gromacs/bin/gmx");
        let host = host_from_draft(
            "h".to_string(),
            &draft_with_gmx("/opt/gromacs-2022.5/bin/gmx"),
            Some(&prior),
        )
        .expect("draft is valid");

        let entry = host.engines.entry(EngineId::GROMACS).expect("entry");
        assert_eq!(entry.launch.program, "/opt/gromacs-2022.5/bin/gmx");
        assert!(
            entry.verified.is_none(),
            "the 2026.2 proof must not survive onto the 2022.5 path"
        );
        // The worker deployment identity is not an engine verification; it stays.
        assert_eq!(host.worker_deployment.as_deref(), Some("dev:abc"));
    }

    #[test]
    fn saving_an_unchanged_gmx_path_keeps_its_version() {
        let prior = prior_host("/usr/local/gromacs/bin/gmx");
        let host = host_from_draft(
            "h".to_string(),
            &draft_with_gmx("/usr/local/gromacs/bin/gmx"),
            Some(&prior),
        )
        .expect("draft is valid");

        let verified = host
            .engines
            .entry(EngineId::GROMACS)
            .and_then(|entry| entry.verified.as_ref())
            .expect("an unchanged launch keeps its proof");
        assert_eq!(verified.version, "2026.2");
        assert_eq!(verified.checked_at, 123);
    }

    /// Clearing the field un-configures the engine, so its verification goes too.
    #[test]
    fn clearing_the_gmx_path_drops_the_engine_entirely() {
        let prior = prior_host("/usr/local/gromacs/bin/gmx");
        let host =
            host_from_draft("h".to_string(), &draft_with_gmx(""), Some(&prior)).expect("valid");
        assert!(!host.engines.contains(EngineId::GROMACS));
    }

    /// A remote engine may sit behind a launcher, exactly as a local one may.
    #[test]
    fn a_remote_engine_can_carry_a_command_prefix() {
        let mut draft = draft_with_gmx("gmx");
        draft
            .engines
            .get_mut(EngineId::GROMACS.as_str())
            .expect("gromacs draft")
            .command_prefix = "apptainer exec gromacs.sif".to_string();

        let host = host_from_draft("h".to_string(), &draft, None).expect("draft is valid");
        let launch = host.engines.get(EngineId::GROMACS).expect("launch");
        assert_eq!(launch.command_prefix, ["apptainer", "exec", "gromacs.sif"]);
        assert_eq!(launch.program, "gmx");
    }
}
