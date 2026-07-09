//! Confirmation shown before the user leaves with unsaved project or Scratch
//! data. The dialog only emits actions; the dispatcher owns all persistence and
//! workspace transitions.

use eframe::egui::{self, Align, Button, Color32, Layout, RichText};

use crate::frontend::{actions::AppAction, state::AppState};

const WINDOW_WIDTH: f32 = 460.0;
const WINDOW_HEIGHT_SINGLE_LINE: f32 = 112.0;
const WINDOW_HEIGHT_WRAPPED: f32 = 140.0;
const BUTTON_ROW_HEIGHT: f32 = 28.0;

pub fn show(state: &mut AppState, ctx: &egui::Context, actions: &mut Vec<AppAction>) {
    let Some(confirmation) = state.ui.leave_confirmation.as_ref() else {
        return;
    };

    let intent = confirmation.intent.clone();
    let save_error = confirmation.save_error.clone();
    let scratch = state.scratch_has_unsaved_content();
    let has_drafts = state.has_unsaved_workspace_drafts();
    let workspace = state.workspace_label();

    super::modal::render_backdrop(state, ctx, "leave_confirm_backdrop");

    let has_save_error = save_error.is_some();
    let title = if has_save_error {
        "Could not save project"
    } else if scratch {
        "Save Scratch before leaving?"
    } else {
        "Save changes before leaving?"
    };

    let body = if let Some(error) = save_error.as_ref() {
        format!("SilicoLab could not save {workspace}. {error}")
    } else if scratch {
        format!(
            "Scratch is not stored after SilicoLab closes. Create a project to keep this workspace before you {}.",
            intent.action_label()
        )
    } else if has_drafts {
        format!(
            "Save {workspace} before you {}. Open editors or task drafts that were not applied will be discarded.",
            intent.action_label()
        )
    } else {
        format!(
            "Save the latest changes to {workspace} before you {}.",
            intent.action_label()
        )
    };
    let window_height = if has_save_error || scratch || has_drafts || body.chars().count() > 78 {
        WINDOW_HEIGHT_WRAPPED
    } else {
        WINDOW_HEIGHT_SINGLE_LINE
    };

    let mut cancel = false;

    egui::Window::new("leave_confirm")
        .id(egui::Id::new("leave_confirm"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .order(egui::Order::Foreground)
        .frame(super::modal::window_frame(ctx, egui::Margin::same(18)))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .fixed_size([WINDOW_WIDTH, window_height])
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            let pal = crate::frontend::theme::palette(ui);

            ui.horizontal(|ui| {
                ui.heading(title);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(RichText::new(egui_phosphor::regular::X))
                        .on_hover_text("Cancel")
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });

            ui.add_space(8.0);
            ui.add(
                egui::Label::new(RichText::new(body).color(pal.text_muted))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(18.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), BUTTON_ROW_HEIGHT),
                Layout::right_to_left(Align::Center),
                |ui| {
                    let primary_label = if scratch {
                        "Create Project"
                    } else {
                        intent.save_button_label()
                    };
                    if ui
                        .add(
                            Button::new(RichText::new(primary_label).color(Color32::WHITE))
                                .fill(pal.status_blue),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::SaveAndLeave);
                    }

                    let discard_label = if scratch { "Discard" } else { "Don't Save" };
                    if ui
                        .add(
                            Button::new(RichText::new(discard_label).color(Color32::WHITE))
                                .fill(pal.status_red),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::DiscardAndLeave);
                    }

                    if ui.button("Cancel").clicked() {
                        actions.push(AppAction::CancelLeave);
                    }
                },
            );
        });

    if cancel || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        actions.push(AppAction::CancelLeave);
    }
}
