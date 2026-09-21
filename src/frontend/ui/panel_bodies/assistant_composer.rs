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

const ATTACHMENT_CHIP_HEIGHT: f32 = 30.0;
const ATTACHMENT_ROW_HEIGHT: f32 = ATTACHMENT_CHIP_HEIGHT + 8.0;

/// Height of the input box, which grows by one row while PDFs are attached.
pub(crate) fn composer_height(attachment_count: usize) -> f32 {
    if attachment_count == 0 {
        ASSISTANT_COMPOSER_HEIGHT
    } else {
        ASSISTANT_COMPOSER_HEIGHT + ATTACHMENT_ROW_HEIGHT
    }
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
) -> bool {
    let busy = state.ui.agent.is_busy();
    let assistant_enabled = state.config.assistant.enabled;
    let key_present = state.ui.agent.key_available.unwrap_or(false);
    let provider =
        crate::frontend::agent::registry::provider_spec(&state.ui.agent.selection.provider);
    let model_selected = !state.ui.agent.selection.model.trim().is_empty()
        || provider.is_some_and(|provider| {
            matches!(
                provider.kind,
                crate::frontend::agent::registry::ProviderKind::ExternalAgent(_)
            )
        });
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
                    if label_resp.on_hover_text("Open in Activity").clicked()
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
    let can_submit = assistant_enabled && key_present && model_selected;
    let will_queue = busy || state.ui.agent.pending_approval().is_some();
    let hint = if !assistant_enabled {
        "Assistant disabled"
    } else if !key_present {
        "Set up API access above"
    } else if !model_selected {
        "Choose a model below"
    } else if busy {
        "Working… (Enter queues a follow-up)"
    } else {
        "Ask the assistant anything"
    };

    let composer_radius = radius::LARGE;
    let inner_radius = radius::concentric(composer_radius, 5);
    let mut composer_focused = false;
    let mut send = false;
    let mut open_assistant_settings = false;
    let mut composer_rect = egui::Rect::NOTHING;
    let composer_height = composer_height(state.ui.agent.attachments.len());
    assistant_inset_row(ui, composer_height, |ui| {
        let content_width = ui.available_width().max(96.0);
        let frame_inner_width = (content_width - 16.0).max(80.0);
        let response = Frame::default()
            .fill(pal.input_fill)
            .stroke(Stroke::new(1.0_f32, pal.hairline))
            .corner_radius(CornerRadius::same(composer_radius))
            .inner_margin(Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.set_width(frame_inner_width);
                if !state.ui.agent.attachments.is_empty()
                    && let Some(index) = render_attachment_chips(
                        ui,
                        pal,
                        &state.ui.agent.attachments,
                        frame_inner_width,
                        true,
                    )
                {
                    actions.push(AppAction::RemoveAgentAttachment(index));
                }
                let response = ui.add_sized(
                    [frame_inner_width, 42.0],
                    egui::TextEdit::multiline(&mut state.ui.agent.input)
                        .desired_width(frame_inner_width)
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

                ui.add_space(6.0);
                let (toolbar_rect, _) = ui
                    .allocate_exact_size(egui::vec2(frame_inner_width, 28.0), egui::Sense::hover());
                const BUTTON_SIZE: f32 = 28.0;
                const CONTROL_GAP: f32 = 4.0;
                const ACTION_GAP: f32 = 6.0;

                // Give actions explicit right-anchored rectangles. The controls
                // are clipped to the remaining rectangle and therefore cannot
                // paint over or displace Send, even at the 240px sidebar minimum.
                let send_rect = egui::Rect::from_min_size(
                    egui::pos2(toolbar_rect.right() - BUTTON_SIZE, toolbar_rect.top()),
                    egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(send_rect)
                        .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
                    |ui| {
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
                            send_resp.on_hover_text("Send")
                        };
                        if send_resp.clicked() {
                            send = true;
                        }
                    },
                );

                let mut controls_right = send_rect.left() - ACTION_GAP;
                if busy {
                    let stop_rect = egui::Rect::from_min_size(
                        egui::pos2(controls_right - BUTTON_SIZE, toolbar_rect.top()),
                        egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(stop_rect)
                            .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
                        |ui| {
                            let stop = Button::new(
                                RichText::new(egui_phosphor::regular::STOP).color(pal.text_primary),
                            )
                            .fill(pal.neutral_overlay(20))
                            .corner_radius(CornerRadius::same(inner_radius))
                            .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                            if ui.add(stop).on_hover_text("Stop current task").clicked() {
                                actions.push(AppAction::CancelAgent);
                            }
                        },
                    );
                    controls_right = stop_rect.left() - ACTION_GAP;
                }

                let controls_rect = egui::Rect::from_min_max(
                    toolbar_rect.left_top(),
                    egui::pos2(
                        controls_right.max(toolbar_rect.left()),
                        toolbar_rect.bottom(),
                    ),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(controls_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.set_clip_rect(controls_rect);
                        ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                        ui.spacing_mut().interact_size.y = BUTTON_SIZE;
                        if controls_rect.width() >= BUTTON_SIZE + CONTROL_GAP + 96.0 {
                            let attach = Button::new(
                                RichText::new(egui_phosphor::regular::PAPERCLIP)
                                    .color(pal.text_primary),
                            )
                            .fill(pal.neutral_overlay(12))
                            .corner_radius(CornerRadius::same(inner_radius))
                            .min_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE));
                            if ui
                                .add(attach)
                                .on_hover_text("Add a PDF for the assistant to read")
                                .clicked()
                                && let Some(paths) = rfd::FileDialog::new()
                                    .add_filter("PDF", &["pdf"])
                                    .pick_files()
                            {
                                actions.push(AppAction::AttachAgentDocuments(paths));
                            }
                        }
                        let controls_width = ui.available_width();
                        if controls_width <= 1.0 {
                            return;
                        }
                        let mode_width = if controls_width >= 150.0 {
                            72.0
                        } else if controls_width >= 96.0 {
                            56.0
                        } else {
                            (controls_width * 0.42).max(28.0)
                        };
                        let model_width =
                            (controls_width - mode_width - CONTROL_GAP).clamp(28.0, 132.0);

                        let selection = state.ui.agent.selection.clone();
                        let provider =
                            crate::frontend::agent::registry::provider_spec(&selection.provider)
                                .unwrap_or(&crate::frontend::agent::registry::PROVIDERS[0]);
                        // External CLIs ignore SilicoLab's approval policy; their
                        // own sandbox posture is the relevant control instead.
                        if matches!(
                            provider.kind,
                            crate::frontend::agent::registry::ProviderKind::ExternalAgent(_)
                        ) {
                            render_external_access_picker(state, ui, actions, mode_width);
                        } else {
                            render_approval_mode_picker(state, ui, actions, mode_width);
                        }

                        open_assistant_settings |= render_assistant_model_picker(
                            state,
                            ui,
                            actions,
                            provider,
                            &selection.model,
                            model_width,
                        );
                    },
                );
            });
        composer_rect = response.response.rect;
    });
    if composer_focused {
        ui.painter().rect_stroke(
            composer_rect,
            CornerRadius::same(composer_radius),
            Stroke::new(1.0_f32, pal.accent),
            egui::StrokeKind::Inside,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
    if can_submit && send {
        let message = state.ui.agent.input.trim().to_string();
        if !message.is_empty() || !state.ui.agent.attachments.is_empty() {
            actions.push(AppAction::SendAgentMessage(message));
            state.ui.agent.input.clear();
        }
    }
    open_assistant_settings
}

/// One row of attached-PDF chips. With `removable`, hovering a chip reveals a
/// remove button on its top-right corner; the clicked chip's index is returned.
pub(super) fn render_attachment_chips(
    ui: &mut egui::Ui,
    pal: &Palette,
    attachments: &[crate::io::llm::types::DocumentRef],
    row_width: f32,
    removable: bool,
) -> Option<usize> {
    const GAP: f32 = 6.0;
    const REMOVE_RADIUS: f32 = 8.0;
    let count = attachments.len() as f32;
    let chip_width = ((row_width - GAP * (count - 1.0)) / count).clamp(44.0, 168.0);
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(row_width, ATTACHMENT_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let mut removed = None;
    for (index, document) in attachments.iter().enumerate() {
        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(
                row_rect.left() + index as f32 * (chip_width + GAP),
                row_rect.top() + 4.0,
            ),
            egui::vec2(chip_width, ATTACHMENT_CHIP_HEIGHT),
        );
        if chip_rect.right() > row_rect.right() + 0.5 {
            break;
        }
        let chip = ui
            .interact(
                chip_rect,
                ui.id().with(("assistant-attachment", index)),
                egui::Sense::hover(),
            )
            .on_hover_text(format!(
                "{}\n{} pages",
                document.path.display(),
                document.pages
            ));
        ui.painter().rect(
            chip_rect,
            CornerRadius::same(radius::CONTROL),
            pal.neutral_overlay(14),
            Stroke::new(1.0_f32, pal.hairline),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(chip_rect.shrink2(egui::vec2(8.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.label(RichText::new(egui_phosphor::regular::FILE_PDF).color(pal.accent));
                ui.add(
                    egui::Label::new(
                        RichText::new(&document.name)
                            .small()
                            .color(pal.text_primary),
                    )
                    .truncate(),
                );
            },
        );

        let remove_center = egui::pos2(chip_rect.right() - 2.0, chip_rect.top() + 2.0);
        let remove_rect =
            egui::Rect::from_center_size(remove_center, egui::Vec2::splat(REMOVE_RADIUS * 2.0));
        if removable && (chip.hovered() || ui.rect_contains_pointer(remove_rect)) {
            let remove = ui
                .interact(
                    remove_rect,
                    ui.id().with(("assistant-attachment-remove", index)),
                    egui::Sense::click(),
                )
                .on_hover_text("Remove");
            ui.painter()
                .circle_filled(remove_center, REMOVE_RADIUS, pal.text_primary);
            ui.painter().text(
                remove_center,
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::X,
                egui::FontId::proportional(10.0),
                pal.input_fill,
            );
            if remove.clicked() {
                removed = Some(index);
            }
        }
    }
    removed
}

/// Compact, point-of-action permission control inspired by agent-first IDEs.
/// The persisted approval policy remains the single source of truth; this is a
/// second presentation of the same dispatcher action used in Settings.
fn render_approval_mode_picker(
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    width: f32,
) {
    use crate::backend::config::ApprovalMode;

    let current = state.config.assistant.approval_mode;
    let selected = compact_approval_label(current);
    let can_switch = state.ui.agent.can_manage_conversations();
    let response = ui
        .add_enabled_ui(can_switch, |ui| {
            egui::ComboBox::from_id_salt("assistant.composer_approval_mode")
                .selected_text(assistant_text(selected))
                .width(width.max(34.0))
                .truncate()
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.set_min_width(250.0);
                    for mode in ApprovalMode::all() {
                        if ui
                            .selectable_label(mode == current, assistant_text(mode.label()))
                            .clicked()
                            && mode != current
                        {
                            actions.push(AppAction::SetApprovalMode(mode));
                            ui.close();
                        }
                    }
                })
        })
        .inner;
    response
        .response
        .on_hover_text(format!("Agent permissions\n{}", current.label()));
}

/// Per-conversation sandbox posture for external CLI agents, shown in place of
/// the approval picker (which those agents ignore). Persisted on the active
/// conversation via the same dispatcher action used nowhere else.
fn render_external_access_picker(
    state: &AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    width: f32,
) {
    use crate::backend::config::ExternalAgentAccess;

    let current = state.ui.agent.external_access;
    let can_switch = state.ui.agent.can_manage_conversations();
    let response = ui
        .add_enabled_ui(can_switch, |ui| {
            egui::ComboBox::from_id_salt("assistant.composer_external_access")
                .selected_text(assistant_text(current.short_label()))
                .width(width.max(34.0))
                .truncate()
                .show_ui(ui, |ui| {
                    crate::frontend::theme::stabilize_selectable_rows(ui);
                    ui.set_min_width(250.0);
                    for access in ExternalAgentAccess::all() {
                        if ui
                            .selectable_label(access == current, assistant_text(access.label()))
                            .clicked()
                            && access != current
                        {
                            actions.push(AppAction::SetAssistantExternalAccess(access));
                            ui.close();
                        }
                    }
                })
        })
        .inner;
    response
        .response
        .on_hover_text(format!("External agent access\n{}", current.label()));
}

fn compact_approval_label(mode: crate::backend::config::ApprovalMode) -> &'static str {
    use crate::backend::config::ApprovalMode;
    match mode {
        ApprovalMode::Manual => "Manual",
        ApprovalMode::AutoSafe => "Safe",
        ApprovalMode::Auto => "Auto",
        ApprovalMode::Plan => "Plan",
    }
}
