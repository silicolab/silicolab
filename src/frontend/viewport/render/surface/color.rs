use eframe::egui::Color32;
use nalgebra::Vector3;

use crate::frontend::LightPreset;
use crate::frontend::viewport::camera::Projector;

use super::super::{darken, lighten, mix_color, normalize_vector3};

pub(super) fn shade_union_surface_color(
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
        normalize_vector3(Vector3::new(-0.30, 0.42, 1.0), Vector3::new(0.0, 0.0, 1.0));
    let half_vector = normalize_vector3(
        light_direction + Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let diffuse = view_normal.dot(&light_direction).max(0.0);
    let rim = (1.0 - view_normal.z.abs()).powi(2);
    let specular = view_normal.dot(&half_vector).max(0.0).powf(7.5);
    let (ambient, diffuse_strength, rim_strength, specular_strength) = match light_preset {
        LightPreset::Soft => (0.78, 0.16, 0.10, 0.05),
        LightPreset::Gentle => (0.82, 0.11, 0.07, 0.03),
        LightPreset::Studio => (0.70, 0.24, 0.12, 0.08),
    };
    let brightness =
        (ambient + diffuse * diffuse_strength + rim * rim_strength + specular * specular_strength)
            .clamp(0.0, 1.0);
    let lit = if brightness >= 0.72 {
        lighten(base_color, (brightness - 0.72) * 0.55)
    } else {
        darken(base_color, (0.72 - brightness) * 0.32)
    };
    Color32::from_rgba_unmultiplied(lit.r(), lit.g(), lit.b(), base_color.a())
}

/// Opaque base tint for the filled surface. The transparency is applied once,
/// after shading, by the caller — folding it in here as well would premultiply
/// the colour twice (see [`super::build_surface_fill_triangles`]).
pub(super) fn surface_fill_color(base_color: Color32) -> Color32 {
    mix_color(base_color, Color32::WHITE, 0.18)
}

pub(super) fn mesh_stroke_color(base_color: Color32, transparency: f32) -> Color32 {
    let tinted = darken(base_color, 0.12);
    let alpha = ((1.0 - transparency.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(tinted.r(), tinted.g(), tinted.b(), alpha)
}

pub(super) fn surface_alpha(transparency: f32) -> u8 {
    // Linear over the full range so the slider spans completely transparent
    // (alpha 0) to completely opaque (alpha 255), matching the GPU surface.
    let opacity = 1.0 - transparency.clamp(0.0, 1.0);
    (opacity * 255.0).round() as u8
}
