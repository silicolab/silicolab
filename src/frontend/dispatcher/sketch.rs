//! Dispatcher handlers for the 2D molecule sketcher.
//!
//! The live canvas is transient UI state mutated directly each frame; these
//! handlers own only the boundary transitions: opening the sketcher, committing
//! the drawing into a new workspace entry (the single mutation that crosses
//! `AppAction → dispatch`), and discarding it.

use crate::frontend::SketcherState;
use crate::frontend::state::AppState;
use crate::io::structure_io;
use crate::workflows::sketch_to_structure::sketch_to_structure;

use super::add_and_show_entry;

/// Open an empty sketcher.
pub(crate) fn sketch_molecule(state: &mut AppState) {
    state.ui.sketcher = Some(SketcherState::new());
    state.set_message("Sketching a new molecule".to_string());
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

    let structure = sketch_to_structure(&sketcher.sketch, title);
    let atom_count = structure.atoms.len();
    let save_path = structure_io::default_structure_save_path(&structure, None);
    let entry_id = add_and_show_entry(state, structure, None, save_path);
    state.set_message(format!(
        "Built sketched molecule as entry #{entry_id} ({atom_count} atoms)"
    ));
}

/// Discard the sketch and close the sketcher.
pub(crate) fn cancel_sketch(state: &mut AppState) {
    state.ui.sketcher = None;
    state.set_message("Sketch canceled".to_string());
}
