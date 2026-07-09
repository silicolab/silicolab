use super::*;

use crate::{
    frontend::state::{ExportLayout, ExportPrompt, ExportScope},
    io::{structure_export, structure_format::StructureFormat},
};

pub(crate) fn open_export_dialog(state: &mut AppState, entry_id: Option<u64>) {
    if state.entries.records.is_empty() {
        state.set_message("Nothing to export".to_string());
        return;
    }

    let selected = scope_entry_ids(state, entry_id);
    let scope = if !selected.is_empty() {
        ExportScope::Selected
    } else if state.has_active_entry() {
        ExportScope::Active
    } else {
        ExportScope::All
    };
    open_prompt(state, scope, selected);
}

pub(crate) fn cancel_export(state: &mut AppState) {
    state.ui.pending_export = None;
}

pub(crate) fn run_export(state: &mut AppState) {
    let Some(prompt) = state.ui.pending_export.take() else {
        return;
    };
    let ids = hydrated_entry_ids(
        state,
        &entry_ids_for_scope(state, prompt.scope, &prompt.selected_entry_ids),
    );
    if ids.is_empty() {
        state.set_message("Nothing to export".to_string());
        return;
    }

    match prompt.effective_layout(ids.len()) {
        ExportLayout::SingleFile => export_to_file(state, &ids, prompt.format),
        ExportLayout::FilePerStructure => export_to_directory(state, &ids, prompt.format),
    }
}

fn open_prompt(state: &mut AppState, scope: ExportScope, selected: Vec<u64>) {
    let format = structure_io::preferred_save_format(state.structure(), Some(state.save_path()));
    state.ui.pending_export = Some(ExportPrompt::new(scope, format, selected));
}

/// Entries the sidebar selection covers, in project order. A selected group
/// stands for every entry inside it.
pub(crate) fn selection_entry_ids(state: &AppState) -> Vec<u64> {
    let list = &state.ui.entry_list;
    state
        .entries
        .records
        .iter()
        .filter(|entry| {
            list.selected_entry_ids.contains(&entry.id)
                || list.selected_group_ids.contains(&entry.group_id)
        })
        .map(|entry| entry.id)
        .collect()
}

fn scope_entry_ids(state: &AppState, clicked: Option<u64>) -> Vec<u64> {
    let selected = selection_entry_ids(state);
    match clicked {
        Some(entry_id) if !selected.contains(&entry_id) => vec![entry_id],
        _ => selected,
    }
}

pub(crate) fn entry_ids_for_scope(
    state: &AppState,
    scope: ExportScope,
    selected: &[u64],
) -> Vec<u64> {
    match scope {
        ExportScope::Selected => selected.to_vec(),
        ExportScope::Active => state.entries.active_entry_id().into_iter().collect(),
        ExportScope::All => state.entries.records.iter().map(|entry| entry.id).collect(),
    }
}

/// Materialize the geometry of any lazily-loaded entry and drop the ones that
/// still hold a placeholder — exporting those would write empty files.
pub(crate) fn hydrated_entry_ids(state: &mut AppState, ids: &[u64]) -> Vec<u64> {
    for &entry_id in ids {
        state.ensure_entry_loaded(entry_id);
    }
    ids.iter()
        .copied()
        .filter(|entry_id| {
            state
                .entries
                .entry(*entry_id)
                .is_some_and(|entry| entry.loaded)
        })
        .collect()
}

/// Write `ids` into one file. Shared by the dialog and the `export` console
/// command, so both agree on hydration and naming.
pub(crate) fn write_ids_to_file(
    state: &mut AppState,
    ids: &[u64],
    path: &Path,
) -> anyhow::Result<()> {
    let ids = hydrated_entry_ids(state, ids);
    let structures = structures_for(state, &ids);
    structure_export::write_structures_to_file(&structures, path)
}

/// Write one file per entry into `directory`, reporting the planned path and the
/// outcome of each. A structure a format cannot represent must not discard the
/// ones that wrote fine.
pub(crate) fn write_ids_to_directory(
    state: &mut AppState,
    ids: &[u64],
    directory: &Path,
    format: StructureFormat,
) -> (Vec<PathBuf>, Vec<anyhow::Result<()>>) {
    let ids = hydrated_entry_ids(state, ids);
    let paths = plan_paths(state, &ids, directory, format);
    let results = {
        let structures = structures_for(state, &ids);
        structure_export::write_each(&structures, &paths)
    };
    for ((entry_id, path), result) in ids.iter().zip(&paths).zip(&results) {
        if result.is_ok() {
            remember_export_target(state, *entry_id, path.clone());
        }
    }
    (paths, results)
}

pub(crate) fn plan_paths(
    state: &AppState,
    ids: &[u64],
    directory: &Path,
    format: StructureFormat,
) -> Vec<PathBuf> {
    let names = entry_names(state, ids);
    let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    structure_export::plan_export_paths(&name_refs, directory, format)
}

fn structures_for<'a>(state: &'a AppState, ids: &[u64]) -> Vec<&'a Structure> {
    ids.iter()
        .filter_map(|entry_id| state.entries.entry(*entry_id))
        .map(|entry| &entry.structure)
        .collect()
}

fn entry_names(state: &AppState, ids: &[u64]) -> Vec<String> {
    ids.iter()
        .filter_map(|entry_id| state.entries.entry(*entry_id))
        .map(|entry| entry.name.clone())
        .collect()
}

fn export_to_file(state: &mut AppState, ids: &[u64], format: StructureFormat) {
    let mut dialog = rfd::FileDialog::new()
        .set_file_name(format!(
            "{}.{}",
            combined_file_stem(state, ids),
            format.extension()
        ))
        .add_filter(format.label(), &[format.extension()]);
    // Re-exporting a structure should land where it landed last time.
    if let Some(parent) = previous_export_directory(state, ids) {
        dialog = dialog.set_directory(parent);
    }

    let Some(path) = dialog
        .save_file()
        .map(|path| structure_io::path_with_format_extension(&path, format))
    else {
        state.set_message("Export canceled".to_string());
        return;
    };

    match write_ids_to_file(state, ids, &path) {
        Ok(()) => {
            // Only an unambiguous one-structure file becomes that entry's
            // remembered target; a combined file belongs to no single entry.
            if let [entry_id] = ids {
                remember_export_target(state, *entry_id, path.clone());
            }
            state.set_message(format!(
                "Exported {} to {}",
                structure_count(ids.len()),
                path.display()
            ));
        }
        Err(error) => state.set_message(format!("Export failed: {error}")),
    }
}

fn export_to_directory(state: &mut AppState, ids: &[u64], format: StructureFormat) {
    let Some(directory) = rfd::FileDialog::new().pick_folder() else {
        state.set_message("Export canceled".to_string());
        return;
    };

    let existing = plan_paths(state, ids, &directory, format)
        .iter()
        .filter(|path| path.exists())
        .count();
    if existing > 0 && !confirm_overwrite(existing) {
        state.set_message("Export canceled".to_string());
        return;
    }

    let (paths, results) = write_ids_to_directory(state, ids, &directory, format);
    state.set_message(directory_export_summary(&directory, &paths, &results));
}

pub(crate) fn directory_export_summary(
    directory: &Path,
    paths: &[PathBuf],
    results: &[anyhow::Result<()>],
) -> String {
    let failures = results
        .iter()
        .zip(paths)
        .filter_map(|(result, path)| {
            result.as_ref().err().map(|error| {
                let name = path.file_name().unwrap_or(path.as_os_str());
                format!("{}: {error}", name.to_string_lossy())
            })
        })
        .collect::<Vec<_>>();

    let mut message = format!(
        "Exported {} to {}",
        structure_count(results.len() - failures.len()),
        directory.display()
    );
    if let Some(first) = failures.first() {
        message.push_str(&format!("; {} failed ({first})", failures.len()));
    }
    message
}

fn remember_export_target(state: &mut AppState, entry_id: u64, path: PathBuf) {
    if let Some(entry) = state.entries.entry_mut(entry_id) {
        entry.save_path = path;
    }
}

/// The folder a single entry was last exported into, so the dialog reopens
/// there. Only an absolute remembered path counts as a place the user chose; the
/// relative default (`edited.xyz`) means the entry has never been exported.
fn previous_export_directory(state: &AppState, ids: &[u64]) -> Option<PathBuf> {
    let [entry_id] = ids else {
        return None;
    };
    let path = &state.entries.entry(*entry_id)?.save_path;
    path.is_absolute()
        .then(|| path.parent().map(Path::to_path_buf))
        .flatten()
}

pub(crate) fn writable_format_of(path: &Path) -> Option<StructureFormat> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(StructureFormat::from_extension)
        .filter(|format| format.supports_write())
}

fn combined_file_stem(state: &AppState, ids: &[u64]) -> String {
    match ids {
        [entry_id] => state
            .entries
            .entry(*entry_id)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| "structure".to_string()),
        _ => "structures".to_string(),
    }
}

pub(crate) fn structure_count(count: usize) -> String {
    match count {
        1 => "1 structure".to_string(),
        _ => format!("{count} structures"),
    }
}

fn confirm_overwrite(count: usize) -> bool {
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Overwrite existing files?")
        .set_description(format!(
            "{} already exist in this folder and will be replaced.",
            structure_count(count).replace("structure", "file")
        ))
        .set_buttons(rfd::MessageButtons::OkCancel)
        .show();
    matches!(answer, rfd::MessageDialogResult::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Atom;
    use nalgebra::Point3;

    fn carbon(title: &str) -> Structure {
        Structure::new(
            title,
            vec![Atom {
                element: "C".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            }],
        )
    }

    /// Three entries; the second and third share the group `"pair"`.
    fn state_with_entries() -> AppState {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        for name in ["one", "two", "three"] {
            state
                .entries
                .add_entry(carbon(name), None, PathBuf::from(format!("{name}.xyz")));
        }
        for entry in state.entries.records.iter_mut().skip(1) {
            entry.group_id = "pair".to_string();
        }
        state
    }

    fn ids(state: &AppState) -> Vec<u64> {
        state.entries.records.iter().map(|entry| entry.id).collect()
    }

    #[test]
    fn selection_covers_entries_and_whole_groups() {
        let mut state = state_with_entries();
        let all = ids(&state);
        state.ui.entry_list.selected_entry_ids.insert(all[0]);
        state
            .ui
            .entry_list
            .selected_group_ids
            .insert("pair".to_string());

        assert_eq!(selection_entry_ids(&state), all);
    }

    #[test]
    fn right_click_inside_the_selection_keeps_it() {
        let mut state = state_with_entries();
        let all = ids(&state);
        state.ui.entry_list.selected_entry_ids.insert(all[0]);
        state.ui.entry_list.selected_entry_ids.insert(all[1]);

        assert_eq!(scope_entry_ids(&state, Some(all[1])), vec![all[0], all[1]]);
    }

    /// Right-clicking an unselected entry exports just it, as a file manager
    /// would — the selection must not silently widen the write.
    #[test]
    fn right_click_outside_the_selection_narrows_to_that_entry() {
        let mut state = state_with_entries();
        let all = ids(&state);
        state.ui.entry_list.selected_entry_ids.insert(all[0]);

        assert_eq!(scope_entry_ids(&state, Some(all[2])), vec![all[2]]);
    }

    #[test]
    fn dialog_defaults_to_the_sidebar_selection() {
        let mut state = state_with_entries();
        let all = ids(&state);
        state.ui.entry_list.selected_entry_ids.insert(all[1]);

        open_export_dialog(&mut state, None);

        let prompt = state.ui.pending_export.expect("dialog opened");
        assert_eq!(prompt.scope, ExportScope::Selected);
        assert_eq!(prompt.selected_entry_ids, vec![all[1]]);
    }

    #[test]
    fn dialog_falls_back_to_the_active_entry_without_a_selection() {
        let mut state = state_with_entries();

        open_export_dialog(&mut state, None);

        assert_eq!(
            state.ui.pending_export.expect("dialog opened").scope,
            ExportScope::Active
        );
    }

    #[test]
    fn scope_all_covers_every_entry_in_project_order() {
        let state = state_with_entries();

        assert_eq!(
            entry_ids_for_scope(&state, ExportScope::All, &[]),
            ids(&state)
        );
    }

    /// A lazily-loaded entry holds a placeholder structure. If hydration cannot
    /// fill it, exporting it would write an empty file, so it is dropped.
    #[test]
    fn unhydratable_entries_are_dropped_rather_than_exported_empty() {
        let mut state = state_with_entries();
        let all = ids(&state);
        state.entries.entry_mut(all[1]).unwrap().loaded = false;

        assert_eq!(hydrated_entry_ids(&mut state, &all), vec![all[0], all[2]]);
    }

    #[test]
    fn a_relative_save_path_is_not_a_remembered_export_directory() {
        let state = state_with_entries();
        let all = ids(&state);

        assert!(previous_export_directory(&state, &[all[0]]).is_none());
    }

    #[test]
    fn several_entries_never_reuse_one_remembered_directory() {
        let state = state_with_entries();

        assert!(previous_export_directory(&state, &ids(&state)).is_none());
    }
}
