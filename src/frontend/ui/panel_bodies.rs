use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke,
};

use crate::{
    backend::tasks::{TaskPanelKind, TaskStatus},
    frontend::{
        actions::AppAction,
        state::{AppState, PrimaryView},
        status_text,
    },
};
pub(super) fn render_output_panel(state: &mut AppState, ui: &mut egui::Ui) {
    ui.set_width(ui.available_width());
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for line in &state.output_log {
                ui.add(
                    egui::Label::new(RichText::new(line).monospace())
                        .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
}

pub(super) fn render_console_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    const PROMPT_ROW_HEIGHT: f32 = 34.0;
    const INPUT_OUTER_HEIGHT: f32 = 28.0;
    const INPUT_X_MARGIN: f32 = 8.0;
    const DIVIDER_HEIGHT: f32 = 1.0;
    const BOTTOM_PADDING: f32 = 4.0;

    ui.set_width(ui.available_width());
    // Keep chronological output in top-down visual order while reserving fixed
    // space for the prompt row so the panel cannot grow frame-over-frame.
    let log_height =
        (ui.available_height() - PROMPT_ROW_HEIGHT - DIVIDER_HEIGHT - BOTTOM_PADDING).max(0.0);
    let log_text = state.output_log.join("\n");

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), log_height),
        Layout::top_down(Align::Min),
        |ui| {
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(RichText::new(log_text).monospace())
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
        },
    );
    weak_panel_hairline(ui, 14);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), PROMPT_ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            let pal = crate::frontend::theme::palette(ui);
            let input_radius =
                crate::frontend::theme::radius::concentric(crate::frontend::theme::radius::CARD, 2);
            ui.spacing_mut().item_spacing.x = 8.0;

            ui.monospace("sls>");

            let button_width = 46.0;
            let text_edit_width = (ui.available_width()
                - button_width
                - ui.spacing().item_spacing.x
                - INPUT_X_MARGIN * 2.0)
                .max(96.0);

            let response = Frame::default()
                .fill(pal.input_fill)
                .stroke(Stroke::new(1.0, pal.hairline))
                .corner_radius(egui::CornerRadius::same(input_radius))
                .inner_margin(Margin::symmetric(INPUT_X_MARGIN as i8, 3))
                .show(ui, |ui| {
                    ui.add_sized(
                        [text_edit_width, INPUT_OUTER_HEIGHT - 8.0],
                        egui::TextEdit::singleline(&mut state.ui.console.input)
                            .desired_width(f32::INFINITY)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO)
                            .hint_text("view background white"),
                    )
                })
                .inner;

            let mut run = false;
            if ui.button("Run").clicked() {
                run = true;
            }

            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                run = true;
            }

            if run {
                let command = state.ui.console.input.trim().to_string();
                if !command.is_empty() {
                    actions.push(AppAction::RunConsoleCommand(command));
                    state.ui.console.input.clear();
                }
            }
        },
    );
    ui.add_space(BOTTOM_PADDING);
}

pub(super) fn weak_panel_hairline(ui: &mut egui::Ui, alpha: u8) {
    let pal = crate::frontend::theme::palette(ui);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, pal.neutral_overlay(alpha)),
    );
}

/// Clearance reserved below the assistant composer so its lower edge clears the host
/// area's bottom margin (where the panel content rect clips) and the status bar.
const COMPOSER_BOTTOM_PAD: f32 = 8.0;
const ASSISTANT_SIDE_PAD: f32 = 8.0;
const ASSISTANT_COMPOSER_HEIGHT: f32 = 58.0;

pub(super) fn render_assistant_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::agent::registry;
    use crate::frontend::theme::radius;

    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());

    let busy = state.ui.agent.is_busy();
    let assistant_enabled = state.config.assistant.enabled;
    let provider = registry::active_provider(&state.config.assistant);
    // Cached (the live check reads env + the key store); refreshed off the hot path.
    let key_present = state.ui.agent.key_available.unwrap_or(false);
    let pending_call = state.ui.agent.pending_approval().cloned();

    let panel_rect = ui.available_rect_before_wrap();
    let content_rect = panel_rect.shrink2(egui::vec2(ASSISTANT_SIDE_PAD, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_width(content_rect.width());

            let status_height = 28.0
                + if state.ui.agent.last_usage.is_some() {
                    24.0
                } else {
                    0.0
                };
            let approval_height = if pending_call.is_some() { 98.0 } else { 0.0 };
            let footer_height = status_height
                + approval_height
                + ASSISTANT_COMPOSER_HEIGHT
                + 24.0
                + COMPOSER_BOTTOM_PAD;
            let transcript_height = (ui.available_height() - footer_height).max(0.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), transcript_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if state.ui.agent.transcript.is_empty() {
                                render_assistant_empty_state(ui, &pal, key_present, provider);
                            }
                            for entry in &state.ui.agent.transcript {
                                render_transcript_entry(ui, &pal, entry);
                            }
                            // Live streaming preview of the in-flight assistant text.
                            if !state.ui.agent.streaming_text.is_empty() {
                                message_role(
                                    ui,
                                    egui_phosphor::regular::SPARKLE,
                                    "SilicoLab Agent",
                                    pal.accent,
                                );
                                ui.add_space(2.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "{}...",
                                            state.ui.agent.streaming_text
                                        ))
                                        .color(pal.text_primary),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Wrap),
                                );
                            }
                        });
                },
            );

            weak_panel_hairline(ui, 14);
            ui.add_space(3.0);

            assistant_inset_row(ui, status_height.max(18.0), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} | {}",
                            provider.label, state.config.assistant.model
                        ))
                        .small()
                        .color(pal.text_tertiary),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                if let Some(usage) = &state.ui.agent.last_usage {
                    let session = &state.ui.agent.session_usage;
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "last in {} out {}; session in {} out {}",
                                compact(usage.input_total()),
                                compact(usage.output),
                                compact(session.input_total()),
                                compact(session.output),
                            ))
                            .small()
                            .color(pal.text_tertiary),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            });

            ui.add_space(3.0);

            if let Some(call) = &pending_call {
                let command = call
                    .input
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or(&call.name);
                assistant_inset_row(ui, approval_height, |ui| {
                    let frame_inner_width = (ui.available_width() - 20.0).max(48.0);
                    Frame::default()
                        .fill(blend(pal.status_amber, pal.input_fill, 0.86))
                        .stroke(Stroke::new(1.0, blend(pal.status_amber, pal.hairline, 0.4)))
                        .corner_radius(CornerRadius::same(radius::CARD))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(frame_inner_width);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{}  Approve to run",
                                        egui_phosphor::regular::WARNING
                                    ))
                                    .strong()
                                    .color(pal.status_amber),
                                );
                            });
                            ui.add_space(2.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(command).monospace().color(pal.text_primary),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let approve = Button::new(
                                    RichText::new(format!(
                                        "{}  Approve",
                                        egui_phosphor::regular::CHECK
                                    ))
                                    .color(Color32::WHITE),
                                )
                                .fill(pal.status_green)
                                .corner_radius(CornerRadius::same(radius::CONTROL));
                                if ui.add(approve).clicked() {
                                    actions.push(AppAction::ApproveToolCall(call.id.clone()));
                                }
                                if ui
                                    .add(
                                        Button::new(
                                            RichText::new("Reject").color(pal.text_primary),
                                        )
                                        .fill(pal.neutral_overlay(16))
                                        .corner_radius(CornerRadius::same(radius::CONTROL)),
                                    )
                                    .clicked()
                                {
                                    actions.push(AppAction::RejectToolCall(call.id.clone()));
                                }
                            });
                        });
                });
                ui.add_space(4.0);
            }

            let send_enabled = assistant_enabled && key_present && !busy && pending_call.is_none();
            let hint = if !assistant_enabled {
                "Assistant disabled"
            } else if !key_present {
                "Set the API key env var"
            } else if busy {
                "Working..."
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
                            let text_width = (ui.available_width() - BUTTON_SIZE - 6.0).max(48.0);

                            let response = ui.add_sized(
                                [text_width, 38.0],
                                egui::TextEdit::multiline(&mut state.ui.agent.input)
                                    .desired_width(text_width)
                                    .desired_rows(2)
                                    .font(egui::TextStyle::Small)
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
                                if busy {
                                    let stop = Button::new(
                                        RichText::new(egui_phosphor::regular::X)
                                            .color(pal.text_primary),
                                    )
                                    .fill(pal.neutral_overlay(20))
                                    .corner_radius(CornerRadius::same(inner_radius))
                                    .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                                    if ui.add(stop).clicked() {
                                        actions.push(AppAction::CancelAgent);
                                    }
                                } else {
                                    let (fill, ink) = if send_enabled {
                                        (pal.accent, Color32::WHITE)
                                    } else {
                                        (pal.neutral_overlay(16), pal.text_tertiary)
                                    };
                                    let button = Button::new(
                                        RichText::new(egui_phosphor::regular::ARROW_UP).color(ink),
                                    )
                                    .fill(fill)
                                    .corner_radius(CornerRadius::same(inner_radius))
                                    .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                                    if ui.add_enabled(send_enabled, button).clicked() {
                                        send = true;
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
            if send_enabled && send {
                let message = state.ui.agent.input.trim().to_string();
                if !message.is_empty() {
                    actions.push(AppAction::SendAgentMessage(message));
                    state.ui.agent.input.clear();
                }
            }

            ui.add_space(COMPOSER_BOTTOM_PAD);
        },
    );
    ui.advance_cursor_after_rect(panel_rect);
}

fn assistant_inset_row<R>(
    ui: &mut egui::Ui,
    height: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let width = ui.available_width();
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, height.max(0.0)), egui::Sense::hover());
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        add_contents,
    )
}

/// First-run welcome shown when the transcript is empty: a centered prompt plus
/// a missing-key callout when no credential is configured.
fn render_assistant_empty_state(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    key_present: bool,
    provider: &crate::frontend::agent::registry::ProviderSpec,
) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(egui_phosphor::regular::SPARKLE)
                .size(28.0)
                .color(pal.accent),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("How can I help?")
                .strong()
                .color(pal.text_primary),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "Fetch a structure, restyle the view, or set up a calculation — try \
                 \"fetch 1ubq and show it as cartoon\". I drive SilicoLab with the same \
                 console commands you would type.",
            )
            .small()
            .color(pal.text_tertiary),
        );
    });
    if !key_present {
        ui.add_space(10.0);
        ui.label(
            RichText::new(format!(
                "{}  No API key found. Add one in Settings ▸ Assistant, set {} and restart, \
                 or pick another provider there.",
                egui_phosphor::regular::WARNING,
                provider.key_env
            ))
            .small()
            .color(pal.status_amber),
        );
    }
}

/// A small "icon + role" header above an assistant or user message.
fn message_role(ui: &mut egui::Ui, icon: &str, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new(icon).small().color(color));
        ui.label(RichText::new(label).small().strong().color(color));
    });
}

fn render_transcript_entry(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    entry: &crate::frontend::agent::TranscriptEntry,
) {
    use crate::frontend::agent::TranscriptEntry;
    use crate::frontend::theme::radius;
    match entry {
        TranscriptEntry::User(text) => {
            ui.add_space(10.0);
            // The user's turn reads as a soft bubble so it stands apart from the
            // assistant's flush-left prose.
            let frame_inner_width = (ui.available_width() - 20.0).max(48.0);
            Frame::default()
                .fill(pal.neutral_overlay(20))
                .corner_radius(CornerRadius::same(radius::CARD))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(frame_inner_width);
                    ui.add(
                        egui::Label::new(RichText::new(text).color(pal.text_primary))
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
        }
        TranscriptEntry::Assistant(text) => {
            ui.add_space(10.0);
            message_role(
                ui,
                egui_phosphor::regular::SPARKLE,
                "SilicoLab Agent",
                pal.accent,
            );
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(RichText::new(text).color(pal.text_primary))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(3.0);
            completed_badge(ui, pal);
        }
        TranscriptEntry::ToolCall { summary } => {
            ui.add_space(4.0);
            render_tool_chip(
                ui,
                pal,
                egui_phosphor::regular::TERMINAL,
                summary,
                pal.text_tertiary,
            );
        }
        TranscriptEntry::ToolResult { summary, is_error } => {
            let (icon, color) = if *is_error {
                (egui_phosphor::regular::X_CIRCLE, pal.status_red)
            } else {
                (egui_phosphor::regular::CHECK_CIRCLE, pal.text_tertiary)
            };
            ui.add_space(2.0);
            render_tool_chip(ui, pal, icon, summary, color);
        }
        TranscriptEntry::Notice(text) => {
            ui.add_space(6.0);
            ui.label(
                RichText::new(text)
                    .small()
                    .italics()
                    .color(pal.text_tertiary),
            );
        }
    }
}

/// A subdued rounded chip for tool calls and their results — monospace detail on
/// a faint fill so machine chatter recedes behind the conversation.
fn render_tool_chip(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    icon: &str,
    summary: &str,
    color: Color32,
) {
    use crate::frontend::theme::radius;
    let frame_inner_width = (ui.available_width() - 16.0).max(48.0);
    Frame::default()
        .fill(pal.neutral_overlay(12))
        .corner_radius(CornerRadius::same(radius::CONTROL))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(frame_inner_width);
            ui.vertical(|ui| {
                ui.set_width(frame_inner_width);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(icon).small().color(color));
                });
                ui.add(
                    egui::Label::new(RichText::new(summary).small().monospace().color(color))
                        .wrap_mode(egui::TextWrapMode::Wrap),
                );
            });
        });
}

fn completed_badge(ui: &mut egui::Ui, pal: &crate::frontend::theme::Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(
            RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                .small()
                .color(pal.status_green),
        );
        ui.label(
            RichText::new("Completed")
                .small()
                .strong()
                .color(pal.status_green),
        );
    });
}

/// Linear blend toward `b` (`t` = 0 → `a`, `t` = 1 → `b`), used for tinted
/// callout fills that stay readable on either theme.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::frontend::theme::mix(a, b, t)
}

/// Compact token count, e.g. `1234` → `1.2k`.
fn compact(value: u32) -> String {
    if value < 1000 {
        value.to_string()
    } else {
        format!("{:.1}k", value as f32 / 1000.0)
    }
}

pub(super) fn render_status_bar(state: &mut AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(status_text(state.structure(), &state.ui.selection))
                .color(pal.text_primary),
        );
        ui.separator();
        ui.label(RichText::new(&state.message).color(pal.text_primary));
    });
}

fn task_status_badge(pal: &crate::frontend::theme::Palette, status: TaskStatus) -> RichText {
    let color = match status {
        TaskStatus::Ready => pal.status_blue,
        TaskStatus::WaitingInput => pal.status_amber,
        TaskStatus::Running => pal.status_green,
        TaskStatus::Completed => pal.status_green,
        TaskStatus::Failed => pal.status_red,
    };

    RichText::new(status.label()).strong().color(color)
}

pub(super) fn render_task_monitor_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
        ui.label(RichText::new("Task Monitor").strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button(format!("{}  Open Tasks", egui_phosphor::regular::LIGHTNING))
                .clicked()
            {
                state.ui.layout.active_primary_view = PrimaryView::Tasks;
                state.ui.layout.show_primary_sidebar = true;
            }
        });
    });
    ui.separator();

    render_active_task_summary(state, ui);
    ui.add_space(8.0);

    if state.tasks.tasks.is_empty() {
        ui.label("No task run yet.");
        return;
    }

    let task_rows = state
        .tasks
        .tasks
        .iter()
        .rev()
        .map(|task| {
            (
                task.id,
                task.controller_id,
                task.title.clone(),
                task.status,
                task.backend.label(),
                task.outcome.label(),
                task.theme.clone(),
                task.method.clone(),
                task.application.clone(),
                task.panel,
                task.run_dir.clone(),
                task.source_entry_id,
                task.result_entry_id,
                task.engine_label.clone(),
            )
        })
        .collect::<Vec<_>>();

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (
                task_id,
                controller_id,
                title,
                status,
                backend,
                outcome,
                theme,
                method,
                application,
                panel,
                run_dir,
                source_entry_id,
                result_entry_id,
                engine_label,
            ) in task_rows
            {
                let row = Frame::group(ui.style())
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let response = ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(title).strong());
                                    ui.label(
                                        RichText::new(format!(
                                            "{theme} / {method} / {application}"
                                        ))
                                        .small()
                                        .color(pal.text_tertiary),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .button(format!(
                                            "{}  Open",
                                            egui_phosphor::regular::FOLDER_OPEN
                                        ))
                                        .clicked()
                                    {
                                        actions.push(AppAction::OpenTaskPanel(task_id));
                                    }
                                    if ui
                                        .button(format!("{}  Run", egui_phosphor::regular::PLAY))
                                        .clicked()
                                    {
                                        actions.push(AppAction::RunTask(task_id));
                                    }
                                    ui.label(task_status_badge(&pal, status));
                                });
                            });
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(format!("{controller_id} / {backend} / {outcome}"))
                                    .small()
                                    .color(pal.text_tertiary),
                            );
                            if let Some(engine_label) = engine_label {
                                ui.label(
                                    RichText::new(format!("Engine: {engine_label}"))
                                        .small()
                                        .color(pal.text_tertiary),
                                );
                            }
                            if let Some(run_dir) = run_dir {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("Run Dir:").small().color(pal.text_tertiary),
                                    );
                                    ui.monospace(run_dir.display().to_string());
                                });
                            }
                            if source_entry_id.is_some() || result_entry_id.is_some() {
                                ui.label(
                                    RichText::new(format!(
                                        "Source Entry: {}    Result Entry: {}",
                                        source_entry_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| "-".to_string()),
                                        result_entry_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| "-".to_string())
                                    ))
                                    .small()
                                    .color(pal.text_tertiary),
                                );
                            }
                        });
                        response.response
                    })
                    .inner;
                if row.double_clicked() {
                    if panel != TaskPanelKind::None {
                        actions.push(AppAction::OpenTaskPanel(task_id));
                    } else {
                        actions.push(AppAction::RunTask(task_id));
                    }
                }
                ui.add_space(6.0);
            }
        });
}

fn render_active_task_summary(state: &AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);
    let frame = Frame::group(ui.style()).inner_margin(Margin::same(8));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Active").strong());
        ui.add_space(4.0);

        if let Some(task_run_id) = state.active_task_run
            && let Some(task) = state.tasks.task_run(task_run_id)
        {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&task.title).strong());
                ui.label(task_status_badge(&pal, task.status));
            });
            ui.label(
                RichText::new(format!(
                    "{} / {} / {}",
                    task.controller_id,
                    task.backend.label(),
                    task.outcome.label()
                ))
                .small()
                .color(pal.text_tertiary),
            );
        } else {
            ui.label(
                RichText::new("No active task.")
                    .small()
                    .color(pal.text_tertiary),
            );
        }

        if let Some(engine_job) = state.jobs.engine.as_ref() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Engine Job: {} / {}",
                    engine_job.engine, engine_job.job_kind
                ))
                .small(),
            );
            if let Some(stage) = engine_job.latest_stage.as_ref() {
                ui.label(
                    RichText::new(format!("Stage: {stage}"))
                        .small()
                        .color(pal.text_tertiary),
                );
            }
            for line in engine_job.log_tail.iter().rev().take(6).rev() {
                ui.monospace(line);
            }
        } else if let Some(optimizer) = state.jobs.optimizer.as_ref() {
            ui.add_space(6.0);
            if let Some(report) = optimizer.latest_report.as_ref() {
                ui.label(
                    RichText::new(format!(
                        "Optimizer: {} steps, energy {:.3} -> {:.3}",
                        report.steps, report.initial_energy, report.final_energy
                    ))
                    .small()
                    .color(pal.text_tertiary),
                );
            } else {
                ui.label(
                    RichText::new("Optimizer running...")
                        .small()
                        .color(pal.text_tertiary),
                );
            }
        }
    });
}
