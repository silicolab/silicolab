//! Self-avoiding growth of one chain: each new unit is docked onto the current
//! tail along the cap direction in the backbone rotamer and junction torsion
//! with the fewest contacts (looking one unit ahead), then (optionally) a
//! junction that still has a hard clash is relaxed with UFF.
//!
//! Every choice is a deterministic function of the seed and the geometry; no
//! step depends on the clock, so a script reproduces its chain run to run.

use anyhow::{Result, bail};
use nalgebra::{Point3, Rotation3, Unit, Vector3};

use crate::domain::{Atom, Bond, BondType, Structure};
use crate::engines::forcefield::{self, AtomOptimizationScope, OptimizationOptions};
use crate::md::solvation::splitmix64;

use super::unit::RepeatUnit;

/// Torsion candidates tried per added unit, evenly spaced over a full turn.
const TORSION_CANDIDATES: usize = 24;
/// Below these separations a non-bonded pair counts as a contact (Å): one for
/// pairs with a hydrogen, one for heavy–heavy pairs.
const CONTACT_WITH_H: f32 = 2.0;
const CONTACT_HEAVY: f32 = 2.8;
/// Separations below these count as a hard clash in the final report (Å).
const CLASH_WITH_H: f32 = 1.5;
const CLASH_HEAVY: f32 = 2.2;
/// Best-scoring placements of a unit that get the one-step lookahead.
const LOOKAHEAD_CANDIDATES: usize = 8;
/// UFF steps for one junction relaxation. A step limit, not a time budget, ends
/// it so the result does not depend on the machine.
const RELAX_STEPS: usize = 80;
/// Frozen residues kept behind the two relaxed ones so the junction feels the
/// chain it grows from.
const RELAX_BUFFER_RESIDUES: usize = 2;
/// Fallback single-bond length (Å) when UFF has no entry for the junction pair.
const FALLBACK_BOND: f32 = 1.5;

/// A chain under construction. Atoms are stored unit by unit so a residue is a
/// contiguous index range; the live tail cap is dropped when the next unit docks.
pub struct Chain {
    pub atoms: Vec<Atom>,
    pub bonds: Vec<Bond>,
    /// Residue (unit ordinal) of each atom.
    pub residue_of_atom: Vec<usize>,
    pub atom_names: Vec<String>,
    /// Head atom of the first unit (chain index).
    head: Option<usize>,
    /// `(tail atom, tail cap)` of the last unit, both chain indices.
    tail: Option<(usize, usize)>,
    pub relax: bool,
    pub relax_skipped: bool,
    seed: u64,
}

impl Chain {
    pub fn new(seed: u64, relax: bool) -> Self {
        Self {
            atoms: Vec::new(),
            bonds: Vec::new(),
            residue_of_atom: Vec::new(),
            atom_names: Vec::new(),
            head: None,
            tail: None,
            relax,
            relax_skipped: false,
            seed,
        }
    }

    /// Append one unit as residue `residue`; `next` is the unit that will follow
    /// it, if any, for the placement lookahead.
    pub fn add_unit(
        &mut self,
        unit: &RepeatUnit,
        residue: usize,
        next: Option<&RepeatUnit>,
    ) -> Result<()> {
        match self.tail {
            None => self.place_first(unit, residue),
            Some((tail, tail_cap)) => self.dock(unit, residue, tail, tail_cap, next)?,
        }
        Ok(())
    }

    fn place_first(&mut self, unit: &RepeatUnit, residue: usize) {
        let base = self.atoms.len();
        self.atoms.extend(unit.structure.atoms.iter().cloned());
        self.bonds.extend(
            unit.structure
                .bonds
                .iter()
                .map(|bond| Bond::with_type(bond.a + base, bond.b + base, bond.bond_type)),
        );
        self.residue_of_atom
            .extend(std::iter::repeat_n(residue, unit.structure.atoms.len()));
        self.atom_names.extend(unit.atom_names.iter().cloned());
        self.head = Some(base + unit.head);
        self.tail = Some((base + unit.tail, base + unit.tail_cap));
    }

    fn dock(
        &mut self,
        unit: &RepeatUnit,
        residue: usize,
        tail: usize,
        tail_cap: usize,
        next: Option<&RepeatUnit>,
    ) -> Result<()> {
        let unit_atoms = &unit.structure.atoms;
        let (anchor, axis) = docking_frame(
            self.atoms[tail].position,
            self.atoms[tail_cap].position,
            &self.atoms[tail].element,
            &unit_atoms[unit.head].element,
        );
        let skip = [tail, tail_cap];
        let obstacles = self.obstacles_near(anchor, reach(unit), &skip);
        let mut candidates: Vec<(f32, Vec<Point3<f32>>)> =
            placements(unit, anchor, axis, self.phase(residue))
                .into_iter()
                .map(|placed| (contact_score(unit, &placed, &obstacles), placed))
                .collect();
        // Stable sort keeps the generation order among equal scores, so ties
        // break the same way on every run.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(next) = next {
            candidates.truncate(LOOKAHEAD_CANDIDATES);
            for (score, placed) in &mut candidates {
                *score += self.best_next_score(unit, placed, &skip, next, residue + 1);
            }
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let Some((_, placed)) = candidates.into_iter().next() else {
            bail!("repeat unit `{}` has no conformer to place", unit.smiles);
        };
        let hard_clash = has_hard_clash(unit, &placed, &obstacles);

        // The old tail cap is replaced by the new unit's head; the new unit's
        // head cap is never added.
        self.remove_atom(tail_cap);
        let tail = if tail > tail_cap { tail - 1 } else { tail };
        let base = self.atoms.len();
        let mut chain_index = vec![usize::MAX; unit_atoms.len()];
        let mut next = base;
        for (index, atom) in unit_atoms.iter().enumerate() {
            if index == unit.head_cap {
                continue;
            }
            chain_index[index] = next;
            next += 1;
            self.atoms.push(Atom {
                element: atom.element.clone(),
                position: placed[index],
                charge: atom.charge,
            });
            self.residue_of_atom.push(residue);
            self.atom_names.push(unit.atom_names[index].clone());
        }
        for bond in &unit.structure.bonds {
            if bond.a == unit.head_cap || bond.b == unit.head_cap {
                continue;
            }
            self.bonds.push(Bond::with_type(
                chain_index[bond.a],
                chain_index[bond.b],
                bond.bond_type,
            ));
        }
        let head = chain_index[unit.head];
        self.bonds
            .push(Bond::with_type(tail, head, BondType::Single));
        self.tail = Some((chain_index[unit.tail], chain_index[unit.tail_cap]));

        if self.relax && hard_clash {
            self.relax_junction(residue);
        }
        Ok(())
    }

    fn phase(&self, residue: usize) -> f32 {
        (splitmix64(self.seed ^ residue as u64) & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU
    }

    /// Chain atoms within `radius` of `centre` as contact obstacles, leaving out
    /// the atoms in `skip` (the docking pair, whose geometry the bond fixes).
    fn obstacles_near(&self, centre: Point3<f32>, radius: f32, skip: &[usize]) -> Vec<Obstacle> {
        self.atoms
            .iter()
            .enumerate()
            .filter(|(index, atom)| {
                !skip.contains(index) && (atom.position - centre).norm() < radius
            })
            .map(|(_, atom)| Obstacle {
                position: atom.position,
                hydrogen: atom.element == "H",
            })
            .collect()
    }

    /// The lowest contact score the following unit could reach if `unit` were
    /// placed at `placed`: a one-step lookahead, so a unit is not parked where it
    /// leaves the next one no clash-free torsion.
    fn best_next_score(
        &self,
        unit: &RepeatUnit,
        placed: &[Point3<f32>],
        skip: &[usize],
        next: &RepeatUnit,
        next_residue: usize,
    ) -> f32 {
        let (anchor, axis) = docking_frame(
            placed[unit.tail],
            placed[unit.tail_cap],
            &unit.structure.atoms[unit.tail].element,
            &next.structure.atoms[next.head].element,
        );
        let radius = reach(next);
        let mut obstacles = self.obstacles_near(anchor, radius, skip);
        obstacles.extend(
            placed
                .iter()
                .enumerate()
                .filter(|(index, position)| {
                    *index != unit.head_cap
                        && *index != unit.tail
                        && *index != unit.tail_cap
                        && (**position - anchor).norm() < radius
                })
                .map(|(index, position)| Obstacle {
                    position: *position,
                    hydrogen: unit.structure.atoms[index].element == "H",
                }),
        );
        placements(next, anchor, axis, self.phase(next_residue))
            .iter()
            .map(|candidate| contact_score(next, candidate, &obstacles))
            .fold(f32::INFINITY, f32::min)
    }

    /// Chain indices of the first unit's head atom and the last unit's tail atom.
    pub fn ends(&self) -> Option<(usize, usize)> {
        Some((self.head?, self.tail?.0))
    }

    fn remove_atom(&mut self, index: usize) {
        if let Some(head) = self.head.as_mut()
            && *head > index
        {
            *head -= 1;
        }
        self.atoms.remove(index);
        self.residue_of_atom.remove(index);
        self.atom_names.remove(index);
        self.bonds.retain(|bond| bond.a != index && bond.b != index);
        for bond in &mut self.bonds {
            if bond.a > index {
                bond.a -= 1;
            }
            if bond.b > index {
                bond.b -= 1;
            }
        }
    }

    /// Relax the two newest residues with UFF, holding a short buffer of the
    /// chain behind them; atoms further back do not enter the force field, so
    /// the cost per junction does not grow with the chain. A failure (an
    /// element UFF lacks) leaves the docked geometry and is reported.
    fn relax_junction(&mut self, residue: usize) {
        let local: Vec<usize> = (0..self.atoms.len())
            .filter(|&index| self.residue_of_atom[index] + RELAX_BUFFER_RESIDUES + 1 >= residue)
            .collect();
        let mut local_index = vec![usize::MAX; self.atoms.len()];
        for (position, &index) in local.iter().enumerate() {
            local_index[index] = position;
        }
        let movable: Vec<usize> = local
            .iter()
            .enumerate()
            .filter(|(_, index)| self.residue_of_atom[**index] + 1 >= residue)
            .map(|(position, _)| position)
            .collect();
        let bonds: Vec<Bond> = self
            .bonds
            .iter()
            .filter(|bond| local_index[bond.a] != usize::MAX && local_index[bond.b] != usize::MAX)
            .map(|bond| Bond::with_type(local_index[bond.a], local_index[bond.b], bond.bond_type))
            .collect();
        let atoms: Vec<Atom> = local
            .iter()
            .map(|&index| self.atoms[index].clone())
            .collect();
        let mut structure = Structure::with_bonds("junction", atoms, bonds);
        let options = OptimizationOptions {
            max_steps: RELAX_STEPS,
            gradient_tolerance: 2.0e-3,
            initial_step_size: 0.01,
            max_atom_step: 0.1,
            atoms: AtomOptimizationScope::Selected(movable),
            max_duration: std::time::Duration::MAX,
            ..OptimizationOptions::default()
        };
        match forcefield::optimize_geometry(&mut structure, options) {
            Ok(_) => {
                for (&index, relaxed) in local.iter().zip(structure.atoms) {
                    self.atoms[index].position = relaxed.position;
                }
            }
            Err(_) => self.relax_skipped = true,
        }
    }

    /// Non-bonded pairs closer than the hard clash limits.
    pub fn clash_count(&self) -> usize {
        let n = self.atoms.len();
        let mut bonded = vec![Vec::new(); n];
        for bond in &self.bonds {
            bonded[bond.a].push(bond.b);
            bonded[bond.b].push(bond.a);
        }
        let mut count = 0;
        for a in 0..n {
            for b in (a + 1)..n {
                if bonded[a].contains(&b) || bonded[a].iter().any(|m| bonded[*m].contains(&b)) {
                    continue;
                }
                let limit = if self.atoms[a].element == "H" || self.atoms[b].element == "H" {
                    CLASH_WITH_H
                } else {
                    CLASH_HEAVY
                };
                if (self.atoms[a].position - self.atoms[b].position).norm() < limit {
                    count += 1;
                }
            }
        }
        count
    }
}

/// A chain atom a candidate placement must keep clear of.
struct Obstacle {
    position: Point3<f32>,
    hydrogen: bool,
}

/// Where a unit docking on a chain end puts its head atom, and the bond axis:
/// along the end's cap direction, at the single-bond length of the pair.
fn docking_frame(
    tail: Point3<f32>,
    tail_cap: Point3<f32>,
    tail_element: &str,
    head_element: &str,
) -> (Point3<f32>, Vector3<f32>) {
    let axis = (tail_cap - tail)
        .try_normalize(1.0e-4)
        .unwrap_or_else(|| Vector3::new(1.0, 0.0, 0.0));
    let bond_length =
        forcefield::equilibrium_bond_length(tail_element, head_element, BondType::Single)
            .unwrap_or(FALLBACK_BOND);
    (tail + axis * bond_length, axis)
}

/// Farthest any atom of any conformer gets from the unit's head, plus the
/// contact range: no chain atom beyond it can touch a placement.
fn reach(unit: &RepeatUnit) -> f32 {
    unit.conformers
        .iter()
        .flat_map(|positions| {
            let head = positions[unit.head];
            positions.iter().map(move |p| (p - head).norm())
        })
        .fold(0.0f32, f32::max)
        + CONTACT_HEAVY
}

/// Every backbone rotamer of `unit` docked at `anchor` and spun through the
/// torsion grid about `axis`, starting at `phase`.
fn placements(
    unit: &RepeatUnit,
    anchor: Point3<f32>,
    axis: Vector3<f32>,
    phase: f32,
) -> Vec<Vec<Point3<f32>>> {
    let spin_axis = Unit::new_normalize(axis);
    let mut out = Vec::with_capacity(unit.conformers.len() * TORSION_CANDIDATES);
    for positions in &unit.conformers {
        let head_pos = positions[unit.head];
        let cap_dir = (positions[unit.head_cap] - head_pos)
            .try_normalize(1.0e-4)
            .unwrap_or_else(|| Vector3::new(-1.0, 0.0, 0.0));
        let align = Rotation3::rotation_between(&cap_dir, &(-axis)).unwrap_or_else(|| {
            Rotation3::from_axis_angle(&perpendicular(cap_dir), std::f32::consts::PI)
        });
        for candidate in 0..TORSION_CANDIDATES {
            let angle =
                phase + candidate as f32 * std::f32::consts::TAU / TORSION_CANDIDATES as f32;
            let spin = Rotation3::from_axis_angle(&spin_axis, angle);
            out.push(
                positions
                    .iter()
                    .map(|position| anchor + spin * (align * (position - head_pos)))
                    .collect(),
            );
        }
    }
    out
}

/// Sum of squared contact penetrations between a placement and the obstacles.
/// The head and its cap are left out: the new bond fixes their geometry.
fn contact_score(unit: &RepeatUnit, placed: &[Point3<f32>], obstacles: &[Obstacle]) -> f32 {
    let mut score = 0.0;
    for (index, position) in placed.iter().enumerate() {
        if index == unit.head_cap || index == unit.head {
            continue;
        }
        let new_is_h = unit.structure.atoms[index].element == "H";
        for obstacle in obstacles {
            let limit = if new_is_h || obstacle.hydrogen {
                CONTACT_WITH_H
            } else {
                CONTACT_HEAVY
            };
            let distance = (obstacle.position - position).norm();
            if distance < limit {
                let penetration = limit - distance;
                score += penetration * penetration;
            }
        }
    }
    score
}

/// Whether a placement comes closer to an obstacle than the hard clash limits
/// the final report counts; only such junctions are worth a UFF relaxation.
fn has_hard_clash(unit: &RepeatUnit, placed: &[Point3<f32>], obstacles: &[Obstacle]) -> bool {
    placed.iter().enumerate().any(|(index, position)| {
        if index == unit.head_cap || index == unit.head {
            return false;
        }
        let new_is_h = unit.structure.atoms[index].element == "H";
        obstacles.iter().any(|obstacle| {
            let limit = if new_is_h || obstacle.hydrogen {
                CLASH_WITH_H
            } else {
                CLASH_HEAVY
            };
            (obstacle.position - position).norm() < limit
        })
    })
}

fn perpendicular(v: Vector3<f32>) -> Unit<Vector3<f32>> {
    let reference = if v.z.abs() < 0.9 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        Vector3::new(1.0, 0.0, 0.0)
    };
    Unit::new_normalize(v.cross(&reference))
}
