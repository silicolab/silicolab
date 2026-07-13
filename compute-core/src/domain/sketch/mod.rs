//! Pure 2D molecular-sketch data model.
//!
//! A [`Sketch`] is the editable, UI-free representation behind the 2D molecule
//! sketcher: heavy atoms with 2D canvas coordinates, a formal charge, and bonds
//! with an order. It owns no rendering and no IO — the frontend draws it and the
//! [`crate::workflows::sketch_to_structure`] workflow lifts it into a 3D
//! [`crate::domain::Structure`].
//!
//! Implicit hydrogens are *not* stored; they are derived on demand from the
//! valence model in the private `valence` module so the depiction stays a clean heavy-atom
//! skeleton. The same model decides when an atom is over-bonded so the canvas
//! can flag it.

mod layout;
mod templates;
mod valence;

use nalgebra::{Point2, Rotation2, Vector2};

use crate::domain::BondType;

pub use layout::{clean_up, seed_layout};
pub use templates::RingTemplate;
pub use valence::{bond_order_value, implicit_hydrogens, is_overvalent};

/// Default 2D bond length in model units (Ångström-ish; ~C–C single bond). The
/// canvas applies its own zoom on top of this.
pub const BOND_LENGTH: f32 = 1.5;

/// A heavy atom in the 2D sketch.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchAtom {
    /// Element symbol, kept normalized (`"C"`, `"Cl"`, …) via
    /// [`crate::domain::chemistry::normalized_symbol`].
    pub element: String,
    /// 2D position in model space (y points "up", as in chemistry depictions).
    pub pos: Point2<f32>,
    /// Formal charge, in electron units.
    pub charge: i32,
    /// An explicitly pinned hydrogen count (e.g. a SMILES bracket atom such as
    /// `[nH]` or `[CH2]`). When `Some`, the count is authoritative and the
    /// valence model is bypassed; when `None`, hydrogens are derived from
    /// valence.
    pub explicit_hydrogens: Option<u32>,
}

impl SketchAtom {
    pub fn new(element: impl Into<String>, pos: Point2<f32>) -> Self {
        Self {
            element: crate::domain::chemistry::normalized_symbol(&element.into()),
            pos,
            charge: 0,
            explicit_hydrogens: None,
        }
    }
}

/// A bond between two sketch atoms, identified by their indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SketchBond {
    pub a: usize,
    pub b: usize,
    pub order: BondType,
}

/// An editable 2D molecule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sketch {
    pub atoms: Vec<SketchAtom>,
    pub bonds: Vec<SketchBond>,
}

impl Sketch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// Add an atom and return its index. The element is normalized.
    pub fn add_atom(&mut self, element: impl Into<String>, pos: Point2<f32>) -> usize {
        let index = self.atoms.len();
        self.atoms.push(SketchAtom::new(element, pos));
        index
    }

    /// Connect two atoms. No-op (returns `None`) for self-bonds, out-of-range
    /// indices, or a duplicate of an existing bond. Otherwise returns the new
    /// bond index.
    pub fn add_bond(&mut self, a: usize, b: usize, order: BondType) -> Option<usize> {
        if a == b || a >= self.atoms.len() || b >= self.atoms.len() {
            return None;
        }
        if self.bond_between(a, b).is_some() {
            return None;
        }
        let index = self.bonds.len();
        self.bonds.push(SketchBond { a, b, order });
        Some(index)
    }

    /// Find the bond between two atoms regardless of endpoint order.
    pub fn bond_between(&self, a: usize, b: usize) -> Option<usize> {
        self.bonds
            .iter()
            .position(|bond| (bond.a == a && bond.b == b) || (bond.a == b && bond.b == a))
    }

    pub fn set_bond_order(&mut self, bond: usize, order: BondType) {
        if let Some(bond) = self.bonds.get_mut(bond) {
            bond.order = order;
        }
    }

    /// Step a bond's order single → double → triple → single. Aromatic bonds are
    /// reset to single (the explicit-order tools handle aromatic directly).
    pub fn cycle_bond_order(&mut self, bond: usize) {
        if let Some(bond) = self.bonds.get_mut(bond) {
            bond.order = match bond.order {
                BondType::Single => BondType::Double,
                BondType::Double => BondType::Triple,
                BondType::Triple | BondType::Aromatic => BondType::Single,
            };
        }
    }

    pub fn remove_bond(&mut self, bond: usize) {
        if bond < self.bonds.len() {
            self.bonds.remove(bond);
        }
    }

    /// Remove an atom together with its incident bonds, reindexing the rest.
    pub fn remove_atom(&mut self, atom: usize) {
        if atom >= self.atoms.len() {
            return;
        }
        self.atoms.remove(atom);
        self.bonds.retain(|bond| bond.a != atom && bond.b != atom);
        for bond in &mut self.bonds {
            if bond.a > atom {
                bond.a -= 1;
            }
            if bond.b > atom {
                bond.b -= 1;
            }
        }
    }

    /// Remove a set of atoms (and any bond touching one of them) in a single
    /// pass, reindexing the survivors.
    pub fn remove_atoms(&mut self, victims: &[usize]) {
        if victims.is_empty() {
            return;
        }
        let doomed = vec_to_mask(victims, self.atoms.len());
        // Build the old→new index remap for surviving atoms.
        let mut remap = vec![usize::MAX; self.atoms.len()];
        let mut next = 0usize;
        for (old, dead) in doomed.iter().enumerate() {
            if !dead {
                remap[old] = next;
                next += 1;
            }
        }
        let mut atom_index = 0usize;
        self.atoms.retain(|_| {
            let keep = !doomed[atom_index];
            atom_index += 1;
            keep
        });
        self.bonds.retain(|bond| !doomed[bond.a] && !doomed[bond.b]);
        for bond in &mut self.bonds {
            bond.a = remap[bond.a];
            bond.b = remap[bond.b];
        }
    }

    /// Neighbors of an atom as `(other_index, bond_order)` pairs.
    pub fn neighbors(&self, atom: usize) -> Vec<(usize, BondType)> {
        self.bonds
            .iter()
            .filter_map(|bond| {
                if bond.a == atom {
                    Some((bond.b, bond.order))
                } else if bond.b == atom {
                    Some((bond.a, bond.order))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Sum of bond orders incident to an atom (Aromatic counts as 1.5).
    pub fn bond_order_sum(&self, atom: usize) -> f32 {
        self.neighbors(atom)
            .iter()
            .map(|(_, order)| bond_order_value(*order))
            .sum()
    }

    /// Hydrogen count for an atom: the pinned value if one was set (a SMILES
    /// bracket atom), otherwise derived from the charge-aware valence model.
    pub fn implicit_hydrogens(&self, atom: usize) -> u32 {
        let Some(data) = self.atoms.get(atom) else {
            return 0;
        };
        if let Some(pinned) = data.explicit_hydrogens {
            return pinned;
        }
        let sum = self.bond_order_sum(atom);
        implicit_hydrogens(&data.element, data.charge, sum)
    }

    /// Whether an atom carries more bonds than its element/charge allows. Atoms
    /// in aromatic rings are never flagged (aromatic valence is fuzzy and would
    /// give false positives for donors like the pyrrole nitrogen).
    pub fn atom_overvalent(&self, atom: usize) -> bool {
        let Some(data) = self.atoms.get(atom) else {
            return false;
        };
        let neighbors = self.neighbors(atom);
        if neighbors
            .iter()
            .any(|(_, order)| *order == BondType::Aromatic)
        {
            return false;
        }
        let sum: f32 = neighbors
            .iter()
            .map(|(_, order)| bond_order_value(*order))
            .sum();
        is_overvalent(&data.element, data.charge, sum)
    }

    /// Step an atom's formal charge by `delta`, clamped to a sane window.
    pub fn adjust_charge(&mut self, atom: usize, delta: i32) {
        if let Some(atom) = self.atoms.get_mut(atom) {
            atom.charge = (atom.charge + delta).clamp(-4, 4);
        }
    }

    pub fn centroid(&self) -> Point2<f32> {
        if self.atoms.is_empty() {
            return Point2::origin();
        }
        let sum = self
            .atoms
            .iter()
            .fold(Vector2::zeros(), |acc, atom| acc + atom.pos.coords);
        Point2::from(sum / self.atoms.len() as f32)
    }

    /// Axis-aligned bounds `(min, max)` of all atom positions, or `None` if empty.
    pub fn bounds(&self) -> Option<(Point2<f32>, Point2<f32>)> {
        let first = self.atoms.first()?.pos;
        let mut min = first;
        let mut max = first;
        for atom in &self.atoms {
            min.x = min.x.min(atom.pos.x);
            min.y = min.y.min(atom.pos.y);
            max.x = max.x.max(atom.pos.x);
            max.y = max.y.max(atom.pos.y);
        }
        Some((min, max))
    }

    pub fn translate(&mut self, delta: Vector2<f32>) {
        for atom in &mut self.atoms {
            atom.pos += delta;
        }
    }

    pub fn translate_atoms(&mut self, atoms: &[usize], delta: Vector2<f32>) {
        for &index in atoms {
            if let Some(atom) = self.atoms.get_mut(index) {
                atom.pos += delta;
            }
        }
    }

    pub fn rotate(&mut self, center: Point2<f32>, angle: f32) {
        let rotation = Rotation2::new(angle);
        for atom in &mut self.atoms {
            atom.pos = center + rotation * (atom.pos - center);
        }
    }

    pub fn rotate_atoms(&mut self, atoms: &[usize], center: Point2<f32>, angle: f32) {
        let rotation = Rotation2::new(angle);
        for &index in atoms {
            if let Some(atom) = self.atoms.get_mut(index) {
                atom.pos = center + rotation * (atom.pos - center);
            }
        }
    }

    pub fn flip_horizontal(&mut self) {
        let center = self.centroid();
        for atom in &mut self.atoms {
            atom.pos.x = 2.0 * center.x - atom.pos.x;
        }
    }

    pub fn flip_vertical(&mut self) {
        let center = self.centroid();
        for atom in &mut self.atoms {
            atom.pos.y = 2.0 * center.y - atom.pos.y;
        }
    }

    /// Nearest atom to `point` within `radius`, if any.
    pub fn nearest_atom(&self, point: Point2<f32>, radius: f32) -> Option<usize> {
        self.atoms
            .iter()
            .enumerate()
            .map(|(index, atom)| (index, (atom.pos - point).norm()))
            .filter(|(_, distance)| *distance <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index)
    }

    /// Nearest bond (by distance from its segment) to `point` within `radius`.
    pub fn nearest_bond(&self, point: Point2<f32>, radius: f32) -> Option<usize> {
        self.bonds
            .iter()
            .enumerate()
            .filter_map(|(index, bond)| {
                let a = self.atoms.get(bond.a)?.pos;
                let b = self.atoms.get(bond.b)?.pos;
                Some((index, point_segment_distance(point, a, b)))
            })
            .filter(|(_, distance)| *distance <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index)
    }

    /// A sensible outward direction for a new bond grown from `atom`, aiming for
    /// ~120° from its existing bonds so chains zig-zag instead of overlapping.
    /// Returns a default direction for an out-of-range index.
    pub fn grow_direction(&self, atom: usize) -> Vector2<f32> {
        let Some(center_atom) = self.atoms.get(atom) else {
            return Vector2::new(1.0, 0.0);
        };
        let center = center_atom.pos;
        let existing: Vec<Vector2<f32>> = self
            .neighbors(atom)
            .iter()
            .filter_map(|(other, _)| (self.atoms[*other].pos - center).try_normalize(1.0e-4))
            .collect();

        match existing.as_slice() {
            [] => Vector2::new(1.0, 0.0),
            [only] => {
                // 120° from the single existing bond. Pick whichever of the two
                // 120° options points farther from that neighbour's *own* other
                // substituents, producing a natural zig-zag.
                let away = -only;
                let up = Rotation2::new(60.0_f32.to_radians()) * away;
                let down = Rotation2::new(-60.0_f32.to_radians()) * away;
                let neighbor_index = self.neighbors(atom)[0].0;
                let reference: Vector2<f32> = self
                    .neighbors(neighbor_index)
                    .iter()
                    .filter(|(other, _)| *other != atom)
                    .filter_map(|(other, _)| {
                        (self.atoms[*other].pos - self.atoms[neighbor_index].pos)
                            .try_normalize(1.0e-4)
                    })
                    .fold(Vector2::zeros(), |acc, dir| acc + dir);
                if reference.norm() < 1.0e-4 || up.dot(&reference) <= down.dot(&reference) {
                    up
                } else {
                    down
                }
            }
            many => {
                let sum: Vector2<f32> = many.iter().sum();
                sum.try_normalize(1.0e-4)
                    .map(|dir| -dir)
                    .unwrap_or_else(|| {
                        // Opposed bonds (e.g. linear): go perpendicular instead.
                        Vector2::new(-many[0].y, many[0].x)
                    })
            }
        }
    }

    /// Position for a new atom grown off `atom` at the default bond length.
    /// Falls back to the origin for an out-of-range index.
    pub fn grow_position(&self, atom: usize) -> Point2<f32> {
        let base = self
            .atoms
            .get(atom)
            .map(|a| a.pos)
            .unwrap_or_else(Point2::origin);
        base + self.grow_direction(atom) * BOND_LENGTH
    }
}

fn vec_to_mask(indices: &[usize], len: usize) -> Vec<bool> {
    let mut mask = vec![false; len];
    for &index in indices {
        if index < len {
            mask[index] = true;
        }
    }
    mask
}

/// Distance from `point` to the segment `a`–`b`.
pub fn point_segment_distance(point: Point2<f32>, a: Point2<f32>, b: Point2<f32>) -> f32 {
    let ab = b - a;
    let length_squared = ab.norm_squared();
    if length_squared <= 1.0e-9 {
        return (point - a).norm();
    }
    let t = ((point - a).dot(&ab) / length_squared).clamp(0.0, 1.0);
    let projection = a + ab * t;
    (point - projection).norm()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carbon_chain() -> Sketch {
        let mut sketch = Sketch::new();
        let a = sketch.add_atom("C", Point2::new(0.0, 0.0));
        let b = sketch.add_atom("C", Point2::new(1.5, 0.0));
        sketch.add_bond(a, b, BondType::Single);
        sketch
    }

    #[test]
    fn add_bond_rejects_duplicates_and_self() {
        let mut sketch = carbon_chain();
        assert_eq!(sketch.add_bond(0, 1, BondType::Double), None); // duplicate
        assert_eq!(sketch.add_bond(0, 0, BondType::Single), None); // self
        assert_eq!(sketch.bonds.len(), 1);
    }

    #[test]
    fn cycle_bond_order_wraps() {
        let mut sketch = carbon_chain();
        sketch.cycle_bond_order(0);
        assert_eq!(sketch.bonds[0].order, BondType::Double);
        sketch.cycle_bond_order(0);
        assert_eq!(sketch.bonds[0].order, BondType::Triple);
        sketch.cycle_bond_order(0);
        assert_eq!(sketch.bonds[0].order, BondType::Single);
    }

    #[test]
    fn remove_atom_reindexes_bonds() {
        let mut sketch = Sketch::new();
        let a = sketch.add_atom("C", Point2::new(0.0, 0.0));
        let b = sketch.add_atom("C", Point2::new(1.5, 0.0));
        let c = sketch.add_atom("C", Point2::new(3.0, 0.0));
        sketch.add_bond(a, b, BondType::Single);
        sketch.add_bond(b, c, BondType::Single);
        sketch.remove_atom(a);
        assert_eq!(sketch.atoms.len(), 2);
        // The surviving B–C bond should now connect atoms 0 and 1.
        assert_eq!(sketch.bonds.len(), 1);
        assert_eq!(sketch.bonds[0].a, 0);
        assert_eq!(sketch.bonds[0].b, 1);
    }

    #[test]
    fn remove_atoms_batch_reindexes() {
        let mut sketch = Sketch::new();
        for i in 0..5 {
            sketch.add_atom("C", Point2::new(i as f32 * 1.5, 0.0));
        }
        for i in 0..4 {
            sketch.add_bond(i, i + 1, BondType::Single);
        }
        sketch.remove_atoms(&[1, 3]);
        assert_eq!(sketch.atoms.len(), 3);
        // Only the 2–? chain survives; bonds touching 1 or 3 are gone.
        assert!(sketch.bonds.iter().all(|b| b.a < 3 && b.b < 3));
    }

    #[test]
    fn grow_direction_spreads_from_existing_bond() {
        let sketch = carbon_chain();
        let direction = sketch.grow_direction(1);
        let existing = (sketch.atoms[0].pos - sketch.atoms[1].pos).normalize();
        let angle = direction
            .dot(&existing)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!((angle - 120.0).abs() < 1.0, "angle was {angle}");
    }

    #[test]
    fn point_segment_distance_is_perpendicular() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let distance = point_segment_distance(Point2::new(1.0, 1.0), a, b);
        assert!((distance - 1.0).abs() < 1.0e-5);
    }
}
