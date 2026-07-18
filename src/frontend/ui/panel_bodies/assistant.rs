use super::*;

use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke,
};

use crate::frontend::{actions::AppAction, agent::AssistantConversationId, state::AppState};

/// Clearance reserved below the assistant composer so its lower edge clears the host
/// area's bottom margin (where the panel content rect clips) and the status bar.
const COMPOSER_BOTTOM_PAD: f32 = 8.0;
const ASSISTANT_SIDE_PAD: f32 = 8.0;
/// Fixed two-row composer height. The text editor owns the full first row and
/// the mode/model/actions own the second, so long input can never push the
/// persistent Send/Stop controls outside the panel.
pub(crate) const ASSISTANT_COMPOSER_HEIGHT: f32 = 92.0;
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

    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());

    let selection = state.ui.agent.selection.clone();
    let provider = registry::provider_spec(&selection.provider).unwrap_or(&registry::PROVIDERS[0]);
    let model_selected = !selection.model.trim().is_empty()
        || matches!(provider.kind, registry::ProviderKind::ExternalAgent(_));
    // Cached (the live check reads env + the key store); refreshed off the hot path.
    let key_present = state.ui.agent.key_available.unwrap_or(false);
    // Cloned so the cards can render without holding a borrow on `state`.
    let gated_calls = crate::frontend::agent::gated_pending(state);
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
    // Background jobs running for the active conversation. Build this from the
    // unified snapshot, then filter by the conversation-owned agent ids.
    let active_agent_ids: std::collections::HashSet<u64> = state
        .jobs
        .agent_jobs
        .iter()
        .filter(|job| job.conversation == active_id)
        .map(|job| job.id)
        .collect();
    let running_jobs: Vec<crate::frontend::jobs::LiveJobSnapshot> = state
        .jobs
        .list_live_snapshots(state.active_task_run)
        .into_iter()
        .filter(|job| match job.id {
            crate::frontend::jobs::JobControlId::Agent(id) => active_agent_ids.contains(&id),
            _ => false,
        })
        .collect();

    let panel_rect = ui.available_rect_before_wrap();
    let content_rect = panel_rect.shrink2(egui::vec2(ASSISTANT_SIDE_PAD, 0.0));
    let mut open_assistant_settings = false;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_width(content_rect.width());

            let toolbar_height = 32.0;
            let panel_width = ui.available_width();
            let approval_height = approval_block_height(&gated_calls, panel_width);
            let running_height = running_strip_height(&running_jobs);
            let queued_height = queued_strip_height(&queued);
            let footer_height = approval_height
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
                                open_assistant_settings |= render_assistant_empty_state(
                                    ui,
                                    &pal,
                                    state.config.assistant.enabled,
                                    key_present,
                                    model_selected,
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

            ui.add_space(6.0);

            if !gated_calls.is_empty() {
                assistant_inset_row(ui, approval_height, |ui| {
                    if gated_calls.len() > 1 {
                        ui.label(
                            RichText::new(format!(
                                "{}  {} commands need your approval",
                                egui_phosphor::regular::WARNING,
                                gated_calls.len()
                            ))
                            .strong()
                            .color(pal.status_amber),
                        );
                        ui.add_space(4.0);
                    }
                    for call in &gated_calls {
                        render_approval_card(ui, &pal, actions, call, panel_width);
                        ui.add_space(6.0);
                    }
                });
                ui.add_space(4.0);
            }

            open_assistant_settings |=
                render_assistant_composer(state, ui, actions, &pal, &running_jobs, &queued);

            ui.add_space(COMPOSER_BOTTOM_PAD);
        },
    );
    if open_assistant_settings {
        state.ui.settings.search_query.clear();
        state.ui.settings.selected_category =
            crate::frontend::ui::settings_registry::SettingCategory::Assistant;
        state.ui.layout.settings_open = true;
    }
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
            let delete_confirm_id = ("del_conversation", active_id);
            let confirming_delete = crate::frontend::ui::widgets::destructive_confirmation_is_armed(
                ui,
                delete_confirm_id,
            );

            if confirming_delete {
                let is_last = conversations.len() <= 1;
                let (prompt, confirm_word) = if is_last {
                    ("Clear this conversation?", "Clear")
                } else {
                    ("Delete this conversation?", "Delete")
                };
                if crate::frontend::ui::widgets::confirm_destructive(
                    ui,
                    delete_confirm_id,
                    prompt,
                    confirm_word,
                    |ui| ui.button(confirm_word),
                ) {
                    actions.push(AppAction::DeleteAssistantConversation(active_id));
                }
            } else if is_renaming {
                let reserved_width =
                    2.0 * ASSISTANT_TOOLBAR_BUTTON_WIDTH + 2.0 * ASSISTANT_TOOLBAR_GAP;
                let edit_width = (ui.available_width() - reserved_width).max(72.0);
                let response = ui.add_enabled(
                    can_manage,
                    egui::TextEdit::singleline(&mut state.ui.agent.rename_buffer)
                        .desired_width(edit_width),
                );
                if !response.has_focus() {
                    response.request_focus();
                }
                let name_filled = !state.ui.agent.rename_buffer.trim().is_empty();
                let submit = response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && name_filled;
                let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
                if submit {
                    actions.push(AppAction::RenameAssistantConversation {
                        id: active_id,
                        title: state.ui.agent.rename_buffer.clone(),
                    });
                }
                if cancel {
                    state.ui.agent.renaming_conversation = None;
                    state.ui.agent.rename_buffer.clear();
                }
                if ui
                    .add_enabled(
                        can_manage && name_filled,
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
                let is_last = conversations.len() <= 1;
                let (prompt, confirm_word, hover) = if is_last {
                    ("Clear this conversation?", "Clear", "Clear conversation")
                } else {
                    ("Delete this conversation?", "Delete", "Delete conversation")
                };
                if crate::frontend::ui::widgets::confirm_destructive(
                    ui,
                    delete_confirm_id,
                    prompt,
                    confirm_word,
                    |ui| {
                        ui.add_enabled(
                            can_manage,
                            Button::new(RichText::new(egui_phosphor::regular::TRASH))
                                .min_size(button_size),
                        )
                        .on_hover_text(hover)
                    },
                ) {
                    actions.push(AppAction::DeleteAssistantConversation(active_id));
                }
            }
        });
    });
}

fn call_command_text(call: &crate::io::llm::types::ToolCall) -> &str {
    call.input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or(&call.name)
}

/// Height of one approval card: command lines, plus an impact line and the
/// "always allow" row when present.
fn approval_card_height(call: &crate::io::llm::types::ToolCall, panel_width: f32) -> f32 {
    use crate::frontend::console::RiskLevel;
    let command = call_command_text(call);
    let text_width = (panel_width - 40.0).max(48.0);
    let chars_per_line = (text_width / 7.0).max(8.0);
    let command_lines = (command.chars().count() as f32 / chars_per_line)
        .ceil()
        .max(1.0);
    let mut height = 74.0 + command_lines * 22.0;
    if crate::frontend::agent::impact_hint(call).is_some() {
        height += 18.0;
    }
    if crate::frontend::agent::tools::risk_of_call(call) != RiskLevel::Destructive {
        height += 28.0; // the "always allow" button row
    }
    height
}

/// Total height of the approval block: every card, gaps, and a multi-call header.
fn approval_block_height(calls: &[crate::io::llm::types::ToolCall], panel_width: f32) -> f32 {
    if calls.is_empty() {
        return 0.0;
    }
    let header = if calls.len() > 1 { 24.0 } else { 0.0 };
    let gaps = calls.len() as f32 * 6.0;
    let cards: f32 = calls
        .iter()
        .map(|call| approval_card_height(call, panel_width))
        .sum();
    header + gaps + cards
}

/// One approval card: the command, its risk and (for compute) cost, and the
/// approve / reject / remember controls. Destructive calls omit the remember
/// controls — they always prompt, so "always allow" would be a lie.
fn render_approval_card(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    actions: &mut Vec<AppAction>,
    call: &crate::io::llm::types::ToolCall,
    panel_width: f32,
) {
    use crate::frontend::console::RiskLevel;
    use crate::frontend::theme::radius;

    let command = call_command_text(call);
    let risk = crate::frontend::agent::tools::risk_of_call(call);
    let impact = crate::frontend::agent::impact_hint(call);
    let accent = if risk == RiskLevel::Destructive {
        pal.status_red
    } else {
        pal.status_amber
    };
    let card_height = approval_card_height(call, panel_width);

    assistant_inset_row(ui, card_height, |ui| {
        let frame_inner_width = (panel_width - 20.0).max(48.0);
        Frame::default()
            .fill(blend(accent, pal.input_fill, 0.86))
            .stroke(Stroke::new(1.0_f32, blend(accent, pal.hairline, 0.4)))
            .corner_radius(CornerRadius::same(radius::CARD))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(frame_inner_width);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}  Approve to run · {}",
                            egui_phosphor::regular::WARNING,
                            risk.label()
                        ))
                        .strong()
                        .color(accent),
                    );
                });
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(assistant_text(command).color(pal.text_primary))
                        .wrap_mode(egui::TextWrapMode::Wrap),
                );
                if let Some(impact) = &impact {
                    ui.label(RichText::new(impact).small().color(pal.text_tertiary));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let approve = Button::new(
                        RichText::new(format!("{}  Approve", egui_phosphor::regular::CHECK))
                            .color(Color32::WHITE),
                    )
                    .fill(pal.status_green)
                    .corner_radius(CornerRadius::same(radius::CONTROL));
                    if ui.add(approve).clicked() {
                        actions.push(AppAction::ApproveToolCall(call.id.clone()));
                    }
                    if ui
                        .add(
                            Button::new(RichText::new("Reject").color(pal.text_primary))
                                .fill(pal.neutral_overlay(16))
                                .corner_radius(CornerRadius::same(radius::CONTROL)),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::RejectToolCall(call.id.clone()));
                    }
                });
                if risk != RiskLevel::Destructive {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let verb = crate::frontend::agent::tools::call_allow_key(call);
                        if ui
                            .add(small_allow_button(pal, &format!("Always allow `{verb}`")))
                            .clicked()
                        {
                            actions.push(AppAction::AlwaysAllowCommand(call.id.clone()));
                        }
                        if ui
                            .add(small_allow_button(
                                pal,
                                &format!("Always allow all {} commands", risk.label()),
                            ))
                            .clicked()
                        {
                            actions.push(AppAction::AlwaysAllowRisk(call.id.clone()));
                        }
                    });
                }
            });
    });
}

/// A small, low-emphasis "remember this decision" button.
fn small_allow_button(pal: &crate::frontend::theme::Palette, text: &str) -> Button<'static> {
    use crate::frontend::theme::radius;
    Button::new(
        RichText::new(text.to_string())
            .small()
            .color(pal.text_muted),
    )
    .fill(pal.neutral_overlay(10))
    .corner_radius(CornerRadius::same(radius::CONTROL))
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
    assistant_enabled: bool,
    key_present: bool,
    model_selected: bool,
    provider: &crate::frontend::agent::registry::ProviderSpec,
    width: f32,
) -> bool {
    let mut open_settings = false;
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
        let title = if assistant_enabled && key_present && model_selected {
            "How can I help?"
        } else if assistant_enabled && key_present {
            "Choose a model"
        } else {
            "Set up your Assistant"
        };
        ui.label(RichText::new(title).strong().color(pal.text_primary));
        ui.add_space(2.0);
        let description = if assistant_enabled && key_present && model_selected {
            "Fetch a structure, restyle the view, or set up a calculation — try \
             “fetch 1ubq and show it as cartoon”. I drive SilicoLab with the same \
             console commands you would type."
        } else if assistant_enabled && key_present {
            "Refresh models from your local server or enter its exact model id before your first message."
        } else if !assistant_enabled && key_present {
            "The Assistant is turned off. Enable it to start using the model already configured."
        } else if !assistant_enabled {
            "The Assistant is turned off. Enable it, choose a provider, and add its API key."
        } else {
            "Connect a model before your first message. It takes three quick steps:"
        };
        ui.label(RichText::new(description).small().color(pal.text_tertiary));
        if !assistant_enabled || !key_present || !model_selected {
            ui.add_space(8.0);
            let steps = if assistant_enabled && key_present && !model_selected {
                [
                    "1  Start Ollama or a compatible server",
                    "2  Refresh models or enter a model id",
                    "3  Return here and start chatting",
                ]
            } else if !assistant_enabled && key_present {
                [
                    "1  Enable the Assistant",
                    "2  Review the default model",
                    "3  Return here and start chatting",
                ]
            } else {
                [
                    "1  Choose a provider",
                    "2  Add its API key",
                    "3  Return here and start chatting",
                ]
            };
            for step in steps {
                ui.label(RichText::new(step).small().color(pal.text_primary));
            }
            ui.add_space(10.0);
            let label = if assistant_enabled && key_present && !model_selected {
                "Choose model"
            } else if assistant_enabled {
                "Set up Assistant"
            } else {
                "Enable and set up Assistant"
            };
            if ui
                .add(
                    Button::new(RichText::new(label).color(Color32::WHITE))
                        .fill(pal.accent)
                        .corner_radius(CornerRadius::same(crate::frontend::theme::radius::CONTROL)),
                )
                .clicked()
            {
                open_settings = true;
            }
            ui.add_space(5.0);
            ui.label(
                RichText::new(format!("Current choice: {}", provider.label))
                    .small()
                    .color(pal.text_tertiary),
            );
        }
    });
    open_settings
}

/// Linear blend toward `b` (`t` = 0 → `a`, `t` = 1 → `b`), used for tinted
/// callout fills that stay readable on either theme.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::frontend::theme::mix(a, b, t)
}
