//! Lipidation and prenylation, sharing the Cys/Gly anchors. STRUCTURE-FIRST:
//! these build chemically accurate 3D only — MD/force-field parameters for the
//! lipid and prenyl groups are not provided.

use anyhow::Result;

use crate::domain::modification::{AcylKind, PrenylKind};
use crate::domain::{BondType, ProteinAnchor, ResidueId, Structure};
use crate::engines::forcefield;

use super::attach::weld;
use super::{fragments, host};

/// S-acylate a Cys SG (palmitoyl thioester) or N-acylate the chain N-terminus
/// (myristoyl amide), per `kind`.
pub fn acylate_protein(
    protein: &Structure,
    residue: ResidueId,
    kind: AcylKind,
) -> Result<Structure> {
    match kind {
        AcylKind::Palmitoyl => {
            let acceptor = host::resolve_acceptor(protein, residue, ProteinAnchor::CysSg)?;
            let bond_length =
                forcefield::equilibrium_bond_length("S", "C", BondType::Single).unwrap_or(1.81);
            let fragment = fragments::acyl(16);
            weld(
                protein,
                acceptor,
                &fragment,
                bond_length,
                BondType::Single,
                "palmitoyl",
            )
        }
        AcylKind::Myristoyl => {
            let acceptor = host::resolve_n_terminus(protein, residue)?;
            let bond_length =
                forcefield::equilibrium_bond_length("N", "C", BondType::Single).unwrap_or(1.34);
            let fragment = fragments::acyl(14);
            weld(
                protein,
                acceptor,
                &fragment,
                bond_length,
                BondType::Single,
                "myristoyl",
            )
        }
    }
}

/// S-prenylate a Cys SG with a farnesyl (C15) or geranylgeranyl (C20) thioether.
pub fn prenylate_protein(
    protein: &Structure,
    residue: ResidueId,
    kind: PrenylKind,
) -> Result<Structure> {
    let (units, suffix) = match kind {
        PrenylKind::Farnesyl => (3, "farnesyl"),
        PrenylKind::GeranylGeranyl => (4, "geranylgeranyl"),
    };
    let acceptor = host::resolve_acceptor(protein, residue, ProteinAnchor::CysSg)?;
    let bond_length =
        forcefield::equilibrium_bond_length("S", "C", BondType::Single).unwrap_or(1.81);
    let fragment = fragments::isoprenoid(units);
    weld(
        protein,
        acceptor,
        &fragment,
        bond_length,
        BondType::Single,
        suffix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::ptm::fragments::Fragment;
    use crate::workflows::ptm::testkit::{self, Backbone, sidechain_backbone, single_residue};

    fn cys() -> Structure {
        single_residue(
            "CYS",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("CB", "C", [1.45, 1.5, 0.0]),
                ("SG", "S", [2.7, 2.6, 0.0]),
                ("HG", "H", [3.6, 2.2, 0.0]),
            ],
            &[(0, 1), (1, 2), (2, 3), (1, 4), (4, 5), (5, 6)],
            sidechain_backbone(),
        )
    }

    fn gly() -> Structure {
        single_residue(
            "GLY",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("H", "H", [-0.6, 0.7, 0.2]),
            ],
            &[(0, 1), (1, 2), (2, 3), (0, 4)],
            Backbone {
                alpha: Some(1),
                nitrogen: Some(0),
                carbon: Some(2),
                oxygen: Some(3),
            },
        )
    }

    fn target() -> ResidueId {
        ResidueId::new('A', 1, ' ')
    }

    fn assert_count(protein: &Structure, result: &Structure, fragment: &Fragment) {
        assert_eq!(
            result.atoms.len(),
            protein.atoms.len() - 1 + fragment.structure.atoms.len() - fragment.leaving.len()
        );
    }

    #[test]
    fn palmitoylates_cysteine_thioester() {
        let protein = cys();
        let result = acylate_protein(&protein, target(), AcylKind::Palmitoyl).expect("palmitoyl");
        assert_count(&protein, &result, &fragments::acyl(16));
        assert!(
            testkit::junction(&result, "SG", "C1"),
            "SG–C1 thioester formed"
        );
        assert!(
            testkit::residue_has_atom(&result, target(), "SG"),
            "SG intact"
        );
    }

    #[test]
    fn myristoylates_n_terminus_amide() {
        let protein = gly();
        let result = acylate_protein(&protein, target(), AcylKind::Myristoyl).expect("myristoyl");
        assert_count(&protein, &result, &fragments::acyl(14));
        assert!(testkit::junction(&result, "N", "C1"), "N–C1 amide formed");
    }

    #[test]
    fn farnesylates_cysteine_thioether() {
        let protein = cys();
        let result = prenylate_protein(&protein, target(), PrenylKind::Farnesyl).expect("farnesyl");
        assert_count(&protein, &result, &fragments::isoprenoid(3));
        assert!(
            testkit::junction(&result, "SG", "C1"),
            "SG–C1 thioether formed"
        );
    }

    #[test]
    fn geranylgeranylates_cysteine() {
        let protein = cys();
        let result = prenylate_protein(&protein, target(), PrenylKind::GeranylGeranyl)
            .expect("geranylgeranyl");
        assert_count(&protein, &result, &fragments::isoprenoid(4));
        assert!(
            testkit::junction(&result, "SG", "C1"),
            "SG–C1 thioether formed"
        );
    }
}
