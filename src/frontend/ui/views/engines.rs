use eframe::egui::{self, Align, Layout, RichText};

use crate::engines::registry::{EngineId, EngineLaunch};
use crate::frontend::actions::AppAction;
use crate::frontend::state::{AppState, EngineDraft};
use crate::frontend::ui::settings_registry::caption_text;
use crate::frontend::ui::widgets::status_pill;

/// Owned snapshot of one engine capability, decoupled from the registry
/// borrow so we can freely mutate the per-engine drafts while rendering.
pub(crate) struct EngineRowView {
    id: EngineId,
    name: &'static str,
    description: &'static str,
    built_in: bool,
    available: bool,
    version: Option<String>,
    launch: Option<EngineLaunch>,
}

/// Render a `SystemTime` as a coarse relative age ("just now", "5m ago").
/// Avoids a date-formatting dependency; granularity is fine for "how stale is
/// this detection".
pub(crate) fn humanize_since(time: std::time::SystemTime) -> String {
    let Ok(elapsed) = time.elapsed() else {
        return "moments ago".to_string();
    };
    let secs = elapsed.as_secs();
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

pub(crate) fn render_engine_settings(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    // No section header here: the settings registry already wraps this editor in
    // the "Engines" group's collapsing header. Just the right-aligned Re-detect
    // action. The right-to-left layout must live inside a `horizontal` row: a
    // bare `with_layout` in a vertical ui claims the entire remaining pane
    // height, leaving the rest of the editor below a huge blank band.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Re-detect")
                .on_hover_text("Run each engine's --version (can be slow for WSL)")
                .clicked()
            {
                actions.push(AppAction::DetectEngineVersions);
            }
        });
    });

    // Availability is resolved lazily and cheaply (no subprocess). Version
    // strings are NOT probed here — that spawns `--version` per engine and a
    // WSL launch cold-starts the VM, which made first open slow. Versions are
    // detected only on explicit "Re-detect" / "Apply & Detect".
    let Some(registry) = state.ui.settings.engine_registry.as_ref() else {
        actions.push(AppAction::RefreshEngineRegistry);
        return;
    };

    let versions_caption = match state.ui.settings.engine_versions_checked_at {
        Some(checked_at) => format!("Versions last checked {}", humanize_since(checked_at)),
        None => "Versions not checked yet — click Re-detect".to_string(),
    };
    let pal = crate::frontend::theme::palette(ui);
    ui.label(caption_text(versions_caption, pal.text_muted));

    let rows: Vec<EngineRowView> = registry
        .capabilities()
        .iter()
        .map(|cap| EngineRowView {
            id: cap.id,
            name: cap.name,
            description: cap.description,
            built_in: cap.built_in,
            available: cap.available(),
            version: cap.version.clone(),
            launch: cap.launch.clone(),
        })
        .collect();

    for row in rows {
        // The name carries no status color; a trailing tag communicates the
        // engine's *type* (built-in) or, for external engines, its detection
        // status. Built-ins are always ready, so they get a calm accent-tinted
        // "Built-in" pill instead of the green check that would otherwise mark
        // every row "OK" and rob the check of meaning. The check/cross is
        // reserved for external engines, where availability actually varies.
        ui.horizontal(|ui| {
            ui.label(RichText::new(row.name).strong());
            if row.built_in {
                status_pill(ui, "Built-in", pal.blue_overlay(40), pal.accent);
            } else if row.available {
                ui.label(caption_text(
                    format!("{}  Detected", egui_phosphor::regular::CHECK_CIRCLE),
                    pal.status_green,
                ));
            } else {
                ui.label(caption_text(
                    format!("{}  Not found", egui_phosphor::regular::X_CIRCLE),
                    pal.text_muted,
                ));
            }
        });
        if let Some(version) = &row.version {
            ui.label(caption_text(format!("version {version}"), pal.text_muted));
        }
        ui.label(caption_text(row.description, pal.text_muted));

        if row.built_in {
            ui.add_space(6.0);
            continue;
        }

        // Seed the editable draft once, preferring an explicit override, then
        // the auto-detected launch, then empty.
        let key = row.id.as_str().to_string();
        if !state.ui.settings.engine_drafts.contains_key(&key) {
            let seed = state
                .config
                .engine_overrides
                .get(row.id)
                .map(EngineDraft::from_launch)
                .or_else(|| row.launch.as_ref().map(EngineDraft::from_launch))
                .unwrap_or_default();
            state.ui.settings.engine_drafts.insert(key.clone(), seed);
        }
        let draft = state
            .ui
            .settings
            .engine_drafts
            .get_mut(&key)
            .expect("draft seeded above");

        ui.horizontal(|ui| {
            ui.label("Command prefix:");
            ui.add(
                egui::TextEdit::singleline(&mut draft.command_prefix).desired_width(f32::INFINITY),
            );
        });
        ui.label(caption_text(
            "e.g. `wsl.exe -e` to run inside WSL; leave blank for a native install",
            pal.text_muted,
        ));
        ui.horizontal(|ui| {
            ui.label("Program:");
            // Reserve the Browse button on the right and let the text field fill
            // the space between it and the label. A plain left-to-right layout
            // gives the singleline edit an infinite desired width, which eats the
            // whole row and pushes Browse off the (clipped) right edge.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Browse").clicked() {
                    actions.push(AppAction::BrowseEngineProgram(row.id));
                }
                ui.add(egui::TextEdit::singleline(&mut draft.program).desired_width(f32::INFINITY));
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Apply & Detect").clicked() {
                actions.push(AppAction::ApplyEngineOverride(row.id));
            }
            if crate::frontend::ui::widgets::confirm_destructive(
                ui,
                ("clear_engine_override", row.id.as_str()),
                "Clear this engine override?",
                "Clear",
                |ui| ui.button("Clear"),
            ) {
                actions.push(AppAction::ClearEngineOverride(row.id));
            }
        });
        ui.add_space(8.0);
    }
}
