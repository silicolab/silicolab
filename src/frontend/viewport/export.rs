use std::path::Path;

use anyhow::Result;
use eframe::egui::{Pos2, Rect, Vec2};

use crate::{
    domain::Structure,
    frontend::{AtomSelection, ViewportVisualState},
};

use super::{
    camera::{Projector, ViewCamera, view_center_and_radius},
    composer::RepresentationComposer,
    render::{HeadlessCanvas, build_viewport_geometry, submit_scene_to_canvas},
};

pub(crate) struct ViewportPngExport<'a> {
    pub(crate) camera: ViewCamera,
    pub(crate) selection: &'a AtomSelection,
    pub(crate) visual_state: &'a ViewportVisualState,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_path: &'a Path,
}

pub(crate) fn export_viewport_png(
    structure: &Structure,
    export: ViewportPngExport<'_>,
) -> Result<()> {
    let ViewportPngExport {
        camera,
        selection,
        visual_state,
        width,
        height,
        output_path,
    } = export;
    let mut canvas = HeadlessCanvas::new(width, height, visual_state.background_color);
    if structure.atoms.is_empty() {
        return canvas.save(output_path);
    }

    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32));
    let (center, radius) = view_center_and_radius(structure, visual_state.show_cell);
    let viewport = Projector::new(
        rect,
        center,
        rect.width().min(rect.height()) * 0.35 * (1.0 + camera.zoom) / radius,
        radius * 3.2,
        camera.yaw,
        camera.pitch,
        camera.pan,
    );
    let geometry = build_viewport_geometry(structure, &viewport);
    let scene_result = RepresentationComposer::for_export(
        structure,
        &geometry,
        &viewport,
        selection,
        visual_state,
    )
    .build();
    submit_scene_to_canvas(&mut canvas, &scene_result.scene);
    canvas.save(output_path)
}
