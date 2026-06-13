use eframe::egui::{self, Align, Button, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui};

use crate::{
    backend::tasks::{TaskPanelKind, TaskStatus},
    frontend::{
        actions::AppAction,
        state::{AppState, PanelTab, PrimaryView},
        status_text,
    },
};

use super::{core_button_text_color, with_core_button_style};
pub(super) fn render_bottom_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 30.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            let pal = crate::frontend::theme::palette(ui);
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().button_padding = egui::vec2(10.0, 5.0);

            for tab in PanelTab::all() {
                let selected = state.ui.layout.active_panel_tab == *tab;
                let response = ui
                    .scope(|ui| {
                        configure_panel_tab_button_visuals(ui, selected);
                        ui.add(
                            Button::new(
                                RichText::new(tab.label())
                                    .color(core_button_text_color(&pal, selected)),
                            )
                            .selected(selected),
                        )
                    })
                    .inner;
                if response.clicked() {
                    state.ui.layout.active_panel_tab = *tab;
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if with_core_button_style(ui, false, |ui| {
                    ui.add_sized(
                        [28.0, 28.0],
                        Button::new(
                            RichText::new(egui_phosphor::regular::CARET_DOWN)
                                .color(core_button_text_color(&pal, false)),
                        ),
                    )
                })
                .on_hover_text("Hide panel")
                .clicked()
                {
                    state.ui.layout.show_panel = false;
                }
            });
        },
    );
    weak_panel_hairline(ui, 22);

    // Render the active tab directly in the panel body; each tab fills the
    // remaining height with a scroll area (`auto_shrink([false, false])`). The
    // panel's height is fixed by `exact_size` in `render_workspace` — see the
    // note there about the runaway growth that a resizable panel hit.
    ui.set_width(ui.available_width());
    match state.ui.layout.active_panel_tab {
        PanelTab::Output => render_output_panel(state, ui),
        PanelTab::Console => render_console_panel(state, ui, actions),
        PanelTab::TaskMonitor => render_task_monitor_panel(state, ui, actions),
    }
}

fn configure_panel_tab_button_visuals(ui: &mut Ui, selected: bool) {
    let pal = crate::frontend::theme::palette(ui);
    let inactive_fill = egui::Color32::TRANSPARENT;
    let hovered_fill = pal.neutral_overlay(18);
    let selected_fill = pal.blue_overlay(58);
    let selected_hover_fill = pal.blue_overlay(74);
    let text_color = core_button_text_color(&pal, selected);
    let selected_text = core_button_text_color(&pal, true);
    let visuals = &mut ui.style_mut().visuals.widgets;

    visuals.inactive.weak_bg_fill = inactive_fill;
    visuals.inactive.bg_fill = inactive_fill;
    visuals.inactive.bg_stroke = Stroke::NONE;
    visuals.inactive.fg_stroke.color = text_color;

    visuals.hovered.weak_bg_fill = hovered_fill;
    visuals.hovered.bg_fill = hovered_fill;
    visuals.hovered.bg_stroke = Stroke::NONE;
    visuals.hovered.fg_stroke.color = selected_text;

    visuals.active.weak_bg_fill = selected_hover_fill;
    visuals.active.bg_fill = selected_hover_fill;
    visuals.active.bg_stroke = Stroke::NONE;
    visuals.active.fg_stroke.color = selected_text;

    visuals.open.weak_bg_fill = selected_fill;
    visuals.open.bg_fill = selected_fill;
    visuals.open.bg_stroke = Stroke::NONE;
    visuals.open.fg_stroke.color = selected_text;
}

fn render_output_panel(state: &mut AppState, ui: &mut egui::Ui) {
    ui.set_width(ui.available_width());
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for line in &state.output_log {
                ui.monospace(line);
            }
        });
}

fn render_console_panel(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
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
                            .wrap_mode(egui::TextWrapMode::Extend),
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

fn weak_panel_hairline(ui: &mut egui::Ui, alpha: u8) {
    let pal = crate::frontend::theme::palette(ui);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, pal.neutral_overlay(alpha)),
    );
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

fn render_task_monitor_panel(
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
