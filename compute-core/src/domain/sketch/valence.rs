//! Charge-aware valence model for the 2D sketcher.
//!
//! Two questions the canvas asks of every atom:
//!   * how many implicit hydrogens does it carry? (drives the `NH₂`/`OH` labels
//!     and the hydrogens filled at build time), and
//!   * is it over-bonded for its element and charge? (drives the warning
//!     underline).
//!
//! Implicit-H filling deliberately mirrors
//! [`crate::domain::chemistry::add_missing_hydrogens`]: it fills only *up to* the
//! element's preferred (lowest) valence and never invents hydrogens for an atom
//! that already meets it. That keeps the on-canvas count identical to what the
//! 3D build will actually add. The over-valence check uses a separate, more
//! permissive maximum so hypervalent sulfur/phosphorus are not falsely flagged.

use crate::domain::{BondType, chemistry::normalized_symbol};

/// Numeric bond order used for valence sums (Aromatic ≈ 1.5).
pub fn bond_order_value(order: BondType) -> f32 {
    match order {
        BondType::Single => 1.0,
        BondType::Double => 2.0,
        BondType::Triple => 3.0,
        BondType::Aromatic => 1.5,
    }
}

/// Outer-shell electron count of the neutral main-group atom, or `None` for
/// elements we do not model implicit hydrogens for (metals, noble gases, …).
fn group_valence_electrons(symbol: &str) -> Option<i32> {
    Some(match symbol {
        "H" => 1,
        "B" => 3,
        "C" | "Si" | "Ge" => 4,
        "N" | "P" | "As" => 5,
        "O" | "S" | "Se" | "Te" => 6,
        "F" | "Cl" | "Br" | "I" | "At" => 7,
        _ => return None,
    })
}

/// Octet-rule bond count for `effective` valence electrons (group electrons
/// minus formal charge): everything up to four electrons pairs into bonds, and
/// beyond four the octet caps the count at `8 - e`.
fn octet_bonds(effective_electrons: i32) -> i32 {
    if effective_electrons <= 4 {
        effective_electrons
    } else {
        8 - effective_electrons
    }
}

/// The element's preferred (lowest) valence given its formal charge. This is the
/// target implicit-H filling aims for.
fn preferred_valence(symbol: &str, charge: i32) -> Option<i32> {
    let group = group_valence_electrons(symbol)?;
    if symbol == "H" {
        return Some((group - charge).clamp(0, 1));
    }
    Some(octet_bonds(group - charge).max(0))
}

/// The largest valence the element tolerates for its charge — used only to
/// decide whether an atom is over-bonded. Hypervalent period-3+ elements expand.
fn max_valence(symbol: &str, charge: i32) -> Option<i32> {
    let octet = preferred_valence(symbol, charge)?;
    let hypervalent = match symbol {
        "P" | "As" => 5,
        "S" | "Se" | "Te" => 6,
        "Si" => 4,
        _ => octet,
    };
    Some(octet.max(hypervalent))
}

/// Implicit hydrogens on an atom of `element`/`charge` carrying `bond_order_sum`
/// worth of explicit bonds. Returns 0 for un-modeled elements and never goes
/// negative (an over-bonded atom simply gets no hydrogens).
pub fn implicit_hydrogens(element: &str, charge: i32, bond_order_sum: f32) -> u32 {
    let symbol = normalized_symbol(element);
    let Some(target) = preferred_valence(&symbol, charge) else {
        return 0;
    };
    // Match `add_missing_hydrogens`: already-saturated atoms (within a small
    // tolerance, so aromatic 1.5 sums behave) get nothing more.
    if bond_order_sum + 0.1 >= target as f32 {
        return 0;
    }
    (target as f32 - bond_order_sum).round().max(0.0) as u32
}

/// Whether `bond_order_sum` exceeds what the element/charge can support.
pub fn is_overvalent(element: &str, charge: i32, bond_order_sum: f32) -> bool {
    let symbol = normalized_symbol(element);
    match max_valence(&symbol, charge) {
        Some(max) => bond_order_sum > max as f32 + 0.1,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_organic_valences() {
        assert_eq!(implicit_hydrogens("C", 0, 0.0), 4); // methane
        assert_eq!(implicit_hydrogens("C", 0, 1.0), 3); // methyl
        assert_eq!(implicit_hydrogens("N", 0, 0.0), 3); // ammonia
        assert_eq!(implicit_hydrogens("O", 0, 1.0), 1); // hydroxyl
        assert_eq!(implicit_hydrogens("O", 0, 2.0), 0); // ether
        assert_eq!(implicit_hydrogens("F", 0, 1.0), 0); // fluoride substituent
        assert_eq!(implicit_hydrogens("S", 0, 1.0), 1); // thiol
        assert_eq!(implicit_hydrogens("P", 0, 0.0), 3); // phosphine
    }

    #[test]
    fn charge_shifts_capacity() {
        assert_eq!(implicit_hydrogens("N", 1, 0.0), 4); // ammonium
        assert_eq!(implicit_hydrogens("N", -1, 1.0), 1); // amide anion R-NH⁻
        assert_eq!(implicit_hydrogens("O", -1, 1.0), 0); // alkoxide R-O⁻
        assert_eq!(implicit_hydrogens("O", -1, 0.0), 1); // hydroxide
        assert_eq!(implicit_hydrogens("C", 1, 0.0), 3); // carbocation CH₃⁺
        assert_eq!(implicit_hydrogens("C", -1, 0.0), 3); // carbanion CH₃⁻
        assert_eq!(implicit_hydrogens("B", -1, 0.0), 4); // borohydride BH₄⁻
    }

    #[test]
    fn aromatic_carbon_and_heteroatoms() {
        // Two aromatic bonds → sum 3.0.
        assert_eq!(implicit_hydrogens("C", 0, 3.0), 1); // benzene CH
        assert_eq!(implicit_hydrogens("N", 0, 3.0), 0); // pyridine N
        assert_eq!(implicit_hydrogens("O", 0, 3.0), 0); // furan O
        assert_eq!(implicit_hydrogens("S", 0, 3.0), 0); // thiophene S
    }

    #[test]
    fn hypervalent_sulfur_phosphorus_not_flagged() {
        assert!(!is_overvalent("S", 0, 6.0)); // sulfate-like
        assert!(!is_overvalent("S", 0, 4.0)); // sulfoxide-like
        assert!(!is_overvalent("P", 0, 5.0)); // phosphate-like
        assert_eq!(implicit_hydrogens("S", 0, 6.0), 0);
        assert_eq!(implicit_hydrogens("P", 0, 5.0), 0);
    }

    #[test]
    fn over_bonding_is_flagged() {
        assert!(is_overvalent("C", 0, 5.0)); // 5 bonds on carbon
        assert!(is_overvalent("N", 0, 4.0)); // 4 bonds on neutral N
        assert!(!is_overvalent("N", 1, 4.0)); // but fine on N⁺
        assert!(is_overvalent("O", 0, 3.0)); // 3 bonds on neutral O
        assert!(!is_overvalent("O", 1, 3.0)); // fine on O⁺
    }

    #[test]
    fn unknown_elements_are_inert() {
        assert_eq!(implicit_hydrogens("Fe", 0, 2.0), 0);
        assert!(!is_overvalent("Fe", 0, 8.0));
    }
}
