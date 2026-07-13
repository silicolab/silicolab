use std::f32::consts::TAU;

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
    MULTI_BOND_OFFSET, MULTI_BOND_RADIUS, POINT_DISC_SEGMENTS, PrimitiveMeshVertex,
    PrimitiveTriangle, SINGLE_BOND_RADIUS, STICK_DOUBLE_BOND_OFFSET, STICK_DOUBLE_BOND_RADIUS,
    STICK_TRIPLE_BOND_OFFSET, STICK_TRIPLE_BOND_RADIUS, ViewportVisualState, atom_marker_radius,
    atom_render_color_with_settings, atom_visible, bond_trim_radius, darken, initial_cartoon_side,
    normalize_vector3,
};

mod solids;
use solids::{
    CylinderCaps, CylinderSpan, SplitCylinderStyle, append_sphere_triangles, append_split_cylinder,
};

#[derive(Clone, Copy)]
struct TrimmedBondSegment {
    start: Point3<f32>,
    end: Point3<f32>,
    axis: Vector3<f32>,
    length: f32,
}

#[derive(Clone, Copy)]
struct BondCylinderStyle {
    start_atom_radius: f32,
    end_atom_radius: f32,
    start_color: Color32,
    end_color: Color32,
    involves_stick: bool,
}

const BOND_CAP_BURY_EPSILON: f32 = 0.001;

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

pub(super) fn effective_stick_degrees(structure: &Structure, atom_draw: &[AtomDraw]) -> Vec<usize> {
    let mut degrees = vec![0; structure.atoms.len()];
    for bond in &structure.bonds {
        if bond.a == bond.b {
            continue;
        }
        let (Some(a), Some(b)) = (atom_draw.get(bond.a), atom_draw.get(bond.b)) else {
            continue;
        };
        if !(a.visible && b.visible)
            || !(a.style.draws_stick_bonds() || b.style.draws_stick_bonds())
        {
            continue;
        }
        let (Some(atom_a), Some(atom_b)) =
            (structure.atoms.get(bond.a), structure.atoms.get(bond.b))
        else {
            continue;
        };
        let length_squared = (atom_b.position - atom_a.position).norm_squared();
        if !length_squared.is_finite() || length_squared <= 1e-10 {
            continue;
        }
        if a.style == AtomStyle::Stick {
            degrees[bond.a] += 1;
        }
        if b.style == AtomStyle::Stick {
            degrees[bond.b] += 1;
        }
    }
    degrees
}

pub(crate) fn build_ball_and_stick_scene(
    structure: &Structure,
    geometry: &ViewportGeometry,
    viewport: &Projector,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
) -> RenderScene {
    let atom_draw = build_atom_draw_table(structure, selection, visual_state);
    let stick_degrees = effective_stick_degrees(structure, &atom_draw);

    // An atom is drawn when its resolved style places it in the ball-and-stick
    // scene (i.e. not Hidden and not drawn via the cartoon path).
    let visible_atoms = geometry
        .atoms
        .iter()
        .filter(|atom| atom_draw[atom.index].visible)
        .collect::<Vec<_>>();

    let mut opaque_triangles = Vec::new();
    let mut lines = Vec::new();

    for bond in &geometry.bonds {
        let a = atom_draw[bond.a];
        let b = atom_draw[bond.b];
        if !(a.visible && b.visible) {
            continue;
        }
        if a.style.draws_stick_bonds() || b.style.draws_stick_bonds() {
            append_bond_triangles(
                &mut opaque_triangles,
                bond,
                viewport,
                BondCylinderStyle {
                    start_atom_radius: bond_trim_radius(&structure.atoms[bond.a].element, a.style),
                    end_atom_radius: bond_trim_radius(&structure.atoms[bond.b].element, b.style),
                    start_color: a.color,
                    end_color: b.color,
                    involves_stick: a.style == AtomStyle::Stick || b.style == AtomStyle::Stick,
                },
            );
        } else if a.style.draws_line_bonds() || b.style.draws_line_bonds() {
            push_split_bond_line(&mut lines, viewport, bond, structure, a.color, b.color);
        }
    }

    for atom_projection in &visible_atoms {
        let index = atom_projection.index;
        let draw = atom_draw[index];
        let atom = &structure.atoms[index];
        let marker_radius = if draw.style == AtomStyle::Stick {
            (stick_degrees[index] >= 2).then_some(SINGLE_BOND_RADIUS)
        } else {
            atom_marker_radius(&atom.element, draw.style)
        };
        match marker_radius {
            Some(mut radius) => {
                if draw.style != AtomStyle::Stick {
                    if selection.primary() == Some(index) {
                        radius *= 1.18;
                    } else if selection.contains(index) {
                        radius *= 1.10;
                    }
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

fn double_bond_profile(involves_stick: bool) -> (f32, f32) {
    if involves_stick {
        (STICK_DOUBLE_BOND_RADIUS, STICK_DOUBLE_BOND_OFFSET)
    } else {
        (MULTI_BOND_RADIUS, MULTI_BOND_OFFSET * 0.5)
    }
}

fn triple_bond_profile(involves_stick: bool) -> (f32, f32) {
    if involves_stick {
        (STICK_TRIPLE_BOND_RADIUS, STICK_TRIPLE_BOND_OFFSET)
    } else {
        (MULTI_BOND_RADIUS, MULTI_BOND_OFFSET)
    }
}

fn bond_bundle_envelope(bond_type: BondType, involves_stick: bool) -> f32 {
    match bond_type {
        BondType::Single => SINGLE_BOND_RADIUS,
        BondType::Double | BondType::Triple if involves_stick => SINGLE_BOND_RADIUS,
        BondType::Double => MULTI_BOND_OFFSET * 0.5 + MULTI_BOND_RADIUS,
        BondType::Triple => MULTI_BOND_OFFSET + MULTI_BOND_RADIUS,
        BondType::Aromatic => SINGLE_BOND_RADIUS.max(AROMATIC_DASH_OFFSET + AROMATIC_DASH_RADIUS),
    }
}

fn embedded_axial_trim(atom_radius: f32, bundle_envelope: f32) -> f32 {
    let tangent = (atom_radius * atom_radius - bundle_envelope * bundle_envelope)
        .max(0.0)
        .sqrt();
    (tangent - BOND_CAP_BURY_EPSILON).max(0.0)
}

fn aromatic_dash_centerline(cursor: f32, length: f32) -> Option<(f32, f32)> {
    let visible_end = (cursor + AROMATIC_DASH_LENGTH).min(length);
    let centerline_start = cursor + AROMATIC_DASH_RADIUS;
    let centerline_end = visible_end - AROMATIC_DASH_RADIUS;
    if centerline_end - centerline_start <= 0.0001 {
        return None;
    }
    Some((centerline_start, centerline_end))
}

fn append_bond_triangles(
    triangles: &mut Vec<PrimitiveTriangle>,
    bond: &RenderedBondSegment,
    viewport: &Projector,
    style: BondCylinderStyle,
) {
    let bundle_envelope = bond_bundle_envelope(bond.bond_type, style.involves_stick);
    let start_trim = embedded_axial_trim(style.start_atom_radius, bundle_envelope);
    let end_trim = embedded_axial_trim(style.end_atom_radius, bundle_envelope);
    let Some(trimmed) = trimmed_bond_segment(bond.start, bond.end, start_trim, end_trim) else {
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
                start_color: style.start_color,
                end_color: style.end_color,
                orientation_hint: offset_direction,
            },
            split_caps,
        ),
        BondType::Double => {
            let (radius, offset_distance) = double_bond_profile(style.involves_stick);
            for offset_sign in [-1.0_f32, 1.0] {
                let offset = offset_direction * (offset_distance * offset_sign);
                append_split_cylinder(
                    triangles,
                    viewport,
                    CylinderSpan {
                        start: trimmed.start + offset,
                        end: trimmed.end + offset,
                    },
                    SplitCylinderStyle {
                        radius,
                        start_color: style.start_color,
                        end_color: style.end_color,
                        orientation_hint: offset_direction,
                    },
                    split_caps,
                );
            }
        }
        BondType::Triple => {
            let (radius, offset_distance) = triple_bond_profile(style.involves_stick);
            append_split_cylinder(
                triangles,
                viewport,
                CylinderSpan {
                    start: trimmed.start,
                    end: trimmed.end,
                },
                SplitCylinderStyle {
                    radius,
                    start_color: style.start_color,
                    end_color: style.end_color,
                    orientation_hint: offset_direction,
                },
                split_caps,
            );
            for offset_sign in [-1.0_f32, 1.0] {
                let offset = offset_direction * (offset_distance * offset_sign);
                append_split_cylinder(
                    triangles,
                    viewport,
                    CylinderSpan {
                        start: trimmed.start + offset,
                        end: trimmed.end + offset,
                    },
                    SplitCylinderStyle {
                        radius,
                        start_color: style.start_color,
                        end_color: style.end_color,
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
                    start_color: style.start_color,
                    end_color: style.end_color,
                    orientation_hint: offset_direction,
                },
                split_caps,
            );

            let dash_origin = trimmed.start + offset_direction * AROMATIC_DASH_OFFSET;
            let dash_axis = trimmed.axis;
            let mut cursor = 0.0;
            while cursor < trimmed.length {
                if let Some((dash_start, dash_end)) =
                    aromatic_dash_centerline(cursor, trimmed.length)
                {
                    append_split_cylinder(
                        triangles,
                        viewport,
                        CylinderSpan {
                            start: dash_origin + dash_axis * dash_start,
                            end: dash_origin + dash_axis * dash_end,
                        },
                        SplitCylinderStyle {
                            radius: AROMATIC_DASH_RADIUS,
                            start_color: style.start_color,
                            end_color: style.end_color,
                            orientation_hint: offset_direction,
                        },
                        split_caps,
                    );
                }
                cursor += AROMATIC_DASH_LENGTH + AROMATIC_GAP_LENGTH;
            }
        }
    }
}

fn trimmed_bond_segment(
    start: Point3<f32>,
    end: Point3<f32>,
    start_radius: f32,
    end_radius: f32,
) -> Option<TrimmedBondSegment> {
    let bond_vector = end - start;
    let axis = bond_vector.try_normalize(0.0001)?;
    let bond_length = bond_vector.norm();
    // Never eat more than a third of the bond at either end, so a short bond
    // between two large atoms still shows some cylinder.
    let start_trim = start_radius.min(bond_length * 0.35);
    let end_trim = end_radius.min(bond_length * 0.35);
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

#[cfg(test)]
mod tests {
    use super::super::scene::build_viewport_geometry;
    use super::*;
    use crate::domain::{Atom, Bond, Structure};
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

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn stick_bond_triangle_count(bond_type: BondType) -> usize {
        let structure = Structure::with_bonds(
            "stick bond",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(-0.5, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, bond_type)],
        );
        let viewport = test_projector();
        let geometry = build_viewport_geometry(&structure, &viewport);
        build_ball_and_stick_scene(
            &structure,
            &geometry,
            &viewport,
            &AtomSelection::default(),
            &all_atoms_styled(2, AtomStyle::Stick),
        )
        .triangle_count()
    }

    #[test]
    fn small_systems_use_full_sphere_meshes() {
        // Spheres are hundreds of triangles each, far above the dots LOD.
        let count = scene_triangle_count(50);
        assert!(count > 50 * 100, "expected sphere meshes, got {count}");
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
    fn stick_bond_orders_keep_independent_rounded_rods() {
        let single = stick_bond_triangle_count(BondType::Single);
        assert!(single > 0);
        assert_eq!(stick_bond_triangle_count(BondType::Double), single * 2);
        assert_eq!(stick_bond_triangle_count(BondType::Triple), single * 3);
        assert_eq!(stick_bond_triangle_count(BondType::Aromatic), single * 4);
    }

    #[test]
    fn ball_bond_trims_bury_the_full_bundle_envelope() {
        let atom_radius = 0.6;
        for (bond_type, involves_stick) in [
            (BondType::Single, false),
            (BondType::Double, true),
            (BondType::Triple, true),
            (BondType::Double, false),
            (BondType::Triple, false),
            (BondType::Aromatic, false),
        ] {
            let envelope = bond_bundle_envelope(bond_type, involves_stick);
            let trim = embedded_axial_trim(atom_radius, envelope);
            assert!(trim * trim + envelope * envelope < atom_radius * atom_radius);
            let tangent = (atom_radius * atom_radius - envelope * envelope)
                .max(0.0)
                .sqrt();
            assert_close(trim, tangent - BOND_CAP_BURY_EPSILON);
        }

        assert_eq!(
            embedded_axial_trim(AROMATIC_DASH_RADIUS, SINGLE_BOND_RADIUS),
            0.0
        );
    }

    #[test]
    fn aromatic_dash_centerlines_preserve_visible_length_and_gap() {
        let first = aromatic_dash_centerline(0.0, 1.0).expect("first dash");
        let pitch = AROMATIC_DASH_LENGTH + AROMATIC_GAP_LENGTH;
        let second = aromatic_dash_centerline(pitch, 1.0).expect("second dash");

        assert_close(first.0 - AROMATIC_DASH_RADIUS, 0.0);
        assert_close(first.1 + AROMATIC_DASH_RADIUS, AROMATIC_DASH_LENGTH);
        assert_close(
            second.0 - AROMATIC_DASH_RADIUS - (first.1 + AROMATIC_DASH_RADIUS),
            AROMATIC_GAP_LENGTH,
        );
        assert!(aromatic_dash_centerline(0.8, 0.8 + AROMATIC_DASH_RADIUS * 2.0).is_none());
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
