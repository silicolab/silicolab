//! Project housekeeping files that live alongside the databases:
//!
//! - a top-level plaintext **manifest** so a directory self-identifies as a
//!   SilicoLab project and its format version can be checked *before* opening any
//!   database;
//! - a **session lock** that detects when a previous session did not shut down
//!   cleanly (e.g. a crash);
//! - a **maintenance log** recording database compaction (`VACUUM`) runs.
//!
//! All generated files are plaintext and carry a "do not edit" banner.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::backend::project::{PROJECT_FORMAT_VERSION, ProjectSession};
use crate::backend::structure_codec::PAYLOAD_FORMAT;

/// Top-level descriptor written at the project root (not inside `.silicolab/`) so
/// the folder is recognizable and version-checkable at a glance.
pub const MANIFEST_FILE: &str = "silicolab.project";
const LOCK_FILE: &str = "session.lock";
const MAINTENANCE_LOG_FILE: &str = "maintenance.log";

const DO_NOT_EDIT: &str = "# SilicoLab-generated file — safe to read, do not edit.";

/// Write (or refresh) the top-level project manifest. Cheap enough to call on
/// every save so the recorded versions always match what wrote the databases.
pub fn write_manifest(project: &ProjectSession) -> Result<()> {
    let path = project.root.join(MANIFEST_FILE);
    let contents = format!(
        "{DO_NOT_EDIT}\n\
         silicolab_project_format {PROJECT_FORMAT_VERSION}\n\
         compounds_payload_format {PAYLOAD_FORMAT}\n\
         created_with silicolab {}\n",
        env!("CARGO_PKG_VERSION"),
    );
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Validate a project's manifest before opening its databases. A project written
/// by a newer, incompatible format is refused with a clear error; a missing
/// manifest (older project) is tolerated.
pub fn check_manifest_compatibility(project: &ProjectSession) -> Result<()> {
    let path = project.root.join(MANIFEST_FILE);
    let Ok(contents) = fs::read_to_string(&path) else {
        // No manifest: an older or hand-made project. Let the DB layer try.
        return Ok(());
    };
    if let Some(format) = parse_manifest_field(&contents, "silicolab_project_format")
        && format > PROJECT_FORMAT_VERSION
    {
        bail!(
            "project was created with a newer SilicoLab format (v{format}); this build supports up to v{PROJECT_FORMAT_VERSION}"
        );
    }
    Ok(())
}

fn parse_manifest_field(contents: &str, key: &str) -> Option<u32> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next() == Some(key))
            .then(|| parts.next())
            .flatten()?
            .parse()
            .ok()
    })
}

/// Acquire the session lock, returning `true` if a stale lock from a previous
/// run was found (i.e. the last session did not shut down cleanly).
pub fn acquire_lock(project: &ProjectSession) -> bool {
    let path = project.silicolab_dir.join(LOCK_FILE);
    let was_stale = path.exists();
    let contents = format!(
        "{DO_NOT_EDIT}\nopened {}\npid {}\n",
        utc_timestamp(),
        std::process::id(),
    );
    // Best-effort: a failed lock write must never block opening the project.
    let _ = fs::write(&path, contents);
    was_stale
}

/// Release the session lock on a clean shutdown / project switch.
pub fn release_lock(project: &ProjectSession) {
    let _ = fs::remove_file(project.silicolab_dir.join(LOCK_FILE));
}

/// Compact both project databases with `VACUUM` and append the result to the
/// maintenance log. Run at checkpoints (e.g. closing a project), never on every
/// autosave — incremental saves churn the geometry blob and leave free pages
/// that this reclaims.
pub fn run_maintenance(project: &ProjectSession) -> Result<()> {
    let project_db = vacuum_database(&project.project_db)?;
    let compounds_db = vacuum_database(&project.compounds_db)?;
    append_maintenance_log(
        project,
        &format!("VACUUM project.db {project_db}; compounds.db {compounds_db}"),
    );
    Ok(())
}

fn vacuum_database(path: &Path) -> Result<String> {
    let before = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let connection =
        Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    connection
        .execute_batch("VACUUM")
        .with_context(|| format!("failed to vacuum {}", path.display()))?;
    drop(connection);
    let after = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    Ok(format!("{before} -> {after} bytes"))
}

fn append_maintenance_log(project: &ProjectSession, message: &str) {
    let path = project.silicolab_dir.join(MAINTENANCE_LOG_FILE);
    let line = format!("{}  {message}\n", utc_timestamp());
    let write = || -> std::io::Result<()> {
        if !path.exists() {
            fs::write(&path, format!("{DO_NOT_EDIT}\n"))?;
        }
        let mut file = OpenOptions::new().append(true).open(&path)?;
        file.write_all(line.as_bytes())
    };
    let _ = write();
}

fn utc_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert a day count since the Unix epoch into a (year, month, day) UTC civil
/// date (Howard Hinnant's `civil_from_days`), avoiding a date-library dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::project::ProjectSession;
    use std::path::PathBuf;

    fn session(name: &str) -> ProjectSession {
        let root = PathBuf::from(format!("target/test-housekeeping-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".silicolab")).unwrap();
        ProjectSession::from_root(root, name.to_string())
    }

    #[test]
    fn manifest_roundtrips_and_rejects_newer_formats() {
        let session = session("manifest");
        write_manifest(&session).unwrap();
        // Same-version manifest is accepted.
        check_manifest_compatibility(&session).unwrap();
        // A newer format is rejected.
        let path = session.root.join(MANIFEST_FILE);
        fs::write(&path, "silicolab_project_format 9999\n").unwrap();
        assert!(check_manifest_compatibility(&session).is_err());
        // A missing manifest is tolerated.
        fs::remove_file(&path).unwrap();
        check_manifest_compatibility(&session).unwrap();
    }

    #[test]
    fn lock_detects_unclean_shutdown() {
        let session = session("lock");
        assert!(!acquire_lock(&session), "first acquire is clean");
        // Lock left behind (simulating a crash) is detected on next acquire.
        assert!(acquire_lock(&session), "lingering lock is stale");
        release_lock(&session);
        assert!(!acquire_lock(&session), "released lock is clean again");
    }

    #[test]
    fn civil_date_matches_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(18_993), (2022, 1, 1));
    }
}
