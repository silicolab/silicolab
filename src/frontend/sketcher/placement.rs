//! Ring-template placement and fusion, in model space.
//!
//! Given the armed [`RingTemplate`] and the pointer position, decide how a fresh
//! ring lands on the drawing: fused along an existing bond (shared edge), fused
//! at an existing atom (spiro), or placed freely on empty canvas. The result is
//! a [`RingPlacement`] the sketcher previews and then commits, merging shared
//! atoms instead of duplicating them.

use nalgebra::{Point2, Rotation2, Vector2};

use crate::domain::{
    BondType,
    sketch::{RingTemplate, Sketch},
};

/// How an armed ring will land on the canvas.
pub struct RingPlacement {
    /// Final model positions of each ring vertex.
    pub positions: Vec<Point2<f32>>,
    /// For each vertex, the existing atom it fuses onto (if any).
    pub vertices: Vec<Option<usize>>,
    /// Ring edges as ring-local indices.
    pub bonds: Vec<(usize, usize, BondType)>,
}

/// Tolerance (as a fraction of bond length) for snapping a ring vertex onto an
/// existing atom.
const SNAP_FRACTION: f32 = 0.45;

pub fn place_ring(
    sketch: &Sketch,
    template: RingTemplate,
    pointer: Point2<f32>,
    snap_radius: f32,
) -> RingPlacement {
    let (local, bonds) = template.build();

    // 1) Edge fusion — pointer near an existing bond.
    if let Some(bond_index) = sketch.nearest_bond(pointer, snap_radius)
        && let Some(positions) = edge_fuse(sketch, &local, bond_index, pointer)
    {
        return finalize(sketch, positions, bonds);
    }

    // 2) Atom fusion (spiro) — pointer near an existing atom.
    if let Some(atom) = sketch.nearest_atom(pointer, snap_radius) {
        let positions = atom_fuse(sketch, &local, atom);
        return finalize(sketch, positions, bonds);
    }

    // 3) Free placement — centre the ring on the pointer.
    let positions = local.iter().map(|vertex| pointer + vertex.coords).collect();
    finalize(sketch, positions, bonds)
}

/// Map template edge 0 onto an existing bond, choosing the side toward the
/// pointer so the ring grows where the user is aiming.
fn edge_fuse(
    sketch: &Sketch,
    local: &[Point2<f32>],
    bond_index: usize,
    pointer: Point2<f32>,
) -> Option<Vec<Point2<f32>>> {
    let bond = sketch.bonds.get(bond_index)?;
    let a = sketch.atoms.get(bond.a)?.pos;
    let b = sketch.atoms.get(bond.b)?.pos;
    if local.len() < 2 || (b - a).norm() < 1.0e-4 || (local[1] - local[0]).norm() < 1.0e-4 {
        return None;
    }
    let scale = (b - a).norm() / (local[1] - local[0]).norm();

    let forward = similarity(local, local[0], local[1], a, b, scale);
    let flipped = similarity(local, local[0], local[1], b, a, scale);
    let pick = if (centroid(&forward) - pointer).norm() <= (centroid(&flipped) - pointer).norm() {
        forward
    } else {
        flipped
    };
    Some(pick)
}

/// Place the ring sharing one vertex with `atom`, extending away from that
/// atom's existing bonds.
fn atom_fuse(sketch: &Sketch, local: &[Point2<f32>], atom: usize) -> Vec<Point2<f32>> {
    let center = sketch.atoms[atom].pos;
    let neighbor_sum: Vector2<f32> = sketch
        .neighbors(atom)
        .iter()
        .filter_map(|(other, _)| (sketch.atoms[*other].pos - center).try_normalize(1.0e-4))
        .sum();
    let outward = (-neighbor_sum)
        .try_normalize(1.0e-4)
        .unwrap_or_else(|| Vector2::new(0.0, 1.0));

    let v0 = local[0];
    // Direction from vertex 0 toward the (origin-centred) ring centroid.
    let to_centroid = Point2::origin() - v0;
    let angle = outward.y.atan2(outward.x) - to_centroid.y.atan2(to_centroid.x);
    let rotation = Rotation2::new(angle);
    local
        .iter()
        .map(|vertex| center + rotation * (vertex - v0))
        .collect()
}

/// Similarity transform sending `p0 → image0` and `p1 → image1` (rotation +
/// uniform `scale` + translation), applied to every local vertex.
fn similarity(
    local: &[Point2<f32>],
    p0: Point2<f32>,
    p1: Point2<f32>,
    image0: Point2<f32>,
    image1: Point2<f32>,
    scale: f32,
) -> Vec<Point2<f32>> {
    let template_dir = p1 - p0;
    let image_dir = image1 - image0;
    let angle = image_dir.y.atan2(image_dir.x) - template_dir.y.atan2(template_dir.x);
    let rotation = Rotation2::new(angle);
    local
        .iter()
        .map(|vertex| image0 + rotation * ((vertex - p0) * scale))
        .collect()
}

/// Resolve which vertices fuse onto existing atoms (within a snap tolerance).
fn finalize(
    sketch: &Sketch,
    positions: Vec<Point2<f32>>,
    bonds: Vec<(usize, usize, BondType)>,
) -> RingPlacement {
    let tolerance = SNAP_FRACTION * crate::domain::sketch::BOND_LENGTH;
    let vertices = positions
        .iter()
        .map(|position| sketch.nearest_atom(*position, tolerance))
        .collect();
    RingPlacement {
        positions,
        vertices,
        bonds,
    }
}

fn centroid(points: &[Point2<f32>]) -> Point2<f32> {
    if points.is_empty() {
        return Point2::origin();
    }
    let sum = points
        .iter()
        .fold(Vector2::zeros(), |acc, point| acc + point.coords);
    Point2::from(sum / points.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Point2;

    #[test]
    fn free_placement_centres_on_pointer() {
        let sketch = Sketch::new();
        let placement = place_ring(&sketch, RingTemplate::Benzene, Point2::new(5.0, 5.0), 0.7);
        assert!(placement.vertices.iter().all(Option::is_none));
        let centre = centroid(&placement.positions);
        assert!((centre - Point2::new(5.0, 5.0)).norm() < 0.1);
    }

    #[test]
    fn edge_fusion_shares_the_bond_atoms() {
        let mut sketch = Sketch::new();
        let a = sketch.add_atom("C", Point2::new(0.0, 0.0));
        let b = sketch.add_atom("C", Point2::new(1.5, 0.0));
        sketch.add_bond(a, b, BondType::Single);
        // Aim just above the bond midpoint.
        let placement = place_ring(&sketch, RingTemplate::Benzene, Point2::new(0.75, 0.8), 0.7);
        let fused = placement
            .vertices
            .iter()
            .filter(|vertex| vertex.is_some())
            .count();
        assert!(
            fused >= 2,
            "expected to share at least the bond's two atoms"
        );
    }
}
