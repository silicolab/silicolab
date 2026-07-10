use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, ScrollArea};

use crate::{
    backend::tasks::{TaskPanelKind, TaskStatus},
    frontend::{
        actions::AppAction,
        state::{AppState, PrimaryView},
        status_text,
    },
};

pub(crate) fn render_status_bar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(status_text(state.structure(), &state.ui.selection))
                .color(pal.text_primary),
        );
        ui.separator();
        ui.label(RichText::new(&state.message).color(pal.text_primary));

        // The system monitor normally lives in the primary-sidebar footer; only
        // fall back to the status bar when that sidebar is hidden, so the gauges
        // stay visible without ever showing in two places at once.
        if state.config.show_utilization_bars && !state.ui.layout.show_primary_sidebar {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                super::render_status_monitor(state, ui, actions);
            });
        }
    });
}

fn task_status_badge(pal: &crate::frontend::theme::Palette, status: TaskStatus) -> RichText {
    let color = match status {
        TaskStatus::Ready => pal.status_blue,
        TaskStatus::WaitingInput => pal.status_amber,
        TaskStatus::Running => pal.status_green,
        TaskStatus::Cancelling => pal.status_amber,
        TaskStatus::Completed => pal.status_green,
        TaskStatus::Failed => pal.status_red,
        TaskStatus::Cancelled => pal.status_amber,
    };

    RichText::new(status.label()).strong().color(color)
}

pub(crate) fn render_task_monitor_panel(
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
            // Opt-in status refresh for detached remote jobs (never automatic).
            if ui
                .button(format!(
                    "{}  Refresh Remote",
                    egui_phosphor::regular::ARROWS_CLOCKWISE
                ))
                .clicked()
            {
                actions.push(AppAction::RefreshRemoteJobs);
            }
        });
    });
    ui.separator();

    render_active_task_summary(state, ui);
    ui.add_space(8.0);

    render_controlled_jobs(state, ui, actions);

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

fn job_status_badge(
    pal: &crate::frontend::theme::Palette,
    status: crate::frontend::jobs::JobStatus,
    remote: bool,
) -> RichText {
    let color = match status {
        crate::frontend::jobs::JobStatus::Queued => pal.status_blue,
        crate::frontend::jobs::JobStatus::Running => pal.status_green,
        crate::frontend::jobs::JobStatus::Done => pal.status_green,
        crate::frontend::jobs::JobStatus::Failed => pal.status_red,
        crate::frontend::jobs::JobStatus::Cancelling
        | crate::frontend::jobs::JobStatus::Lost
        | crate::frontend::jobs::JobStatus::Cancelled => pal.status_amber,
    };
    let label = if remote {
        format!("Last known: {}", status.label())
    } else {
        status.label().to_string()
    };
    RichText::new(label).strong().color(color)
}

fn render_controlled_jobs(state: &AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let jobs = crate::frontend::jobs::list_controlled_jobs(state);
    if jobs.is_empty() {
        return;
    }
    let pal = crate::frontend::theme::palette(ui);
    ui.label(RichText::new("Jobs").strong());
    if jobs
        .iter()
        .any(|job| job.backend == crate::frontend::jobs::JobBackend::RemoteRegistry)
    {
        ui.label(
            RichText::new("Remote status is last-known; use Refresh Remote to probe it.")
                .small()
                .color(pal.text_tertiary),
        );
    }
    ui.add_space(4.0);
    for job in &jobs {
        Frame::group(ui.style())
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("{} / {}", job.kind.label(), job.label)).strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} / {}{}",
                                job.id.token(),
                                job.backend.label(),
                                job.stage
                                    .as_ref()
                                    .map(|stage| format!(" / {stage}"))
                                    .unwrap_or_default()
                            ))
                            .small()
                            .color(pal.text_tertiary),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !job.status.is_running()
                            && let Some(run_uuid) = job.run_uuid.as_ref()
                            && ui.button("Remove scratch").clicked()
                        {
                            actions.push(AppAction::RemoveRemoteScratch(run_uuid.clone()));
                        }
                        if job.cancel.can_cancel() && ui.button("Cancel").clicked() {
                            actions.push(AppAction::CancelControlledJob(job.id.clone()));
                        }
                        ui.label(job_status_badge(
                            &pal,
                            job.status,
                            job.backend == crate::frontend::jobs::JobBackend::RemoteRegistry,
                        ));
                    });
                });
            });
        ui.add_space(4.0);
    }
    ui.add_space(8.0);
}

fn render_active_task_summary(state: &AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);

    // Collect into owned data first so the borrow on state.tasks ends before
    // we read state.jobs below.
    let running: Vec<(String, TaskStatus, &'static str, &'static str, &'static str)> = state
        .tasks
        .running_task_runs()
        .iter()
        .map(|t| {
            (
                t.title.clone(),
                t.status,
                t.controller_id,
                t.backend.label(),
                t.outcome.label(),
            )
        })
        .collect();

    let frame = Frame::group(ui.style()).inner_margin(Margin::same(8));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Active").strong());
        ui.add_space(4.0);

        if running.is_empty() {
            ui.label(
                RichText::new("No active task.")
                    .small()
                    .color(pal.text_tertiary),
            );
        } else {
            for (title, status, controller_id, backend, outcome) in &running {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title.as_str()).strong());
                    ui.label(task_status_badge(&pal, *status));
                });
                ui.label(
                    RichText::new(format!("{controller_id} / {backend} / {outcome}"))
                        .small()
                        .color(pal.text_tertiary),
                );
                ui.add_space(4.0);
            }
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
