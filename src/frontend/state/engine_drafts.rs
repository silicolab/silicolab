use std::collections::BTreeMap;

/// Editable draft for one engine's launch on one compute target. `command_prefix`
/// is held as a single whitespace-separated line for easy editing (e.g.
/// `wsl.exe -e`); it is split on apply.
#[derive(Debug, Clone, Default)]
pub struct EngineDraft {
    pub command_prefix: String,
    pub program: String,
}

impl EngineDraft {
    pub fn from_launch(launch: &crate::engines::registry::EngineLaunch) -> Self {
        Self {
            command_prefix: launch.command_prefix.join(" "),
            program: launch.program.clone(),
        }
    }

    /// Build an [`EngineLaunch`] from the draft, or `None` if no program is
    /// set (which the dispatcher treats as "clear this override").
    pub fn to_launch(&self) -> Option<crate::engines::registry::EngineLaunch> {
        let program = self.program.trim();
        if program.is_empty() {
            return None;
        }
        Some(crate::engines::registry::EngineLaunch {
            command_prefix: self
                .command_prefix
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            program: program.to_string(),
        })
    }
}

/// A verification that has not (yet) produced a proof. A success is never here —
/// it is persisted onto the launch it verified.
#[derive(Debug, Clone)]
pub enum EngineProbeState {
    Verifying,
    /// The verification did not produce a proof. `launch` is the one that was run,
    /// or `None` when nothing was configured and no candidate answered. Holding the
    /// launch lets the panel drop the message the moment the user edits it.
    Failed {
        launch: Option<crate::engines::registry::EngineLaunch>,
        reason: String,
    },
}

impl EngineProbeState {
    /// Whether this failure still describes `current`. An edited launch has not
    /// been tried, so its predecessor's failure must stop being shown.
    pub fn describes(&self, current: Option<&crate::engines::registry::EngineLaunch>) -> bool {
        match self {
            Self::Verifying => true,
            Self::Failed { launch, .. } => launch.as_ref() == current,
        }
    }
}

/// Editable draft for one remote host in the Settings panel. All fields are held
/// as text for direct editing and parsed/validated on save (`port` and `prelude`
/// in particular).
#[derive(Debug, Clone, Default)]
pub struct RemoteHostDraft {
    pub label: String,
    pub hostname: String,
    pub username: String,
    pub port: String,
    pub work_root: String,
    /// One shell setup line per text row (`module load gromacs`, `source GMXRC`).
    pub prelude: String,
    /// Per-engine launch drafts on this host, keyed by engine id — the same editor
    /// this machine's engines use, so a remote engine can carry a `command_prefix`
    /// (`apptainer exec image.sif`, `srun`) exactly as a local one can.
    pub engines: BTreeMap<String, EngineDraft>,
    pub slurm: bool,
    pub scheduler_prelude: String,
    pub partition: String,
    pub account: String,
    pub qos: String,
    pub reservation: String,
    pub constraint: String,
    pub default_cpus: String,
    pub default_memory_mib: String,
    pub default_walltime_minutes: String,
    pub default_gpu_kind: String,
    pub default_gpu_count: String,
    pub default_gpu_type: String,
    pub gpu_syntax: String,
    pub gres_name: String,
    pub custom_gpu_argument: String,
    pub extra_args: String,
}

impl RemoteHostDraft {
    pub fn from_host(host: &crate::backend::config::RemoteHost) -> Self {
        let engines = crate::engines::registry::external_engine_ids()
            .filter_map(|id| {
                let launch = host.engines.get(id)?;
                Some((id.as_str().to_string(), EngineDraft::from_launch(launch)))
            })
            .collect();
        Self {
            label: host.label.clone(),
            hostname: host.hostname.clone(),
            username: host.username.clone(),
            port: host.port.to_string(),
            work_root: host.work_root.clone(),
            prelude: host.prelude.join("\n"),
            engines,
            slurm: matches!(
                host.scheduler,
                crate::backend::config::SchedulerConfig::Slurm(_)
            ),
            scheduler_prelude: match &host.scheduler {
                crate::backend::config::SchedulerConfig::Slurm(profile) => {
                    profile.scheduler_prelude.join("\n")
                }
                _ => String::new(),
            },
            partition: slurm_value(host, |profile| profile.partition.as_deref()),
            account: slurm_value(host, |profile| profile.account.as_deref()),
            qos: slurm_value(host, |profile| profile.qos.as_deref()),
            reservation: slurm_value(host, |profile| profile.reservation.as_deref()),
            constraint: slurm_value(host, |profile| profile.constraint.as_deref()),
            default_cpus: host
                .resources
                .cpus_per_task
                .map(|value| value.to_string())
                .unwrap_or_default(),
            default_memory_mib: host
                .resources
                .memory_mib
                .map(|value| value.to_string())
                .unwrap_or_default(),
            default_walltime_minutes: host
                .resources
                .walltime_seconds
                .map(|value| (value / 60).to_string())
                .unwrap_or_default(),
            default_gpu_kind: match host.resources.gpu {
                crate::backend::config::GpuRequest::None => "none",
                crate::backend::config::GpuRequest::Any { .. } => "any",
                crate::backend::config::GpuRequest::Typed { .. } => "typed",
            }
            .to_string(),
            default_gpu_count: match &host.resources.gpu {
                crate::backend::config::GpuRequest::None => String::new(),
                crate::backend::config::GpuRequest::Any { count }
                | crate::backend::config::GpuRequest::Typed { count, .. } => count.to_string(),
            },
            default_gpu_type: match &host.resources.gpu {
                crate::backend::config::GpuRequest::Typed { gpu_type, .. } => gpu_type.clone(),
                _ => String::new(),
            },
            gpu_syntax: match &host.scheduler {
                crate::backend::config::SchedulerConfig::Slurm(profile) => match profile.gpu_syntax
                {
                    crate::backend::config::SlurmGpuSyntax::Gpus => "gpus",
                    crate::backend::config::SlurmGpuSyntax::CustomTemplate { .. } => "custom",
                    _ => "gres",
                }
                .to_string(),
                _ => "gres".to_string(),
            },
            gres_name: match &host.scheduler {
                crate::backend::config::SchedulerConfig::Slurm(profile) => {
                    match &profile.gpu_syntax {
                        crate::backend::config::SlurmGpuSyntax::Gres { resource_name } => {
                            resource_name.clone()
                        }
                        _ => "gpu".to_string(),
                    }
                }
                _ => "gpu".to_string(),
            },
            custom_gpu_argument: match &host.scheduler {
                crate::backend::config::SchedulerConfig::Slurm(profile) => {
                    match &profile.gpu_syntax {
                        crate::backend::config::SlurmGpuSyntax::CustomTemplate { argument } => {
                            argument.clone()
                        }
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            },
            extra_args: match &host.scheduler {
                crate::backend::config::SchedulerConfig::Slurm(profile) => {
                    profile.extra_args.join("\n")
                }
                _ => String::new(),
            },
        }
    }
}

fn slurm_value(
    host: &crate::backend::config::RemoteHost,
    get: impl FnOnce(&crate::backend::config::SlurmProfile) -> Option<&str>,
) -> String {
    match &host.scheduler {
        crate::backend::config::SchedulerConfig::Slurm(profile) => {
            get(profile).unwrap_or_default().to_string()
        }
        _ => String::new(),
    }
}

/// Connection status of a remote host, shown as an indicator in the panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RemoteHostStatus {
    /// Not yet probed.
    #[default]
    Unknown,
    /// A probe (passwordless check / detect) is in flight.
    Checking,
    /// Passwordless login works.
    Ready,
    /// Reachable, but passwordless login is not set up yet.
    NeedsSetup,
    /// The probe failed (unreachable / auth error). Carries a short reason.
    Unreachable(String),
}
