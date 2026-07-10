//! The per-run breadcrumb written into each local run dir.
//!
//! A detached remote run records where it lives (`host_id`, label, `user@host`,
//! the remote dir, and the start time) so a later session — or the user — can
//! find and clean up the remote scratch dir, and so the job registry is
//! rebuildable by scanning run dirs if `jobs.db` is ever lost. The shape is
//! typed and versioned so the on-disk format is explicit and forward-compatible.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::RemoteTarget;
use super::launcher::{LaunchHandle, Launcher};
use crate::hosts::JobResources;

/// Filename of the per-run breadcrumb written into the local run dir.
pub const REMOTE_RUN_FILE: &str = "remote_run.json";
/// Schema version of [`RemoteRunRecord`]; bumped if the shape changes.
pub const REMOTE_RUN_RECORD_VERSION: u32 = 2;

/// A typed, versioned, self-describing record of where a detached remote run
/// lives. Written into each local run dir at launch so a later session (or the
/// user) can find and clean up the remote scratch dir, and so the job registry
/// is rebuildable by scanning run dirs if `jobs.db` is ever lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRunRecord {
    /// Schema version. `#[serde(default)]` so a pre-versioned breadcrumb (which
    /// carried no `version` field) still parses, reading back as `0`.
    #[serde(default)]
    pub version: u32,
    pub host_id: String,
    pub host_label: String,
    pub user_host: String,
    pub remote_dir: String,
    pub started_at_unix: u64,
    #[serde(default = "default_scheduler")]
    pub scheduler: String,
    #[serde(default)]
    pub launch_handle: Option<RunLaunchHandle>,
    #[serde(default)]
    pub resources: JobResources,
    #[serde(default)]
    pub submitted_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLaunchHandle {
    pub id: String,
    #[serde(default)]
    pub cluster: Option<String>,
}

/// Write the [`RemoteRunRecord`] breadcrumb into the local run dir at launch.
/// Because the remote command is detached (`setsid`), closing the app leaves it
/// running; this record is what a later session needs to reconnect or clean up.
/// Best-effort — a write failure must never fail the run.
pub fn write_run_record(
    target: &RemoteTarget,
    working_dir: &Path,
    launcher: Launcher,
    handle: Option<&LaunchHandle>,
    resources: &JobResources,
) {
    let started_at_unix = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = RemoteRunRecord {
        version: REMOTE_RUN_RECORD_VERSION,
        host_id: target.host_id.clone(),
        host_label: target.host_label.clone(),
        user_host: target.user_host(),
        remote_dir: target.remote_dir.clone(),
        started_at_unix,
        scheduler: launcher.token().to_string(),
        launch_handle: handle.map(|handle| RunLaunchHandle {
            id: handle.id.clone(),
            cluster: handle.cluster.clone(),
        }),
        resources: resources.clone(),
        submitted_at_ms: started_at_unix.saturating_mul(1000) as i64,
    };
    if let Ok(text) = serde_json::to_string_pretty(&record) {
        let path = working_dir.join(REMOTE_RUN_FILE);
        let temporary = working_dir.join(format!("{REMOTE_RUN_FILE}.tmp"));
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

fn default_scheduler() -> String {
    "direct".to_string()
}

/// Read the [`RemoteRunRecord`] breadcrumb from a local run dir, if present and
/// parseable. Lets the registry be rebuilt from run dirs when `jobs.db` is absent.
pub fn read_run_record(working_dir: &Path) -> Option<RemoteRunRecord> {
    let text = fs::read_to_string(working_dir.join(REMOTE_RUN_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_record_round_trips_through_json() {
        let record = RemoteRunRecord {
            version: REMOTE_RUN_RECORD_VERSION,
            host_id: "hpc".to_string(),
            host_label: "Cluster".to_string(),
            user_host: "alice@login.example.edu".to_string(),
            remote_dir: "~/.silicolab/runs/abc-123".to_string(),
            started_at_unix: 1_700_000_000,
            scheduler: "slurm".to_string(),
            launch_handle: Some(RunLaunchHandle {
                id: "42".to_string(),
                cluster: Some("alpha".to_string()),
            }),
            resources: JobResources {
                cpus_per_task: Some(4),
                ..Default::default()
            },
            submitted_at_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: RemoteRunRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, REMOTE_RUN_RECORD_VERSION);
        assert_eq!(back.host_id, "hpc");
        assert_eq!(back.user_host, "alice@login.example.edu");
        assert_eq!(back.remote_dir, "~/.silicolab/runs/abc-123");
        assert_eq!(back.started_at_unix, 1_700_000_000);
        assert_eq!(back.scheduler, "slurm");
        assert_eq!(back.launch_handle.unwrap().id, "42");
    }

    #[test]
    fn version_defaults_to_zero_for_a_pre_versioned_record() {
        // A breadcrumb written before `version` existed omits the key; the
        // `#[serde(default)]` back-compat contract reads it back as 0.
        let json = r#"{
            "host_id": "hpc",
            "host_label": "Cluster",
            "user_host": "alice@login.example.edu",
            "remote_dir": "~/.silicolab/runs/abc-123",
            "started_at_unix": 1700000000
        }"#;
        let record: RemoteRunRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.version, 0);
        assert_eq!(record.host_id, "hpc");
        assert_eq!(record.scheduler, "direct");
        assert!(record.launch_handle.is_none());
    }

    #[test]
    fn version_one_record_loads_with_direct_defaults() {
        let json = r#"{
            "version": 1,
            "host_id": "hpc",
            "host_label": "Cluster",
            "user_host": "alice@login.example.edu",
            "remote_dir": "~/.silicolab/runs/abc-123",
            "started_at_unix": 1700000000
        }"#;
        let record: RemoteRunRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.version, 1);
        assert_eq!(record.scheduler, "direct");
    }
}
