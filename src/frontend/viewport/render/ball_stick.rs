use std::f32::consts::TAU;
use std::sync::LazyLock;

use eframe::egui::Color32;
use nalgebra::{Point3, Vector3};

use crate::{
    domain::{BondType, Structure},
    frontend::{AtomSelection, state::AtomStyle},
};

use super::super::camera::{Projector, camera_forward_world};
use super::backend::{LineSegmentPrimitive, RenderScene};
use super::scene::{RenderedAtom, RenderedBondSegment, ViewportGeometry};
use super::{
    AROMATIC_DASH_LENGTH, AROMATIC_DASH_OFFSET, AROMATIC_DASH_RADIUS, AROMATIC_GAP_LENGTH,
    BALL_RADIUS_SCALE, BOND_RADIAL_SEGMENTS, MULTI_BOND_OFFSET, MULTI_BOND_RADIUS,
    POINT_DISC_SEGMENTS, POINT_LOD_ATOM_THRESHOLD, PrimitiveMeshVertex, PrimitiveTriangle,
    SINGLE_BOND_RADIUS, SPHERE_LATITUDE_SEGMENTS, SPHERE_LONGITUDE_SEGMENTS, ViewportVisualState,
    atom_ball_radius, atom_render_color_with_settings, atom_visible, darken, desaturate_color,
    initial_cartoon_side, mix_color, normalize_vector3, orthogonalize_to_tangent,
};

#[derive(Clone, Copy)]
struct CylinderSpan {
    start: Point3<f32>,
    end: Point3<f32>,
}

#[derive(Clone, Copy)]
struct CylinderStyle {
    radius: f32,
    color: Color32,
    orientation_hint: Vector3<f32>,
}

#[derive(Clone, Copy)]
struct SplitCylinderStyle {
    radius: f32,
    start_color: Color32,
    end_color: Color32,
    orientation_hint: Vector3<f32>,
}

#[derive(Clone, Copy)]
struct CylinderCaps {
    start: bool,
    end: bool,
}

#[derive(Clone, Copy)]
struct TrimmedBondSegment {
    start: Point3<f32>,
    end: Point3<f32>,
    axis: Vector3<f32>,
    length: f32,
}

/// Per-atom drawing inputs resolved once per frame: whether the atom is drawn in
/// the ball-and-stick scene, its effective [`AtomStyle`], and its render color.
/// Resolving each of these touches a sparse override map plus residue/category
/// classification, and the bond and atom loops below each consult them multiple
/// times — so they are computed once, up front, and indexed by atom.
#[derive(Clone, Copy)]
pub(super) struct AtomDraw {
    pub(super) visible: bool,
    pub(super) style: AtomStyle,
    pub(super) color: Color32,
}

pub(super) fn build_atom_draw_table(
    structure: &Structure,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
) -> Vec<AtomDraw> {
    (0..structure.atoms.len())
        .map(|index| AtomDraw {
            visible: atom_visible(structure, visual_state, index),
            // The *base* style drives ball-and-stick geometry; cartoon/surface
            // are additive overlays drawn by their own passes.
            style: visual_state.resolved_base_style(structure, index),
            color: atom_render_color_with_settings(structure, index, selection, visual_state),
        })
        .collect()
}

pub(crate) fn build_ball_and_stick_scene(
    structure: &Structure,
    geometry: &ViewportGeometry,
    viewport: &Projector,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
) -> RenderScene {
    let atom_draw = build_atom_draw_table(structure, selection, visual_state);

    // An atom is drawn when its resolved style places it in the ball-and-stick
    // scene (i.e. not Hidden and not drawn via the cartoon path).
    let visible_atoms = geometry
        .atoms
        .iter()
        .filter(|atom| atom_draw[atom.index].visible)
        .collect::<Vec<_>>();

    // Large systems (e.g. an explicitly solvated protein, dominated by bulk
    // water) would tessellate into tens of millions of vertices and overflow
    // the egui mesh buffer. Count only atoms whose style draws heavy geometry
    // (spheres/cylinders); cheap dot/wireframe styles never trigger the
    // fallback, so a user who simplifies the solvent keeps their chosen look.
    let heavy_atoms = visible_atoms
        .iter()
        .filter(|atom| atom_draw[atom.index].style.is_heavy())
        .count();
    if heavy_atoms > POINT_LOD_ATOM_THRESHOLD {
        return build_point_cloud_scene(
            structure,
            geometry,
            &visible_atoms,
            &atom_draw,
            viewport,
            selection,
        );
    }

    let mut opaque_triangles = Vec::new();
    let mut lines = Vec::new();

    for bond in &geometry.bonds {
        let a = atom_draw[bond.a];
        let b = atom_draw[bond.b];
        if !(a.visible || b.visible) {
            continue;
        }
        if a.style.draws_stick_bonds() || b.style.draws_stick_bonds() {
            append_bond_triangles(
                &mut opaque_triangles,
                structure,
                bond,
                viewport,
                a.color,
                b.color,
            );
        } else if a.style.draws_line_bonds() || b.style.draws_line_bonds() {
            push_split_bond_line(&mut lines, viewport, bond, structure, a.color, b.color);
        }
    }

    for atom_projection in &visible_atoms {
        let index = atom_projection.index;
        let draw = atom_draw[index];
        match draw.style.sphere_radius_scale() {
            Some(scale) => {
                let atom = &structure.atoms[index];
                let mut radius = atom_ball_radius(&atom.element) * (scale / BALL_RADIUS_SCALE);
                if selection.primary() == Some(index) {
                    radius *= 1.18;
                } else if selection.contains(index) {
                    radius *= 1.10;
                }
                append_sphere_triangles(
                    &mut opaque_triangles,
                    viewport,
                    atom.position,
                    radius,
                    draw.color,
                );
            }
            None if draw.style.draws_point() => {
                let mut radius = point_disc_radius(atom_projection.scale);
                if selection.primary() == Some(index) {
                    radius *= 1.6;
                } else if selection.contains(index) {
                    radius *= 1.3;
                }
                append_atom_point(&mut opaque_triangles, atom_projection, radius, draw.color);
            }
            None => {}
        }
    }

    let mut scene = RenderScene::default();
    scene.push_lines(lines);
    scene.push_opaque_meshes(opaque_triangles);
    scene.sorted()
}

/// Lightweight "dots" representation for atom counts past
/// [`POINT_LOD_ATOM_THRESHOLD`]. Atoms are drawn as flat screen-space discs and
/// bonds as thin line segments — a few triangles per atom instead of hundreds.
fn build_point_cloud_scene(
    structure: &Structure,
    geometry: &ViewportGeometry,
    visible_atoms: &[&RenderedAtom],
    atom_draw: &[AtomDraw],
    viewport: &Projector,
    selection: &AtomSelection,
) -> RenderScene {
    let mut opaque_triangles = Vec::new();
    let mut lines = Vec::new();

    for bond in &geometry.bonds {
        let a = atom_draw[bond.a];
        let b = atom_draw[bond.b];
        if !(a.visible || b.visible) {
            continue;
        }
        push_split_bond_line(&mut lines, viewport, bond, structure, a.color, b.color);
    }

    for atom_projection in visible_atoms {
        let index = atom_projection.index;
        let mut radius = point_disc_radius(atom_projection.scale);
        if selection.primary() == Some(index) {
            radius *= 1.6;
        } else if selection.contains(index) {
            radius *= 1.3;
        }
        append_atom_point(
            &mut opaque_triangles,
            atom_projection,
            radius,
            atom_draw[index].color,
        );
    }

    let mut scene = RenderScene::default();
    scene.push_lines(lines);
    scene.push_opaque_meshes(opaque_triangles);
    scene.sorted()
}

/// Push a wireframe/point-cloud bond as two half-segments split at its midpoint,
/// each half colored by its nearer atom.
/// The nearer atom is resolved from the segment's actual endpoint so periodic
/// bond halves (whose `start` may be either atom) stay correctly colored.
fn push_split_bond_line(
    lines: &mut Vec<LineSegmentPrimitive>,
    viewport: &Projector,
    bond: &RenderedBondSegment,
    structure: &Structure,
    color_a: Color32,
    color_b: Color32,
) {
    let start = viewport.project(bond.start);
    let end = viewport.project(bond.end);
    let mid = viewport.project(Point3::from((bond.start.coords + bond.end.coords) * 0.5));
    // Color the half at `start` by whichever atom that end sits on.
    let a_pos = structure.atoms[bond.a].position;
    let (start_color, end_color) =
        if (bond.start - a_pos).norm_squared() <= (bond.end - a_pos).norm_squared() {
            (color_a, color_b)
        } else {
            (color_b, color_a)
        };
    lines.push(LineSegmentPrimitive {
        start: start.pos,
        end: mid.pos,
        color: start_color,
        width: 1.2,
    });
    lines.push(LineSegmentPrimitive {
        start: mid.pos,
        end: end.pos,
        color: end_color,
        width: 1.2,
    });
}

/// Screen-space radius (pixels) of a point disc, scaled by the atom's
/// perspective factor and clamped so distant atoms stay visible and near atoms
/// do not balloon.
fn point_disc_radius(projection_scale: f32) -> f32 {
    (2.6 * projection_scale).clamp(2.0, 6.5)
}

/// Append a flat, camera-facing disc (triangle fan) for one atom at its
/// projected screen position. Uses a single depth for the whole disc so the
/// existing painter's algorithm depth sort keeps point clouds ordered.
fn append_atom_point(
    triangles: &mut Vec<PrimitiveTriangle>,
    projection: &RenderedAtom,
    radius: f32,
    color: Color32,
) {
    let center = PrimitiveMeshVertex {
        pos: projection.pos,
        depth: projection.depth,
        color,
    };
    let rim_color = darken(color, 0.12);
    let rim = |angle: f32| PrimitiveMeshVertex {
        pos: eframe::egui::Pos2::new(
            projection.pos.x + radius * angle.cos(),
            projection.pos.y + radius * angle.sin(),
        ),
        depth: projection.depth,
        color: rim_color,
    };
    for segment in 0..POINT_DISC_SEGMENTS {
        let start = TAU * segment as f32 / POINT_DISC_SEGMENTS as f32;
        let end = TAU * (segment + 1) as f32 / POINT_DISC_SEGMENTS as f32;
        triangles.push(super::primitive_triangle(center, rim(start), rim(end)));
    }
}

fn append_bond_triangles(
    triangles: &mut Vec<PrimitiveTriangle>,
    structure: &Structure,
    bond: &RenderedBondSegment,
    viewport: &Projector,
    start_color: Color32,
    end_color: Color32,
) {
    let Some(trimmed) = trimmed_bond_segment(structure, bond.a, bond.b, bond.start, bond.end)
    else {
        return;
    };
    let offset_direction =
        bond_offset_direction(viewport, trimmed.start, trimmed.end, bond.aromatic_center);
    let split_caps = CylinderCaps {
        start: true,
        end: true,
    };

    match bond.bond_type {
        BondType::Single => append_split_cylinder(
            triangles,
            viewport,
            CylinderSpan {
                start: trimmed.start,
                end: trimmed.end,
            },
            SplitCylinderStyle {
                radius: SINGLE_BOND_RADIUS,
                start_color,
                end_color,
                orientation_hint: offset_direction,
            },
            split_caps,
        ),
        BondType::Double => {
            for offset_sign in [-1.0_f32, 1.0] {
                let offset = offset_direction * (MULTI_BOND_OFFSET * offset_sign * 0.5);
                append_split_cylinder(
                    triangles,
                    viewport,
                    CylinderSpan {
                        start: trimmed.start + offset,
                        end: trimmed.end + offset,
                    },
                    SplitCylinderStyle {
                        radius: MULTI_BOND_RADIUS,
                        start_color,
                        end_color,
                        orientation_hint: offset_direction,
                    },
                    split_caps,
                );
            }
        }
        BondType::Triple => {
            append_split_cylinder(
                triangles,
                viewport,
                CylinderSpan {
                    start: trimmed.start,
                    end: trimmed.end,
                },
                SplitCylinderStyle {
                    radius: MULTI_BOND_RADIUS,
                    start_color,
                    end_color,
                    orientation_hint: offset_direction,
                },
                split_caps,
            );
            for offset_sign in [-1.0_f32, 1.0] {
                let offset = offset_direction * (MULTI_BOND_OFFSET * offset_sign);
                append_split_cylinder(
                    triangles,
                    viewport,
                    CylinderSpan {
                        start: trimmed.start + offset,
                        end: trimmed.end + offset,
                    },
                    SplitCylinderStyle {
                        radius: MULTI_BOND_RADIUS,
                        start_color,
                        end_color,
                        orientation_hint: offset_direction,
                    },
                    split_caps,
                );
            }
        }
        BondType::Aromatic => {
            append_split_cylinder(
                triangles,
                viewport,
                CylinderSpan {
                    start: trimmed.start,
                    end: trimmed.end,
                },
                SplitCylinderStyle {
                    radius: SINGLE_BOND_RADIUS,
                    start_color,
                    end_color,
                    orientation_hint: offset_direction,
                },
                split_caps,
            );

            let dashed_start = trimmed.start + offset_direction * AROMATIC_DASH_OFFSET;
            let dash_axis = trimmed.axis;
            let mut cursor = 0.0;
            while cursor < trimmed.length {
                let dash_end = (cursor + AROMATIC_DASH_LENGTH).min(trimmed.length);
                append_split_cylinder(
                    triangles,
                    viewport,
                    CylinderSpan {
                        start: dashed_start + dash_axis * cursor,
                        end: dashed_start + dash_axis * dash_end,
                    },
                    SplitCylinderStyle {
                        radius: AROMATIC_DASH_RADIUS,
                        start_color,
                        end_color,
                        orientation_hint: offset_direction,
                    },
                    split_caps,
                );
                cursor += AROMATIC_DASH_LENGTH + AROMATIC_GAP_LENGTH;
            }
        }
    }
}

fn append_sphere_triangles(
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
                triangles.push(super::primitive_triangle(a, b, c));
            } else if latitude + 1 == SPHERE_LATITUDE_SEGMENTS {
                triangles.push(super::primitive_triangle(a, b, d));
            } else {
                triangles.push(super::primitive_triangle(a, b, c));
                triangles.push(super::primitive_triangle(a, c, d));
            }
        }
    }
}

fn append_split_cylinder(
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
        triangles.push(super::primitive_triangle(
            start_ring[index],
            end_ring[index],
            end_ring[next_index],
        ));
        triangles.push(super::primitive_triangle(
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
        triangles.push(super::primitive_triangle(
            center_vertex,
            ring[next_index],
            ring[index],
        ));
    }
}

fn trimmed_bond_segment(
    structure: &Structure,
    start_index: usize,
    end_index: usize,
    start: Point3<f32>,
    end: Point3<f32>,
) -> Option<TrimmedBondSegment> {
    let bond_vector = end - start;
    let axis = bond_vector.try_normalize(0.0001)?;
    let bond_length = bond_vector.norm();
    let start_trim =
        atom_ball_radius(&structure.atoms[start_index].element).min(bond_length * 0.35);
    let end_trim = atom_ball_radius(&structure.atoms[end_index].element).min(bond_length * 0.35);
    if bond_length <= start_trim + end_trim + 0.05 {
        return None;
    }

    let trimmed_start = Point3::from(start.coords + axis * start_trim);
    let trimmed_end = Point3::from(end.coords - axis * end_trim);
    Some(TrimmedBondSegment {
        start: trimmed_start,
        end: trimmed_end,
        axis,
        length: bond_length - start_trim - end_trim,
    })
}

fn bond_offset_direction(
    viewport: &Projector,
    start: Point3<f32>,
    end: Point3<f32>,
    aromatic_center: Option<Point3<f32>>,
) -> Vector3<f32> {
    let axis = normalize_vector3(end - start, Vector3::new(1.0, 0.0, 0.0));
    if let Some(center) = aromatic_center {
        let midpoint = Point3::from((start.coords + end.coords) * 0.5);
        let inward = center - midpoint;
        let projected = inward - axis * inward.dot(&axis);
        if projected.norm_squared() > 0.0001 {
            return normalize_vector3(projected, initial_cartoon_side(axis));
        }
    }

    let camera_forward = camera_forward_world(viewport);
    let offset = axis.cross(&camera_forward);
    if offset.norm_squared() > 0.0001 {
        normalize_vector3(offset, initial_cartoon_side(axis))
    } else {
        initial_cartoon_side(axis)
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
        super::lighten(washed, (brightness - 0.5) * 0.42)
    } else {
        mix_color(
            super::darken(washed, (0.5 - brightness) * 0.38),
            SHADOW_TINT,
            0.18,
        )
    };

    mix_color(shaded, PAPER_TINT, soft_highlight)
}

#[cfg(test)]
mod tests {
    use super::super::scene::build_viewport_geometry;
    use super::*;
    use crate::domain::{Atom, Structure};
    use crate::frontend::{AtomSelection, state::AtomStyle};
    use eframe::egui::{Pos2, Rect, Vec2};
    use nalgebra::Point3;

    fn test_projector() -> Projector {
        Projector::new(
            Rect::from_min_size(Pos2::ZERO, Vec2::splat(2000.0)),
            Point3::origin(),
            10.0,
            1000.0,
            0.0,
            0.0,
            Vec2::ZERO,
        )
    }

    /// A grid of widely spaced atoms so no bonds are inferred, making the
    /// triangle count a clean function of the per-atom primitive.
    fn grid_structure(atom_count: usize) -> Structure {
        let side = (atom_count as f32).cbrt().ceil() as usize + 1;
        let atoms = (0..atom_count)
            .map(|i| Atom {
                element: "C".to_string(),
                position: Point3::new(
                    (i % side) as f32 * 8.0,
                    ((i / side) % side) as f32 * 8.0,
                    (i / (side * side)) as f32 * 8.0,
                ),
                charge: 0.0,
            })
            .collect();
        Structure {
            title: "grid".to_string(),
            atoms,
            bonds: Vec::new(),
            cell: None,
            biopolymer: None,
        }
    }

    fn scene_triangle_count_with(atom_count: usize, visual: &ViewportVisualState) -> usize {
        let structure = grid_structure(atom_count);
        let viewport = test_projector();
        let geometry = build_viewport_geometry(&structure, &viewport);
        build_ball_and_stick_scene(
            &structure,
            &geometry,
            &viewport,
            &AtomSelection::default(),
            visual,
        )
        .triangle_count()
    }

    fn scene_triangle_count(atom_count: usize) -> usize {
        scene_triangle_count_with(atom_count, &ViewportVisualState::default())
    }

    fn all_atoms_styled(atom_count: usize, style: AtomStyle) -> ViewportVisualState {
        let mut visual = ViewportVisualState::default();
        for index in 0..atom_count {
            visual.atom_styles.insert(index, style);
        }
        visual
    }

    #[test]
    fn small_systems_use_full_sphere_meshes() {
        // Spheres are hundreds of triangles each, far above the dots LOD.
        let count = scene_triangle_count(50);
        assert!(count > 50 * 100, "expected sphere meshes, got {count}");
    }

    #[test]
    fn large_systems_fall_back_to_point_dots() {
        let atom_count = POINT_LOD_ATOM_THRESHOLD + 1;
        let count = scene_triangle_count(atom_count);
        // Each atom becomes a flat disc of POINT_DISC_SEGMENTS triangles.
        assert_eq!(count, atom_count * POINT_DISC_SEGMENTS);
        // And the simplified scene stays well under the GPU buffer guard.
        assert!(count < super::super::MAX_RENDER_TRIANGLES);
    }

    #[test]
    fn hidden_style_draws_nothing() {
        let count = scene_triangle_count_with(40, &all_atoms_styled(40, AtomStyle::Hidden));
        assert_eq!(count, 0);
    }

    #[test]
    fn point_style_draws_one_disc_per_atom() {
        let count = scene_triangle_count_with(40, &all_atoms_styled(40, AtomStyle::Point));
        assert_eq!(count, 40 * POINT_DISC_SEGMENTS);
    }

    #[test]
    fn sphere_style_is_heavier_than_ball_and_stick() {
        let spheres = scene_triangle_count_with(40, &all_atoms_styled(40, AtomStyle::Sphere));
        let balls = scene_triangle_count_with(40, &all_atoms_styled(40, AtomStyle::BallAndStick));
        // Same triangle topology per sphere, but identical here (no bonds); the
        // point is that both draw full spheres and dwarf the dots styles.
        assert_eq!(spheres, balls);
        assert!(spheres > 40 * POINT_DISC_SEGMENTS);
    }

    #[test]
    fn cheap_styles_skip_the_large_system_fallback() {
        // A huge solvent box set to Dots must NOT be forced through the point
        // fallback by accident — it should already be points, one disc each.
        let atom_count = POINT_LOD_ATOM_THRESHOLD * 2;
        let count =
            scene_triangle_count_with(atom_count, &all_atoms_styled(atom_count, AtomStyle::Point));
        assert_eq!(count, atom_count * POINT_DISC_SEGMENTS);
    }

    #[test]
    fn apply_atom_styles_stays_sparse_against_category_default() {
        // Grid atoms have no residue → category Other → software default
        // BallAndStick.
        let structure = grid_structure(3);
        let items: Vec<_> = (0..3).map(|i| (i, structure.atom_category(i))).collect();
        let mut visual = ViewportVisualState::default();
        visual.apply_atom_styles(items.clone(), AtomStyle::Sphere);
        assert_eq!(visual.atom_styles.len(), 3);
        // Re-applying the resolved category default removes the overrides.
        visual.apply_atom_styles(items, AtomStyle::BallAndStick);
        assert!(visual.atom_styles.is_empty());
    }

    #[test]
    fn wireframe_bond_splits_into_two_atom_colors() {
        use crate::domain::{Bond, BondType};
        // An O–H bond drawn as wireframe must split into two half-segments — one
        // O-colored, one H-colored — so H–O–H reads as a colored V, not a single
        // line that looks like O–O–O.
        let structure = Structure::with_bonds(
            "oh",
            vec![
                Atom {
                    element: "O".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "H".to_string(),
                    position: Point3::new(1.0, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
        );
        let viewport = test_projector();
        let geometry = build_viewport_geometry(&structure, &viewport);
        let mut visual = ViewportVisualState::default();
        visual.atom_styles.insert(0, AtomStyle::Wireframe);
        visual.atom_styles.insert(1, AtomStyle::Wireframe);
        let scene = build_ball_and_stick_scene(
            &structure,
            &geometry,
            &viewport,
            &AtomSelection::default(),
            &visual,
        );
        let lines = scene.line_segments();
        assert_eq!(lines.len(), 2, "the bond is split into two half-segments");
        assert_ne!(
            lines[0].color, lines[1].color,
            "the two halves carry the O and H colors"
        );
    }
}
