//! Ring templates for the fragments palette.
//!
//! Each template is a regular carbon polygon with side length [`BOND_LENGTH`],
//! centered at the origin and built as a standalone fragment (atoms + bonds).
//! The frontend transforms a copy of it to follow the cursor and fuse it onto
//! the existing drawing.

use nalgebra::Point2;

use super::BOND_LENGTH;
use crate::domain::BondType;

/// A ring fragment's geometry: vertex positions and edges (`a`, `b`, order).
pub type RingGeometry = (Vec<Point2<f32>>, Vec<(usize, usize, BondType)>);

/// A ring fragment the user can stamp onto the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingTemplate {
    Cyclopropane,
    Cyclobutane,
    Cyclopentane,
    Cyclohexane,
    Cycloheptane,
    Cyclooctane,
    Benzene,
    Cyclopentadiene,
}

impl RingTemplate {
    pub fn all() -> &'static [RingTemplate] {
        &[
            RingTemplate::Benzene,
            RingTemplate::Cyclohexane,
            RingTemplate::Cyclopentane,
            RingTemplate::Cyclopentadiene,
            RingTemplate::Cyclopropane,
            RingTemplate::Cyclobutane,
            RingTemplate::Cycloheptane,
            RingTemplate::Cyclooctane,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            RingTemplate::Cyclopropane => "Cyclopropane",
            RingTemplate::Cyclobutane => "Cyclobutane",
            RingTemplate::Cyclopentane => "Cyclopentane",
            RingTemplate::Cyclohexane => "Cyclohexane",
            RingTemplate::Cycloheptane => "Cycloheptane",
            RingTemplate::Cyclooctane => "Cyclooctane",
            RingTemplate::Benzene => "Benzene",
            RingTemplate::Cyclopentadiene => "Cyclopentadiene",
        }
    }

    /// Short label for a compact palette button.
    pub fn short_label(self) -> &'static str {
        match self {
            RingTemplate::Cyclopropane => "C3",
            RingTemplate::Cyclobutane => "C4",
            RingTemplate::Cyclopentane => "C5",
            RingTemplate::Cyclohexane => "C6",
            RingTemplate::Cycloheptane => "C7",
            RingTemplate::Cyclooctane => "C8",
            RingTemplate::Benzene => "Ph",
            RingTemplate::Cyclopentadiene => "Cp",
        }
    }

    pub fn size(self) -> usize {
        match self {
            RingTemplate::Cyclopropane => 3,
            RingTemplate::Cyclobutane => 4,
            RingTemplate::Cyclopentane | RingTemplate::Cyclopentadiene => 5,
            RingTemplate::Cyclohexane | RingTemplate::Benzene => 6,
            RingTemplate::Cycloheptane => 7,
            RingTemplate::Cyclooctane => 8,
        }
    }

    /// Per-edge bond orders for the ring, edge `i` connecting vertex `i` to
    /// `(i + 1) % n`.
    fn edge_orders(self) -> Vec<BondType> {
        let n = self.size();
        match self {
            RingTemplate::Benzene => vec![BondType::Aromatic; n],
            RingTemplate::Cyclopentadiene => {
                // cyclopenta-1,3-diene: double bonds on edges 0 and 2.
                let mut orders = vec![BondType::Single; n];
                orders[0] = BondType::Double;
                orders[2] = BondType::Double;
                orders
            }
            _ => vec![BondType::Single; n],
        }
    }

    /// Build the fragment as `(positions, bonds)`. Positions form a regular
    /// polygon of side [`BOND_LENGTH`] centered at the origin; bonds use indices
    /// into the returned position list.
    pub fn build(self) -> RingGeometry {
        let n = self.size();
        // Circumradius so the polygon side equals BOND_LENGTH.
        let radius = BOND_LENGTH / (2.0 * (std::f32::consts::PI / n as f32).sin());
        // Orient with a flat bottom edge (the common depiction).
        let base = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI / n as f32;
        let positions: Vec<Point2<f32>> = (0..n)
            .map(|i| {
                let angle = base + i as f32 * std::f32::consts::TAU / n as f32;
                Point2::new(radius * angle.cos(), radius * angle.sin())
            })
            .collect();

        let orders = self.edge_orders();
        let bonds = (0..n)
            .map(|i| (i, (i + 1) % n, orders[i]))
            .collect::<Vec<_>>();

        (positions, bonds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_sizes_match() {
        for template in RingTemplate::all() {
            let (positions, bonds) = template.build();
            assert_eq!(positions.len(), template.size());
            assert_eq!(bonds.len(), template.size());
        }
    }

    #[test]
    fn polygon_side_is_bond_length() {
        let (positions, _) = RingTemplate::Cyclohexane.build();
        let side = (positions[1] - positions[0]).norm();
        assert!((side - BOND_LENGTH).abs() < 1.0e-4, "side was {side}");
    }

    #[test]
    fn benzene_is_all_aromatic() {
        let (_, bonds) = RingTemplate::Benzene.build();
        assert!(
            bonds
                .iter()
                .all(|(_, _, order)| *order == BondType::Aromatic)
        );
    }

    #[test]
    fn cyclopentadiene_has_two_double_bonds() {
        let (_, bonds) = RingTemplate::Cyclopentadiene.build();
        let doubles = bonds
            .iter()
            .filter(|(_, _, order)| *order == BondType::Double)
            .count();
        assert_eq!(doubles, 2);
    }
}
