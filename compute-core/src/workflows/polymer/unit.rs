//! A polymer repeat unit: a monomer SMILES whose two `[*]` wildcards mark where
//! the chain continues, embedded once in 3D with hydrogen caps standing in for
//! the neighbouring units.

use std::collections::VecDeque;

use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Unit};

use crate::domain::{BondType, Structure, sketch::Sketch, smiles};
use crate::workflows::sketch_to_structure::sketch_to_structure_reproducible;

/// One repeat unit, capped and embedded. Atom order is the SMILES order followed
/// by the implicit hydrogens, so `atom_names` line up with `structure.atoms`
/// and stay identical in every copy a chain makes of the unit.
#[derive(Debug, Clone)]
pub struct RepeatUnit {
    /// Single-letter label used in a copolymer sequence (`A`, `B`, …).
    pub label: char,
    pub smiles: String,
    /// The capped monomer: both wildcards replaced by hydrogen.
    pub structure: Structure,
    /// Heavy atom the chain enters through, and the cap hydrogen standing in for
    /// the previous unit.
    pub head: usize,
    pub head_cap: usize,
    /// Heavy atom the chain leaves through, and the cap hydrogen standing in for
    /// the next unit.
    pub tail: usize,
    pub tail_cap: usize,
    /// Per-atom names of the form element + ordinal within the unit (`C1`, `H7`).
    pub atom_names: Vec<String>,
    /// Backbone rotamers: the unit's atom positions with each rotatable bond on
    /// the head–tail path turned to its three staggered positions. The first is
    /// the embedded geometry; rotamers with an internal clash are dropped.
    pub conformers: Vec<Vec<Point3<f32>>>,
}

/// Embedding restarts and UFF steps per restart for the unit template; a step
/// limit rather than the clock, so every machine builds the same unit.
const EMBED_RESTARTS: usize = 3;
const EMBED_STEPS: usize = 400;
/// Rotatable backbone bonds sampled per unit; three rotamers each, so at most
/// 27 conformers. Longer backbones keep the remaining bonds as embedded.
const MAX_ROTATABLE_BONDS: usize = 3;
/// Non-bonded separation (Å) below which a rotamer counts as self-clashing.
const SELF_CLASH: f32 = 1.8;

impl RepeatUnit {
    /// Parse and embed a repeat unit. The SMILES must carry exactly two `[*]`
    /// atoms, each single-bonded to one heavy atom; the first in reading order
    /// is the head, the second the tail.
    pub fn parse(label: char, input: &str) -> Result<Self> {
        let mut sketch = smiles::parse(input)?;
        let wildcards: Vec<usize> = sketch
            .atoms
            .iter()
            .enumerate()
            .filter(|(_, atom)| atom.is_attachment())
            .map(|(index, _)| index)
            .collect();
        let [head_cap, tail_cap] = wildcards[..] else {
            bail!(
                "repeat unit `{input}` needs exactly two `[*]` attachment points (found {}); \
                 e.g. polyethylene is `[*]CC[*]`",
                wildcards.len()
            );
        };
        let head = attachment_neighbor(&sketch, head_cap, input)?;
        let tail = attachment_neighbor(&sketch, tail_cap, input)?;
        for cap in [head_cap, tail_cap] {
            let atom = &mut sketch.atoms[cap];
            atom.element = "H".to_string();
            atom.explicit_hydrogens = Some(0);
        }
        let structure =
            sketch_to_structure_reproducible(&sketch, input, EMBED_RESTARTS, EMBED_STEPS)?;
        let atom_names = ordinal_names(&structure);
        let conformers = backbone_conformers(&structure, head, tail);
        Ok(Self {
            label,
            smiles: input.to_string(),
            structure,
            head,
            head_cap,
            tail,
            tail_cap,
            atom_names,
            conformers,
        })
    }

    /// Atom count a unit contributes inside a chain (both caps removed).
    pub fn interior_atom_count(&self) -> usize {
        self.structure.atoms.len() - 2
    }
}

fn attachment_neighbor(sketch: &Sketch, cap: usize, input: &str) -> Result<usize> {
    let bonds: Vec<_> = sketch
        .bonds
        .iter()
        .filter(|bond| bond.a == cap || bond.b == cap)
        .collect();
    let [bond] = bonds[..] else {
        bail!(
            "each `[*]` in repeat unit `{input}` must have exactly one bond (one has {})",
            bonds.len()
        );
    };
    if bond.order != BondType::Single {
        bail!("the `[*]` attachment bonds in repeat unit `{input}` must be single bonds");
    }
    let neighbor = if bond.a == cap { bond.b } else { bond.a };
    if sketch.atoms[neighbor].is_attachment() {
        bail!("repeat unit `{input}` bonds its two `[*]` atoms directly to each other");
    }
    Ok(neighbor)
}

fn neighbours(structure: &Structure) -> Vec<Vec<usize>> {
    let mut neighbours = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbours[bond.a].push(bond.b);
        neighbours[bond.b].push(bond.a);
    }
    neighbours
}

/// Atoms reachable from `start` without crossing the bond `start`–`blocked`.
fn side_of(neighbours: &[Vec<usize>], start: usize, blocked: usize) -> Vec<bool> {
    let mut seen = vec![false; neighbours.len()];
    seen[start] = true;
    let mut queue = VecDeque::from([start]);
    while let Some(atom) = queue.pop_front() {
        for &next in &neighbours[atom] {
            if (atom == start && next == blocked) || seen[next] {
                continue;
            }
            seen[next] = true;
            queue.push_back(next);
        }
    }
    seen
}

fn shortest_path(neighbours: &[Vec<usize>], from: usize, to: usize) -> Vec<usize> {
    let mut previous = vec![usize::MAX; neighbours.len()];
    previous[from] = from;
    let mut queue = VecDeque::from([from]);
    while let Some(atom) = queue.pop_front() {
        if atom == to {
            break;
        }
        for &next in &neighbours[atom] {
            if previous[next] == usize::MAX {
                previous[next] = atom;
                queue.push_back(next);
            }
        }
    }
    if previous[to] == usize::MAX {
        return Vec::new();
    }
    let mut path = vec![to];
    while *path.last().unwrap_or(&from) != from {
        path.push(previous[*path.last().unwrap_or(&from)]);
    }
    path.reverse();
    path
}

/// The embedded geometry plus every staggered combination of the rotatable
/// single bonds on the head–tail path (non-ring bonds whose far side holds the
/// tail), minus combinations with an internal clash.
fn backbone_conformers(structure: &Structure, head: usize, tail: usize) -> Vec<Vec<Point3<f32>>> {
    let base: Vec<Point3<f32>> = structure.atoms.iter().map(|a| a.position).collect();
    let neighbours = neighbours(structure);
    let path = shortest_path(&neighbours, head, tail);
    let rotatable: Vec<(usize, usize, Vec<bool>)> = path
        .windows(2)
        .filter_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let single = structure.bonds.iter().any(|bond| {
                ((bond.a == a && bond.b == b) || (bond.a == b && bond.b == a))
                    && bond.bond_type == BondType::Single
            });
            let moving = side_of(&neighbours, b, a);
            // A ring bond leaves `a` reachable from `b`; rotating it would tear the ring.
            (single && !moving[a]).then_some((a, b, moving))
        })
        .take(MAX_ROTATABLE_BONDS)
        .collect();

    let topological = topological_distances(&neighbours);
    let mut conformers = vec![base.clone()];
    let combinations = 3usize.pow(rotatable.len() as u32);
    for combination in 1..combinations {
        let mut positions = base.clone();
        let mut code = combination;
        for (a, b, moving) in &rotatable {
            let turn = code % 3;
            code /= 3;
            if turn == 0 {
                continue;
            }
            let axis = positions[*b] - positions[*a];
            let Some(axis) = Unit::try_new(axis, 1.0e-4) else {
                continue;
            };
            let rotation =
                Rotation3::from_axis_angle(&axis, turn as f32 * std::f32::consts::TAU / 3.0);
            let pivot = positions[*b];
            for (atom, position) in positions.iter_mut().enumerate() {
                if moving[atom] {
                    *position = pivot + rotation * (*position - pivot);
                }
            }
        }
        if !self_clashes(&positions, &topological) {
            conformers.push(positions);
        }
    }
    conformers
}

/// Bond-count distance between every pair, capped at 4 (enough to tell 1-2 and
/// 1-3 neighbours from non-bonded pairs).
fn topological_distances(neighbours: &[Vec<usize>]) -> Vec<Vec<u8>> {
    let n = neighbours.len();
    (0..n)
        .map(|start| {
            let mut row = vec![4u8; n];
            row[start] = 0;
            let mut queue = VecDeque::from([start]);
            while let Some(atom) = queue.pop_front() {
                let depth = row[atom];
                if depth >= 3 {
                    continue;
                }
                for &next in &neighbours[atom] {
                    if row[next] > depth + 1 {
                        row[next] = depth + 1;
                        queue.push_back(next);
                    }
                }
            }
            row
        })
        .collect()
}

fn self_clashes(positions: &[Point3<f32>], topological: &[Vec<u8>]) -> bool {
    (0..positions.len()).any(|a| {
        ((a + 1)..positions.len())
            .any(|b| topological[a][b] > 3 && (positions[a] - positions[b]).norm() < SELF_CLASH)
    })
}

/// `C1, C2, …, H1, H2, …`: element plus its ordinal among atoms of that element.
fn ordinal_names(structure: &Structure) -> Vec<String> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    structure
        .atoms
        .iter()
        .map(|atom| {
            let count = counts.entry(atom.element.as_str()).or_insert(0);
            *count += 1;
            format!("{}{count}", atom.element)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ethylene_unit_has_two_carbons_and_caps() {
        let unit = RepeatUnit::parse('A', "[*]CC[*]").unwrap();
        assert_eq!(unit.head, 1);
        assert_eq!(unit.tail, 2);
        assert_eq!(unit.head_cap, 0);
        assert_eq!(unit.tail_cap, 3);
        // C2H4 plus two cap hydrogens = ethane.
        assert_eq!(unit.structure.atoms.len(), 8);
        assert_eq!(unit.structure.atoms[unit.head_cap].element, "H");
        assert_eq!(unit.atom_names[unit.head], "C1");
        assert_eq!(unit.interior_atom_count(), 6);
        // The C–C backbone bond gives three staggered rotamers.
        assert_eq!(unit.conformers.len(), 3);
    }

    #[test]
    fn rejects_wrong_attachment_count() {
        let err = RepeatUnit::parse('A', "CC[*]").unwrap_err().to_string();
        assert!(err.contains("exactly two"), "{err}");
    }

    #[test]
    fn rejects_double_bonded_attachment() {
        let err = RepeatUnit::parse('A', "[*]=CC[*]").unwrap_err().to_string();
        assert!(err.contains("single bonds"), "{err}");
    }

    #[test]
    fn styrene_unit_keeps_ring_hydrogens() {
        let unit = RepeatUnit::parse('S', "[*]CC([*])c1ccccc1").unwrap();
        // C8H8 + 2 caps.
        assert_eq!(unit.structure.atoms.len(), 18);
        assert_eq!(unit.head, 1);
        assert_eq!(unit.tail, 2);
        assert!(unit.conformers.len() > 1);
    }

    #[test]
    fn ring_bonds_on_the_backbone_are_not_rotated() {
        let unit = RepeatUnit::parse('P', "[*]c1ccc([*])cc1").unwrap();
        assert_eq!(unit.conformers.len(), 1);
    }
}
