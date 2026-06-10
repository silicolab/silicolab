use eframe::egui::{self, Response, Vec2};

use crate::domain::Structure;

use super::{
    ViewportVisualState,
    camera::ViewCamera,
    render::{PickTarget, pick_atom},
};

#[derive(Default)]
pub struct ViewportInteraction {
    pub clicked_atom: Option<usize>,
    pub hovered_atom: Option<usize>,
    pub hover_changed: bool,
    pub camera_changed: bool,
    pub active_drag: bool,
}

pub(super) struct InteractionSystem<'a> {
    structure: &'a Structure,
    pick_targets: &'a [PickTarget],
    previous_hovered_atom: Option<usize>,
    visual_state: &'a ViewportVisualState,
}

impl<'a> InteractionSystem<'a> {
    pub(super) fn new(
        structure: &'a Structure,
        pick_targets: &'a [PickTarget],
        previous_hovered_atom: Option<usize>,
        visual_state: &'a ViewportVisualState,
    ) -> Self {
        Self {
            structure,
            pick_targets,
            previous_hovered_atom,
            visual_state,
        }
    }

    pub(super) fn run(
        self,
        ui: &egui::Ui,
        response: &Response,
        camera: &mut ViewCamera,
    ) -> ViewportInteraction {
        let camera_changed = update_camera_from_response(ui, response, camera);
        let active_drag = viewport_drag_active(response);
        let pointer_pos = ui.input(|input| input.pointer.interact_pos());
        let hovered_atom = response
            .hovered()
            .then_some(pointer_pos)
            .flatten()
            .and_then(|pointer| {
                pick_atom(
                    self.structure,
                    self.pick_targets,
                    pointer,
                    self.visual_state,
                )
            });
        let clicked_atom = response
            .clicked_by(egui::PointerButton::Primary)
            .then_some(pointer_pos)
            .flatten()
            .and_then(|pointer| {
                pick_atom(
                    self.structure,
                    self.pick_targets,
                    pointer,
                    self.visual_state,
                )
            });

        ViewportInteraction {
            clicked_atom,
            hovered_atom,
            hover_changed: hovered_atom != self.previous_hovered_atom,
            camera_changed,
            active_drag,
        }
    }
}

pub(super) fn update_camera_from_response(
    ui: &egui::Ui,
    response: &Response,
    camera: &mut ViewCamera,
) -> bool {
    let mut camera_changed = false;
    if response.dragged_by(egui::PointerButton::Primary) {
        let delta = ui.input(|input| input.pointer.delta());
        if delta != Vec2::ZERO {
            camera.yaw += delta.x * 0.005;
            camera.pitch = (camera.pitch + delta.y * 0.005).clamp(-1.45, 1.45);
            camera_changed = true;
        }
    }

    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        let delta = ui.input(|input| input.pointer.delta());
        if delta != Vec2::ZERO {
            camera.pan += delta;
            camera_changed = true;
        }
    }

    if response.hovered() {
        ui.input(|input| {
            if input.smooth_scroll_delta.y.abs() > f32::EPSILON {
                let next_zoom =
                    (camera.zoom - input.smooth_scroll_delta.y * 0.001).clamp(-0.8, 2.0);
                if (next_zoom - camera.zoom).abs() > f32::EPSILON {
                    camera.zoom = next_zoom;
                    camera_changed = true;
                }
            }
        });
    }

    camera_changed
}

pub(super) fn viewport_drag_active(response: &Response) -> bool {
    response.dragged_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
}
