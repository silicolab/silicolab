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
    /// Cached `<engine> --version` strings, keyed by engine id. Filled by the
    /// settings "Detect" action so the panel shows versions without re-probing over
    /// SSH on every open.
    #[serde(default)]
    pub engine_versions: HashMap<String, String>,
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
