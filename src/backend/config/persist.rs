//! Reading and writing the app's config files.
//!
//! `settings.json` and `recent_projects.json` both live in [`config_dir`]. Writes
//! go through [`write_atomic`]: a plain truncating write that is interrupted leaves
//! a corrupt file, which used to reset every setting on the next launch.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::{AppConfig, RecentProject, current_timestamp};
use compute_core::hosts::config_dir;

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn recent_projects_path() -> PathBuf {
    config_dir().join("recent_projects.json")
}

/// Load the app config. A missing file is a normal first run (silent default). A
/// file that exists but fails to parse is preserved (see `back_up_corrupt_file`)
/// and a warning is returned, rather than silently resetting every setting — the
/// silent reset is what made an interrupted write look like the app randomly
/// forgetting the assistant model.
pub fn load_config() -> (AppConfig, Option<String>) {
    let path = settings_path();
    if !path.exists() {
        return (AppConfig::default(), None);
    }
    match load_config_from(&path) {
        Ok(config) => (config, None),
        Err(error) => {
            let warning = match back_up_corrupt_file(&path) {
                Ok(backup) => format!(
                    "Settings were unreadable and have been reset to defaults; \
                     the previous file is kept at {} ({error}).",
                    backup.display()
                ),
                Err(backup_error) => format!(
                    "Settings were unreadable and have been reset to defaults \
                     ({error}; could not preserve the old file: {backup_error})."
                ),
            };
            (AppConfig::default(), Some(warning))
        }
    }
}

/// Move a corrupt config file aside to `<name>.corrupt` (numbering on collision)
/// so the next save can write a fresh file without destroying the bad one.
fn back_up_corrupt_file(path: &Path) -> Result<PathBuf> {
    let mut backup = path.with_extension("json.corrupt");
    let mut n = 1;
    while backup.exists() {
        backup = path.with_extension(format!("json.corrupt.{n}"));
        n += 1;
    }
    fs::rename(path, &backup)
        .with_context(|| format!("failed to move {} aside", path.display()))?;
    Ok(backup)
}

pub fn save_config(config: &AppConfig) -> Result<()> {
    save_config_to(&settings_path(), config)
}

pub fn load_recent_projects() -> Vec<RecentProject> {
    load_recent_projects_from(&recent_projects_path()).unwrap_or_default()
}

pub fn save_recent_projects(projects: &[RecentProject]) -> Result<()> {
    save_recent_projects_to(&recent_projects_path(), projects)
}

pub fn remember_recent_project(projects: &mut Vec<RecentProject>, path: &Path, name: &str) {
    let now = current_timestamp();
    if let Some(project) = projects.iter_mut().find(|project| project.path == path) {
        project.name = name.to_string();
        project.last_accessed = now;
    } else {
        projects.push(RecentProject {
            path: path.to_path_buf(),
            name: name.to_string(),
            last_accessed: now,
        });
    }
    projects.sort_by_key(|project| std::cmp::Reverse(project.last_accessed));
    projects.truncate(12);
}

/// Read and parse an `AppConfig` from an arbitrary path. Used by the settings
/// loader and by Advanced ▸ Import; the `Result` lets the importer report
/// malformed input non-fatally rather than panic.
pub fn load_config_from(path: &Path) -> Result<AppConfig> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_str(&source)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let mut config: AppConfig = serde_json::from_value(raw.clone())
        .with_context(|| format!("failed to parse {}", path.display()))?;
    rescue_legacy_worker_ids(&raw, &mut config);
    Ok(config)
}

/// A host written before `worker_deployment` existed kept the worker's identity
/// under `engine_versions["_worker"]`, a map `RemoteHost` no longer has a field
/// for. Lift it across so the next remote job reuses the deployed worker instead
/// of re-uploading it.
///
/// The engine `--version` strings in that same map are deliberately left behind:
/// they were keyed by engine rather than by the launch they were probed from, so
/// nothing can say whether they still describe the configured binary. Unverified
/// is the honest state, and one click re-earns the version.
fn rescue_legacy_worker_ids(raw: &serde_json::Value, config: &mut AppConfig) {
    for (id, host) in &mut config.remote_hosts {
        if host.worker_deployment.is_some() {
            continue;
        }
        host.worker_deployment = raw
            .get("remote_hosts")
            .and_then(|hosts| hosts.get(id))
            .and_then(|host| host.get("engine_versions"))
            .and_then(|versions| versions.get("_worker"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }
}

/// Serialize an `AppConfig` to an arbitrary path. Used by the settings saver and
/// by Advanced ▸ Export.
pub fn save_config_to(path: &Path, config: &AppConfig) -> Result<()> {
    let source = serde_json::to_string_pretty(config)?;
    write_atomic(path, source.as_bytes())
}

/// Write `contents` to `path` atomically: temp file (beside the target so the
/// rename stays on one volume) → fsync → rename over the target. A plain
/// `fs::write` truncates before writing, so a crash mid-write leaves a corrupt
/// file that resets every setting on the next launch.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(contents)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))
}

fn load_recent_projects_from(path: &Path) -> Result<Vec<RecentProject>> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_recent_projects_to(path: &Path, projects: &[RecentProject]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let source = serde_json::to_string_pretty(projects)?;
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::config::{AppConfig, AssistantConfig, RecentProject};

    #[test]
    fn missing_config_uses_default() {
        let loaded = load_config_from(&PathBuf::from("target/no-such-settings.json"));

        assert!(loaded.is_err());
        assert!(
            !AppConfig::default()
                .default_project_dir
                .as_os_str()
                .is_empty()
        );
    }

    #[test]
    fn save_config_to_round_trips_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join("silicolab-cfg-roundtrip");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");

        let config = AppConfig {
            assistant: AssistantConfig {
                provider: "openai".to_string(),
                model: "gpt-5.1".to_string(),
                ..AssistantConfig::default()
            },
            ..AppConfig::default()
        };
        save_config_to(&path, &config).expect("atomic save");

        // No temp file left behind after the rename.
        assert!(!path.with_extension("tmp").exists());
        let back = load_config_from(&path).expect("load back");
        assert_eq!(back.assistant.provider, "openai");
        assert_eq!(back.assistant.model, "gpt-5.1");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_preserved_not_destroyed() {
        let dir = std::env::temp_dir().join("silicolab-cfg-corrupt");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        fs::write(&path, b"{ truncated json").expect("write corrupt");

        let backup = back_up_corrupt_file(&path).expect("back up corrupt file");

        // Bad file moved aside, original path freed for a fresh write.
        assert!(backup.exists());
        assert!(!path.exists());
        assert_eq!(fs::read(&backup).expect("read backup"), b"{ truncated json");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remember_recent_project_updates_existing() {
        let mut projects = vec![RecentProject {
            path: PathBuf::from("old"),
            name: "Old".to_string(),
            last_accessed: 1,
        }];

        remember_recent_project(&mut projects, &PathBuf::from("old"), "Renamed");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Renamed");
    }

    /// A host written before the `engine_versions` pocket was split keeps its worker
    /// identity, so the next remote job reuses the deployed worker. Its engine version
    /// string is dropped: nothing recorded which launch it was probed from, so the
    /// launch reads back as configured-but-unverified.
    #[test]
    fn a_legacy_host_keeps_its_worker_id_and_loses_its_engine_version() {
        let dir = std::env::temp_dir().join("silicolab-cfg-legacy-worker");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("settings.json");
        fs::write(
            &path,
            br#"{
                "default_project_dir": "projects",
                "closed_to_scratch": false,
                "remote_hosts": {
                    "h1": {
                        "id": "h1",
                        "label": "Cluster",
                        "hostname": "login.example.edu",
                        "username": "alice",
                        "engines": {"gromacs": {"program": "/opt/g/bin/gmx"}},
                        "engine_versions": {"_worker": "dev:abc", "gromacs": "2026.2"}
                    }
                }
            }"#,
        )
        .expect("write legacy settings");

        let config = load_config_from(&path).expect("legacy settings load");
        let host = config.remote_hosts.get("h1").expect("host");
        assert_eq!(host.worker_deployment.as_deref(), Some("dev:abc"));
        let entry = host
            .engines
            .entry(crate::engines::registry::EngineId::GROMACS)
            .expect("the launch survives");
        assert_eq!(entry.launch.program, "/opt/g/bin/gmx");
        assert!(
            entry.verified.is_none(),
            "an unattributable version must not read as a verification"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
