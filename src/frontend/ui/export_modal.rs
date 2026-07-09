use eframe::egui::{self, Button, RadioButton};

use crate::frontend::{
    actions::AppAction,
    state::{AppState, ExportLayout, ExportPrompt, ExportScope},
};
use crate::io::{structure_export::plan_file_stems, structure_io::writable_formats};

/// Filenames listed before the preview collapses into a count.
const PREVIEW_LIMIT: usize = 3;

pub(crate) fn render_export_window(
    state: &mut AppState,
    actions: &mut Vec<AppAction>,
    ctx: &egui::Context,
) {
    let Some(mut prompt) = state.ui.pending_export.clone() else {
        return;
    };

    let scopes = [
        (
            ExportScope::Selected,
            "Selected",
            names_for_scope(state, &prompt, ExportScope::Selected),
        ),
        (
            ExportScope::Active,
            "Active structure",
            names_for_scope(state, &prompt, ExportScope::Active),
        ),
        (
            ExportScope::All,
            "All in project",
            names_for_scope(state, &prompt, ExportScope::All),
        ),
    ];

    let mut run = false;
    let mut cancel = false;
    let mut open = true;
    egui::Window::new("Export Structures")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Scope");
            for (scope, label, names) in &scopes {
                let text = format!("{label} ({})", names.len());
                if ui
                    .add_enabled(
                        !names.is_empty(),
                        RadioButton::new(prompt.scope == *scope, text),
                    )
                    .clicked()
                {
                    prompt.scope = *scope;
                }
            }

            let names = scopes
                .iter()
                .find(|(scope, _, _)| *scope == prompt.scope)
                .map(|(_, _, names)| names.as_slice())
                .unwrap_or_default();

            ui.add_space(8.0);
            egui::ComboBox::from_label("Format")
                .selected_text(prompt.format.label())
                .show_ui(ui, |ui| {
                    for format in writable_formats() {
                        ui.selectable_value(&mut prompt.format, *format, format.label());
                    }
                });

            if names.len() > 1 {
                ui.add_space(8.0);
                ui.label("Layout");
                render_layout_choice(ui, &mut prompt, names.len());
            }

            ui.add_space(8.0);
            render_preview(ui, &prompt, names);

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!names.is_empty(), Button::new("Export"))
                    .clicked()
                {
                    run = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    state.ui.pending_export = Some(prompt);
    if run {
        actions.push(AppAction::RunExport);
    } else if cancel || !open {
        actions.push(AppAction::CancelExport);
    }
}

/// The combine choice stays visible but disabled for a format that cannot hold
/// several structures, with the reason on hover: a user who is told why does not
/// go and concatenate the files by hand.
fn render_layout_choice(ui: &mut egui::Ui, prompt: &mut ExportPrompt, count: usize) {
    let can_combine = prompt.can_combine(count);
    let combine = ui
        .add_enabled(
            can_combine,
            RadioButton::new(
                prompt.effective_layout(count) == ExportLayout::SingleFile,
                "Combine into one file",
            ),
        )
        .on_disabled_hover_text(
            prompt
                .format
                .single_structure_reason()
                .unwrap_or("This format holds one structure per file."),
        );
    if combine.clicked() {
        prompt.layout = ExportLayout::SingleFile;
    }
    if ui
        .add(RadioButton::new(
            prompt.effective_layout(count) == ExportLayout::FilePerStructure,
            "One file per structure",
        ))
        .clicked()
    {
        prompt.layout = ExportLayout::FilePerStructure;
    }
}

fn render_preview(ui: &mut egui::Ui, prompt: &ExportPrompt, names: &[String]) {
    if names.is_empty() {
        ui.label("Nothing to export.");
        return;
    }

    if prompt.effective_layout(names.len()) == ExportLayout::SingleFile {
        ui.label(match names.len() {
            1 => "Writes one file.".to_string(),
            count => format!("Writes {count} structures into one file."),
        });
        return;
    }

    ui.label("Writes into a folder you choose:");
    let stems = plan_file_stems(&names.iter().map(String::as_str).collect::<Vec<_>>());
    for stem in stems.iter().take(PREVIEW_LIMIT) {
        ui.monospace(format!("{stem}.{}", prompt.format.extension()));
    }
    if let Some(remaining) = stems.len().checked_sub(PREVIEW_LIMIT).filter(|n| *n > 0) {
        ui.label(format!("…and {remaining} more"));
    }
}

fn names_for_scope(state: &AppState, prompt: &ExportPrompt, scope: ExportScope) -> Vec<String> {
    crate::frontend::dispatcher::entry_ids_for_scope(state, scope, &prompt.selected_entry_ids)
        .iter()
        .filter_map(|entry_id| state.entries.entry(*entry_id))
        .map(|entry| entry.name.clone())
        .collect()
}
