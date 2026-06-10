use anyhow::Result;

use crate::domain::Structure;

pub fn to_xyz(structure: &Structure) -> String {
    let mut output = format!("{}\n{}\n", structure.atoms.len(), structure.title);

    for atom in &structure.atoms {
        output.push_str(&format!(
            "{:<2} {:>12.6} {:>12.6} {:>12.6}\n",
            atom.element, atom.position.x, atom.position.y, atom.position.z
        ));
    }

    output
}

pub fn to_cif(structure: &Structure) -> Result<String> {
    super::formats::cif::to_cif(structure)
}

pub fn to_pdb(structure: &Structure) -> Result<String> {
    super::formats::pdb::to_pdb(structure)
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::{to_cif, to_pdb, to_xyz};
    use crate::{
        domain::{Atom, Bond, BondType, Structure, UnitCell},
        io::formats::{cif::parse_cif, gro::parse_gro, pdb::parse_pdb, xyz::parse_xyz},
    };

    #[test]
    fn writes_xyz() {
        let structure = parse_xyz(
            "\
1
helium
He 1 2 3
",
        )
        .expect("xyz");

        assert!(to_xyz(&structure).contains("He     1.000000"));
    }

    #[test]
    fn writes_cif_with_fractional_positions() {
        let structure = parse_cif(
            "\
data_test
_cell_length_a 10
_cell_length_b 10
_cell_length_c 10
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_
_atom_site_type_symbol
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
C 0.25 0.5 0.75
",
        )
        .expect("cif");

        let output = to_cif(&structure).expect("serialized cif");
        let roundtrip = parse_cif(&output).expect("roundtrip cif");
        let cell = roundtrip.cell.as_ref().expect("roundtrip cell");

        assert!(output.contains("_cell.length_a"));
        assert!(output.contains("_atom_site.Cartn_x"));
        assert!((cell.a - 10.0).abs() < 0.0001);
        assert!((roundtrip.atoms[0].position.x - 2.5).abs() < 0.0001);
        assert!((roundtrip.atoms[0].position.y - 5.0).abs() < 0.0001);
        assert!((roundtrip.atoms[0].position.z - 7.5).abs() < 0.0001);
    }

    #[test]
    fn gro_imports_periodic_structure() {
        let structure = parse_gro(
            "\
generated
    1
    1MOL     C1    1   0.100   0.200   0.300
   1.00000   2.00000   3.00000
",
        )
        .expect("gro");

        let output = to_cif(&structure).expect("serialized cif");
        let roundtrip = parse_cif(&output).expect("roundtrip cif");
        let cell = roundtrip.cell.as_ref().expect("roundtrip cell");

        assert!(output.contains("_cell.length_a"));
        assert!((cell.a - 10.0).abs() < 0.0001);
        assert!((cell.b - 20.0).abs() < 0.0001);
        assert!((cell.c - 30.0).abs() < 0.0001);
    }

    #[test]
    fn writes_pdb_with_cell_atoms_and_bonds() {
        let structure = Structure::with_cell_and_bonds(
            "ethene",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(1.34, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Double)],
            UnitCell::from_parameters(10.0, 11.0, 12.0, 90.0, 91.0, 92.0),
        );

        let output = to_pdb(&structure).expect("serialized pdb");
        let roundtrip = parse_pdb(&output).expect("roundtrip pdb");
        let cell = roundtrip.cell.as_ref().expect("roundtrip cell");

        assert!(output.contains("TITLE     ethene"));
        assert!(output.contains("CRYST1"));
        assert!(output.contains("ATOM      1"));
        assert!(output.contains("ATOM      2"));
        assert!(output.contains("CONECT    1    2    2"));
        assert!(output.contains("CONECT    2    1    1"));
        assert!(output.ends_with("END\n"));
        assert!((cell.a - 10.0).abs() < 0.0001);
        assert!((cell.b - 11.0).abs() < 0.0001);
        assert!((cell.c - 12.0).abs() < 0.0001);
        assert_eq!(roundtrip.bonds.len(), 1);
        assert_eq!(roundtrip.bonds[0].bond_type, BondType::Double);
    }

    #[test]
    fn pdb_write_preserves_residue_identity_from_biopolymer() {
        let source = "\
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  N   ALA A   2       2.900   0.000   0.000  1.00  0.00           N
END
";
        let structure = parse_pdb(source).expect("parsed");
        assert!(
            structure.biopolymer.is_some(),
            "fixture should be a biopolymer"
        );

        let output = to_pdb(&structure).expect("serialized pdb");
        assert!(
            output.contains("GLY A   1"),
            "missing GLY residue: {output}"
        );
        assert!(
            output.contains("ALA A   2"),
            "missing ALA residue: {output}"
        );
        assert!(
            !output.contains("MOL"),
            "should not flatten to MOL: {output}"
        );

        let reparsed = parse_pdb(&output).expect("roundtrip");
        let bio = reparsed.biopolymer.expect("roundtrip biopolymer");
        let names: Vec<&str> = bio
            .residues
            .iter()
            .map(|r| r.residue_name.as_str())
            .collect();
        assert_eq!(names, vec!["GLY", "ALA"]);
    }

    #[test]
    fn pdb_write_falls_back_to_mol_for_plain_molecules() {
        let structure = Structure::with_bonds(
            "methane",
            vec![Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            Vec::new(),
        );
        let output = to_pdb(&structure).expect("serialized");
        assert!(
            output.contains("MOL"),
            "plain molecule should use MOL: {output}"
        );
    }

    #[test]
    fn pdb_roundtrip_preserves_double_bond_via_conect_multiplicity() {
        let structure = Structure::with_bonds(
            "ethene",
            vec![
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(0.0, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "C".to_string(),
                    position: Point3::new(1.34, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            vec![Bond::with_type(0, 1, BondType::Double)],
        );

        let output = to_pdb(&structure).expect("serialized pdb");
        let roundtrip = parse_pdb(&output).expect("roundtrip pdb");

        assert_eq!(roundtrip.bonds.len(), 1);
        assert_eq!(roundtrip.bonds[0].bond_type, BondType::Double);
    }
}
