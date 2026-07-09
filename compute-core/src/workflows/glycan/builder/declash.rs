//! Rigid-fragment declashing for freshly stitched glycans.

use std::collections::HashSet;

use nalgebra::{Point3, Rotation3, Unit};

use crate::domain::Structure;

/// Resolve the residual steric clashes that stitching and idealized templates can
/// leave — the inter-residue overlap of branched arms at a glycosidic junction,
/// or the intra-residue overlap of two exocyclic arms (the sialic-acid acetamido
/// and glycerol tails). The subsystem's contract is *declash, not energy-
/// minimize*: this only rotates rigid fragments about rotatable single bonds —
/// every bond length, angle and ring stays exactly as built — until nothing
/// overlaps. A no-op for already-clean single residues and linear chains.
pub(super) fn declash(structure: &mut Structure) {
    let axes = rotatable_axes(structure);
    if axes.is_empty() {
        return;
    }
    let excluded = bonded_exclusions(structure);
    for _ in 0..MAX_DECLASH_ROUNDS {
        let mut improved = false;
        for axis in &axes {
            if relieve_axis(structure, axis, &excluded) {
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
}

const MAX_DECLASH_ROUNDS: usize = 8;
const DECLASH_STEPS: usize = 24;

/// A rigid rotation degree of freedom: spin `subtree` about the line through
/// `pivot` (an on-axis pinned atom not in the subtree) and `axis_partner`. Both
/// the bond's atoms lie on the axis, so every bond — the rotated one included — is
/// length-preserved, and intra-subtree geometry is rigid: only the fragment's
/// orientation relative to the rest of the molecule changes.
struct TorsionAxis {
    pivot: usize,
    axis_partner: usize,
    subtree: Vec<usize>,
    in_subtree: Vec<bool>,
}

/// One rotation axis per rotatable single bond: every bond that is a graph bridge
/// (cutting it splits the molecule, so ring bonds are excluded) with a non-terminal
/// atom on each side. The smaller fragment is the one rotated. This covers the
/// glycosidic φ/ψ torsions and the exocyclic chain torsions alike.
fn rotatable_axes(structure: &Structure) -> Vec<TorsionAxis> {
    let atom_count = structure.atoms.len();
    let neighbors = neighbor_lists(structure);

    let mut axes = Vec::new();
    for bond in &structure.bonds {
        let (a, b) = (bond.a, bond.b);
        // A terminal atom (only this bond) has nothing to swing.
        if neighbors[a].len() < 2 || neighbors[b].len() < 2 {
            continue;
        }
        let Some(b_side) = subtree_excluding(&neighbors, b, a) else {
            continue; // ring bond: not a bridge
        };
        // Rotate the smaller fragment about the bond; its pivot is the bond atom on
        // the larger side, which stays put.
        let (pivot, partner, subtree) = if b_side.len() * 2 <= atom_count {
            (a, b, b_side)
        } else {
            let a_side = subtree_excluding(&neighbors, a, b).expect("bridge from a");
            (b, a, a_side)
        };
        axes.push(TorsionAxis::new(pivot, partner, subtree, atom_count));
    }
    axes
}

impl TorsionAxis {
    fn new(pivot: usize, axis_partner: usize, subtree: Vec<usize>, atom_count: usize) -> Self {
        let mut in_subtree = vec![false; atom_count];
        for &atom in &subtree {
            in_subtree[atom] = true;
        }
        Self {
            pivot,
            axis_partner,
            subtree,
            in_subtree,
        }
    }
}

/// Rotate `axis.subtree` to the multiple of 15° that most relieves its clashes
/// with the rest of the molecule. Returns whether it moved.
fn relieve_axis(
    structure: &mut Structure,
    axis: &TorsionAxis,
    excluded: &HashSet<(usize, usize)>,
) -> bool {
    let pivot = structure.atoms[axis.pivot].position;
    let Some(direction) =
        (structure.atoms[axis.axis_partner].position - pivot).try_normalize(1.0e-5)
    else {
        return false;
    };
    let unit = Unit::new_normalize(direction);

    let base_penalty = axis_penalty(structure, axis, excluded);
    if base_penalty <= 1.0e-3 {
        return false;
    }

    let original: Vec<Point3<f32>> = axis
        .subtree
        .iter()
        .map(|&atom| structure.atoms[atom].position)
        .collect();

    let mut best_angle = 0.0_f32;
    let mut best_penalty = base_penalty;
    for step in 1..DECLASH_STEPS {
        let angle = step as f32 * std::f32::consts::TAU / DECLASH_STEPS as f32;
        let rotation = Rotation3::from_axis_angle(&unit, angle);
        apply_rotation(structure, axis, &original, pivot, &rotation);
        let penalty = axis_penalty(structure, axis, excluded);
        if penalty < best_penalty - 1.0e-3 {
            best_penalty = penalty;
            best_angle = angle;
        }
    }

    let rotation = Rotation3::from_axis_angle(&unit, best_angle);
    apply_rotation(structure, axis, &original, pivot, &rotation);
    best_angle != 0.0
}

fn apply_rotation(
    structure: &mut Structure,
    axis: &TorsionAxis,
    original: &[Point3<f32>],
    pivot: Point3<f32>,
    rotation: &Rotation3<f32>,
) {
    for (slot, &atom) in axis.subtree.iter().enumerate() {
        structure.atoms[atom].position = pivot + rotation * (original[slot] - pivot);
    }
}

/// Sum of squared steric overlaps between the rotated subtree and the rest of the
/// molecule (1–2 and 1–3 bonded pairs excluded). Subtree-internal distances are
/// rigid, so they are skipped.
fn axis_penalty(
    structure: &Structure,
    axis: &TorsionAxis,
    excluded: &HashSet<(usize, usize)>,
) -> f32 {
    let mut penalty = 0.0;
    for &i in &axis.subtree {
        for j in 0..structure.atoms.len() {
            if axis.in_subtree[j] {
                continue;
            }
            let key = if i < j { (i, j) } else { (j, i) };
            if excluded.contains(&key) {
                continue;
            }
            let distance = (structure.atoms[i].position - structure.atoms[j].position).norm();
            let target = clash_target(&structure.atoms[i].element, &structure.atoms[j].element);
            if distance < target {
                let overlap = target - distance;
                penalty += overlap * overlap;
            }
        }
    }
    penalty
}

/// Minimum acceptable non-bonded contact distance (Å) — below the van der Waals
/// sum so ordinary close packing is not treated as a clash, but well above a
/// fused overlap.
fn clash_target(first: &str, second: &str) -> f32 {
    match (first == "H", second == "H") {
        (true, true) => 1.6,
        (false, false) => 2.4,
        _ => 1.9,
    }
}

fn neighbor_lists(structure: &Structure) -> Vec<Vec<usize>> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }
    neighbors
}

/// Atoms reachable from `start` without crossing the `start`–`blocked` bond.
/// Returns `None` when `blocked` is reachable by another path, i.e. the bond is
/// part of a cycle and rotating about it would tear the molecule.
fn subtree_excluding(neighbors: &[Vec<usize>], start: usize, blocked: usize) -> Option<Vec<usize>> {
    let mut visited = vec![false; neighbors.len()];
    visited[start] = true;
    let mut stack = vec![start];
    let mut subtree = vec![start];
    while let Some(atom) = stack.pop() {
        for &next in &neighbors[atom] {
            if next == blocked {
                if atom == start {
                    continue; // the cut bond itself
                }
                return None; // a cycle reaches the pinned partner
            }
            if !visited[next] {
                visited[next] = true;
                subtree.push(next);
                stack.push(next);
            }
        }
    }
    Some(subtree)
}

/// 1–2 and 1–3 bonded atom pairs, which are never steric clashes.
pub(super) fn bonded_exclusions(structure: &Structure) -> HashSet<(usize, usize)> {
    let neighbors = neighbor_lists(structure);
    let ordered = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
    let mut excluded = std::collections::HashSet::new();
    for (atom, bonded) in neighbors.iter().enumerate() {
        for &near in bonded {
            excluded.insert(ordered(atom, near));
            for &far in &neighbors[near] {
                if far != atom {
                    excluded.insert(ordered(atom, far));
                }
            }
        }
    }
    excluded
}
