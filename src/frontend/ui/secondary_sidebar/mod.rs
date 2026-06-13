use eframe::egui::{self, Align, Button, Frame, Layout, Margin, RichText, Stroke};

use crate::{
    backend::tasks::TaskPanelKind,
    frontend::{
        actions::AppAction,
        state::{AppState, CoordinateOptimizationScope, PanelTab},
    },
};

use super::{core_button_text_color, docked_sidebar_scroll_area, with_core_button_style};

mod md_run;
mod md_system;
mod stage_detail;
mod task_panels;

pub(crate) use md_run::*;
pub(crate) use md_system::*;
pub(crate) use stage_detail::*;
pub(crate) use task_panels::*;

pub(crate) fn render_secondary_sidebar(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    docked_sidebar_scroll_area()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            render_secondary_sidebar_content(state, ui, actions);
        });
}

pub(crate) fn render_secondary_sidebar_content(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    let pal = crate::frontend::theme::palette(ui);
    let panels = state
        .tasks
        .panels
        .iter()
        .map(|panel| panel.task_run_id)
        .collect::<Vec<_>>();

    // Single header row: task tabs (or a hint) on the left, the hide-sidebar
    // button pinned to the right so it never costs a dedicated row.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if with_core_button_style(ui, false, |ui| {
                ui.add_sized(
                    [28.0, 28.0],
                    Button::new(
                        RichText::new(egui_phosphor::regular::CARET_RIGHT)
                            .color(core_button_text_color(&pal, false)),
                    ),
                )
            })
            .on_hover_text("Hide sidebar")
            .clicked()
            {
                state.ui.layout.show_secondary_sidebar = false;
            }

            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if panels.is_empty() {
                    ui.label("Double-click a task to open.");
                    return;
                }
                for task_run_id in &panels {
                    let Some(task) = state.tasks.task_run(*task_run_id) else {
                        continue;
                    };
                    let title = task.title.clone();
                    let active = state.tasks.active_panel == Some(*task_run_id);
                    Frame::group(ui.style())
                        .stroke(Stroke::new(
                            1.0,
                            if active { pal.accent } else { pal.hairline },
                        ))
                        .inner_margin(Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.selectable_label(active, title).clicked() {
                                    actions.push(AppAction::ActivateTaskPanel(*task_run_id));
                                }
                                if ui
                                    .add(egui::Button::new(egui_phosphor::regular::X).frame(false))
                                    .on_hover_text("Close task panel")
                                    .clicked()
                                {
                                    actions.push(AppAction::CloseTaskPanel(*task_run_id));
                                }
                            });
                        });
                }
            });
        });
    });
    ui.separator();

    if panels.is_empty() {
        return;
    }

    let Some(active_task_run_id) = state.tasks.active_panel else {
        ui.label("Select a task tab to continue.");
        return;
    };
    let Some(task) = state.tasks.task_run(active_task_run_id).cloned() else {
        ui.label("Task panel is unavailable.");
        return;
    };

    ui.label(RichText::new(task.title).strong());
    ui.label(
        RichText::new(format!(
            "{} / {} / {}",
            task.theme, task.method, task.application
        ))
        .small()
        .color(pal.text_tertiary),
    );
    ui.separator();

    match task.panel {
        TaskPanelKind::ReticularBuilder => render_framework_task_panel(state, ui, actions),
        TaskPanelKind::NanosheetBuilder => render_nanosheet_task_panel(state, ui, actions),
        TaskPanelKind::BuildingBlockEditor => render_building_block_task_panel(state, ui, actions),
        TaskPanelKind::OptimizationPrompt => render_optimization_task_panel(state, ui, actions),
        TaskPanelKind::QmPrompt => render_qm_task_panel(state, ui, actions),
        TaskPanelKind::SupercellPrompt => render_supercell_task_panel(state, ui, actions),
        TaskPanelKind::ProteinPrepPrompt => render_protein_prep_task_panel(state, ui, actions),
        TaskPanelKind::MdSystemPrompt => render_md_system_task_panel(state, ui, actions),
        TaskPanelKind::MdRunPrompt => render_md_run_task_panel(state, ui, actions),
        TaskPanelKind::None => {
            ui.label("This task runs directly and does not need a panel.");
            if ui
                .button(format!("{}  Close", egui_phosphor::regular::X))
                .clicked()
            {
                actions.push(AppAction::CloseTaskPanel(active_task_run_id));
            }
        }
    }
}
