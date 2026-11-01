//! Polymer annotations on a [`Sketch`]: structural repeat units (SRU Sgroups)
//! and the geometry of their brackets.

use anyhow::{Result, bail};
use nalgebra::{Point2, Vector2};

use super::{BOND_LENGTH, Sketch};

/// Half the length of a generated bracket, in model units.
const BRACKET_HALF_LENGTH: f32 = BOND_LENGTH * 0.75;

/// How consecutive repeat units join (the V3000 `CONNECT` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatConnectivity {
    HeadToTail,
    HeadToHead,
    #[default]
    EitherUnknown,
}

/// One bracket segment. Walking from the first point to the second keeps the
/// repeat unit on the right, the Molfile `BRKXYZ` convention.
pub type Bracket = [Point2<f32>; 2];

/// A bracketed structural repeat unit. A copolymer is several of these.
#[derive(Debug, Clone, PartialEq)]
pub struct RepeatUnit {
    /// Sketch atoms inside the brackets.
    pub atoms: Vec<usize>,
    /// The two sketch bonds that cross the brackets. Each has exactly one
    /// endpoint in [`Self::atoms`].
    pub crossing_bonds: [usize; 2],
    /// Subscript such as `n`, `x`, or `10`. Empty when the source had none.
    pub label: String,
    pub connectivity: RepeatConnectivity,
    /// Bracket segments in `BRKXYZ` order.
    pub brackets: Vec<Bracket>,
}

impl Sketch {
    /// Check that every repeat unit refers to real atoms and to bonds that
    /// actually cross its boundary.
    pub fn validate_repeat_units(&self) -> Result<()> {
        for (index, unit) in self.repeat_units.iter().enumerate() {
            let number = index + 1;
            if unit.atoms.is_empty() {
                bail!("repeat unit {number} contains no atoms");
            }
            let mut inside = vec![false; self.atoms.len()];
            for &atom in &unit.atoms {
                match inside.get_mut(atom) {
                    Some(true) => bail!("repeat unit {number} lists atom {} twice", atom + 1),
                    Some(slot) => *slot = true,
                    None => bail!("repeat unit {number} refers to a missing atom"),
                }
            }
            if unit.crossing_bonds[0] == unit.crossing_bonds[1] {
                bail!("repeat unit {number} lists the same crossing bond twice");
            }
            for &bond in &unit.crossing_bonds {
                let Some(data) = self.bonds.get(bond) else {
                    bail!("repeat unit {number} refers to a missing bond");
                };
                let ends = (inside.get(data.a), inside.get(data.b));
                let (Some(a), Some(b)) = ends else {
                    bail!("bond {} refers to a missing atom", bond + 1);
                };
                if a == b {
                    bail!(
                        "bond {} does not cross the brackets of repeat unit {number}",
                        bond + 1
                    );
                }
            }
            let crossings = self
                .bonds
                .iter()
                .filter(|bond| {
                    let inside_at = |atom: usize| inside.get(atom).copied().unwrap_or(false);
                    inside_at(bond.a) != inside_at(bond.b)
                })
                .count();
            if crossings != 2 {
                bail!(
                    "repeat unit {number} has {crossings} bonds crossing its brackets; exactly 2 \
                     are supported"
                );
            }
        }
        Ok(())
    }

    /// Replace every repeat unit's brackets with ones generated from its
    /// crossing bonds.
    pub fn refit_repeat_unit_brackets(&mut self) {
        for index in 0..self.repeat_units.len() {
            self.repeat_units[index].brackets = self.fitted_brackets(&self.repeat_units[index]);
        }
    }

    /// Brackets for `unit`, perpendicular to each crossing bond at its midpoint,
    /// or a third of the way along when two units share the bond, so a copolymer
    /// reads `]x [ ]y` instead of overlapping. `unit` should belong to this
    /// sketch, and bonds or atoms that do not exist are skipped.
    pub fn fitted_brackets(&self, unit: &RepeatUnit) -> Vec<Bracket> {
        unit.crossing_bonds
            .iter()
            .filter_map(|&index| {
                let bond = self.bonds.get(index)?;
                let sharing = self
                    .repeat_units
                    .iter()
                    .filter(|unit| unit.crossing_bonds.contains(&index))
                    .count();
                let (inner, outer) = if unit.atoms.contains(&bond.a) {
                    (bond.a, bond.b)
                } else {
                    (bond.b, bond.a)
                };
                let inner = self.atoms.get(inner)?.pos;
                let outer = self.atoms.get(outer)?.pos;
                let outward = (outer - inner)
                    .try_normalize(1.0e-4)
                    .unwrap_or_else(|| Vector2::new(1.0, 0.0));
                let along = Vector2::new(outward.y, -outward.x) * BRACKET_HALF_LENGTH;
                let fraction = if sharing > 1 { 1.0 / 3.0 } else { 0.5 };
                let middle = inner + (outer - inner) * fraction;
                Some([middle - along, middle + along])
            })
            .collect()
    }

    /// Apply an atom and bond renumbering to the repeat units. A unit that lost
    /// a crossing bond or all of its atoms no longer describes a repeat and is
    /// removed.
    pub(super) fn reindex_repeat_units(
        &mut self,
        atom_map: &[Option<usize>],
        bond_map: &[Option<usize>],
    ) {
        let before = self.repeat_units.len();
        self.repeat_units.retain_mut(|unit| {
            let crossing = unit
                .crossing_bonds
                .map(|bond| bond_map.get(bond).copied().flatten());
            let [Some(first), Some(second)] = crossing else {
                return false;
            };
            unit.crossing_bonds = [first, second];
            unit.atoms = unit
                .atoms
                .iter()
                .filter_map(|&atom| atom_map.get(atom).copied().flatten())
                .collect();
            !unit.atoms.is_empty()
        });
        // A survivor may have shared a crossing bond with the removed unit.
        if self.repeat_units.len() != before {
            self.refit_repeat_unit_brackets();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::BondType;

    /// `*-C-C-*` with both carbons bracketed.
    fn unit_sketch() -> Sketch {
        let mut sketch = Sketch::new();
        for x in 0..4 {
            sketch.add_atom("C", Point2::new(x as f32 * BOND_LENGTH, 0.0));
        }
        sketch.atoms[0] = super::super::SketchAtom::attachment(Point2::new(0.0, 0.0));
        sketch.atoms[3] = super::super::SketchAtom::attachment(Point2::new(4.5, 0.0));
        for a in 0..3 {
            sketch.add_bond(a, a + 1, BondType::Single);
        }
        sketch.repeat_units.push(RepeatUnit {
            atoms: vec![1, 2],
            crossing_bonds: [0, 2],
            label: "n".to_string(),
            connectivity: RepeatConnectivity::HeadToTail,
            brackets: Vec::new(),
        });
        sketch.refit_repeat_unit_brackets();
        sketch
    }

    #[test]
    fn fitted_brackets_keep_the_unit_on_the_right() {
        let sketch = unit_sketch();
        let [left, right] = sketch.repeat_units[0].brackets[..] else {
            panic!("expected two brackets");
        };
        assert!((left[0].x - 0.75).abs() < 1.0e-5);
        assert!(left[0].y < left[1].y);
        assert!((right[0].x - 3.75).abs() < 1.0e-5);
        assert!(right[0].y > right[1].y);
    }

    #[test]
    fn units_sharing_a_crossing_bond_get_separate_brackets() {
        let mut sketch = unit_sketch();
        sketch.repeat_units[0].atoms = vec![1];
        sketch.repeat_units[0].crossing_bonds = [0, 1];
        sketch.repeat_units.push(RepeatUnit {
            atoms: vec![2],
            crossing_bonds: [1, 2],
            label: "y".to_string(),
            connectivity: RepeatConnectivity::HeadToTail,
            brackets: Vec::new(),
        });
        sketch.refit_repeat_unit_brackets();
        let x_closing = sketch.repeat_units[0].brackets[1][0].x;
        let y_opening = sketch.repeat_units[1].brackets[0][0].x;
        assert!((x_closing - 2.0).abs() < 1.0e-5, "{x_closing}");
        assert!((y_opening - 2.5).abs() < 1.0e-5, "{y_opening}");
    }

    #[test]
    fn validation_rejects_a_bond_that_does_not_cross() {
        let mut sketch = unit_sketch();
        assert!(sketch.validate_repeat_units().is_ok());
        sketch.repeat_units[0].crossing_bonds = [0, 1];
        let error = sketch.validate_repeat_units().unwrap_err().to_string();
        assert!(error.contains("does not cross"), "{error}");
    }

    #[test]
    fn validation_rejects_an_unlisted_crossing_bond() {
        let mut sketch = unit_sketch();
        let methyl = sketch.add_atom("C", Point2::new(1.5, 1.5));
        sketch.add_bond(1, methyl, BondType::Single);
        let error = sketch.validate_repeat_units().unwrap_err().to_string();
        assert!(error.contains("3 bonds crossing"), "{error}");
    }

    #[test]
    fn batch_removal_reindexes_crossing_bonds() {
        let mut sketch = unit_sketch();
        let extra = sketch.add_atom("O", Point2::new(0.0, 3.0));
        sketch.remove_atoms(&[extra]);
        assert_eq!(sketch.repeat_units[0].crossing_bonds, [0, 2]);
        sketch.remove_atoms(&[0, 3]);
        assert!(sketch.repeat_units.is_empty());
    }

    #[test]
    fn removing_an_inner_atom_drops_the_unit_with_its_crossing_bond() {
        let mut sketch = unit_sketch();
        sketch.remove_atom(1);
        assert!(sketch.repeat_units.is_empty());
    }

    #[test]
    fn removing_an_unrelated_atom_reindexes_the_unit() {
        let mut sketch = unit_sketch();
        let extra = sketch.add_atom("O", Point2::new(0.0, 3.0));
        let first = sketch.atoms.remove(extra);
        sketch.atoms.insert(0, first);
        for bond in &mut sketch.bonds {
            bond.a += 1;
            bond.b += 1;
        }
        sketch.repeat_units[0].atoms = vec![2, 3];
        sketch.remove_atom(0);
        assert_eq!(sketch.repeat_units[0].atoms, vec![1, 2]);
        assert_eq!(sketch.repeat_units[0].crossing_bonds, [0, 2]);
        assert!(sketch.validate_repeat_units().is_ok());
    }

    #[test]
    fn removing_an_earlier_bond_shifts_crossing_bonds() {
        let mut sketch = unit_sketch();
        let extra = sketch.add_atom("O", Point2::new(0.0, 3.0));
        sketch.add_bond(extra, 1, BondType::Single);
        sketch.bonds.rotate_right(1);
        sketch.repeat_units[0].crossing_bonds = [1, 3];
        sketch.remove_bond(0);
        assert_eq!(sketch.repeat_units[0].crossing_bonds, [0, 2]);
    }

    #[test]
    fn moving_atoms_refits_the_brackets() {
        let mut sketch = unit_sketch();
        sketch.repeat_units[0].brackets.clear();
        sketch.translate_atoms(&[3], Vector2::new(0.0, 1.0));
        assert_eq!(sketch.repeat_units[0].brackets.len(), 2);
    }
}
