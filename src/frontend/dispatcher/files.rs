use super::*;

pub(crate) fn open_file(state: &mut AppState) {
    let Some(path) = StructureService::open_dialog() else {
        return;
    };

    open_paths(state, [path]);
}

pub(crate) fn open_pdb_fetch_dialog(state: &mut AppState) {
    state.ui.pending_pdb_fetch.get_or_insert_with(String::new);
}

pub(crate) fn fetch_pdb(state: &mut AppState) {
    let Some(id) = state
        .ui
        .pending_pdb_fetch
        .as_ref()
        .map(|id| id.trim().to_string())
    else {
        return;
    };

    match pdb_fetch::fetch_pdb(
        &id,
        pdb_fetch::RCSB_DEFAULT_BASE_URL,
        &state.structures_dir(),
    ) {
        Ok(fetched) => {
            state.ui.pending_pdb_fetch = None;
            open_paths(state, [fetched.path]);
        }
        Err(error) => state.status_error(format!("Fetch failed: {error}")),
    }
}

pub fn open_paths(state: &mut AppState, paths: impl IntoIterator<Item = PathBuf>) {
    state.save_viewport_for_active_entry();
    let mut opened = Vec::<(u64, PathBuf)>::new();
    let mut failed = Vec::<String>::new();

    for path in paths {
        match load_document(&path) {
            Ok(document) => match import_document(&mut state.entries, document, path.clone()) {
                Some(entry_id) => opened.push((entry_id, path)),
                None => {
                    failed.push(format!("{}: no models found", path.display()));
                }
            },
            Err(error) => failed.push(format!("{}: {error}", path.display())),
        }
    }

    let Some((entry_id, last_path)) = opened.last() else {
        if let Some(error) = failed.first() {
            state.status_error(format!("Failed to open {error}"));
        }
        return;
    };

    state.ui.entry_list.selected_entry_ids.clear();
    state.ui.entry_list.selected_entry_ids.insert(*entry_id);
    load_active_entry(state);
    state.ui.selection.clear();
    state.status_success(format_open_results(opened.len(), failed.len(), last_path));
}

pub(crate) fn format_open_results(
    opened_count: usize,
    failed_count: usize,
    last_path: &std::path::Path,
) -> String {
    match (opened_count, failed_count) {
        (1, 0) => format!("Opened {}", last_path.display()),
        (_, 0) => format!("Opened {opened_count} files"),
        (1, 1) => format!("Opened {}; 1 file failed", last_path.display()),
        (1, _) => format!(
            "Opened {}; {failed_count} files failed",
            last_path.display()
        ),
        (_, 1) => format!("Opened {opened_count} files; 1 file failed"),
        _ => format!("Opened {opened_count} files; {failed_count} files failed"),
    }
}

pub(crate) fn edit_structure(state: &mut AppState) {
    if !require_active_entry(state, "Edit Structure") {
        return;
    }
    state.cancel_transient_jobs();
    state.ui.pending_optimization = None;
    state.edit_origin = Some(state.capture_edit_snapshot());
    state.ui.editor = Some(crate::frontend::StructureEditor::new(state.structure()));
}

pub(crate) fn apply_structure_edits(state: &mut AppState) {
    if let Some(editor) = &state.ui.editor {
        let draft = editor.draft.clone();
        let before = state
            .edit_origin
            .clone()
            .unwrap_or_else(|| state.capture_edit_snapshot());
        state.cancel_transient_jobs();
        state.ui.pending_optimization = None;
        *state.structure_mut() = draft;
        state.mark_structure_changed();
        state.set_source_path(None);
        state
            .ui
            .selection
            .retain_valid(state.structure().atoms.len());
        state.history.push_undo(before);
        state.edit_origin = None;
        state.ui.editor = None;
        state.status_success("Applied structure edits".to_string());
    }
}

pub(crate) fn cancel_structure_edits(state: &mut AppState) {
    if let Some(before) = state.edit_origin.take() {
        state.restore_edit_snapshot(before);
    } else if let Some(editor) = &state.ui.editor {
        *state.structure_mut() = editor.original.clone();
        state.mark_structure_changed();
        state.ui.editor = None;
    } else {
        return;
    }
    state.status_neutral("Edit canceled".to_string());
}
