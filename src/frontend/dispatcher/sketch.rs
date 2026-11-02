//! Dispatcher handlers for the 2D molecule sketcher.
//!
//! The live canvas is transient UI state mutated directly each frame; these
//! handlers own only the boundary transitions: opening the sketcher, committing
//! the drawing into a new workspace entry (the single mutation that crosses
//! `AppAction → dispatch`), discarding it, and moving it to and from molfiles.

use std::path::Path;

use anyhow::{Context, Result};

use crate::frontend::SketcherState;
use crate::frontend::state::AppState;
use crate::io::{molfile, structure_io};
use crate::workflows::sketch_to_structure::sketch_to_structure;

use super::add_and_show_entry;

/// Open an empty sketcher.
pub(crate) fn sketch_molecule(state: &mut AppState) {
    state.ui.sketcher = Some(SketcherState::new());
    state.status_neutral("Sketching a new molecule".to_string());
}

/// Build the current sketch into a relaxed 3D structure and add it as a new,
/// active workspace entry. Mirrors `new_empty_entry`'s save → add → show path.
pub(crate) fn commit_sketch(state: &mut AppState) {
    let Some(sketcher) = state.ui.sketcher.take() else {
        return;
    };
    if sketcher.sketch.is_empty() {
        // Nothing to build — keep the sketcher open.
        state.ui.sketcher = Some(sketcher);
        return;
    }

    let title = {
        let trimmed = sketcher.title.trim();
        if trimmed.is_empty() {
            "Sketch".to_string()
        } else {
            trimmed.to_string()
        }
    };

    let structure = match sketch_to_structure(&sketcher.sketch, title) {
        Ok(structure) => structure,
        Err(error) => {
            state.status_error(format!("Sketch not built: {error}"));
            state.ui.sketcher = Some(sketcher);
            return;
        }
    };
    let atom_count = structure.atoms.len();
    let save_path = structure_io::default_structure_save_path(&structure, None);
    let entry_id = add_and_show_entry(state, structure, None, save_path);
    state.status_success(format!(
        "Built sketched molecule as entry #{entry_id} ({atom_count} atoms)"
    ));
}

/// Discard the sketch and close the sketcher.
pub(crate) fn cancel_sketch(state: &mut AppState) {
    state.ui.sketcher = None;
    state.status_neutral("Sketch canceled".to_string());
}

/// Ask for a molfile and load it into the open sketcher.
pub(crate) fn open_sketch_molfile(state: &mut AppState) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("MDL Molfile", &["mol"])
        .pick_file()
    else {
        return;
    };
    load_sketch_molfile(state, &path);
}

fn load_sketch_molfile(state: &mut AppState, path: &Path) {
    let Some(sketcher) = &mut state.ui.sketcher else {
        return;
    };
    let read = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))
        .and_then(|text| molfile::read_molfile(&text));
    match read {
        Ok(read) => {
            let warnings = read.warnings.join("; ");
            sketcher.load(read.sketch, read.title);
            if warnings.is_empty() {
                state.status_success(format!("Opened {}", path.display()));
            } else {
                state.status_warning(format!("Opened {}: {warnings}", path.display()));
            }
        }
        Err(error) => state.status_error(format!("Molfile not opened: {error:#}")),
    }
}

/// Ask for a path and write the open sketch there as a molfile.
pub(crate) fn save_sketch_molfile(state: &mut AppState) {
    let Some(sketcher) = &state.ui.sketcher else {
        return;
    };
    let name = sketcher.title.trim();
    let name = if name.is_empty() { "sketch" } else { name };
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(format!("{name}.mol"))
        .add_filter("MDL Molfile", &["mol"])
        .save_file()
    else {
        return;
    };
    match write_sketch_molfile(state, &path) {
        Ok(()) => state.status_success(format!("Saved {}", path.display())),
        Err(error) => state.status_error(format!("Molfile not saved: {error:#}")),
    }
}

fn write_sketch_molfile(state: &AppState, path: &Path) -> Result<()> {
    let Some(sketcher) = &state.ui.sketcher else {
        return Ok(());
    };
    let text = molfile::write_molfile(&sketcher.sketch, sketcher.title.trim())?;
    std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_sketch(smiles: &str) -> AppState {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let sketch = crate::domain::smiles::parse(smiles).unwrap();
        state.ui.sketcher = Some(SketcherState::with_sketch(sketch, "polymer"));
        state
    }

    #[test]
    fn polymer_fragment_stays_open_with_the_reason() {
        let mut state = state_with_sketch("[*]CC[*]");
        commit_sketch(&mut state);
        assert!(state.ui.sketcher.is_some());
        let notice = state.status_notice().unwrap();
        assert!(
            notice.text.contains("2 attachment point(s)"),
            "{}",
            notice.text
        );
    }

    #[test]
    fn molfile_save_and_open_keep_the_sketch() {
        let path =
            std::env::temp_dir().join(format!("silicolab-sketch-{}.mol", std::process::id()));
        let mut state = state_with_sketch("[*]CC[*]");
        write_sketch_molfile(&state, &path).unwrap();

        state.ui.sketcher = Some(SketcherState::new());
        load_sketch_molfile(&mut state, &path);
        std::fs::remove_file(&path).unwrap();

        let sketcher = state.ui.sketcher.as_ref().unwrap();
        assert_eq!(sketcher.sketch.attachment_count(), 2);
        assert_eq!(sketcher.title, "polymer");
    }

    #[test]
    fn bad_molfile_reports_an_error_and_keeps_the_drawing() {
        let mut state = state_with_sketch("CCO");
        load_sketch_molfile(&mut state, Path::new("/nonexistent/sketch.mol"));
        assert_eq!(state.ui.sketcher.as_ref().unwrap().sketch.atoms.len(), 3);
        let notice = state.status_notice().unwrap();
        assert!(notice.text.contains("could not read"), "{}", notice.text);
    }
}
