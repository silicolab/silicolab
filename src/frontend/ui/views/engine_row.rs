//! The editor for one engine's launch on one compute target.
//!
//! One widget serves this machine and every remote host, because "how do I launch
//! engine E on target T, and does it work" is one question. The status it shows is
//! derived from the launch on screen, never remembered separately: a verification
//! belongs to the launch it was taken against, and a failure is dropped the moment
//! the user edits the launch that caused it.

use eframe::egui::{self, Align, Layout, RichText};

use crate::backend::config::ComputeTarget;
use crate::engines::registry::{EngineId, EngineLaunch, EngineStatus};
use crate::frontend::actions::AppAction;
use crate::frontend::state::{EngineDraft, EngineProbeState};
use crate::frontend::theme::Palette;
use crate::frontend::ui::settings_registry::caption_text;

/// Everything the row needs about one engine on one target, gathered by the caller
/// before it takes a mutable borrow of the draft.
pub(crate) struct EngineRow<'a> {
    pub target: ComputeTarget,
    pub engine: EngineId,
    pub name: &'a str,
    pub description: &'a str,
    /// The launch as stored, with its verification. `None` when nothing is configured.
    pub status: EngineStatus,
    /// An in-flight or failed verification for this target and engine.
    pub probe: Option<EngineProbeState>,
    /// A local program can be picked from a file dialog; a remote path cannot.
    pub browsable: bool,
    pub auto_discovery: bool,
}

/// Render `row`'s status line, launch fields, and actions, editing `draft` in place.
pub(crate) fn engine_row(
    ui: &mut egui::Ui,
    row: EngineRow<'_>,
    draft: &mut EngineDraft,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let drafted = draft.to_launch();

    ui.horizontal(|ui| {
        ui.label(RichText::new(row.name).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            status_label(ui, &pal, &row, drafted.as_ref());
        });
    });
    ui.label(caption_text(row.description, pal.text_muted));

    ui.horizontal(|ui| {
        ui.label("Command prefix");
        ui.add(egui::TextEdit::singleline(&mut draft.command_prefix).desired_width(f32::INFINITY));
    });
    ui.label(caption_text(prefix_hint(&row.target), pal.text_tertiary));

    ui.horizontal(|ui| {
        ui.label("Program");
        // Reserve Browse on the right and let the field fill the gap: a left-to-right
        // singleline edit takes an infinite desired width and pushes Browse off the
        // clipped right edge.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if row.browsable && ui.button("Browse").clicked() {
                actions.push(AppAction::BrowseEngineProgram(row.engine));
            }
            ui.add(egui::TextEdit::singleline(&mut draft.program).desired_width(f32::INFINITY));
        });
    });
    ui.label(caption_text(
        if row.auto_discovery {
            "Leave empty to look for it on this target automatically."
        } else {
            "Required: specify the engine executable path."
        },
        pal.text_tertiary,
    ));

    let verifying = matches!(row.probe, Some(EngineProbeState::Verifying));
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!verifying, |ui| {
            if ui
                .button("Verify")
                .on_hover_text(verify_hint(drafted.is_some()))
                .clicked()
            {
                actions.push(AppAction::VerifyEngine {
                    target: row.target.clone(),
                    engine: row.engine,
                });
            }
        });
        if verifying {
            ui.spinner();
        }
        if row.status.launch().is_some()
            && crate::frontend::ui::widgets::confirm_destructive(
                ui,
                ("clear_engine_launch", row.engine.as_str()),
                "Clear this engine's launch?",
                "Clear",
                |ui| ui.button("Clear"),
            )
        {
            actions.push(AppAction::ClearEngineLaunch {
                target: row.target,
                engine: row.engine,
            });
        }
    });
}

/// The one place an engine's state becomes words. Everything here is derived from
/// the launch currently in the field, so an edited-but-unverified launch can never
/// wear the version — or the failure — of the launch it replaced.
fn status_label(
    ui: &mut egui::Ui,
    pal: &Palette,
    row: &EngineRow<'_>,
    drafted: Option<&EngineLaunch>,
) {
    use egui_phosphor::regular;

    match &row.probe {
        Some(EngineProbeState::Verifying) => {
            ui.label(caption_text("Verifying…", pal.text_muted));
            return;
        }
        // A failure describes the launch it was taken against. Once the user edits
        // that launch, the failure is about a binary no longer on screen.
        Some(probe @ EngineProbeState::Failed { reason, .. }) if probe.describes(drafted) => {
            ui.label(caption_text(
                format!("{}  Failed", regular::X_CIRCLE),
                pal.status_red,
            ))
            .on_hover_text(reason);
            return;
        }
        _ => {}
    }

    // Same rule for a success: a proof of the stored launch says nothing about the
    // one the user has typed over it.
    if drafted != row.status.launch() {
        ui.label(caption_text("Not verified — edited", pal.text_muted));
        return;
    }

    match &row.status {
        EngineStatus::Verified {
            version,
            checked_at,
            ..
        } => {
            ui.label(caption_text(
                format!("{}  {version}", regular::CHECK_CIRCLE),
                pal.status_green,
            ))
            .on_hover_text(format!(
                "Verified {}",
                super::engines::humanize_epoch(*checked_at)
            ));
        }
        EngineStatus::Unverified { .. } => {
            ui.label(caption_text("Not verified", pal.text_muted))
                .on_hover_text("A launch is configured, but it has never been run.");
        }
        EngineStatus::NotConfigured => {
            ui.label(caption_text("Not configured", pal.text_muted));
        }
        EngineStatus::BuiltIn { .. } => {}
    }
}

/// Why the failure of a verification differs by target, so the hint does too.
fn prefix_hint(target: &ComputeTarget) -> &'static str {
    match target {
        ComputeTarget::Local => {
            "e.g. `wsl.exe -e` to run inside WSL; leave blank for a native install"
        }
        ComputeTarget::Remote(_) => {
            "e.g. `apptainer exec gromacs.sif`; leave blank for a plain executable"
        }
    }
}

fn verify_hint(configured: bool) -> &'static str {
    if configured {
        "Run this program and confirm it is the engine"
    } else {
        "Look for the engine on this target and fill in what it finds"
    }
}
