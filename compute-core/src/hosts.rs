//! Remote-host descriptor and the per-user config directory.
//!
//! `RemoteHost` describes an SSH-reachable machine that engine jobs can be
//! submitted to; it lives here, at the bottom of the compute crate, because the
//! remote engine transport depends on it. `config_dir` is the per-user SilicoLab
//! directory (`~/.silicolab`) that also holds the SSH key/known-hosts the remote
//! bootstrap writes, so the two are kept together.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::launch::EngineLaunches;

/// A remote host SilicoLab can submit external-engine jobs to over SSH. Stored in
/// the app config keyed by [`RemoteHost::id`]. Connection is key-based only — no
/// passwords are ever serialized here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteHost {
    /// Stable, opaque identifier (the app's compute-target selection references
    /// this). Never shown to the user; survives label/hostname edits.
    pub id: String,
    /// Human-facing name shown in the target picker and settings.
    pub label: String,
    /// Hostname or IP the OS `ssh` client connects to.
    pub hostname: String,
    pub username: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Remote root under which per-run scratch dirs (`<work_root>/runs/<uuid>`) are
    /// created. Defaults to `~/.silicolab`; `$HOME` is expanded by the remote shell.
    #[serde(default = "default_work_root")]
    pub work_root: String,
    /// Shell lines run on the remote *before* the engine, joined with `&&`. A
    /// non-interactive SSH shell does not source the login environment, so this is
    /// where `module load gromacs` / `source /opt/gromacs/bin/GMXRC` /
    /// `conda activate …` belong. Empty for a host where `gmx` is already on the
    /// non-interactive PATH.
    #[serde(default)]
    pub prelude: Vec<String>,
    /// Per-engine launch on this host, each carrying the verification taken against
    /// it. `program` is the remote path to the engine (or a bare name the prelude
    /// puts on PATH); `command_prefix` wraps it (`apptainer exec …`, `srun`), and is
    /// empty for a plain executable. A job submitted to this host carries the launch
    /// resolved from here in its `request.json` — the worker never rediscovers the
    /// engine.
    #[serde(default)]
    pub engines: EngineLaunches,
    /// Identity of the worker binary last deployed here, a cache hint for
    /// `remote::deploy`. Not an engine version: the worker is SilicoLab itself.
    #[serde(default)]
    pub worker_deployment: Option<String>,
    /// Per-host default resource request, resolved as per-job override → this →
    /// the app-wide core count. `cpus_per_task` caps the worker's thread pool so a
    /// job is a good citizen on a shared node; the rest render as scheduler
    /// directives and are inert under the Direct launcher.
    #[serde(default)]
    pub resources: ResourceSpec,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

impl Default for RemoteHost {
    /// The same values `serde` fills in for a host record that omits them.
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            hostname: String::new(),
            username: String::new(),
            port: default_ssh_port(),
            work_root: default_work_root(),
            prelude: Vec::new(),
            engines: EngineLaunches::new(),
            worker_deployment: None,
            resources: ResourceSpec::default(),
            scheduler: SchedulerConfig::default(),
        }
    }
}

/// What a job asks the node (or a scheduler) for, launcher-agnostic. Every field
/// is optional so an empty spec is valid and means "let the node decide".
pub type ResourceSpec = JobResources;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobResources {
    /// `cores` is the pre-scheduler name this field shipped under; the alias keeps
    /// an existing host's core default alive across the upgrade.
    #[serde(default, alias = "cores")]
    pub cpus_per_task: Option<u32>,
    #[serde(default, alias = "mem_mb")]
    pub memory_mib: Option<u64>,
    #[serde(default)]
    pub walltime_seconds: Option<u64>,
    #[serde(default)]
    pub gpu: GpuRequest,
    #[serde(default)]
    pub gpu_explicit: bool,
}

impl JobResources {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.cpus_per_task == Some(0) {
            anyhow::bail!("CPU count must be greater than zero");
        }
        if self.memory_mib == Some(0) {
            anyhow::bail!("memory must be greater than zero");
        }
        if self.walltime_seconds == Some(0) {
            anyhow::bail!("walltime must be greater than zero");
        }
        self.gpu.validate()
    }

    pub fn resolved_with(&self, defaults: &Self) -> Self {
        Self {
            cpus_per_task: self.cpus_per_task.or(defaults.cpus_per_task),
            memory_mib: self.memory_mib.or(defaults.memory_mib),
            walltime_seconds: self.walltime_seconds.or(defaults.walltime_seconds),
            gpu: if self.gpu_explicit {
                self.gpu.clone()
            } else {
                defaults.gpu.clone()
            },
            gpu_explicit: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GpuRequest {
    #[default]
    None,
    Any {
        count: u32,
    },
    Typed {
        gpu_type: String,
        count: u32,
    },
}

impl GpuRequest {
    pub fn count(&self) -> u32 {
        match self {
            Self::None => 0,
            Self::Any { count } | Self::Typed { count, .. } => *count,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::None => Ok(()),
            Self::Any { count } if *count > 0 => Ok(()),
            Self::Typed { gpu_type, count } if *count > 0 && valid_scheduler_value(gpu_type) => {
                Ok(())
            }
            Self::Any { .. } | Self::Typed { count: 0, .. } => {
                anyhow::bail!("GPU count must be greater than zero")
            }
            Self::Typed { .. } => anyhow::bail!("GPU type contains invalid characters"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerConfig {
    #[default]
    Direct,
    Slurm(SlurmProfile),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlurmProfile {
    #[serde(default)]
    pub scheduler_prelude: Vec<String>,
    #[serde(default)]
    pub partition: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub qos: Option<String>,
    #[serde(default)]
    pub reservation: Option<String>,
    #[serde(default)]
    pub constraint: Option<String>,
    #[serde(default)]
    pub gpu_syntax: SlurmGpuSyntax,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl SlurmProfile {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (label, value) in [
            ("partition", self.partition.as_deref()),
            ("account", self.account.as_deref()),
            ("QOS", self.qos.as_deref()),
            ("reservation", self.reservation.as_deref()),
        ] {
            if value.is_some_and(|value| !valid_scheduler_value(value)) {
                anyhow::bail!("{label} contains invalid characters");
            }
        }
        if self
            .constraint
            .as_deref()
            .is_some_and(|value| !valid_constraint_value(value))
        {
            anyhow::bail!("constraint contains invalid characters");
        }
        self.gpu_syntax.validate()?;
        for argument in &self.extra_args {
            validate_extra_arg(argument)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlurmGpuSyntax {
    Gres { resource_name: String },
    Gpus,
    CustomTemplate { argument: String },
}

impl Default for SlurmGpuSyntax {
    fn default() -> Self {
        Self::Gres {
            resource_name: "gpu".to_string(),
        }
    }
}

impl SlurmGpuSyntax {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Gres { resource_name } if valid_scheduler_value(resource_name) => Ok(()),
            Self::Gres { .. } => anyhow::bail!("GRES resource name contains invalid characters"),
            Self::Gpus => Ok(()),
            Self::CustomTemplate { argument } => {
                let remainder = argument.replace("{count}", "").replace("{type}", "");
                if !argument.contains("{count}")
                    || remainder.contains('{')
                    || remainder.contains('}')
                    || argument.contains(['\n', '\r', '\0'])
                {
                    anyhow::bail!("custom GPU argument has invalid placeholders");
                }
                Ok(())
            }
        }
    }
}

fn valid_scheduler_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '+' | ',' | '/')
        })
}

/// `--constraint` takes a feature *expression*, not a bare name: `a&b`, `a|b`,
/// `[a*2&b]`. Every rendered argument is shell-quoted, so the extra operators
/// stay inert.
fn valid_constraint_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '_' | '-'
                        | '.'
                        | ':'
                        | '+'
                        | ','
                        | '/'
                        | '&'
                        | '|'
                        | '*'
                        | '['
                        | ']'
                        | '('
                        | ')'
                )
        })
}

fn validate_extra_arg(argument: &str) -> anyhow::Result<()> {
    const OWNED: &[&str] = &[
        "--wrap",
        "--chdir",
        "--output",
        "--error",
        "--open-mode",
        "--parsable",
        "--nodes",
        "--ntasks",
        "--job-name",
        "--export",
    ];
    let key = argument.split('=').next().unwrap_or(argument);
    if !argument.starts_with('-')
        || argument.contains(['\n', '\r', '\0'])
        || argument.chars().any(char::is_whitespace)
        || OWNED.contains(&key)
    {
        anyhow::bail!("invalid or conflicting Slurm argument `{argument}`");
    }
    Ok(())
}

fn default_ssh_port() -> u16 {
    22
}

fn default_work_root() -> String {
    "~/.silicolab".to_string()
}

/// The per-user SilicoLab directory: `settings.json`, `recent_projects.json`, and
/// the SSH key/known-hosts the remote bootstrap writes all live here.
pub fn config_dir() -> PathBuf {
    home_dir().join(".silicolab")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_host_without_resources_or_engine_maps() {
        // A host authored before per-host resource defaults (and before the
        // engine maps) must still load, with the new fields defaulting to empty.
        let json = r#"{
            "id": "h1",
            "label": "Box",
            "hostname": "example.com",
            "username": "alice"
        }"#;
        let host: RemoteHost = serde_json::from_str(json).expect("legacy host parses");
        assert_eq!(host.port, 22);
        assert_eq!(host.work_root, "~/.silicolab");
        assert!(host.prelude.is_empty());
        assert!(host.engines.is_empty());
        assert!(host.worker_deployment.is_none());
        assert_eq!(host.resources, ResourceSpec::default());
        assert_eq!(host.resources.cpus_per_task, None);
        assert_eq!(host.scheduler, SchedulerConfig::Direct);
    }

    /// A host authored before the `engine_versions` pocket was split keeps its
    /// launches, and the now-unknown field is simply ignored. Rescuing the worker
    /// identity out of it is `backend::config`'s job, not this struct's.
    #[test]
    fn a_host_with_the_old_engine_versions_pocket_still_parses() {
        let json = r#"{
            "id": "h1",
            "label": "Box",
            "hostname": "example.com",
            "username": "alice",
            "engines": {"gromacs": {"program": "/opt/g/bin/gmx"}},
            "engine_versions": {"_worker": "dev:abc", "gromacs": "2026.2"}
        }"#;
        let host: RemoteHost = serde_json::from_str(json).expect("legacy host parses");

        let entry = host
            .engines
            .entry(crate::launch::EngineId::GROMACS)
            .expect("the launch itself survives");
        assert_eq!(entry.launch.program, "/opt/g/bin/gmx");
        assert!(
            entry.verified.is_none(),
            "an unattributable version must not read as a verification"
        );
        assert!(
            !serde_json::to_string(&host)
                .unwrap()
                .contains("engine_versions")
        );
    }

    #[test]
    fn resource_spec_roundtrips_and_empty_is_default() {
        let spec = ResourceSpec {
            cpus_per_task: Some(8),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<ResourceSpec>(&json).unwrap(), spec);
        // An empty object yields all-defaults (forward-compatible).
        assert_eq!(
            serde_json::from_str::<ResourceSpec>("{}").unwrap(),
            ResourceSpec::default()
        );
    }

    #[test]
    fn resource_and_slurm_validation_rejects_unsafe_values() {
        assert!(GpuRequest::Any { count: 0 }.validate().is_err());
        assert!(
            GpuRequest::Typed {
                gpu_type: "a100;id".into(),
                count: 1,
            }
            .validate()
            .is_err()
        );
        let mut profile = SlurmProfile {
            partition: Some("debug".into()),
            ..Default::default()
        };
        assert!(profile.validate().is_ok());
        profile.extra_args.push("--wrap=oops".into());
        assert!(profile.validate().is_err());
    }

    #[test]
    fn constraint_accepts_slurm_feature_expressions() {
        let profile = SlurmProfile {
            constraint: Some("gpu&highmem".into()),
            ..Default::default()
        };
        assert!(profile.validate().is_ok());
        let profile = SlurmProfile {
            constraint: Some("a; rm -rf /".into()),
            ..Default::default()
        };
        assert!(profile.validate().is_err());
    }

    #[test]
    fn a_pre_scheduler_resource_spec_keeps_its_core_default() {
        let spec: ResourceSpec =
            serde_json::from_str(r#"{"cores": 8, "walltime": "01:00:00", "extra": []}"#).unwrap();
        assert_eq!(spec.cpus_per_task, Some(8));
    }
}
