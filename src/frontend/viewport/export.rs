use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui::{Color32, Pos2, Rect, Vec2};

use crate::{
    domain::Structure,
    frontend::{AtomSelection, ViewportVisualState},
};

use super::{
    SurfaceCache, SurfaceCacheKey,
    camera::{Projector, ViewCamera, view_center_and_radius},
    gpu,
    render::{build_molecule_instances, build_surface_world_mesh},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ImageExportBackground {
    #[default]
    Viewport,
    White,
    Transparent,
    Custom(Color32),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedExportBackground {
    pub(crate) clear: Color32,
    pub(crate) substrate: Color32,
}

impl ImageExportBackground {
    pub(crate) fn resolve(
        self,
        visual: &ViewportVisualState,
        theme: Color32,
    ) -> ResolvedExportBackground {
        let color = match self {
            Self::Viewport => visual.resolve_background(theme),
            Self::White | Self::Transparent => Color32::WHITE,
            Self::Custom(color) => color,
        };
        ResolvedExportBackground {
            clear: if self == Self::Transparent {
                Color32::TRANSPARENT
            } else {
                color
            },
            substrate: color,
        }
    }
}

pub(crate) struct ViewportPngExport<'a> {
    pub(crate) background: ResolvedExportBackground,
    pub(crate) camera: ViewCamera,
    pub(crate) selection: &'a AtomSelection,
    pub(crate) visual_state: &'a ViewportVisualState,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_path: &'a Path,
}

pub(crate) struct PendingViewportPngExport {
    pub(crate) background: ImageExportBackground,
    pub(crate) structure: Structure,
    pub(crate) camera: ViewCamera,
    pub(crate) selection: AtomSelection,
    pub(crate) visual_state: ViewportVisualState,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) output_path: PathBuf,
}

impl PendingViewportPngExport {
    pub(crate) fn execute(
        self,
        exporter: &gpu::GpuExporter,
        ctx: &eframe::egui::Context,
    ) -> Result<()> {
        export_viewport_png(
            exporter,
            &self.structure,
            ViewportPngExport {
                background: self.background.resolve(
                    &self.visual_state,
                    crate::frontend::theme::Palette::for_scheme(
                        crate::frontend::theme::active_scheme(ctx),
                        ctx.global_style().visuals.dark_mode,
                    )
                    .viewport_bg,
                ),
                camera: self.camera,
                selection: &self.selection,
                visual_state: &self.visual_state,
                width: self.width,
                height: self.height,
                output_path: &self.output_path,
            },
        )
    }
}

pub(crate) fn export_viewport_png(
    exporter: &gpu::GpuExporter,
    structure: &Structure,
    export: ViewportPngExport<'_>,
) -> Result<()> {
    let ViewportPngExport {
        background,
        camera,
        selection,
        visual_state,
        width,
        height,
        output_path,
    } = export;
    if structure.atoms.is_empty() {
        let instances = Default::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32));
        let viewport = Projector::new(
            rect,
            nalgebra::Point3::origin(),
            1.0,
            1.0,
            camera.yaw,
            camera.pitch,
            camera.pan,
        );
        return gpu::export_png(
            exporter,
            &instances,
            &viewport,
            width,
            height,
            background,
            visual_state.lighting,
            output_path,
        );
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
    let mut instances = build_molecule_instances(structure, selection, visual_state);
    let surface_key = SurfaceCacheKey::new(0, 0, structure, visual_state);
    instances.surface = build_surface_world_mesh(
        structure,
        &surface_key,
        visual_state,
        &mut SurfaceCache::default(),
    );
    instances.surface_wireframe = visual_state.surface.style == super::SurfaceStyle::Mesh;
    gpu::export_png(
        exporter,
        &instances,
        &viewport,
        width,
        height,
        background,
        visual_state.lighting,
        output_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_resolution_keeps_explicit_sentinel_color() {
        let mut visual = ViewportVisualState::default();
        for theme in [Color32::WHITE, Color32::BLACK] {
            assert_eq!(
                ImageExportBackground::Viewport
                    .resolve(&visual, theme)
                    .clear,
                theme
            );
        }
        visual.background_color = Some(ViewportVisualState::DEFAULT_BACKGROUND);
        assert_eq!(
            ImageExportBackground::Viewport
                .resolve(&visual, Color32::BLACK)
                .clear,
            ViewportVisualState::DEFAULT_BACKGROUND
        );
        let transparent = ImageExportBackground::Transparent.resolve(&visual, Color32::BLACK);
        assert_eq!(transparent.clear, Color32::TRANSPARENT);
        assert_eq!(transparent.substrate, Color32::WHITE);
    }
}
