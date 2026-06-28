//! Phosphorylation: weld an idealized phosphate onto a Ser/Thr/Tyr hydroxyl
//! (O–P phosphoester) or a His imidazole nitrogen (N–P phosphoramidate).

use anyhow::Result;

use crate::domain::{BondType, ProteinAnchor, ResidueId, Structure};
use crate::engines::forcefield;

use super::attach::weld;
use super::{fragments, host};

/// Phosphorylate `residue` at `anchor` (Ser OG, Thr OG1, Tyr OH, or His ND1/NE2),
/// forming the anchorX–P bond to a neutral phosphate group.
pub fn phosphorylate_protein(
    protein: &Structure,
    residue: ResidueId,
    anchor: ProteinAnchor,
) -> Result<Structure> {
    let acceptor = host::resolve_acceptor(protein, residue, anchor)?;
    let anchor_element = protein.atoms[acceptor.anchor_atom].element.clone();
    let bond_length =
        forcefield::equilibrium_bond_length(&anchor_element, "P", BondType::Single).unwrap_or(1.6);
    let fragment = fragments::phosphate();
    weld(
        protein,
        acceptor,
        &fragment,
        bond_length,
        BondType::Single,
        "phospho",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::ptm::testkit::{self, sidechain_backbone, single_residue};

    fn ser() -> Structure {
        single_residue(
            "SER",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("CB", "C", [1.45, 1.5, 0.0]),
                ("OG", "O", [2.6, 2.4, 0.0]),
                ("HG", "H", [3.4, 2.0, 0.0]),
            ],
            &[(0, 1), (1, 2), (2, 3), (1, 4), (4, 5), (5, 6)],
            sidechain_backbone(),
        )
    }

    fn tyr() -> Structure {
        single_residue(
            "TYR",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("CB", "C", [1.45, 1.5, 0.0]),
                ("CZ", "C", [2.6, 2.4, 0.0]),
                ("OH", "O", [3.8, 3.2, 0.0]),
                ("HH", "H", [4.6, 2.9, 0.0]),
            ],
            &[(0, 1), (1, 2), (2, 3), (1, 4), (4, 5), (5, 6), (6, 7)],
            sidechain_backbone(),
        )
    }

    fn target() -> ResidueId {
        ResidueId::new('A', 1, ' ')
    }

    #[test]
    fn phosphorylates_serine_o_linked() {
        let protein = ser();
        let fragment = fragments::phosphate();
        let result = phosphorylate_protein(&protein, target(), ProteinAnchor::SerOg)
            .expect("phosphorylation");

        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + fragment.structure.atoms.len() - fragment.leaving.len()
        );
        assert!(testkit::junction(&result, "OG", "P"), "OG–P bond formed");
        assert!(
            testkit::residue_has_atom(&result, target(), "OG"),
            "OG intact"
        );
    }

    #[test]
    fn phosphorylates_tyrosine() {
        let protein = tyr();
        let result =
            phosphorylate_protein(&protein, target(), ProteinAnchor::TyrOh).expect("phospho-Tyr");
        assert!(testkit::junction(&result, "OH", "P"), "OH–P bond formed");
        assert!(
            testkit::residue_has_atom(&result, target(), "OH"),
            "OH intact"
        );
    }

    #[test]
    fn wrong_anchor_for_residue_errors() {
        let protein = ser();
        assert!(phosphorylate_protein(&protein, target(), ProteinAnchor::TyrOh).is_err());
    }
}
