use super::*;

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke,
};

use crate::frontend::{actions::AppAction, agent::AssistantConversationId, state::AppState};

/// Clearance reserved below the assistant composer so its lower edge clears the host
/// area's bottom margin (where the panel content rect clips) and the status bar.
const COMPOSER_BOTTOM_PAD: f32 = 8.0;
const ASSISTANT_SIDE_PAD: f32 = 8.0;
pub(crate) const ASSISTANT_COMPOSER_HEIGHT: f32 = 58.0;
// Must stay >= an icon button's natural width (glyph advance + 2 * button_padding.x,
// ~29px at the current 8px padding). This is both the buttons' `min_size` width and
// the per-button term in `reserved_width`; keeping it >= natural makes min_size win,
// so the rendered width equals this constant and the combo's `reserved_width` budget
// is exact. Underestimating (the old 26) let the padded buttons overflow the row,
// expanding the panel past its clip rect and shaving the right edge of every row.
const ASSISTANT_TOOLBAR_BUTTON_WIDTH: f32 = 30.0;
const ASSISTANT_TOOLBAR_BUTTON_HEIGHT: f32 = 24.0;
const ASSISTANT_TOOLBAR_GAP: f32 = 6.0;

pub(crate) fn render_assistant_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use crate::frontend::agent::TranscriptEntry;
    use crate::frontend::agent::registry;
    use crate::frontend::agent::session::AgentPhase;
    use crate::frontend::theme::radius;

    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());

    let provider = registry::active_provider(&state.config.assistant);
    // Cached (the live check reads env + the key store); refreshed off the hot path.
    let key_present = state.ui.agent.key_available.unwrap_or(false);
    let pending_call = state.ui.agent.pending_approval().cloned();
    let active_id = state.ui.agent.active_conversation;
    // Snapshot the queued (type-ahead) follow-ups so the strip can render without
    // holding a borrow on `state` while the composer mutably borrows the input.
    let queued: Vec<String> = state
        .ui
        .agent
        .queued
        .iter()
        .map(|item| item.preview().to_string())
        .collect();
    // Background jobs running for the active conversation. Tagged jobs from other
    // conversations stay hidden.
    let running_jobs: Vec<(u64, String, u64)> = state
        .jobs
        .agent_jobs
        .iter()
        .filter(|job| job.conversation == active_id)
        .map(|job| (job.id, job.label.clone(), job.task_run_id))
        .collect();

    let panel_rect = ui.available_rect_before_wrap();
    let content_rect = panel_rect.shrink2(egui::vec2(ASSISTANT_SIDE_PAD, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_width(content_rect.width());

            let toolbar_height = 32.0;
            let status_height = 28.0
                + if state.ui.agent.last_usage.is_some() {
                    24.0
                } else {
                    0.0
                };
            let panel_width = ui.available_width();
            let approval_height = pending_call
                .as_ref()
                .map(|call| {
                    let command = call
                        .input
                        .get("command")
                        .and_then(|value| value.as_str())
                        .unwrap_or(&call.name);
                    approval_row_height(command, panel_width)
                })
                .unwrap_or(0.0);
            let running_height = running_strip_height(&running_jobs);
            let queued_height = queued_strip_height(&queued);
            let footer_height = status_height
                + approval_height
                + running_height
                + queued_height
                + ASSISTANT_COMPOSER_HEIGHT
                + toolbar_height
                + 24.0
                + COMPOSER_BOTTOM_PAD;
            let transcript_width = panel_width;
            let transcript_content_width =
                (transcript_width - ASSISTANT_SCROLLBAR_RESERVE).max(48.0);
            let transcript_height = (ui.available_height() - footer_height).max(0.0);

            assistant_toolbar(state, ui, actions, active_id, toolbar_height);
            ui.add_space(4.0);
            weak_panel_hairline(ui, 10);
            ui.add_space(2.0);

            ui.allocate_ui_with_layout(
                egui::vec2(transcript_width, transcript_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ScrollArea::vertical()
                        .max_width(transcript_width)
                        .auto_shrink([false, false])
                        .content_margin(Margin::ZERO)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(transcript_content_width);
                            if state.ui.agent.transcript.is_empty() {
                                render_assistant_empty_state(
                                    ui,
                                    &pal,
                                    key_present,
                                    provider,
                                    transcript_content_width,
                                );
                            }
                            let mut agent_header_shown = false;
                            for entry in &state.ui.agent.transcript {
                                let show_agent_header = match entry {
                                    TranscriptEntry::User(_) => {
                                        agent_header_shown = false;
                                        false
                                    }
                                    TranscriptEntry::Assistant(_)
                                    | TranscriptEntry::Tool { .. } => {
                                        let show = !agent_header_shown;
                                        agent_header_shown = true;
                                        show
                                    }
                                    TranscriptEntry::Notice(_) => false,
                                };
                                render_transcript_entry(
                                    ui,
                                    &pal,
                                    &mut state.ui.markdown_cache,
                                    entry,
                                    transcript_content_width,
                                    show_agent_header,
                                );
                            }
                            // Live streaming preview of the in-flight assistant text.
                            if !state.ui.agent.streaming_text.is_empty() {
                                if agent_header_shown {
                                    ui.add_space(6.0);
                                } else {
                                    agent_message_header(ui, &pal);
                                }
                                render_markdown(
                                    ui,
                                    &pal,
                                    &mut state.ui.markdown_cache,
                                    &format!("{}...", state.ui.agent.streaming_text),
                                );
                            }
                            if state.ui.agent.phase == AgentPhase::Done
                                && !matches!(
                                    state.ui.agent.transcript.last(),
                                    None | Some(TranscriptEntry::Notice(_))
                                )
                            {
                                ui.add_space(3.0);
                                completed_badge(ui, &pal);
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
                    let frame_inner_width = (panel_width - 20.0).max(48.0);
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
                                egui::Label::new(assistant_text(command).color(pal.text_primary))
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

            render_assistant_composer(state, ui, actions, &pal, &running_jobs, &queued);

            ui.add_space(COMPOSER_BOTTOM_PAD);
        },
    );
    ui.advance_cursor_after_rect(panel_rect);
}

fn assistant_toolbar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    active_id: crate::frontend::agent::AssistantConversationId,
    height: f32,
) {
    let can_manage = state.ui.agent.can_manage_conversations();
    let conversations: Vec<(AssistantConversationId, String)> = state
        .ui
        .agent
        .conversations
        .iter()
        .map(|conversation| (conversation.id, conversation.title.clone()))
        .collect();
    let active_title = conversations
        .iter()
        .find(|(id, _)| *id == active_id)
        .map(|(_, title)| title.as_str())
        .unwrap_or("Assistant");
    let is_renaming = state.ui.agent.renaming_conversation == Some(active_id);
    assistant_inset_row(ui, height, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ASSISTANT_TOOLBAR_GAP;
            let button_size = egui::vec2(
                ASSISTANT_TOOLBAR_BUTTON_WIDTH,
                ASSISTANT_TOOLBAR_BUTTON_HEIGHT,
            );

            if is_renaming {
                let reserved_width =
                    2.0 * ASSISTANT_TOOLBAR_BUTTON_WIDTH + 2.0 * ASSISTANT_TOOLBAR_GAP;
                let edit_width = (ui.available_width() - reserved_width).max(72.0);
                let response = ui.add_enabled(
                    can_manage,
                    egui::TextEdit::singleline(&mut state.ui.agent.rename_buffer)
                        .desired_width(edit_width),
                );
                let submit =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if submit {
                    actions.push(AppAction::RenameAssistantConversation {
                        id: active_id,
                        title: state.ui.agent.rename_buffer.clone(),
                    });
                }
                if ui
                    .add_enabled(
                        can_manage,
                        Button::new(RichText::new(egui_phosphor::regular::CHECK))
                            .min_size(button_size),
                    )
                    .on_hover_text("Save name")
                    .clicked()
                {
                    actions.push(AppAction::RenameAssistantConversation {
                        id: active_id,
                        title: state.ui.agent.rename_buffer.clone(),
                    });
                }
                if ui
                    .add_enabled(
                        can_manage,
                        Button::new(RichText::new(egui_phosphor::regular::X)).min_size(button_size),
                    )
                    .on_hover_text("Cancel rename")
                    .clicked()
                {
                    state.ui.agent.renaming_conversation = None;
                    state.ui.agent.rename_buffer.clear();
                }
            } else {
                let reserved_width =
                    3.0 * ASSISTANT_TOOLBAR_BUTTON_WIDTH + 3.0 * ASSISTANT_TOOLBAR_GAP;
                let combo_width = (ui.available_width() - reserved_width).max(72.0);
                let combo_response = egui::ComboBox::from_id_salt("assistant.conversation")
                    .selected_text(assistant_text(active_title))
                    .width(combo_width)
                    .truncate()
                    .show_ui(ui, |ui| {
                        crate::frontend::theme::stabilize_selectable_rows(ui);
                        ui.set_width(combo_width);
                        for (id, title) in &conversations {
                            let response = ui
                                .add_enabled_ui(can_manage, |ui| {
                                    ui.add_sized(
                                        [combo_width, ASSISTANT_TOOLBAR_BUTTON_HEIGHT],
                                        egui::Button::selectable(
                                            *id == active_id,
                                            assistant_text(title),
                                        )
                                        .truncate(),
                                    )
                                })
                                .inner
                                .on_hover_text(title);
                            if response.clicked() {
                                actions.push(AppAction::SwitchAssistantConversation(*id));
                            }
                        }
                    });
                combo_response.response.on_hover_text(active_title);

                if ui
                    .add_enabled(
                        can_manage,
                        Button::new(RichText::new(egui_phosphor::regular::PLUS))
                            .min_size(button_size),
                    )
                    .on_hover_text("New conversation")
                    .clicked()
                {
                    actions.push(AppAction::NewAssistantConversation);
                }
                if ui
                    .add_enabled(
                        can_manage,
                        Button::new(RichText::new(egui_phosphor::regular::PENCIL_SIMPLE))
                            .min_size(button_size),
                    )
                    .on_hover_text("Rename conversation")
                    .clicked()
                {
                    state.ui.agent.renaming_conversation = Some(active_id);
                    state.ui.agent.rename_buffer = active_title.to_string();
                }
                if ui
                    .add_enabled(
                        can_manage,
                        Button::new(RichText::new(egui_phosphor::regular::TRASH))
                            .min_size(button_size),
                    )
                    .on_hover_text("Delete conversation")
                    .clicked()
                {
                    actions.push(AppAction::DeleteAssistantConversation(active_id));
                }
            }
        });
    });
}

fn approval_row_height(command: &str, panel_width: f32) -> f32 {
    let text_width = (panel_width - 40.0).max(48.0);
    let chars_per_line = (text_width / 7.0).max(8.0);
    let command_lines = (command.chars().count() as f32 / chars_per_line).ceil();
    74.0 + command_lines.max(1.0) * 22.0
}

pub(crate) fn assistant_inset_row<R>(
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
    width: f32,
) {
    ui.set_width(width);
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.set_width(width);
        ui.label(
            RichText::new(egui_phosphor::regular::SPARKLE)
                .size(28.0)
                .color(pal.accent_soft()),
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
