//! Remote-host descriptor and the per-user config directory.
//!
//! `RemoteHost` describes an SSH-reachable machine that engine jobs can be
//! submitted to; it lives here, at the bottom of the compute crate, because the
//! remote engine transport depends on it. `config_dir` is the per-user SilicoLab
//! directory (`~/.silicolab`) that also holds the SSH key/known-hosts the remote
//! bootstrap writes, so the two are kept together.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::launch::EngineLaunch;

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
    /// Per-engine launch on this host, keyed by [`crate::engines::registry::EngineId`]
    /// string. `program` is the remote path to the engine; `command_prefix` is
    /// normally empty (the remote shell, not a local launcher, runs it).
    #[serde(default)]
    pub engines: HashMap<String, EngineLaunch>,
    /// Cached `<engine> --version` strings, keyed by engine id, plus the reserved
    /// `_worker` deployment identity. Engine entries let settings show versions
    /// without re-probing over SSH on every open.
    #[serde(default)]
    pub engine_versions: HashMap<String, String>,
    /// Per-host default resource request. Only `cores` is consumed today — it caps
    /// the worker's thread pool so a job is a good citizen on a shared node;
    /// resolution is per-job override → this → the app-wide core count. The other
    /// fields parse forward-compatibly for scheduler-directive rendering.
    #[serde(default)]
    pub resources: ResourceSpec,
}

/// What a job asks the node (or a scheduler) for, launcher-agnostic. Every field
/// is optional so an empty spec is valid and means "let the node decide". Only
/// `cores` is consumed today (it sizes the worker's thread pool); the rest are
/// reserved and round-trip untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSpec {
    /// Requested core count. Resolves per-job override → per-host default → the
    /// app-wide core count, then is clamped to the target's inventory before it
    /// reaches `request.json`.
    #[serde(default)]
    pub cores: Option<usize>,
    #[serde(default)]
    pub mem_mb: Option<u64>,
    #[serde(default)]
    pub walltime: Option<String>,
    #[serde(default)]
    pub extra: Vec<String>,
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
        assert!(host.engine_versions.is_empty());
        assert_eq!(host.resources, ResourceSpec::default());
        assert_eq!(host.resources.cores, None);
    }

    #[test]
    fn resource_spec_roundtrips_and_empty_is_default() {
        let spec = ResourceSpec {
            cores: Some(8),
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
}
