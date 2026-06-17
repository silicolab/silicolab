//! Tessellation of the ball-and-stick *solid* primitives — full spheres and
//! (split) cylinders — into shaded screen-space triangles, plus the hand-drawn
//! "paper" shading model they all use. The flat point-disc and line-bond paths
//! stay with the scene builder in the parent module; only the heavy, depth-shaded
//! geometry lives here.

use std::f32::consts::TAU;
use std::sync::LazyLock;

use eframe::egui::Color32;
use nalgebra::{Point3, Vector3};

use super::super::super::camera::Projector;
use super::super::{
    BOND_RADIAL_SEGMENTS, PrimitiveMeshVertex, PrimitiveTriangle, SPHERE_LATITUDE_SEGMENTS,
    SPHERE_LONGITUDE_SEGMENTS, darken, desaturate_color, initial_cartoon_side, lighten, mix_color,
    normalize_vector3, orthogonalize_to_tangent, primitive_triangle,
};

#[derive(Clone, Copy)]
pub(super) struct CylinderSpan {
    pub(super) start: Point3<f32>,
    pub(super) end: Point3<f32>,
}

#[derive(Clone, Copy)]
struct CylinderStyle {
    radius: f32,
    color: Color32,
    orientation_hint: Vector3<f32>,
}

#[derive(Clone, Copy)]
pub(super) struct SplitCylinderStyle {
    pub(super) radius: f32,
    pub(super) start_color: Color32,
    pub(super) end_color: Color32,
    pub(super) orientation_hint: Vector3<f32>,
}

#[derive(Clone, Copy)]
pub(super) struct CylinderCaps {
    pub(super) start: bool,
    pub(super) end: bool,
}

pub(super) fn append_sphere_triangles(
    triangles: &mut Vec<PrimitiveTriangle>,
    viewport: &Projector,
    center: Point3<f32>,
    radius: f32,
    color: Color32,
) {
    let shade = surface_shade(color);
    let mut rings = Vec::with_capacity(SPHERE_LATITUDE_SEGMENTS + 1);
    for latitude in 0..=SPHERE_LATITUDE_SEGMENTS {
        let polar = std::f32::consts::PI * latitude as f32 / SPHERE_LATITUDE_SEGMENTS as f32;
        let (sin_polar, cos_polar) = polar.sin_cos();
        let mut ring = Vec::with_capacity(SPHERE_LONGITUDE_SEGMENTS + 1);
        for longitude in 0..=SPHERE_LONGITUDE_SEGMENTS {
            let azimuth = TAU * longitude as f32 / SPHERE_LONGITUDE_SEGMENTS as f32;
            let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
            let normal = Vector3::new(cos_azimuth * sin_polar, cos_polar, sin_azimuth * sin_polar);
            ring.push(primitive_vertex(
                viewport,
                center + normal * radius,
                normal,
                shade,
            ));
        }
        rings.push(ring);
    }

    for latitude in 0..SPHERE_LATITUDE_SEGMENTS {
        for longitude in 0..SPHERE_LONGITUDE_SEGMENTS {
            let a = rings[latitude][longitude];
            let b = rings[latitude + 1][longitude];
            let c = rings[latitude + 1][longitude + 1];
            let d = rings[latitude][longitude + 1];

            if latitude == 0 {
                triangles.push(primitive_triangle(a, b, c));
            } else if latitude + 1 == SPHERE_LATITUDE_SEGMENTS {
                triangles.push(primitive_triangle(a, b, d));
            } else {
                triangles.push(primitive_triangle(a, b, c));
                triangles.push(primitive_triangle(a, c, d));
            }
        }
    }
}

pub(super) fn append_split_cylinder(
    triangles: &mut Vec<PrimitiveTriangle>,
    viewport: &Projector,
    span: CylinderSpan,
    style: SplitCylinderStyle,
    caps: CylinderCaps,
) {
    let midpoint = Point3::from((span.start.coords + span.end.coords) * 0.5);
    append_cylinder_triangles(
        triangles,
        viewport,
        CylinderSpan {
            start: span.start,
            end: midpoint,
        },
        CylinderStyle {
            radius: style.radius,
            color: style.start_color,
            orientation_hint: style.orientation_hint,
        },
        CylinderCaps {
            start: caps.start,
            end: false,
        },
    );
    append_cylinder_triangles(
        triangles,
        viewport,
        CylinderSpan {
            start: midpoint,
            end: span.end,
        },
        CylinderStyle {
            radius: style.radius,
            color: style.end_color,
            orientation_hint: style.orientation_hint,
        },
        CylinderCaps {
            start: false,
            end: caps.end,
        },
    );
}

fn append_cylinder_triangles(
    triangles: &mut Vec<PrimitiveTriangle>,
    viewport: &Projector,
    span: CylinderSpan,
    style: CylinderStyle,
    caps: CylinderCaps,
) {
    let axis_vector = span.end - span.start;
    let Some(axis) = axis_vector.try_normalize(0.0001) else {
        return;
    };
    let side = orthogonalize_to_tangent(style.orientation_hint, axis, initial_cartoon_side(axis));
    let normal = normalize_vector3(axis.cross(&side), Vector3::new(0.0, 1.0, 0.0));
    let shade = surface_shade(style.color);
    let mut start_ring = Vec::with_capacity(BOND_RADIAL_SEGMENTS);
    let mut end_ring = Vec::with_capacity(BOND_RADIAL_SEGMENTS);

    for index in 0..BOND_RADIAL_SEGMENTS {
        let angle = TAU * index as f32 / BOND_RADIAL_SEGMENTS as f32;
        let (sin_angle, cos_angle) = angle.sin_cos();
        let radial = side * cos_angle + normal * sin_angle;
        start_ring.push(primitive_vertex(
            viewport,
            span.start + radial * style.radius,
            radial,
            shade,
        ));
        end_ring.push(primitive_vertex(
            viewport,
            span.end + radial * style.radius,
            radial,
            shade,
        ));
    }

    for index in 0..BOND_RADIAL_SEGMENTS {
        let next_index = (index + 1) % BOND_RADIAL_SEGMENTS;
        triangles.push(primitive_triangle(
            start_ring[index],
            end_ring[index],
            end_ring[next_index],
        ));
        triangles.push(primitive_triangle(
            start_ring[index],
            end_ring[next_index],
            start_ring[next_index],
        ));
    }

    if caps.start {
        append_cylinder_cap(
            triangles,
            viewport,
            span.start,
            -axis,
            &start_ring,
            style.color,
        );
    }
    if caps.end {
        append_cylinder_cap(triangles, viewport, span.end, axis, &end_ring, style.color);
    }
}

fn append_cylinder_cap(
    triangles: &mut Vec<PrimitiveTriangle>,
    viewport: &Projector,
    center: Point3<f32>,
    normal: Vector3<f32>,
    ring: &[PrimitiveMeshVertex],
    color: Color32,
) {
    let center_vertex =
        primitive_vertex(viewport, center, normal, surface_shade(darken(color, 0.06)));
    for index in 0..ring.len() {
        let next_index = (index + 1) % ring.len();
        triangles.push(primitive_triangle(
            center_vertex,
            ring[next_index],
            ring[index],
        ));
    }
}

/// Paper/shadow tints used by the hand-drawn shading model. Constant across the
/// whole frame.
const PAPER_TINT: Color32 = Color32::from_rgb(246, 243, 236);
const SHADOW_TINT: Color32 = Color32::from_rgb(120, 129, 144);

/// View-space lighting directions. They are identical for every vertex of a
/// frame, so they are normalized once on first use rather than per vertex.
static LIGHT_DIRECTION: LazyLock<Vector3<f32>> = LazyLock::new(|| {
    normalize_vector3(Vector3::new(-0.35, 0.45, 1.0), Vector3::new(0.0, 0.0, 1.0))
});
static HALF_VECTOR: LazyLock<Vector3<f32>> = LazyLock::new(|| {
    normalize_vector3(
        *LIGHT_DIRECTION + Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, 0.0, 1.0),
    )
});

/// The normal-independent half of the surface shading. These color mixes depend
/// only on a primitive's base color, so they are computed once per
/// sphere/cylinder and reused across its (hundreds of) surface vertices instead
/// of being recomputed per vertex.
#[derive(Clone, Copy)]
struct SurfaceShade {
    washed: Color32,
}

fn surface_shade(base_color: Color32) -> SurfaceShade {
    let neutral = desaturate_color(base_color, 0.42);
    let softened = mix_color(base_color, neutral, 0.34);
    let washed = mix_color(softened, PAPER_TINT, 0.14);
    SurfaceShade { washed }
}

fn primitive_vertex(
    viewport: &Projector,
    position: Point3<f32>,
    normal: Vector3<f32>,
    shade: SurfaceShade,
) -> PrimitiveMeshVertex {
    let projected = viewport.project(position);
    PrimitiveMeshVertex {
        pos: projected.pos,
        depth: projected.depth,
        color: shade_surface_color(viewport, shade, normal),
    }
}

fn shade_surface_color(
    viewport: &Projector,
    shade: SurfaceShade,
    surface_normal: Vector3<f32>,
) -> Color32 {
    let view_normal = normalize_vector3(
        viewport.rotate_to_view(surface_normal),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let light_direction = *LIGHT_DIRECTION;
    let half_vector = *HALF_VECTOR;
    let diffuse = view_normal.dot(&light_direction).max(0.0);
    let rim = (1.0 - view_normal.z.abs()).powi(2) * 0.10;
    let soft_highlight = view_normal.dot(&half_vector).max(0.0).powf(5.5) * 0.07;
    let washed = shade.washed;
    let brightness = (0.46 + diffuse * 0.22 + rim * 0.55).clamp(0.0, 1.0);
    let shaded = if brightness >= 0.5 {
        lighten(washed, (brightness - 0.5) * 0.42)
    } else {
        mix_color(darken(washed, (0.5 - brightness) * 0.38), SHADOW_TINT, 0.18)
    };

    mix_color(shaded, PAPER_TINT, soft_highlight)
}
