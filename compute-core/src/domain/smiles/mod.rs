//! A compact, dependency-free SMILES parser ([`parse`]) and best-effort writer
//! ([`to_smiles`]).
//!
//! [`parse`] yields a [`crate::domain::sketch::Sketch`] with a force-directed 2D
//! depiction (atoms arrive without coordinates, so
//! [`crate::domain::sketch::seed_layout`] lays them out). [`to_smiles`] walks the
//! bond graph to emit a (non-canonical) SMILES string. Stereochemistry is not
//! modeled in v1.

mod parser;
mod writer;

pub use parser::parse;
pub use writer::to_smiles;

#[cfg(test)]
mod tests {
    use super::{parse, to_smiles};
    use crate::domain::{BondType, sketch::Sketch};

    fn heavy_atoms(sketch: &Sketch) -> usize {
        sketch.atoms.iter().filter(|a| a.element != "H").count()
    }

    #[test]
    fn parses_ethanol() {
        let sketch = parse("CCO").unwrap();
        assert_eq!(heavy_atoms(&sketch), 3);
        assert_eq!(sketch.bonds.len(), 2);
        assert_eq!(sketch.atoms[2].element, "O");
    }

    #[test]
    fn parses_benzene_as_aromatic_ring() {
        let sketch = parse("c1ccccc1").unwrap();
        assert_eq!(heavy_atoms(&sketch), 6);
        let aromatic = sketch
            .bonds
            .iter()
            .filter(|b| b.order == BondType::Aromatic)
            .count();
        assert_eq!(aromatic, 6);
    }

    #[test]
    fn parses_branches_and_double_bonds() {
        // acetone
        let sketch = parse("CC(=O)C").unwrap();
        assert_eq!(heavy_atoms(&sketch), 4);
        assert!(sketch.bonds.iter().any(|b| b.order == BondType::Double));
    }

    #[test]
    fn parses_charges_and_brackets() {
        // acetate: charge on the terminal O
        let sketch = parse("CC(=O)[O-]").unwrap();
        let charged = sketch.atoms.iter().filter(|a| a.charge != 0).count();
        assert_eq!(charged, 1);
        assert_eq!(
            sketch
                .atoms
                .iter()
                .find(|a| a.charge == -1)
                .unwrap()
                .element,
            "O"
        );
    }

    #[test]
    fn pyrrole_nitrogen_keeps_its_hydrogen() {
        // The explicit [nH] count is pinned, since the valence model would give
        // an aromatic N zero implicit H.
        let sketch = parse("c1cc[nH]c1").unwrap();
        assert_eq!(heavy_atoms(&sketch), 5);
        let nitrogen = sketch.atoms.iter().position(|a| a.element == "N").unwrap();
        assert_eq!(sketch.atoms[nitrogen].explicit_hydrogens, Some(1));
        assert_eq!(sketch.implicit_hydrogens(nitrogen), 1);
    }

    #[test]
    fn bracket_hydrogen_count_is_authoritative() {
        // A carbene [CH2] must keep exactly two hydrogens, not fill to four.
        let sketch = parse("[CH2]").unwrap();
        assert_eq!(sketch.atoms[0].explicit_hydrogens, Some(2));
        assert_eq!(sketch.implicit_hydrogens(0), 2);
        // A bare radical carbon [C] keeps zero.
        let bare = parse("[C]").unwrap();
        assert_eq!(bare.implicit_hydrogens(0), 0);
    }

    #[test]
    fn rejects_unbalanced_parens_and_bad_elements() {
        assert!(parse("CC(C").is_err());
        assert!(parse("CC)C").is_err());
        assert!(parse("C1CC").is_err()); // unclosed ring
        assert!(parse("[Xx]").is_err()); // not an element
        assert!(parse("[Q]").is_err());
    }

    #[test]
    fn round_trips_simple_molecules() {
        for smiles in ["CCO", "CC(=O)C", "c1ccccc1", "CCN", "C#N", "CC(=O)[O-]"] {
            let original = parse(smiles).unwrap();
            let emitted = to_smiles(&original);
            let reparsed = parse(&emitted).unwrap();
            assert_eq!(
                heavy_atoms(&original),
                heavy_atoms(&reparsed),
                "{smiles} -> {emitted}: heavy-atom count changed"
            );
            assert_eq!(
                original
                    .bonds
                    .iter()
                    .filter(|b| sketch_heavy(&original, b.a) && sketch_heavy(&original, b.b))
                    .count(),
                reparsed
                    .bonds
                    .iter()
                    .filter(|b| sketch_heavy(&reparsed, b.a) && sketch_heavy(&reparsed, b.b))
                    .count(),
                "{smiles} -> {emitted}: heavy bond count changed"
            );
        }
    }

    fn sketch_heavy(sketch: &Sketch, atom: usize) -> bool {
        sketch.atoms[atom].element != "H"
    }
}
