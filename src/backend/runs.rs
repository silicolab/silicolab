use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::backend::tasks::TaskRun;

pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Serialize)]
pub struct RunManifest<'a> {
    pub schema_version: u32,
    pub run_id: u64,
    /// Durable, globally-unique identity of this run (see [`TaskRun::run_uuid`]).
    pub run_uuid: &'a str,
    pub task_id: &'a str,
    pub title: &'a str,
    pub status: &'a str,
    pub outcome: &'a str,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_entry_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_entry_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<&'a str>,
}

impl<'a> RunManifest<'a> {
    pub fn from_task(task: &'a TaskRun) -> Self {
        Self {
            schema_version: 1,
            run_id: task.id,
            run_uuid: &task.run_uuid,
            task_id: task.controller_id,
            title: &task.title,
            status: task.status.label(),
            outcome: task.outcome.label(),
            created_at_ms: task.created_at_ms,
            finished_at_ms: task.finished_at_ms,
            source_entry_id: task.source_entry_id,
            result_entry_id: task.result_entry_id,
            engine: task.engine_label.as_deref(),
        }
    }
}

/// File name of the machine-readable series data saved beside a QM run's
/// `output.txt`.
pub const SERIES_FILE: &str = "series.json";

/// Numeric results of one QM run, exactly as surfaced by the engine
/// (`{"version":1,"scf_trace":[...],"opt_trace":[...],"frequencies":[...]}`).
/// The chart pipeline's on-disk source of truth: raw vectors, not chart specs,
/// so chart styling can evolve without invalidating saved runs. The schema is
/// versioned and additive; missing arrays read as empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QmSeries {
    pub version: u32,
    #[serde(default)]
    pub scf_trace: Vec<f64>,
    #[serde(default)]
    pub opt_trace: Vec<f64>,
    #[serde(default)]
    pub frequencies: Vec<f64>,
}

impl QmSeries {
    pub fn from_outcome(outcome: &crate::engines::qm::QmOutcome) -> Self {
        Self {
            version: 1,
            scf_trace: outcome.scf_trace.clone(),
            opt_trace: outcome.opt_trace.clone(),
            frequencies: outcome.frequencies.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.scf_trace.is_empty() && self.opt_trace.is_empty() && self.frequencies.is_empty()
    }
}

pub fn save_qm_series_file(run_dir: &Path, series: &QmSeries) -> Result<PathBuf> {
    let path = run_dir.join(SERIES_FILE);
    let json = serde_json::to_string_pretty(series).context("serialize QM series")?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn load_qm_series_file(path: &Path) -> Result<QmSeries> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let series: QmSeries =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if series.version != 1 {
        bail!("unsupported series.json version {}", series.version);
    }
    Ok(series)
}

/// Runaway guard for run-directory numbering (far beyond any realistic number of
/// runs in one project).
const MAX_RUN_DIR_SEQUENCE: u32 = 100_000_000;

/// The default user-facing run name for a task whose controller id is `prefix`:
/// `{prefix}-{N}`, where `N` is the lowest positive integer whose directory does
/// not already exist under `base_dir`. This is only a *suggested* name — the user
/// is free to rename it — and it is decoupled from the task's id and UUID.
pub fn default_run_name(base_dir: &Path, prefix: &str) -> String {
    for sequence in 1..=MAX_RUN_DIR_SEQUENCE {
        let candidate = format!("{prefix}-{sequence}");
        if !base_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{prefix}-{MAX_RUN_DIR_SEQUENCE}")
}

/// Create a fresh run directory under `base_dir` using `desired_name` (a
/// human-readable name, e.g. `build-md-system-1`, sanitized for the filesystem).
/// If that name is already taken, a `-2`, `-3`, ... suffix is appended until a
/// free name is found, so a user-chosen name never collides with an existing run.
///
/// The directory is created with [`fs::create_dir`] (not `create_dir_all`), so
/// claiming a name and detecting collisions is a single atomic step — two runs
/// racing for the same name can never both win.
pub fn ensure_run_dir(base_dir: &Path, desired_name: &str) -> Result<PathBuf> {
    fs::create_dir_all(base_dir)
        .with_context(|| format!("failed to create {}", base_dir.display()))?;

    let base = sanitize_run_name(desired_name);
    for attempt in 0..MAX_RUN_DIR_SEQUENCE {
        let name = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", attempt + 1)
        };
        let candidate = base_dir.join(&name);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create {}", candidate.display()));
            }
        }
    }
    bail!("could not find a free run directory name for '{base}'")
}

/// Reduce a user-supplied name to a safe single-path-component directory name:
/// keep alphanumerics, `-`, `_`, and `.`, collapse everything else (including
/// path separators and whitespace) to `-`, and fall back to `run` if empty.
fn sanitize_run_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "run".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn write_manifest(task: &TaskRun) -> Result<()> {
    let Some(run_dir) = task.run_dir.as_ref() else {
        return Ok(());
    };
    fs::create_dir_all(run_dir)
        .with_context(|| format!("failed to create {}", run_dir.display()))?;
    let manifest = RunManifest::from_task(task);
    let json = serde_json::to_string_pretty(&manifest)?;
    fs::write(run_dir.join(MANIFEST_FILE), json)
        .with_context(|| format!("failed to write {}", run_dir.join(MANIFEST_FILE).display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("silicolab_runs_test_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn default_run_name_skips_existing() {
        let base = temp_base("default");
        fs::create_dir_all(base.join("run-md-1")).unwrap();
        fs::create_dir_all(base.join("run-md-2")).unwrap();
        assert_eq!(default_run_name(&base, "run-md"), "run-md-3");
        // A fresh prefix starts at 1.
        assert_eq!(
            default_run_name(&base, "build-md-system"),
            "build-md-system-1"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_run_dir_dedups_and_sanitizes() {
        let base = temp_base("ensure");
        let first = ensure_run_dir(&base, "my run/1").unwrap();
        // Path separators and spaces collapse to a safe single component.
        assert_eq!(first.file_name().unwrap().to_str().unwrap(), "my-run-1");
        // Re-requesting the same name yields a distinct, suffixed directory.
        let second = ensure_run_dir(&base, "my run/1").unwrap();
        assert_eq!(second.file_name().unwrap().to_str().unwrap(), "my-run-1-2");
        assert_ne!(first, second);
        // An all-punctuation name falls back rather than producing an empty name.
        let fallback = ensure_run_dir(&base, "///").unwrap();
        assert_eq!(fallback.file_name().unwrap().to_str().unwrap(), "run");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn qm_series_round_trips_and_rejects_future_versions() {
        let base = temp_base("series");
        fs::create_dir_all(&base).unwrap();
        let series = QmSeries {
            version: 1,
            scf_trace: vec![-74.1, -74.9],
            opt_trace: vec![-74.9],
            frequencies: vec![4401.2],
        };
        let path = save_qm_series_file(&base, &series).unwrap();
        assert_eq!(path.file_name().unwrap(), SERIES_FILE);
        assert_eq!(load_qm_series_file(&path).unwrap(), series);
        fs::write(&path, r#"{"version":2}"#).unwrap();
        assert!(load_qm_series_file(&path).is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn qm_series_missing_arrays_default_to_empty() {
        let series: QmSeries = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert!(series.scf_trace.is_empty());
        assert!(series.opt_trace.is_empty());
        assert!(series.frequencies.is_empty());
    }

    #[test]
    fn qm_series_from_outcome_copies_the_traces() {
        let outcome = crate::engines::qm::QmOutcome {
            energy_hartree: -74.96,
            converged: true,
            optimized_structure: None,
            summary: String::new(),
            scf_trace: vec![-74.1, -74.96],
            opt_trace: vec![-74.96],
            frequencies: Vec::new(),
        };
        let series = QmSeries::from_outcome(&outcome);
        assert_eq!(series.version, 1);
        assert_eq!(series.scf_trace, outcome.scf_trace);
        assert_eq!(series.opt_trace, outcome.opt_trace);
        assert!(!series.is_empty());
        assert!(QmSeries::default().is_empty());
    }
}
