use super::*;

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke,
};

use crate::frontend::actions::AppAction;
use crate::frontend::state::AppState;
use crate::frontend::theme::{Palette, radius};

/// One row in the running-jobs / queued strips above the composer.
const STRIP_ROW_HEIGHT: f32 = 22.0;
/// Cap on strip rows drawn; the remainder collapse into a "+N more" line.
const MAX_QUEUED_SHOWN: usize = 4;

/// Reserved height for the running-jobs strip (0 when there are none). Shared with
/// the panel so the transcript above can be sized before this strip is drawn.
pub(crate) fn running_strip_height(running_jobs: &[crate::frontend::jobs::LiveJobSnapshot]) -> f32 {
    if running_jobs.is_empty() {
        0.0
    } else {
        running_jobs.len().min(MAX_QUEUED_SHOWN) as f32 * STRIP_ROW_HEIGHT + 6.0
    }
}

/// Reserved height for the queued type-ahead strip (0 when empty), including the
/// "+N more" overflow row.
pub(crate) fn queued_strip_height(queued: &[String]) -> f32 {
    if queued.is_empty() {
        return 0.0;
    }
    let shown = queued.len().min(MAX_QUEUED_SHOWN);
    let overflow = if queued.len() > MAX_QUEUED_SHOWN {
        1.0
    } else {
        0.0
    };
    (shown as f32 + overflow) * STRIP_ROW_HEIGHT + 6.0
}

/// Render the assistant composer footer: the running-jobs strip, the queued
/// type-ahead strip, and the input box with its Send/Stop buttons. The caller
/// passes the per-frame `running_jobs`/`queued` snapshots and their reserved
/// heights (already folded into the panel's layout) so this only draws.
pub(crate) fn render_assistant_composer(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &Palette,
    running_jobs: &[crate::frontend::jobs::LiveJobSnapshot],
    queued: &[String],
) {
    let busy = state.ui.agent.is_busy();
    let assistant_enabled = state.config.assistant.enabled;
    let key_present = state.ui.agent.key_available.unwrap_or(false);
    let running_height = running_strip_height(running_jobs);
    let queued_height = queued_strip_height(queued);

    // Background computations running for this conversation, with cancel.
    if !running_jobs.is_empty() {
        assistant_inset_row(ui, running_height, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for job in running_jobs.iter().take(MAX_QUEUED_SHOWN) {
                ui.horizontal(|ui| {
                    if assistant_remove_button(ui, "Cancel job") {
                        actions.push(AppAction::CancelControlledJob(job.id.clone()));
                    }
                    ui.add(egui::Spinner::new().size(12.0));
                    let label_resp = ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}  {}", job.id.token(), job.label))
                                .small()
                                .color(pal.text_primary),
                        )
                        .truncate()
                        .sense(egui::Sense::click()),
                    );
                    if label_resp.on_hover_text("Open in Task Monitor").clicked()
                        && let Some(task_run_id) = job.task_run_id
                    {
                        actions.push(AppAction::OpenTaskPanel(task_run_id));
                    }
                });
            }
        });
        ui.add_space(2.0);
    }

    // Queued (type-ahead) follow-ups waiting to be sent once the agent is free.
    if !queued.is_empty() {
        assistant_inset_row(ui, queued_height, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (idx, text) in queued.iter().enumerate().take(MAX_QUEUED_SHOWN) {
                ui.horizontal(|ui| {
                    if assistant_remove_button(ui, "Remove from queue") {
                        actions.push(AppAction::RemoveQueuedAgentInput(idx));
                    }
                    ui.add(
                        egui::Label::new(RichText::new(text).small().color(pal.text_primary))
                            .truncate(),
                    );
                });
            }
            if queued.len() > MAX_QUEUED_SHOWN {
                ui.add(egui::Label::new(
                    RichText::new(format!("+{} more queued", queued.len() - MAX_QUEUED_SHOWN))
                        .small()
                        .color(pal.text_tertiary),
                ));
            }
        });
        ui.add_space(2.0);
    }

    // The user can submit whenever the assistant is enabled and keyed. If a
    // turn/computation is in flight (or a call awaits approval) the message is
    // queued rather than sent now — `send_agent_message` makes that call.
    let can_submit = assistant_enabled && key_present;
    let will_queue = busy || state.ui.agent.pending_approval().is_some();
    let hint = if !assistant_enabled {
        "Assistant disabled"
    } else if !key_present {
        "Set the API key env var"
    } else if busy {
        "Working… (Enter queues a follow-up)"
    } else {
        "Ask the assistant anything"
    };

    let composer_radius = radius::CARD;
    let inner_radius = radius::concentric(composer_radius, 4);
    let mut composer_focused = false;
    let mut send = false;
    let mut composer_rect = egui::Rect::NOTHING;
    assistant_inset_row(ui, ASSISTANT_COMPOSER_HEIGHT, |ui| {
        let content_width = ui.available_width().max(96.0);
        let frame_inner_width = (content_width - 16.0).max(80.0);
        let response = Frame::default()
            .fill(pal.input_fill)
            .stroke(Stroke::new(1.0, pal.hairline))
            .corner_radius(CornerRadius::same(composer_radius))
            .inner_margin(Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.set_width(frame_inner_width);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    const BUTTON_SIZE: f32 = 26.0;
                    // While busy the row carries both Stop and Send, so reserve
                    // width for two buttons; otherwise just the Send button.
                    let buttons_reserved = if busy {
                        2.0 * BUTTON_SIZE + 6.0
                    } else {
                        BUTTON_SIZE
                    };
                    let text_width = (ui.available_width() - buttons_reserved - 6.0).max(48.0);

                    let response = ui.add_sized(
                        [text_width, 38.0],
                        egui::TextEdit::multiline(&mut state.ui.agent.input)
                            .desired_width(text_width)
                            .desired_rows(2)
                            .font(assistant_body_font_id())
                            .frame(Frame::NONE)
                            .hint_text(hint),
                    );
                    composer_focused = response.has_focus();
                    if response.has_focus()
                        && ui.input(|input| {
                            input.key_pressed(egui::Key::Enter) && !input.modifiers.shift
                        })
                    {
                        send = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Primary send/queue button (rightmost). Enabled whenever
                        // the assistant is keyed; it queues if a turn is in flight.
                        let (fill, ink) = if can_submit {
                            (pal.accent, Color32::WHITE)
                        } else {
                            (pal.neutral_overlay(16), pal.text_tertiary)
                        };
                        let send_button =
                            Button::new(RichText::new(egui_phosphor::regular::ARROW_UP).color(ink))
                                .fill(fill)
                                .corner_radius(CornerRadius::same(inner_radius))
                                .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                        let send_resp = ui.add_enabled(can_submit, send_button);
                        let send_resp = if will_queue {
                            send_resp.on_hover_text("Queue a follow-up")
                        } else {
                            send_resp
                        };
                        if send_resp.clicked() {
                            send = true;
                        }
                        // While a turn/computation runs, a Stop button to its left.
                        if busy {
                            let stop = Button::new(
                                RichText::new(egui_phosphor::regular::X).color(pal.text_primary),
                            )
                            .fill(pal.neutral_overlay(20))
                            .corner_radius(CornerRadius::same(inner_radius))
                            .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                            if ui.add(stop).on_hover_text("Stop").clicked() {
                                actions.push(AppAction::CancelAgent);
                            }
                        }
                    });
                });
            });
        composer_rect = response.response.rect;
    });
    if composer_focused {
        ui.painter().rect_stroke(
            composer_rect,
            CornerRadius::same(composer_radius),
            Stroke::new(1.0, pal.accent),
            egui::StrokeKind::Inside,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
    if can_submit && send {
        let message = state.ui.agent.input.trim().to_string();
        if !message.is_empty() {
            actions.push(AppAction::SendAgentMessage(message));
            state.ui.agent.input.clear();
        }
    }
}
