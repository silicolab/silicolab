//! Builds the camera-independent GPU instance set (sphere + cylinder) for the
//! ball-and-stick representation, reusing the same per-atom style/visibility/
//! color resolution and bond segmentation as the CPU path so the two stay
//! visually consistent.

use eframe::egui::Color32;
use nalgebra::{Point3, Vector3};

use crate::{
    domain::{BondType, Structure},
    frontend::{AtomSelection, ViewportVisualState, state::AtomStyle},
};

use super::super::gpu::{CylinderInstance, MoleculeInstances, SphereInstance};
use super::ball_stick::build_atom_draw_table;
use super::cartoon::build_biopolymer_cartoon_world_mesh;
use super::scene::{BondWorldSegment, bond_world_segments};
use super::{
    AROMATIC_DASH_LENGTH, AROMATIC_DASH_OFFSET, AROMATIC_DASH_RADIUS, AROMATIC_GAP_LENGTH,
    MULTI_BOND_OFFSET, MULTI_BOND_RADIUS, SINGLE_BOND_RADIUS, WIREFRAME_BOND_RADIUS,
    atom_ball_radius, atom_marker_radius,
};

/// World radius (relative to the full ball-and-stick radius) used to draw atoms
/// whose style is the lightweight "dots" point representation. On the GPU these
/// become small shaded spheres rather than flat screen-space discs.
const POINT_SPHERE_SCALE: f32 = 0.5;

pub(crate) fn build_molecule_instances(
    structure: &Structure,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
) -> MoleculeInstances {
    let atom_draw = build_atom_draw_table(structure, selection, visual_state);

    let mut spheres = Vec::new();
    for (index, draw) in atom_draw.iter().enumerate() {
        if !draw.visible {
            continue;
        }
        if let Some(radius) = sphere_radius(structure, index, draw.style, selection) {
            let position = structure.atoms[index].position;
            spheres.push(SphereInstance {
                pos_radius: [position.x, position.y, position.z, radius],
                color: draw.color.to_normalized_gamma_f32(),
            });
        }
    }

    let adjacency = bond_adjacency(structure);
    let mut cylinders = Vec::new();
    for segment in bond_world_segments(structure) {
        let start = atom_draw[segment.start_atom];
        let end = atom_draw[segment.end_atom];
        if !(start.visible || end.visible) {
            continue;
        }
        if start.style.draws_stick_bonds() || end.style.draws_stick_bonds() {
            append_bond_cylinders(
                &mut cylinders,
                structure,
                &adjacency,
                &segment,
                start.color,
                end.color,
            );
        } else if start.style.draws_line_bonds() || end.style.draws_line_bonds() {
            append_wireframe_bond(&mut cylinders, &segment, start.color, end.color);
        }
    }

    MoleculeInstances {
        spheres,
        cylinders,
        cartoon: build_biopolymer_cartoon_world_mesh(structure, visual_state),
        ..Default::default()
    }
}

fn bond_adjacency(structure: &Structure) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        adjacency[bond.a].push(bond.b);
        adjacency[bond.b].push(bond.a);
    }
    adjacency
}

/// A Wireframe (line) bond on the GPU path. The GPU has no screen-space line
/// primitive, so the bond becomes one slim split-colored rod from atom to atom,
/// reusing the cylinder pipeline. Wireframe atoms draw no node, so — like the
/// CPU line path — bond order and aromatic dashes are ignored; the rod is the
/// whole bond.
fn append_wireframe_bond(
    cylinders: &mut Vec<CylinderInstance>,
    segment: &BondWorldSegment,
    color_a: Color32,
    color_b: Color32,
) {
    if let Some(cylinder) = cylinder_instance(
        segment.start,
        segment.end,
        WIREFRAME_BOND_RADIUS,
        color_a,
        color_b,
    ) {
        cylinders.push(cylinder);
    }
}

/// Emit the cylinder(s) for one bond segment: a single stick, two parallel
/// sticks for a double bond, three for a triple, or a full stick plus an inner
/// dashed line for an aromatic bond. The offset for multi-bonds is in a
/// view-independent plane (the ring plane, or the local sp2 plane derived from a
/// neighbor) so it rotates with the molecule.
fn append_bond_cylinders(
    cylinders: &mut Vec<CylinderInstance>,
    structure: &Structure,
    adjacency: &[Vec<usize>],
    segment: &BondWorldSegment,
    color_a: Color32,
    color_b: Color32,
) {
    let (start, end) = (segment.start, segment.end);
    let mut push = |s: Point3<f32>, e: Point3<f32>, radius: f32, ca: Color32, cb: Color32| {
        if let Some(cylinder) = cylinder_instance(s, e, radius, ca, cb) {
            cylinders.push(cylinder);
        }
    };

    if !segment.full_bond {
        push(start, end, SINGLE_BOND_RADIUS, color_a, color_b);
        return;
    }

    match segment.bond_type {
        BondType::Single => push(start, end, SINGLE_BOND_RADIUS, color_a, color_b),
        BondType::Double => {
            let offset = bond_offset_direction(structure, adjacency, segment);
            for sign in [-1.0_f32, 1.0] {
                let shift = offset * (MULTI_BOND_OFFSET * 0.5 * sign);
                push(
                    start + shift,
                    end + shift,
                    MULTI_BOND_RADIUS,
                    color_a,
                    color_b,
                );
            }
        }
        BondType::Triple => {
            push(start, end, MULTI_BOND_RADIUS, color_a, color_b);
            let offset = bond_offset_direction(structure, adjacency, segment);
            for sign in [-1.0_f32, 1.0] {
                let shift = offset * (MULTI_BOND_OFFSET * sign);
                push(
                    start + shift,
                    end + shift,
                    MULTI_BOND_RADIUS,
                    color_a,
                    color_b,
                );
            }
        }
        BondType::Aromatic => {
            push(start, end, SINGLE_BOND_RADIUS, color_a, color_b);
            append_aromatic_dashes(cylinders, structure, adjacency, segment, color_a, color_b);
        }
    }
}

/// The inner dashed line of an aromatic bond, offset toward the ring center.
fn append_aromatic_dashes(
    cylinders: &mut Vec<CylinderInstance>,
    structure: &Structure,
    adjacency: &[Vec<usize>],
    segment: &BondWorldSegment,
    color_a: Color32,
    color_b: Color32,
) {
    let axis_vector = segment.end - segment.start;
    let length = axis_vector.norm();
    let Some(axis) = axis_vector.try_normalize(1e-5) else {
        return;
    };
    let offset = bond_offset_direction(structure, adjacency, segment);
    let dash_origin = segment.start + offset * AROMATIC_DASH_OFFSET;
    let mut cursor = 0.0;
    while cursor < length {
        let dash_end = (cursor + AROMATIC_DASH_LENGTH).min(length);
        let color = if (cursor + dash_end) * 0.5 < length * 0.5 {
            color_a
        } else {
            color_b
        };
        if let Some(cylinder) = cylinder_instance(
            dash_origin + axis * cursor,
            dash_origin + axis * dash_end,
            AROMATIC_DASH_RADIUS,
            color,
            color,
        ) {
            cylinders.push(cylinder);
        }
        cursor += AROMATIC_DASH_LENGTH + AROMATIC_GAP_LENGTH;
    }
}

/// A unit direction perpendicular to the bond, chosen in a view-independent
/// plane: toward the aromatic ring center when known, otherwise within the local
/// sp2 plane defined by a neighboring atom, falling back to an arbitrary
/// perpendicular.
fn bond_offset_direction(
    structure: &Structure,
    adjacency: &[Vec<usize>],
    segment: &BondWorldSegment,
) -> Vector3<f32> {
    let axis = (segment.end - segment.start)
        .try_normalize(1e-5)
        .unwrap_or_else(|| Vector3::new(1.0, 0.0, 0.0));

    if let Some(center) = segment.aromatic_center {
        let midpoint = Point3::from((segment.start.coords + segment.end.coords) * 0.5);
        let inward = center - midpoint;
        let projected = inward - axis * inward.dot(&axis);
        if projected.norm_squared() > 1e-4 {
            return projected.normalize();
        }
    }

    for &atom in &[segment.start_atom, segment.end_atom] {
        let origin = structure.atoms[atom].position;
        for &neighbor in &adjacency[atom] {
            if neighbor == segment.start_atom || neighbor == segment.end_atom {
                continue;
            }
            let direction = structure.atoms[neighbor].position - origin;
            let projected = direction - axis * direction.dot(&axis);
            if projected.norm_squared() > 1e-4 {
                return projected.normalize();
            }
        }
    }

    perpendicular_basis(axis).0
}

fn sphere_radius(
    structure: &Structure,
    index: usize,
    style: AtomStyle,
    selection: &AtomSelection,
) -> Option<f32> {
    let element = &structure.atoms[index].element;
    let mut radius = if let Some(marker) = atom_marker_radius(element, style) {
        marker
    } else if style.draws_point() {
        atom_ball_radius(element) * POINT_SPHERE_SCALE
    } else {
        return None;
    };
    if selection.primary() == Some(index) {
        radius *= 1.18;
    } else if selection.contains(index) {
        radius *= 1.10;
    }
    Some(radius)
}

fn cylinder_instance(
    start: Point3<f32>,
    end: Point3<f32>,
    radius: f32,
    color_a: Color32,
    color_b: Color32,
) -> Option<CylinderInstance> {
    let axis_vector = end - start;
    let length = axis_vector.norm();
    let axis = axis_vector.try_normalize(1e-5)?;
    let (side_u, side_v) = perpendicular_basis(axis);
    Some(CylinderInstance {
        start_len: [start.x, start.y, start.z, length],
        axis_radius: [axis.x, axis.y, axis.z, radius],
        side_u: [side_u.x, side_u.y, side_u.z, 0.0],
        side_v: [side_v.x, side_v.y, side_v.z, 0.0],
        color_a: color_a.to_normalized_gamma_f32(),
        color_b: color_b.to_normalized_gamma_f32(),
    })
}

fn perpendicular_basis(axis: Vector3<f32>) -> (Vector3<f32>, Vector3<f32>) {
    let reference = if axis.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let u = axis.cross(&reference).normalize();
    let v = axis.cross(&u);
    (u, v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType, Structure};
    use nalgebra::Point3;

    /// Two carbons bonded along x, both forced to `style`.
    fn bonded_pair(style: AtomStyle) -> (Structure, ViewportVisualState) {
        let structure = Structure::with_bonds(
            "pair",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(1.5, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Single)],
        );
        let mut visual = ViewportVisualState::default();
        visual.atom_styles.insert(0, style);
        visual.atom_styles.insert(1, style);
        (structure, visual)
    }

    #[test]
    fn wireframe_bonds_emit_one_thin_rod_and_no_nodes() {
        // The GPU path has no line primitive, so wireframe used to produce nothing
        // — atoms vanished. The bond must now come through as a single slim rod,
        // with no atom node (wireframe is bond-only).
        let (structure, visual) = bonded_pair(AtomStyle::Wireframe);
        let instances = build_molecule_instances(&structure, &AtomSelection::default(), &visual);
        assert!(instances.spheres.is_empty(), "wireframe draws no atom node");
        assert_eq!(instances.cylinders.len(), 1, "the bond becomes one rod");
        assert_eq!(instances.cylinders[0].axis_radius[3], WIREFRAME_BOND_RADIUS);
    }

    #[test]
    fn stick_joint_matches_its_bond_radius() {
        // The joint node must be exactly as thick as the bond so a stick chain
        // reads as one smooth tube, not a string of beads at every atom.
        let (structure, visual) = bonded_pair(AtomStyle::Stick);
        let instances = build_molecule_instances(&structure, &AtomSelection::default(), &visual);
        assert_eq!(instances.spheres.len(), 2, "each atom caps its bond");
        for sphere in &instances.spheres {
            assert_eq!(sphere.pos_radius[3], SINGLE_BOND_RADIUS);
        }
        assert_eq!(instances.cylinders.len(), 1);
        assert_eq!(instances.cylinders[0].axis_radius[3], SINGLE_BOND_RADIUS);
    }

    #[test]
    fn ball_and_stick_node_stays_fatter_than_the_bond() {
        // Ball-and-stick is unchanged: its element-sized ball is still distinctly
        // larger than the bond, so the Stick fix did not flatten it.
        let (structure, visual) = bonded_pair(AtomStyle::BallAndStick);
        let instances = build_molecule_instances(&structure, &AtomSelection::default(), &visual);
        assert_eq!(instances.spheres.len(), 2);
        assert!(instances.spheres[0].pos_radius[3] > SINGLE_BOND_RADIUS);
    }
}
