//! A small reusable library of user-supplied force-field parameter blocks
//! (`.itp` fragments), stored under the app config directory so they are
//! available across projects.
//!
//! Materials the built-in tables cannot parameterize (exotic transition-metal
//! dichalcogenides, metals, …) can still be simulated if the user supplies the
//! missing Lennard-Jones / bonded parameters. Each library entry is one `.itp`
//! fragment — `[ atomtypes ]` (and optionally `[ bondtypes ]` etc.) — that the
//! framework MD build inlines into the generated topology. The convention is
//! that an atom type is named after its element symbol (e.g. an atomtype `Pt`
//! parameterizes platinum), matching the built-in tables.
//!
//! The fragment syntax is this program's own force-field text format; it happens
//! to coincide with the include-topology syntax the MD engine consumes, so the
//! engine layer can pass it through unchanged. The storage layer here treats a
//! fragment as opaque text — all engine-specific parsing lives in the engine
//! modules.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::config::config_dir;

/// Directory holding the custom force-field library (`~/.silicolab/force_fields`).
pub fn force_fields_dir() -> PathBuf {
    config_dir().join("force_fields")
}

/// Reduce a user-entered name to a safe, stable file stem: ASCII alphanumerics,
/// `-` and `_` are kept; runs of anything else collapse to a single `_`.
fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Names of the saved custom force fields, sorted. Reads the default library.
pub fn list_force_fields() -> Vec<String> {
    list_force_fields_in(&force_fields_dir())
}

/// Load a saved custom force field's `.itp` text from the default library.
pub fn load_force_field(name: &str) -> Result<String> {
    load_force_field_in(&force_fields_dir(), name)
}

/// Save (or overwrite) a custom force field in the default library.
pub fn save_force_field(name: &str, itp: &str) -> Result<()> {
    save_force_field_in(&force_fields_dir(), name, itp)
}

/// Delete a custom force field from the default library.
pub fn delete_force_field(name: &str) -> Result<()> {
    delete_force_field_in(&force_fields_dir(), name)
}

fn list_force_fields_in(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("itp")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names
}

fn load_force_field_in(dir: &Path, name: &str) -> Result<String> {
    let path = entry_path(dir, name)?;
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn save_force_field_in(dir: &Path, name: &str, itp: &str) -> Result<()> {
    let path = entry_path(dir, name)?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating force-field library {}", dir.display()))?;
    std::fs::write(&path, itp).with_context(|| format!("writing {}", path.display()))
}

fn delete_force_field_in(dir: &Path, name: &str) -> Result<()> {
    let path = entry_path(dir, name)?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
    }
    Ok(())
}

/// The `<dir>/<sanitized>.itp` path for a name, rejecting names that sanitize to
/// nothing (so a stray entry can never escape the library directory).
fn entry_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let stem = sanitize_name(name);
    if stem.is_empty() {
        bail!("a force-field name must contain letters or digits");
    }
    Ok(dir.join(format!("{stem}.itp")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("silicolab_ff_lib_{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_list_load_round_trips() {
        let dir = temp_dir("roundtrip");
        save_force_field_in(
            &dir,
            "MoS2 custom",
            "[ atomtypes ]\nMo 42 95.95 0 A 0.30 0.05\n",
        )
        .unwrap();
        // The space in the name is sanitized to an underscore for the file stem.
        let names = list_force_fields_in(&dir);
        assert_eq!(names, vec!["MoS2_custom".to_string()]);
        let itp = load_force_field_in(&dir, "MoS2_custom").unwrap();
        assert!(itp.contains("atomtypes"));
    }

    #[test]
    fn delete_removes_the_entry() {
        let dir = temp_dir("delete");
        save_force_field_in(&dir, "scratch", "x").unwrap();
        assert_eq!(list_force_fields_in(&dir).len(), 1);
        delete_force_field_in(&dir, "scratch").unwrap();
        assert!(list_force_fields_in(&dir).is_empty());
        // Deleting a missing entry is a no-op, not an error.
        delete_force_field_in(&dir, "scratch").unwrap();
    }

    #[test]
    fn empty_name_is_rejected() {
        let dir = temp_dir("empty");
        assert!(save_force_field_in(&dir, "   ", "x").is_err());
        assert!(entry_path(&dir, "!!!").is_err());
    }

    #[test]
    fn missing_library_lists_empty() {
        let dir = temp_dir("missing");
        assert!(list_force_fields_in(&dir).is_empty());
    }
}
