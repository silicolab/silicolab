use super::*;

use eframe::egui::{self, Align, Layout, RichText, ScrollArea};

use crate::frontend::actions::AppAction;
use crate::frontend::agent::session::{AgentPhase, build_activity_snapshot, format_elapsed};
use crate::frontend::state::AppState;

/// Icon that conveys an Activity row's state; the subtitle carries the detail.
enum RowIcon {
    Done(bool),
    Running,
    Queued,
}

/// The trailing ✕ action for a row, if any.
enum RowAction {
    CancelJob(u64),
    RemoveQueued(usize),
}

pub(crate) fn render_activity_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let active_id = state.ui.agent.active_conversation;
    let running_jobs: Vec<(u64, String, i64)> = state
        .jobs
        .agent_jobs
        .iter()
        .filter(|job| job.conversation == active_id)
        .map(|job| (job.id, job.label.clone(), job.started_at_ms))
        .collect();
    let snapshot = build_activity_snapshot(state.ui.agent.active(), &running_jobs);
    let now = crate::backend::storage::jobs::now_ms();

    ui.add_space(6.0);

    assistant_inset_row(ui, 22.0, |ui| {
        ui.horizontal(|ui| {
            ui.label(assistant_text("Activity").color(pal.text_primary));
            if snapshot.total_count > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        assistant_small_text(format!(
                            "{} / {}",
                            snapshot.done_count, snapshot.total_count
                        ))
                        .color(pal.text_tertiary),
                    );
                });
            }
        });
    });

    if snapshot.total_count > 0 {
        let frac = (snapshot.done_count as f32 / snapshot.total_count as f32).clamp(0.0, 1.0);
        assistant_inset_row(ui, 9.0, |ui| {
            let width = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 5.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(2), pal.neutral_overlay(22));
            let mut fill = rect;
            fill.set_width(width * frac);
            ui.painter()
                .rect_filled(fill, egui::CornerRadius::same(2), pal.accent);
        });
    }

    if snapshot.busy {
        let label = match snapshot.phase {
            AgentPhase::AwaitingModel => "Thinking",
            AgentPhase::ExecutingTools => "Running tools",
            _ => "Working",
        };
        assistant_inset_row(ui, 20.0, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(
                    assistant_small_text(format!("{label} · step {}", snapshot.iterations))
                        .color(pal.text_tertiary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(assistant_small_text("Stop").color(pal.text_primary))
                                .frame(false),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::CancelAgent);
                    }
                });
            });
        });
    }
    if snapshot.busy || !running_jobs.is_empty() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }

    weak_panel_hairline(ui, 10);

    if snapshot.total_count == 0 && !snapshot.busy {
        ui.add_space(8.0);
        ui.label(assistant_small_text("No running or queued work.").color(pal.text_tertiary));
        return;
    }

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in &snapshot.done {
                activity_row(
                    ui,
                    &pal,
                    RowIcon::Done(row.ok),
                    &row.label,
                    &row.detail,
                    true,
                    None,
                    actions,
                );
            }
            for row in &snapshot.running {
                let elapsed = format_elapsed(now - row.started_at_ms);
                activity_row(
                    ui,
                    &pal,
                    RowIcon::Running,
                    &row.label,
                    &elapsed,
                    false,
                    Some(RowAction::CancelJob(row.job_id)),
                    actions,
                );
            }
            for row in &snapshot.queued {
                activity_row(
                    ui,
                    &pal,
                    RowIcon::Queued,
                    &row.preview,
                    "",
                    false,
                    Some(RowAction::RemoveQueued(row.index)),
                    actions,
                );
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn activity_row(
    ui: &mut egui::Ui,
    pal: &crate::frontend::theme::Palette,
    icon: RowIcon,
    title: &str,
    subtitle: &str,
    strike: bool,
    action: Option<RowAction>,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        match icon {
            RowIcon::Done(ok) => {
                let color = if ok { pal.accent } else { pal.status_red };
                ui.label(RichText::new(egui_phosphor::regular::CHECK_CIRCLE).color(color));
            }
            RowIcon::Running => {
                ui.add(egui::Spinner::new().size(13.0));
            }
            RowIcon::Queued => {
                ui.label(RichText::new(egui_phosphor::regular::CIRCLE).color(pal.text_tertiary));
            }
        }
        ui.vertical(|ui| {
            let title_color = if strike {
                pal.text_tertiary
            } else {
                pal.text_primary
            };
            let mut text = assistant_text(title).color(title_color);
            if strike {
                text = text.strikethrough();
            }
            ui.add(egui::Label::new(text).truncate());
            if !subtitle.is_empty() {
                ui.add(
                    egui::Label::new(assistant_small_text(subtitle).color(pal.text_tertiary))
                        .truncate(),
                );
            }
        });
        if let Some(action) = action {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if assistant_remove_button(ui, "Remove") {
                    match action {
                        RowAction::CancelJob(id) => actions.push(AppAction::CancelAgentJob(id)),
                        RowAction::RemoveQueued(index) => {
                            actions.push(AppAction::RemoveQueuedAgentInput(index))
                        }
                    }
                }
            });
        }
    });
    ui.add_space(4.0);
}
