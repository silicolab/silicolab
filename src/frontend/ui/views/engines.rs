use eframe::egui::{self, RichText};

use crate::backend::config::ComputeTarget;
use crate::engines::registry::{EngineStatus, external_engine_specs};
use crate::frontend::actions::AppAction;
use crate::frontend::state::{AppState, EngineDraft};
use crate::frontend::ui::settings_registry::caption_text;
use crate::frontend::ui::views::engine_row::{EngineRow, engine_row};
use crate::frontend::ui::widgets::status_pill;

/// Render an epoch-seconds timestamp as a coarse relative age ("just now", "5m
/// ago"). Avoids a date-formatting dependency; granularity is fine for "how stale
/// is this verification".
pub(crate) fn humanize_epoch(checked_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let secs = now.saturating_sub(checked_at);
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// This machine's external engines: one launch editor each, identical to the one a
/// remote host shows. Built-in engines are not here — they have no launch to point
/// at and nothing to verify (see [`render_builtin_engines`]).
pub(crate) fn render_engine_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // Resolving a launch is cheap (a config read, or a PATH lookup). Running one is
    // not, so nothing here spawns a subprocess: a version only ever appears because
    // the user asked for a verification, which runs on a worker thread.
    let Some(registry) = state.ui.settings.engine_registry.as_ref() else {
        actions.push(AppAction::RefreshEngineRegistry);
        return;
    };
    // Snapshot before the per-engine draft is borrowed mutably below.
    let statuses: Vec<EngineStatus> = external_engine_specs()
        .iter()
        .map(|spec| {
            registry
                .status(spec.id)
                .cloned()
                .unwrap_or(EngineStatus::NotConfigured)
        })
        .collect();

    for (spec, status) in external_engine_specs().iter().zip(statuses) {
        let probe = state
            .ui
            .settings
            .engine_probe
            .get(&(ComputeTarget::Local, spec.id.as_str()))
            .cloned();

        // Seed the draft from the configured (or auto-found) launch on first show.
        let key = spec.id.as_str().to_string();
        if !state.ui.settings.engine_drafts.contains_key(&key) {
            let seed = status
                .launch()
                .map(EngineDraft::from_launch)
                .unwrap_or_default();
            state.ui.settings.engine_drafts.insert(key.clone(), seed);
        }
        let Some(draft) = state.ui.settings.engine_drafts.get_mut(&key) else {
            continue;
        };

        engine_row(
            ui,
            EngineRow {
                target: ComputeTarget::Local,
                engine: spec.id,
                name: spec.name,
                description: spec.description,
                status,
                probe,
                browsable: true,
            },
            draft,
            actions,
        );
        ui.add_space(8.0);
    }
}

/// The engines compiled into SilicoLab. Read-only: there is nothing to configure
/// and nothing to verify, so they get a calm "Built-in" pill rather than the green
/// check that would otherwise mark most of this panel "OK" and rob the check of
/// meaning.
pub(crate) fn render_builtin_engines(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let Some(registry) = state.ui.settings.engine_registry.as_ref() else {
        actions.push(AppAction::RefreshEngineRegistry);
        return;
    };

    for cap in registry
        .capabilities()
        .iter()
        .filter(|cap| cap.status.built_in())
    {
        ui.horizontal(|ui| {
            ui.label(RichText::new(cap.name).strong());
            status_pill(ui, "Built-in", pal.blue_overlay(40), pal.accent);
            if let Some(version) = cap.status.version() {
                ui.label(caption_text(version.to_string(), pal.text_muted));
            }
        });
        ui.label(caption_text(cap.description, pal.text_muted));
        ui.add_space(6.0);
    }
}
