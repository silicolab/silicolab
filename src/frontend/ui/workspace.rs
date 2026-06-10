use eframe::egui::{self, Button, Frame, Margin, RichText, ScrollArea, Sense, Stroke, Ui};

use crate::frontend::{
    ViewportDrawArgs,
    actions::AppAction,
    draw_viewport,
    state::AppState,
    viewport::{HOVER_FRAME, STRUCTURE_INTERACTION_FRAME},
};

use super::bottom_panel::render_bottom_panel;

/// Structure id used for the viewport's geometry cache while a trajectory is
/// playing. `u64::MAX` cannot collide with a real entry id, so playback never
/// reuses or disturbs the active entry's own cached geometry.
const PLAYBACK_STRUCTURE_ID: u64 = u64::MAX;

pub(super) fn render_workspace(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    if state.ui.layout.show_panel {
        // Fixed height + driven by our own `panel_height`: egui 0.34's resizable
        // `Panel` persists the framed content rect as the next frame's size, and
        // with fill content that feeds back into a few-pixel-per-frame growth
        // that runs away on continuous repaints. `exact_size` pins the height so
        // it can't drift; the divider is resized by a custom handle (see
        // `show_workbench`) that writes `panel_height`, matching the sidebars.
        egui::Panel::bottom("bottom_panel")
            .exact_size(state.ui.layout.panel_height)
            .show_separator_line(false)
            .frame(
                Frame::default()
                    .fill(crate::frontend::theme::palette(ui).sidebar)
                    .inner_margin(Margin::symmetric(10, 8)),
            )
            .show_inside(ui, |ui| render_bottom_panel(state, ui, actions));
    }

    egui::CentralPanel::default()
        .frame(Frame::default().fill(crate::frontend::theme::palette(ui).central))
        .show_inside(ui, |ui| {
            // MD-output entries dock a trajectory playback bar at the bottom of
            // the viewport; the viewport fills the space above it.
            let is_md_entry = state
                .entries
                .active_entry()
                .map(|entry| entry.origin.is_md_run())
                .unwrap_or(false);
            if is_md_entry {
                let pal = crate::frontend::theme::palette(ui);
                egui::Panel::bottom("trajectory_controls")
                    .resizable(false)
                    .frame(
                        Frame::default()
                            .fill(pal.bottom_panel)
                            .inner_margin(Margin::symmetric(10, 6)),
                    )
                    .show_inside(ui, |ui| render_trajectory_controls(state, ui, actions));
            }
            if let Some(entry) = state.entries.active_entry() {
                let entry_id = entry.id;
                let ui_state = &mut state.ui;
                // During playback the viewport renders the trajectory's current
                // frame (the entry's topology with swapped coordinates) under a
                // dedicated structure id so the entry's own geometry cache is
                // untouched, with a fixed view so the camera doesn't drift.
                let playback = ui_state
                    .trajectory
                    .as_ref()
                    .filter(|playback| playback.entry_id == entry_id);
                let (structure, structure_id, structure_revision, view_override) =
                    if let Some(playback) = playback {
                        (
                            &playback.scratch,
                            PLAYBACK_STRUCTURE_ID,
                            playback.current_frame as u64,
                            Some((playback.view_center, playback.view_radius)),
                        )
                    } else {
                        (&entry.structure, entry_id, entry.revision, None)
                    };
                let viewport_interaction = draw_viewport(
                    ui,
                    ViewportDrawArgs {
                        structure,
                        structure_id,
                        structure_revision,
                        camera: &mut ui_state.camera,
                        selection: &ui_state.selection,
                        visual_state: &ui_state.viewport,
                        previous_hovered_atom: ui_state.hovered_atom,
                        cache: &mut ui_state.viewport_cache,
                        gpu_ready: ui_state.gpu_ready,
                        empty_state_hint: None,
                        view_override,
                    },
                );
                if viewport_interaction.hover_changed {
                    ui_state.hovered_atom = viewport_interaction.hovered_atom;
                }
                if viewport_interaction.camera_changed || viewport_interaction.active_drag {
                    ui.ctx().request_repaint_after(STRUCTURE_INTERACTION_FRAME);
                } else if viewport_interaction.hover_changed {
                    ui.ctx().request_repaint_after(HOVER_FRAME);
                }

                let mut assigned_atom = None;
                if let Some(index) = viewport_interaction.clicked_atom {
                    let toggle = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                    actions.push(AppAction::SelectAtom {
                        atom_index: index,
                        toggle,
                    });
                    if let Some(editor) = &mut ui_state.block_editor
                        && editor.apply_picked_atom(index)
                    {
                        assigned_atom = Some(index);
                    }
                }
                if let Some(index) = assigned_atom {
                    state.set_message(format!("Assigned atom {}", index + 1));
                }
            } else if !state.workspace.is_project() && state.entries.tabs.is_empty() {
                render_scratch_workspace(state, ui, actions);
            } else {
                let empty_structure = crate::domain::Structure::empty();
                let ui_state = &mut state.ui;
                let _ = draw_viewport(
                    ui,
                    ViewportDrawArgs {
                        structure: &empty_structure,
                        structure_id: 0,
                        structure_revision: 0,
                        camera: &mut ui_state.camera,
                        selection: &ui_state.selection,
                        visual_state: &ui_state.viewport,
                        previous_hovered_atom: ui_state.hovered_atom,
                        cache: &mut ui_state.viewport_cache,
                        gpu_ready: ui_state.gpu_ready,
                        empty_state_hint: state.entries.tabs.is_empty().then_some(
                            "Open a structure from File > Open, or drag and drop one here.",
                        ),
                        view_override: None,
                    },
                );
            }
        });
}

/// Playback bar for an MD-output entry: a load button before the trajectory is
/// decoded, then play/pause + a frame scrubber + a frame/time readout + close.
fn render_trajectory_controls(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    use egui_phosphor::regular as icons;

    let Some(entry_id) = state.entries.active_entry_id() else {
        return;
    };
    let pal = crate::frontend::theme::palette(ui);
    let loading = state
        .jobs
        .trajectory_load
        .as_ref()
        .map(|load| load.entry_id)
        == Some(entry_id);

    // Discover the per-stage trajectories that live next to the entry's recorded
    // trajectory, so a multi-step run can be replayed stage by stage. A relax-
    // only run (no trajectory) yields an empty list and just shows a hint.
    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let primary = state
        .entries
        .entry(entry_id)
        .and_then(|entry| entry.origin.trajectory().map(|path| path.to_path_buf()));
    let stages = match (&primary, &project_root) {
        (Some(primary), Some(root)) => {
            let mut stages = crate::frontend::trajectory::md_stage_trajectories(primary, root);
            // Fall back to the recorded trajectory itself if the directory scan
            // turned up nothing (e.g. the file was moved out of its run dir).
            if stages.is_empty() {
                stages.push(crate::frontend::trajectory::MdStage {
                    label: "MD".to_string(),
                    path: primary.clone(),
                });
            }
            stages
        }
        _ => Vec::new(),
    };

    // Snapshot the playback cursor so we don't hold a borrow while emitting
    // actions (the dispatcher mutates the same state next frame).
    let playback = state
        .ui
        .trajectory
        .as_ref()
        .filter(|playback| playback.entry_id == entry_id)
        .map(|playback| {
            (
                playback.playing,
                playback.current_frame,
                playback.frame_count(),
                playback.trajectory.time(playback.current_frame),
                playback.source.clone(),
            )
        });
    let active_source = playback.as_ref().map(|(.., source)| source.clone());

    ui.horizontal(|ui| {
        // Stage chips: one per step's trajectory, in run order. Clicking loads
        // that stage; the active stage is highlighted.
        for stage in &stages {
            let active = active_source.as_deref() == Some(stage.path.as_path());
            if ui
                .selectable_label(active, RichText::new(stage.label.as_str()).monospace())
                .on_hover_text("Play this step")
                .clicked()
            {
                actions.push(AppAction::LoadTrajectory(
                    entry_id,
                    Some(stage.path.clone()),
                ));
            }
        }
        if !stages.is_empty() {
            ui.separator();
        }

        if let Some((playing, current, count, time, _)) = playback {
            let icon = if playing { icons::PAUSE } else { icons::PLAY };
            if ui
                .button(RichText::new(icon).size(16.0))
                .on_hover_text(if playing { "Pause" } else { "Play" })
                .clicked()
            {
                actions.push(AppAction::ToggleTrajectoryPlay);
            }

            let last = count.saturating_sub(1);
            let mut frame = current;
            let slider = ui.add(
                egui::Slider::new(&mut frame, 0..=last)
                    .show_value(false)
                    .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
            );
            if slider.changed() {
                actions.push(AppAction::SetTrajectoryFrame(frame));
            }

            ui.label(
                RichText::new(format!("{} / {count}", current + 1))
                    .monospace()
                    .color(pal.text_primary),
            );
            ui.label(RichText::new(format!("{time:.2} ps")).color(pal.text_tertiary));

            if ui
                .button(RichText::new(icons::X).size(14.0))
                .on_hover_text("Close trajectory")
                .clicked()
            {
                actions.push(AppAction::StopTrajectory);
            }
        } else if loading {
            ui.add(egui::Spinner::new());
            ui.label(RichText::new("Decoding trajectory…").color(pal.text_muted));
        } else if stages.len() > 1 {
            ui.label(
                RichText::new("Select a step to play back")
                    .small()
                    .color(pal.text_tertiary),
            );
        } else if let Some(stage) = stages.first() {
            if ui
                .button(RichText::new(format!("{}  Play trajectory", icons::PLAY)))
                .clicked()
            {
                actions.push(AppAction::LoadTrajectory(
                    entry_id,
                    Some(stage.path.clone()),
                ));
            }
            ui.label(
                RichText::new("from MD run output")
                    .small()
                    .color(pal.text_tertiary),
            );
        } else {
            ui.label(
                RichText::new("MD run output")
                    .small()
                    .color(pal.text_tertiary),
            );
            ui.label(
                RichText::new("— no trajectory recorded for this run")
                    .small()
                    .color(pal.text_muted),
            );
        }
    });
}

fn render_scratch_workspace(state: &mut AppState, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
    let content_width = ui.available_width().min(420.0);
    let recent_projects = state.recent_projects.clone();
    let pal = crate::frontend::theme::palette(ui);

    ui.vertical_centered(|ui| {
        ui.add_space(42.0);
        ui.set_width(content_width);
        ui.heading("Scratch temporary workspace");
        ui.add_space(4.0);
        ui.label(
            RichText::new("This workspace is not stored after SilicoLab closes.")
                .color(pal.text_muted),
        );
        ui.add_space(24.0);

        render_scratch_action_button(
            ui,
            egui_phosphor::regular::FOLDER_OPEN,
            "Open Project",
            AppAction::OpenProject,
            actions,
        );
        ui.add_space(8.0);
        render_scratch_action_button(
            ui,
            egui_phosphor::regular::FOLDER_PLUS,
            "Create a new project",
            AppAction::CreateProject,
            actions,
        );
        ui.add_space(8.0);
        render_scratch_action_button(
            ui,
            egui_phosphor::regular::FILE_PLUS,
            "Open file",
            AppAction::OpenFile,
            actions,
        );

        ui.add_space(34.0);
        ui.label(RichText::new("Recent Projects").strong());
        ui.add_space(10.0);

        if recent_projects.is_empty() {
            ui.label(RichText::new("No recent projects.").color(pal.text_tertiary));
            return;
        }

        ScrollArea::vertical()
            .max_height(ui.available_height().max(120.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(content_width);
                for project in recent_projects {
                    let response = Frame::default()
                        .fill(pal.item_fill)
                        .stroke(Stroke::new(1.0, pal.hairline))
                        .corner_radius(egui::CornerRadius::same(
                            crate::frontend::theme::radius::CARD,
                        ))
                        .inner_margin(Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.set_width(content_width - 24.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(egui_phosphor::regular::FOLDER_OPEN)
                                        .color(pal.accent),
                                );
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&project.name).strong());
                                    ui.label(
                                        RichText::new(project.path.display().to_string())
                                            .small()
                                            .color(pal.text_tertiary),
                                    );
                                });
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if response.clicked() {
                        actions.push(AppAction::OpenRecentProject(project.path));
                    }
                    ui.add_space(6.0);
                }
            });
    });
}

fn render_scratch_action_button(
    ui: &mut Ui,
    icon: &'static str,
    label: &'static str,
    action: AppAction,
    actions: &mut Vec<AppAction>,
) {
    let width = ui.available_width();
    let response = ui
        .scope(|ui| {
            let pal = crate::frontend::theme::palette(ui);
            let visuals = &mut ui.style_mut().visuals.widgets;
            visuals.inactive.weak_bg_fill = pal.item_fill;
            visuals.inactive.bg_fill = pal.item_fill;
            visuals.inactive.bg_stroke = Stroke::new(1.0, pal.hairline);
            visuals.hovered.weak_bg_fill = pal.item_fill_hover;
            visuals.hovered.bg_fill = pal.item_fill_hover;
            visuals.hovered.bg_stroke = Stroke::new(1.0, pal.hairline);
            // 44px-tall call-to-action buttons take the large radius step
            // (Apple sizes radii with control height).
            let large = egui::CornerRadius::same(crate::frontend::theme::radius::LARGE);
            for widget in [
                &mut visuals.noninteractive,
                &mut visuals.inactive,
                &mut visuals.hovered,
                &mut visuals.active,
                &mut visuals.open,
            ] {
                widget.corner_radius = large;
            }
            ui.add_sized(
                [width, 44.0],
                Button::new(
                    RichText::new(format!("{icon}  {label}"))
                        .size(14.0)
                        .color(pal.text_primary),
                ),
            )
        })
        .inner;
    if response.clicked() {
        actions.push(action);
    }
}
