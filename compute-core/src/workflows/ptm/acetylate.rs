//! Acetylation: weld an acetyl group as an amide, either onto a Lys side-chain
//! NZ or onto the protein N-terminus (the terminal backbone N located by chain
//! position).

use anyhow::Result;

use crate::domain::{BondType, ProteinAnchor, ResidueId, Structure};
use crate::engines::forcefield;

use super::attach::weld;
use super::{fragments, host};

/// Acetylate `residue`: when `n_terminal`, cap the N-terminus of the residue's
/// chain; otherwise form the Lys NZ–C(=O)CH3 amide. Either way an N–C bond joins
/// the anchor nitrogen to the acetyl carbonyl carbon.
pub fn acetylate_protein(
    protein: &Structure,
    residue: ResidueId,
    n_terminal: bool,
) -> Result<Structure> {
    let acceptor = if n_terminal {
        host::resolve_n_terminus(protein, residue)?
    } else {
        host::resolve_acceptor(protein, residue, ProteinAnchor::LysNz)?
    };
    let bond_length =
        forcefield::equilibrium_bond_length("N", "C", BondType::Single).unwrap_or(1.34);
    let fragment = fragments::acetyl();
    weld(
        protein,
        acceptor,
        &fragment,
        bond_length,
        BondType::Single,
        "acetyl",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::ptm::testkit::{self, sidechain_backbone, single_residue};

    fn lys() -> Structure {
        single_residue(
            "LYS",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("CB", "C", [1.45, 1.5, 0.0]),
                ("CG", "C", [2.9, 2.0, 0.0]),
                ("CD", "C", [3.5, 3.3, 0.0]),
                ("CE", "C", [4.9, 3.5, 0.0]),
                ("NZ", "N", [5.5, 4.8, 0.0]),
                ("HZ1", "H", [6.5, 4.8, 0.0]),
                ("HZ2", "H", [5.0, 5.6, 0.0]),
                ("HZ3", "H", [5.5, 4.0, 0.8]),
                ("H", "H", [-0.6, 0.7, 0.2]),
            ],
            &[
                (0, 1),
                (1, 2),
                (2, 3),
                (1, 4),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 8),
                (8, 9),
                (8, 10),
                (8, 11),
                (0, 12),
            ],
            sidechain_backbone(),
        )
    }

    fn target() -> ResidueId {
        ResidueId::new('A', 1, ' ')
    }

    fn product_count(protein: &Structure, result: &Structure) {
        let fragment = fragments::acetyl();
        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + fragment.structure.atoms.len() - fragment.leaving.len()
        );
    }

    #[test]
    fn acetylates_lysine_side_chain() {
        let protein = lys();
        let result = acetylate_protein(&protein, target(), false).expect("acetylation");
        product_count(&protein, &result);
        assert!(testkit::junction(&result, "NZ", "C"), "NZ–C amide formed");
        assert!(
            testkit::residue_has_atom(&result, target(), "NZ"),
            "NZ intact"
        );
    }

    #[test]
    fn acetylates_n_terminus() {
        let protein = lys();
        let result = acetylate_protein(&protein, target(), true).expect("N-terminal acetylation");
        product_count(&protein, &result);
        assert!(testkit::junction(&result, "N", "C"), "N–C amide formed");
    }
}
