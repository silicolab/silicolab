use super::*;

use std::f32::consts::TAU;

use eframe::egui::Color32;
use nalgebra::{Point3, Vector3};

use crate::frontend::{LightPreset, ViewportCartoonState, ViewportVisualState};

use super::super::super::camera::Projector;
use super::super::{PrimitiveMeshVertex, darken, lighten, normalize_vector3};

/// Corner-rounding of the swept ribbon cross-section, as a fraction of the
/// half-thickness. Wide/thin styles (helix, sheet) become flat ribbons with
/// rounded edges; round styles (coil, where width≈thickness) become tubes.
const CARTOON_ROUNDNESS: f32 = 0.85;

/// A point on the swept ribbon cross-section: a 2D offset in the (side, normal)
/// frame and the 2D outward normal in that frame. The outline is the boundary of
/// a rounded rectangle (Minkowski sum of a rectangle and a disc), so wide/thin
/// styles read as flat ribbons with rounded edges and round styles as tubes.
#[derive(Clone, Copy)]
struct CrossSectionPoint {
    offset: [f32; 2],
    normal: [f32; 2],
}

fn ribbon_cross_section(style: CartoonStyle, segments: usize) -> Vec<CrossSectionPoint> {
    let half_width = style.half_width.max(0.02);
    let half_thickness = style.half_thickness.max(0.02);
    let radius = CARTOON_ROUNDNESS * half_width.min(half_thickness);
    let inner_width = (half_width - radius).max(0.0);
    let inner_thickness = (half_thickness - radius).max(0.0);
    (0..segments)
        .map(|index| {
            // The half-step offset keeps samples off the exact axis directions,
            // where the rounded-rectangle support mapping is ambiguous, so the
            // flat faces come out as clean chords between corner samples.
            let angle = TAU * (index as f32 + 0.5) / segments as f32;
            let (sin_angle, cos_angle) = angle.sin_cos();
            CrossSectionPoint {
                offset: [
                    cos_angle.signum() * inner_width + radius * cos_angle,
                    sin_angle.signum() * inner_thickness + radius * sin_angle,
                ],
                normal: [cos_angle, sin_angle],
            }
        })
        .collect()
}

/// World-space (position, outward normal) ring for the swept cross-section at one
/// sweep sample.
pub(crate) fn cartoon_ring_geometry(
    sample: &CartoonSweepSample,
    segments: usize,
) -> Vec<(Point3<f32>, Vector3<f32>)> {
    ribbon_cross_section(sample.style, segments)
        .iter()
        .map(|cross_section| {
            let position = sample.position
                + sample.side * cross_section.offset[0]
                + sample.normal * cross_section.offset[1];
            let normal = normalize_vector3(
                sample.side * cross_section.normal[0] + sample.normal * cross_section.normal[1],
                sample.normal,
            );
            (position, normal)
        })
        .collect()
}

pub(crate) fn build_cartoon_ring(
    viewport: &Projector,
    sample: &CartoonSweepSample,
    segments: usize,
    visual_state: &ViewportVisualState,
) -> Vec<PrimitiveMeshVertex> {
    cartoon_ring_geometry(sample, segments)
        .into_iter()
        .map(|(position, normal)| {
            let projected = viewport.project(position);
            PrimitiveMeshVertex {
                pos: projected.pos,
                depth: projected.depth,
                color: shade_cartoon_color(
                    viewport,
                    sample.color,
                    normal,
                    visual_state.lighting.preset,
                ),
            }
        })
        .collect()
}

pub(crate) fn cartoon_style(
    kind: CartoonSegmentKind,
    settings: &ViewportCartoonState,
) -> CartoonStyle {
    let section = match kind {
        CartoonSegmentKind::Helix => settings.helix,
        CartoonSegmentKind::Sheet => settings.sheet,
        CartoonSegmentKind::Coil => settings.coil,
    };
    CartoonStyle {
        half_width: section.width * 0.5,
        half_thickness: section.thickness * 0.5,
    }
}

pub(crate) fn cartoon_segment_color(base: Color32, kind: CartoonSegmentKind) -> Color32 {
    match kind {
        CartoonSegmentKind::Helix => lighten(base, 0.04),
        CartoonSegmentKind::Sheet => lighten(base, 0.12),
        CartoonSegmentKind::Coil => darken(base, 0.02),
    }
}

pub(crate) fn lerp_cartoon_style(start: CartoonStyle, end: CartoonStyle, t: f32) -> CartoonStyle {
    CartoonStyle {
        half_width: start.half_width + (end.half_width - start.half_width) * t,
        half_thickness: start.half_thickness + (end.half_thickness - start.half_thickness) * t,
    }
}

pub(crate) fn shade_cartoon_color(
    viewport: &Projector,
    base_color: Color32,
    surface_normal: Vector3<f32>,
    light_preset: LightPreset,
) -> Color32 {
    let view_normal = normalize_vector3(
        viewport.rotate_to_view(surface_normal),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let light_direction =
        normalize_vector3(Vector3::new(-0.35, 0.45, 1.0), Vector3::new(0.0, 0.0, 1.0));
    let diffuse = view_normal.dot(&light_direction).max(0.0);
    let rim = (1.0 - view_normal.z.abs()).powi(2) * 0.18;
    let (ambient, diffuse_strength, rim_strength) = match light_preset {
        LightPreset::Soft => (0.38, 0.55, 1.0),
        LightPreset::Gentle => (0.48, 0.34, 0.65),
        LightPreset::Studio => (0.30, 0.72, 1.2),
    };
    let brightness = (ambient + diffuse * diffuse_strength + rim * rim_strength).clamp(0.0, 1.0);
    if brightness >= 0.5 {
        lighten(base_color, (brightness - 0.5) * 0.75)
    } else {
        darken(base_color, (0.5 - brightness) * 1.2)
    }
}
