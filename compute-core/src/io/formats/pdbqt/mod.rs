//! PDBQT reading, writing, and docking preparation.
//!
//! PDBQT is the input/output format of AutoDock Vina (and the `docking` crate).
//! Compared to PDB it adds a partial-charge column and an AutoDock atom-type
//! column, and ligands carry a `ROOT`/`BRANCH`/`TORSDOF` torsion tree. silicolab
//! reads PDBQT (e.g. docking result poses) into [`Structure`]s and prepares
//! receptor/ligand PDBQT from structures for the docking engine.
//!
//! Preparation is best-effort: it derives AutoDock types from the element and bond
//! graph and a torsion tree from the rotatable bonds. Vina scoring ignores partial
//! charges, so the only chemistry that affects a result is the atom typing.

mod parse;
mod prep;
mod typing;

pub use parse::{parse_pdbqt, parse_pdbqt_document};
pub use prep::{PreparedPdbqt, prepare_ligand_pdbqt, prepare_receptor_pdbqt, to_pdbqt};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, Bond, BondType, Structure};
    use nalgebra::Point3;

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    /// A linear butane skeleton C1-C2-C3-C4 (heavy atoms only). The central C2-C3
    /// bond is the single rotatable bond.
    fn butane() -> Structure {
        let atoms = vec![
            atom("C", 0.0, 0.0, 0.0),
            atom("C", 1.5, 0.0, 0.0),
            atom("C", 2.2, 1.3, 0.0),
            atom("C", 3.7, 1.3, 0.0),
        ];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(2, 3, BondType::Single),
        ];
        Structure::with_bonds("butane", atoms, bonds)
    }

    #[test]
    fn ligand_has_one_rotatable_bond_and_parses_in_the_crate() {
        let prepared = prepare_ligand_pdbqt(&butane()).expect("prepare");
        assert_eq!(
            prepared.torsions, 1,
            "central C-C is the one rotatable bond"
        );
        assert!(prepared.text.contains("ROOT"));
        assert!(prepared.text.contains("BRANCH"));
        assert!(prepared.text.contains("TORSDOF 1"));

        // The crate's own parser must accept what we emit.
        let model = docking::pdbqt::parse_ligand_pdbqt_from_string(
            &prepared.text,
            docking::atom::AtomTyping::Xs,
        )
        .expect("crate parses our ligand PDBQT");
        assert_eq!(model.num_movable_atoms(), 4);
    }

    #[test]
    fn rigid_ring_has_no_torsions() {
        // A 3-membered ring (cyclopropane skeleton): every bond is in the ring, so
        // none are rotatable.
        let atoms = vec![
            atom("C", 0.0, 0.0, 0.0),
            atom("C", 1.5, 0.0, 0.0),
            atom("C", 0.75, 1.3, 0.0),
        ];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(2, 0, BondType::Single),
        ];
        let prepared =
            prepare_ligand_pdbqt(&Structure::with_bonds("ring", atoms, bonds)).expect("prepare");
        assert_eq!(prepared.torsions, 0);
        assert!(prepared.text.contains("TORSDOF 0"));
    }

    #[test]
    fn polar_hydrogen_is_kept_nonpolar_is_merged() {
        // Methanol: C-O-H plus three non-polar H on carbon. The O-H hydrogen is
        // polar (kept as HD); the C-H hydrogens are dropped.
        let atoms = vec![
            atom("C", 0.0, 0.0, 0.0),
            atom("O", 1.4, 0.0, 0.0),
            atom("H", 1.7, 0.9, 0.0),  // polar, on O
            atom("H", -0.4, 1.0, 0.0), // non-polar, on C
            atom("H", -0.4, -0.5, 0.9),
            atom("H", -0.4, -0.5, -0.9),
        ];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(0, 3, BondType::Single),
            Bond::with_type(0, 4, BondType::Single),
            Bond::with_type(0, 5, BondType::Single),
        ];
        let prepared = prepare_receptor_pdbqt(&Structure::with_bonds("methanol", atoms, bonds))
            .expect("prepare");
        let hd_lines = prepared
            .text
            .lines()
            .filter(|l| l.trim_end().ends_with("HD"))
            .count();
        let oa_lines = prepared
            .text
            .lines()
            .filter(|l| l.trim_end().ends_with("OA"))
            .count();
        assert_eq!(hd_lines, 1, "exactly the O-H hydrogen survives as HD");
        assert_eq!(oa_lines, 1, "oxygen typed as acceptor OA");
        // C, O, one HD = 3 atom records (three non-polar H dropped).
        assert_eq!(
            prepared
                .text
                .lines()
                .filter(|l| l.starts_with("ATOM"))
                .count(),
            3
        );
    }

    #[test]
    fn receptor_round_trips_through_the_crate() {
        let prepared = prepare_receptor_pdbqt(&butane()).expect("prepare");
        let model = docking::pdbqt::parse_receptor_pdbqt_from_string(
            &prepared.text,
            docking::atom::AtomTyping::Xs,
        )
        .expect("crate parses our receptor PDBQT");
        assert_eq!(model.grid_atoms.len(), 4);
    }

    #[test]
    fn unsupported_element_errors() {
        let atoms = vec![atom("B", 0.0, 0.0, 0.0)];
        let err = to_pdbqt(&Structure::with_bonds("boron", atoms, Vec::new()))
            .expect_err("boron is unsupported");
        assert!(err.to_string().contains("not supported"), "got: {err}");
    }

    #[test]
    fn parse_document_splits_models() {
        let text = "\
MODEL 1
REMARK VINA RESULT:    -9.2      0.000      0.000
ATOM      1  C   LIG     1       0.000   0.000   0.000  1.00  0.00     0.000 C
ENDMDL
MODEL 2
REMARK VINA RESULT:    -8.7      1.000      2.000
ATOM      1  C   LIG     1       1.000   0.000   0.000  1.00  0.00     0.000 C
ENDMDL
";
        let structures = parse_pdbqt_document(text).expect("parse");
        assert_eq!(structures.len(), 2);
        assert_eq!(structures[0].atoms.len(), 1);
        assert_eq!(structures[0].atoms[0].element, "C");
        assert!(structures[0].title.contains("-9.2"));
    }

    #[test]
    fn score_only_runs_end_to_end() {
        // The whole pipeline: prepare both inputs and feed them to the real engine.
        // score_only is a single-point evaluation (no search), so it is cheap.
        let receptor = prepare_receptor_pdbqt(&butane()).expect("receptor");
        let ligand = prepare_ligand_pdbqt(&butane()).expect("ligand");
        let breakdown = docking::api::score_only(
            &receptor.text,
            &ligand.text,
            [1.8, 0.6, 0.0],
            [20.0, 20.0, 20.0],
        )
        .expect("score_only");
        assert!(breakdown.estimated_free_energy.is_finite());
    }
}
