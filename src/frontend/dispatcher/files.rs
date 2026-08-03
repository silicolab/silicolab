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

pub(crate) fn open_online_structure_search(state: &mut AppState) {
    state
        .ui
        .online_structure_search
        .get_or_insert_with(crate::frontend::state::OnlineStructureSearchState::default);
}

pub(crate) fn start_online_structure_search(state: &mut AppState) {
    let Some(panel) = state.ui.online_structure_search.as_mut() else {
        return;
    };
    let mut query = crate::io::online_structures::StructureQuery::new(&panel.query, panel.kind);
    query.include_disorder = panel.include_disorder;
    query.limit = panel.limit;
    if let Err(error) = query.validate() {
        panel.error = Some(error.to_string());
        return;
    }
    panel.generation = panel.generation.wrapping_add(1);
    panel.phase = crate::frontend::state::OnlineStructurePhase::Searching;
    panel.result = None;
    panel.error = None;
    panel.selected = None;
    state.jobs.online_structure = Some(crate::frontend::jobs::spawn_online_structure_search(
        query,
        panel.generation,
    ));
    state.status_neutral("Searching PubChem and COD...".to_string());
}

pub(crate) fn import_online_crystal(state: &mut AppState, index: usize) {
    let structures_dir = state.structures_dir();
    let Some(panel) = state.ui.online_structure_search.as_mut() else {
        return;
    };
    let Some(result) = panel.result.as_ref() else {
        return;
    };
    let Some(candidate) = result.crystals.get(index).cloned() else {
        panel.error = Some("the selected COD result is no longer available".to_string());
        return;
    };
    if !candidate.is_importable() {
        panel.error = Some(
            "this COD record is marked as disordered and cannot currently be imported".to_string(),
        );
        return;
    }
    let query = result.query.clone();
    panel.generation = panel.generation.wrapping_add(1);
    panel.phase = crate::frontend::state::OnlineStructurePhase::Importing;
    panel.error = None;
    state.jobs.online_structure = Some(crate::frontend::jobs::spawn_cod_fetch(
        query,
        candidate,
        structures_dir,
        panel.generation,
    ));
    state.status_neutral("Downloading and validating the COD crystal...".to_string());
}

pub(crate) fn build_resolved_online_molecule(state: &mut AppState) {
    let Some((query, compound)) = state
        .ui
        .online_structure_search
        .as_ref()
        .and_then(|panel| panel.result.as_ref())
        .and_then(|result| {
            result
                .resolved
                .as_ref()
                .map(|compound| (result.query.clone(), compound.clone()))
        })
    else {
        return;
    };
    match crate::workflows::sketch_to_structure::smiles_to_structure(
        &compound.smiles,
        Some(&compound.title),
    ) {
        Ok(structure) => {
            let save_path = structure_io::default_structure_save_path(&structure, None);
            let entry_id = add_and_show_entry(state, structure, None, save_path);
            state.entries.set_entry_origin(
                entry_id,
                EntryOrigin::Online {
                    source: Box::new(
                        crate::io::online_structures::OnlineStructureSource::from_pubchem(
                            &query, &compound,
                        ),
                    ),
                },
            );
            state.status_success(format!(
                "Built PubChem CID {} as a non-periodic molecule; inspect it before calculating",
                compound.cid
            ));
        }
        Err(error) => state.status_error(format!("Could not build the PubChem molecule: {error}")),
    }
}

pub(crate) fn close_online_structure_search(state: &mut AppState) {
    state.ui.online_structure_search = None;
    state.jobs.online_structure = None;
}

pub(crate) fn show_online_structure_source(state: &mut AppState, entry_id: u64) {
    let Some(source) = state
        .entries
        .entry(entry_id)
        .and_then(|entry| entry.origin.online_source())
    else {
        return;
    };
    let provider = match source.provider {
        crate::io::online_structures::OnlineProvider::Cod => "COD",
        crate::io::online_structures::OnlineProvider::Pubchem => "PubChem",
    };
    let mut lines = vec![
        format!("Provider: {provider}"),
        format!("Record: {}", source.record_id),
        format!("Query: {}", source.query),
        format!("Source: {}", source.source_url),
    ];
    for (label, value) in [
        ("Revision", source.revision.as_deref()),
        ("Formula", source.formula.as_deref()),
        ("SMILES", source.smiles.as_deref()),
        ("Temperature (K)", source.temperature_k.as_deref()),
        ("Space group", source.space_group.as_deref()),
        ("R factor", source.r_factor.as_deref()),
        ("DOI", source.doi.as_deref()),
    ] {
        if let Some(value) = value {
            lines.push(format!("{label}: {value}"));
        }
    }
    if !source.flags.is_empty() {
        lines.push(format!("Flags: {}", source.flags.join(", ")));
    }
    state.ui.text_viewer = Some(crate::frontend::state::TextViewer {
        title: format!("{provider} source"),
        text: lines.join("\n"),
    });
}

pub(crate) fn poll_online_structure_job(state: &mut AppState, ctx: &egui::Context) {
    let Some(job) = state.jobs.online_structure.take() else {
        return;
    };
    match job.receiver.try_recv() {
        Ok(crate::frontend::jobs::OnlineStructureJobOutcome::Search(outcome)) => {
            let Some(panel) = state.ui.online_structure_search.as_mut() else {
                return;
            };
            if panel.generation != job.generation {
                return;
            }
            panel.phase = crate::frontend::state::OnlineStructurePhase::Idle;
            match outcome {
                Ok(result) => {
                    let count = result.crystals.len();
                    panel.result = Some(result);
                    panel.selected = (count > 0).then_some(0);
                    state.status_success(format!("Found {count} compatible COD crystals"));
                }
                Err(error) => {
                    panel.error = Some(error.to_string());
                    state.status_error(format!("Online structure search failed: {error}"));
                }
            }
        }
        Ok(crate::frontend::jobs::OnlineStructureJobOutcome::Fetch(outcome)) => {
            if state
                .ui
                .online_structure_search
                .as_ref()
                .is_none_or(|panel| panel.generation != job.generation)
            {
                return;
            }
            if let Some(panel) = state.ui.online_structure_search.as_mut() {
                panel.phase = crate::frontend::state::OnlineStructurePhase::Idle;
            }
            match outcome {
                Ok(fetched) => {
                    let fetched = *fetched;
                    let cod_id = fetched.source.record_id.clone();
                    let save_path = structure_io::default_structure_save_path(
                        &fetched.structure,
                        Some(&fetched.path),
                    );
                    let entry_id =
                        add_and_show_entry(state, fetched.structure, Some(fetched.path), save_path);
                    state.entries.set_entry_origin(
                        entry_id,
                        EntryOrigin::Online {
                            source: Box::new(fetched.source),
                        },
                    );
                    state.status_success(format!(
                        "Imported COD {cod_id}; inspect the crystal before starting a calculation"
                    ));
                }
                Err(error) => {
                    if let Some(panel) = state.ui.online_structure_search.as_mut() {
                        panel.error = Some(error.to_string());
                    }
                    state.status_error(format!("COD import failed: {error}"));
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            state.jobs.online_structure = Some(job);
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            if let Some(panel) = state.ui.online_structure_search.as_mut() {
                panel.phase = crate::frontend::state::OnlineStructurePhase::Idle;
                panel.error = Some("online structure worker stopped unexpectedly".to_string());
            }
        }
    }
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
