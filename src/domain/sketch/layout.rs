//! 2D auto-layout ("clean up") for sketches.
//!
//! A Fruchterman–Reingold-style force-directed relaxation: bonded atoms are
//! pulled toward [`BOND_LENGTH`] while every atom pair repels, which detangles
//! a messy drawing and gives ring systems an even, readable spacing. This is a
//! pure 2D tidy; turning the result into 3D is the separate
//! [`crate::workflows::sketch_to_structure`] step.
//!
//! It is also used to seed coordinates for structures that arrive without a
//! depiction (e.g. SMILES import): [`seed_layout`] lays atoms out along a
//! breadth-first walk so they are not all stacked at the origin, then
//! [`clean_up`] relaxes them.

use nalgebra::{Point2, Rotation2, Vector2};

use super::{BOND_LENGTH, Sketch};

const ITERATIONS: usize = 320;

/// Strength of the centering force that keeps disconnected fragments bounded.
const GRAVITY: f32 = 0.1;

/// Relax atom positions in place. When `selection` is `Some`, only those atoms
/// move (the rest act as fixed anchors); when `None`, the whole sketch is
/// relaxed. The moved atoms' centroid is restored afterward so the drawing does
/// not drift away from where the user left it.
pub fn clean_up(sketch: &mut Sketch, selection: Option<&[usize]>) {
    let n = sketch.atoms.len();
    if n < 2 {
        return;
    }

    let movable = movable_mask(sketch, selection);
    let before = movable_centroid(sketch, &movable);

    let k = BOND_LENGTH;
    let k_squared = k * k;
    let mut temperature = k * 2.0;

    for _ in 0..ITERATIONS {
        let mut displacement = vec![Vector2::zeros(); n];

        // Repulsion between every pair.
        for i in 0..n {
            for j in (i + 1)..n {
                let (direction, distance) = separation(sketch, i, j);
                let force = k_squared / distance;
                displacement[i] += direction * force;
                displacement[j] -= direction * force;
            }
        }

        // Attraction along bonds.
        for bond in &sketch.bonds {
            let delta = sketch.atoms[bond.a].pos - sketch.atoms[bond.b].pos;
            let distance = delta.norm().max(1.0e-3);
            let direction = delta / distance;
            let force = distance * distance / k;
            displacement[bond.a] -= direction * force;
            displacement[bond.b] += direction * force;
        }

        // Gravity toward the layout centre. Without it, atoms in separate
        // connected components feel only mutual repulsion and drift apart every
        // iteration; this confining force lets them settle at a finite spacing.
        let center = sketch.centroid();
        for (i, atom) in sketch.atoms.iter().enumerate() {
            displacement[i] += (center - atom.pos) * GRAVITY;
        }

        // Apply, capped by the cooling temperature.
        for i in 0..n {
            if !movable[i] {
                continue;
            }
            let length = displacement[i].norm();
            if length > 1.0e-6 {
                sketch.atoms[i].pos += displacement[i] / length * length.min(temperature);
            }
        }

        temperature = (temperature * 0.98).max(k * 0.01);
    }

    // Restore the centroid of the moved atoms.
    let after = movable_centroid(sketch, &movable);
    let correction = before - after;
    for (index, atom) in sketch.atoms.iter_mut().enumerate() {
        if movable[index] {
            atom.pos += correction;
        }
    }
}

/// Lay atoms out along a breadth-first walk of the bond graph so a freshly
/// parsed (coordinate-less) sketch has a sane starting depiction. Existing
/// positions are overwritten.
pub fn seed_layout(sketch: &mut Sketch) {
    let n = sketch.atoms.len();
    if n == 0 {
        return;
    }

    let mut adjacency = vec![Vec::new(); n];
    for bond in &sketch.bonds {
        adjacency[bond.a].push(bond.b);
        adjacency[bond.b].push(bond.a);
    }

    let mut placed = vec![false; n];
    let mut direction_for = vec![Vector2::new(1.0, 0.0); n];

    // Seed each connected component on its own row so they do not overlap.
    let mut component_offset = Vector2::zeros();
    for start in 0..n {
        if placed[start] {
            continue;
        }
        sketch.atoms[start].pos = Point2::from(component_offset);
        placed[start] = true;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);

        while let Some(current) = queue.pop_front() {
            let base = direction_for[current];
            // Fan unplaced neighbours out around the incoming direction.
            let mut slot = 0i32;
            for &neighbor in &adjacency[current] {
                if placed[neighbor] {
                    continue;
                }
                let angle = (slot as f32)
                    * std::f32::consts::FRAC_PI_3
                    * if slot % 2 == 0 { 1.0 } else { -1.0 };
                let direction = Rotation2::new(angle) * base;
                sketch.atoms[neighbor].pos = sketch.atoms[current].pos + direction * BOND_LENGTH;
                direction_for[neighbor] = direction;
                placed[neighbor] = true;
                queue.push_back(neighbor);
                slot += 1;
            }
        }

        component_offset += Vector2::new(0.0, -BOND_LENGTH * 6.0);
    }

    clean_up(sketch, None);
}

fn movable_mask(sketch: &Sketch, selection: Option<&[usize]>) -> Vec<bool> {
    match selection {
        None => vec![true; sketch.atoms.len()],
        Some(indices) => {
            let mut mask = vec![false; sketch.atoms.len()];
            for &index in indices {
                if index < mask.len() {
                    mask[index] = true;
                }
            }
            mask
        }
    }
}

fn movable_centroid(sketch: &Sketch, movable: &[bool]) -> Point2<f32> {
    let mut sum = Vector2::zeros();
    let mut count = 0;
    for (index, atom) in sketch.atoms.iter().enumerate() {
        if movable[index] {
            sum += atom.pos.coords;
            count += 1;
        }
    }
    if count == 0 {
        Point2::origin()
    } else {
        Point2::from(sum / count as f32)
    }
}

/// Unit separation direction (i away from j) and a non-zero distance. Coincident
/// atoms get a deterministic nudge so the layout is reproducible.
fn separation(sketch: &Sketch, i: usize, j: usize) -> (Vector2<f32>, f32) {
    let delta = sketch.atoms[i].pos - sketch.atoms[j].pos;
    let distance = delta.norm();
    if distance < 1.0e-3 {
        let angle = ((i * 17 + j * 31) % 360) as f32 * std::f32::consts::PI / 180.0;
        let nudged = Vector2::new(angle.cos(), angle.sin());
        (nudged, 1.0e-2)
    } else {
        (delta / distance, distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BondType;

    #[test]
    fn clean_up_separates_a_bonded_pair_to_bond_length() {
        let mut sketch = Sketch::new();
        sketch.add_atom("C", Point2::new(0.0, 0.0));
        sketch.add_atom("C", Point2::new(0.05, 0.0)); // nearly coincident
        sketch.add_bond(0, 1, BondType::Single);
        clean_up(&mut sketch, None);
        let distance = (sketch.atoms[0].pos - sketch.atoms[1].pos).norm();
        assert!(
            (distance - BOND_LENGTH).abs() < 0.4 * BOND_LENGTH,
            "distance was {distance}"
        );
    }

    #[test]
    fn seed_layout_produces_distinct_positions() {
        let mut sketch = Sketch::new();
        for _ in 0..6 {
            sketch.add_atom("C", Point2::origin());
        }
        for i in 0..6 {
            sketch.add_bond(i, (i + 1) % 6, BondType::Aromatic);
        }
        seed_layout(&mut sketch);
        // No two atoms should land on top of each other.
        for i in 0..6 {
            for j in (i + 1)..6 {
                let distance = (sketch.atoms[i].pos - sketch.atoms[j].pos).norm();
                assert!(distance > 0.3, "atoms {i},{j} too close: {distance}");
            }
        }
    }

    #[test]
    fn clean_up_respects_selection_anchors() {
        let mut sketch = Sketch::new();
        sketch.add_atom("C", Point2::new(0.0, 0.0));
        sketch.add_atom("C", Point2::new(10.0, 0.0));
        sketch.add_bond(0, 1, BondType::Single);
        // Only atom 1 may move; atom 0 stays put.
        clean_up(&mut sketch, Some(&[1]));
        assert_eq!(sketch.atoms[0].pos, Point2::new(0.0, 0.0));
    }

    #[test]
    fn clean_up_keeps_disconnected_fragments_bounded() {
        // Two separate C–C fragments plus a lone atom. Without the centering
        // force these would repel apart without limit.
        let mut sketch = Sketch::new();
        sketch.add_atom("C", Point2::new(0.0, 0.0));
        sketch.add_atom("C", Point2::new(0.1, 0.0));
        sketch.add_bond(0, 1, BondType::Single);
        sketch.add_atom("C", Point2::new(0.0, 0.2));
        sketch.add_atom("C", Point2::new(0.1, 0.2));
        sketch.add_bond(2, 3, BondType::Single);
        sketch.add_atom("O", Point2::new(0.05, 0.1));
        clean_up(&mut sketch, None);
        let (min, max) = sketch.bounds().unwrap();
        let span = (max - min).norm();
        assert!(span < 30.0, "layout diverged: span was {span}");
    }
}
