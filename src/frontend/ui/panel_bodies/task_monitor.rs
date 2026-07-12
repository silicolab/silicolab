use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, ScrollArea};

use crate::{
    backend::tasks::{TaskPanelKind, TaskStatus},
    frontend::{
        actions::AppAction,
        state::{AppState, OutputTarget, PrimaryView, StatusSeverity},
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

        // The system monitor normally lives in the primary-sidebar footer; only
        // fall back to the status bar when that sidebar is hidden, so the gauges
        // stay visible without ever showing in two places at once.
        if state.config.show_utilization_bars && !state.ui.layout.show_primary_sidebar {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                super::render_status_monitor(state, ui, actions);
                ui.separator();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    render_status_notice(state, ui, actions, &pal);
                });
            });
        } else {
            render_status_notice(state, ui, actions, &pal);
        }
    });
}

/// Render the single status notice: a severity icon and color, the (visually
/// truncated) text as a link when it carries a detail target, a dismiss control
/// for sticky notices, and expiry paused while it is hovered or focused.
fn render_status_notice(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
    pal: &crate::frontend::theme::Palette,
) {
    let Some(notice) = state.status_notice() else {
        state.set_status_paused(false, std::time::Instant::now());
        return;
    };
    let severity = notice.severity;
    let text = notice.text.clone();
    let target = notice.target.clone();
    let (icon, color) = severity_style(severity, pal);

    ui.label(RichText::new(icon).color(color));
    let label = RichText::new(&text).color(color);
    let response = match &target {
        Some(_) => ui.link(label).on_hover_text(&text),
        None => ui.label(label).on_hover_text(&text),
    };
    // Hovering or focusing a notice (linked ones are focusable) freezes its expiry
    // so it cannot vanish while the user reads or reaches for it.
    let paused = response.hovered() || response.has_focus();
    state.set_status_paused(paused, std::time::Instant::now());
    if response.clicked()
        && let Some(target) = target
    {
        actions.push(AppAction::OpenDetailTarget(target));
    }
    if severity.is_sticky()
        && ui
            .add(egui::Button::new(RichText::new(egui_phosphor::regular::X).small()).frame(false))
            .on_hover_text("Dismiss")
            .clicked()
    {
        actions.push(AppAction::AcknowledgeStatus);
    }
}

/// Map severity to a text/icon and color pair, so severity is never signalled by
/// color alone.
fn severity_style(
    severity: StatusSeverity,
    pal: &crate::frontend::theme::Palette,
) -> (&'static str, egui::Color32) {
    match severity {
        StatusSeverity::Neutral => (egui_phosphor::regular::INFO, pal.text_primary),
        StatusSeverity::Success => (egui_phosphor::regular::CHECK_CIRCLE, pal.status_green),
        StatusSeverity::Warning => (egui_phosphor::regular::WARNING, pal.status_amber),
        StatusSeverity::Error => (egui_phosphor::regular::WARNING_OCTAGON, pal.status_red),
    }
}

/// One shared visual tone for a run's status, so a Task row and a live Job card
/// read from the same vocabulary and the same colours.
#[derive(Clone, Copy)]
enum StatusTone {
    /// Blue — queued / ready, nothing happening yet.
    Idle,
    /// Green — running, or a successful terminal state.
    Active,
    /// Amber — cancelling/cancelled/lost, or waiting on the user.
    Attention,
    /// Red — failed or interrupted.
    Failure,
}

impl StatusTone {
    fn color(self, pal: &crate::frontend::theme::Palette) -> egui::Color32 {
        match self {
            Self::Idle => pal.status_blue,
            Self::Active => pal.status_green,
            Self::Attention => pal.status_amber,
            Self::Failure => pal.status_red,
        }
    }
}

fn task_tone(status: TaskStatus) -> StatusTone {
    match status {
        TaskStatus::Ready => StatusTone::Idle,
        TaskStatus::Running | TaskStatus::Completed => StatusTone::Active,
        TaskStatus::WaitingInput | TaskStatus::Cancelling | TaskStatus::Cancelled => {
            StatusTone::Attention
        }
        TaskStatus::Failed | TaskStatus::Interrupted => StatusTone::Failure,
    }
}

fn job_tone(status: crate::frontend::jobs::JobStatus) -> StatusTone {
    use crate::frontend::jobs::JobStatus;
    match status {
        JobStatus::Queued => StatusTone::Idle,
        JobStatus::Running | JobStatus::Done => StatusTone::Active,
        JobStatus::Cancelling | JobStatus::Lost | JobStatus::Cancelled => StatusTone::Attention,
        JobStatus::Failed => StatusTone::Failure,
    }
}

/// The single status-badge renderer: coloured strong text sharing one tone
/// vocabulary across Task rows and live Job cards.
fn status_badge(
    pal: &crate::frontend::theme::Palette,
    label: impl Into<String>,
    tone: StatusTone,
) -> RichText {
    RichText::new(label.into()).strong().color(tone.color(pal))
}

/// A note for a task whose result import needs attention: a remote
/// result awaiting recovery, or one that failed to import. Settled imports
/// (Applied / NotRequired / still Pending) show nothing.
fn import_state_note(
    import_state: crate::backend::run_attempt::ResultImport,
) -> Option<(&'static str, StatusTone)> {
    use crate::backend::run_attempt::ResultImport;
    match import_state {
        ResultImport::PendingRecovery => Some(("Result pending recovery", StatusTone::Attention)),
        ResultImport::Failed => Some(("Result import failed", StatusTone::Failure)),
        ResultImport::NotRequired | ResultImport::Pending | ResultImport::Applied => None,
    }
}

/// The status text for a live job card: a local job shows its status directly; a
/// remote job is prefixed "Last known:" and, when unreachable, annotated
/// with a freshness note so a stale status never reads as live.
fn job_status_label(job: &crate::frontend::jobs::LiveJobSnapshot) -> String {
    let base = if job.backend == crate::frontend::jobs::JobBackend::RemoteRegistry {
        format!("Last known: {}", job.status.label())
    } else {
        job.status.label().to_string()
    };
    if matches!(
        job.observation,
        Some(crate::job::ObservationState::Unreachable)
    ) {
        format!("{base} · connection unavailable")
    } else {
        base
    }
}

pub(crate) fn render_task_monitor_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
        ui.label(RichText::new("Activity").strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button(format!("{}  Launch", egui_phosphor::regular::LIGHTNING))
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
                // Where it actually ran, derived from the real execution,
                // not a predicted catalog label; None until it begins one.
                state.tasks.runs.placement_label(task.id),
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
                placement,
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
                // The durable import state of the task's current attempt: a
                // remote result whose file is missing surfaces as pending recovery
                // rather than being silently dropped.
                let import_state = state.tasks.runs.attempt_import_state(task_id);
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
                                    ui.label(status_badge(&pal, status.label(), task_tone(status)));
                                });
                            });
                            ui.add_space(4.0);
                            let meta = match &placement {
                                Some(placement) => {
                                    format!("{controller_id} / {placement} / {outcome}")
                                }
                                None => format!("{controller_id} / {outcome}"),
                            };
                            ui.label(RichText::new(meta).small().color(pal.text_tertiary));
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
                                ui.horizontal_wrapped(|ui| {
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
                                    // Jump to the run's result — the entry it produced,
                                    // or the input it anchored to for a report.
                                    if let Some(anchor) = result_entry_id.or(source_entry_id)
                                        && ui.small_button("Open Result").clicked()
                                    {
                                        actions.push(AppAction::ActivateEntry(anchor));
                                    }
                                });
                            }
                            if let Some((note, tone)) = import_state_note(import_state) {
                                ui.label(status_badge(&pal, note, tone));
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
        let targeted = job.job_id == state.ui.activity_job_target;
        let stroke = if targeted {
            egui::Stroke::new(2.0_f32, pal.accent)
        } else {
            egui::Stroke::default()
        };
        let response = Frame::group(ui.style())
            .stroke(stroke)
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
                        if job.can_cancel() && ui.button("Cancel").clicked() {
                            actions.push(AppAction::CancelControlledJob(job.id.clone()));
                        }
                        ui.label(status_badge(
                            &pal,
                            job_status_label(job),
                            job_tone(job.status),
                        ));
                    });
                });
                // The card projects the canonical exact-`JobId` log tail (it owns no
                // second buffer) and deep-links Output to that same execution.
                if let Some(job_id) = job.job_id {
                    if ui
                        .small_button(format!(
                            "{}  Open in Output",
                            egui_phosphor::regular::ARROW_SQUARE_OUT
                        ))
                        .clicked()
                    {
                        actions.push(AppAction::RevealOutput(OutputTarget::Job(job_id)));
                    }
                    for entry in state.session_log().tail_for_job(job_id, 6) {
                        ui.monospace(&entry.text);
                    }
                }
            });
        if targeted {
            response.response.scroll_to_me(Some(Align::Center));
        }
        ui.add_space(4.0);
    }
    ui.add_space(8.0);
}

fn render_active_task_summary(state: &AppState, ui: &mut egui::Ui) {
    let pal = crate::frontend::theme::palette(ui);

    // Collect into owned data first so the borrow on state.tasks ends before
    // we read state.jobs below.
    let running: Vec<(
        String,
        TaskStatus,
        &'static str,
        Option<String>,
        &'static str,
    )> = state
        .tasks
        .running_task_runs()
        .iter()
        .map(|t| {
            (
                t.title.clone(),
                t.status,
                t.controller_id,
                state.tasks.runs.placement_label(t.id),
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
            for (title, status, controller_id, placement, outcome) in &running {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title.as_str()).strong());
                    ui.label(status_badge(&pal, status.label(), task_tone(*status)));
                });
                let meta = match placement {
                    Some(placement) => format!("{controller_id} / {placement} / {outcome}"),
                    None => format!("{controller_id} / {outcome}"),
                };
                ui.label(RichText::new(meta).small().color(pal.text_tertiary));
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
            if let Some(job_id) = state
                .jobs
                .local_execution(crate::frontend::jobs::LocalJobSlot::Engine)
            {
                for entry in state.session_log().tail_for_job(job_id, 6) {
                    ui.monospace(&entry.text);
                }
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
