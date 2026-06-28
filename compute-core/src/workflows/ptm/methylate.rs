//! Methylation: weld one, two, or three methyl groups onto a Lys NZ or an Arg
//! NH1/NH2, displacing one N–H per methyl. Each degree re-resolves the anchor on
//! the growing structure so successive methyls take the remaining hydrogens.

use anyhow::Result;

use crate::domain::modification::MethylDegree;
use crate::domain::{BondType, ProteinAnchor, ResidueId, Structure};
use crate::engines::forcefield;

use super::attach::weld;
use super::{fragments, host};

/// Methylate `residue` at `anchor` to `degree` (mono/di/tri), forming N–CH3 bonds.
pub fn methylate_protein(
    protein: &Structure,
    residue: ResidueId,
    anchor: ProteinAnchor,
    degree: MethylDegree,
) -> Result<Structure> {
    let count = match degree {
        MethylDegree::Mono => 1,
        MethylDegree::Di => 2,
        MethylDegree::Tri => 3,
    };
    let bond_length =
        forcefield::equilibrium_bond_length("N", "C", BondType::Single).unwrap_or(1.47);

    let mut current = protein.clone();
    for _ in 0..count {
        let acceptor = host::resolve_acceptor(&current, residue.clone(), anchor)?;
        let fragment = fragments::methyl();
        current = weld(
            &current,
            acceptor,
            &fragment,
            bond_length,
            BondType::Single,
            "methyl",
        )?;
    }
    Ok(current)
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
            ],
            sidechain_backbone(),
        )
    }

    fn arg() -> Structure {
        single_residue(
            "ARG",
            &[
                ("N", "N", [0.0, 0.0, 0.0]),
                ("CA", "C", [1.45, 0.0, 0.0]),
                ("C", "C", [2.9, 0.0, 0.0]),
                ("O", "O", [3.6, 1.0, 0.0]),
                ("CB", "C", [1.45, 1.5, 0.0]),
                ("CG", "C", [2.9, 2.0, 0.0]),
                ("CD", "C", [3.5, 3.3, 0.0]),
                ("NE", "N", [4.9, 3.5, 0.0]),
                ("CZ", "C", [5.6, 4.6, 0.0]),
                ("NH1", "N", [6.9, 4.6, 0.0]),
                ("NH2", "N", [5.0, 5.8, 0.0]),
                ("HH11", "H", [7.4, 3.8, 0.0]),
                ("HH12", "H", [7.4, 5.4, 0.0]),
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
                (9, 11),
                (9, 12),
            ],
            sidechain_backbone(),
        )
    }

    fn target() -> ResidueId {
        ResidueId::new('A', 1, ' ')
    }

    fn methyl_bonds(structure: &Structure, nitrogen: &str) -> usize {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure
            .bonds
            .iter()
            .filter(|bond| {
                let (a, b) = (bio.atom_name(bond.a), bio.atom_name(bond.b));
                (a == Some(nitrogen) && b == Some("C")) || (a == Some("C") && b == Some(nitrogen))
            })
            .count()
    }

    #[test]
    fn trimethylates_lysine() {
        let protein = lys();
        let result = methylate_protein(&protein, target(), ProteinAnchor::LysNz, MethylDegree::Tri)
            .expect("trimethylation");

        let fragment = fragments::methyl();
        let per_step = fragment.structure.atoms.len() - fragment.leaving.len() - 1;
        assert_eq!(result.atoms.len(), protein.atoms.len() + 3 * per_step);
        assert_eq!(methyl_bonds(&result, "NZ"), 3, "three NZ–CH3 bonds");
        assert!(
            testkit::residue_has_atom(&result, target(), "NZ"),
            "NZ intact"
        );
    }

    #[test]
    fn dimethylates_arginine() {
        let protein = arg();
        let result = methylate_protein(&protein, target(), ProteinAnchor::ArgNh1, MethylDegree::Di)
            .expect("dimethylation");
        assert_eq!(methyl_bonds(&result, "NH1"), 2, "two NH1–CH3 bonds");
    }

    #[test]
    fn monomethylates_lysine() {
        let protein = lys();
        let result =
            methylate_protein(&protein, target(), ProteinAnchor::LysNz, MethylDegree::Mono)
                .expect("monomethylation");
        assert_eq!(methyl_bonds(&result, "NZ"), 1, "one NZ–CH3 bond");
    }
}
