use super::{parse_pdb, parse_pdb_document, to_pdb};
use crate::domain::BondType;

#[test]
fn splits_nmr_models_into_separate_structures() {
    let document = parse_pdb_document(
        "\
HEADER    ANTIMICROBIAL PROTEIN                   24-JUN-18   6A5J
TITLE     SOLUTION NMR STRUCTURE OF SMALL PEPTIDE
NUMMDL    2
MODEL        1
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ENDMDL
MODEL        2
ATOM      1  N   GLY A   1       0.100   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.550   0.000   0.000  1.00  0.00           C
ENDMDL
END
",
    )
    .expect("valid multi-model pdb");

    assert_eq!(document.pdb_id.as_deref(), Some("6A5J"));
    assert_eq!(document.title, "SOLUTION NMR STRUCTURE OF SMALL PEPTIDE");
    assert_eq!(document.models.len(), 2);
    assert_eq!(document.models[0].atoms.len(), 2);
    assert_eq!(document.models[1].atoms.len(), 2);
    // The two conformers differ in coordinates.
    assert!((document.models[0].atoms[0].position.x - 0.0).abs() < 0.0001);
    assert!((document.models[1].atoms[0].position.x - 0.1).abs() < 0.0001);
}

#[test]
fn single_model_file_yields_one_structure() {
    let document = parse_pdb_document(
        "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
END
",
    )
    .expect("valid pdb");

    assert_eq!(document.models.len(), 1);
    assert_eq!(document.pdb_id, None);
    // parse_pdb returns that single model.
    let structure = parse_pdb(
        "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
END
",
    )
    .expect("valid pdb");
    assert_eq!(structure.atoms.len(), 2);
}

#[test]
fn parses_cryst1_atoms_and_conect_records() {
    let structure = parse_pdb(
        "\
TITLE     UNK
CRYST1    7.301    7.301    3.400  90.00  90.00  60.00 P 1           1
ATOM      1  C01  UNK    1       7.302   1.374   1.700  1.00  0.00           C
ATOM      2  C02  UNK    1       4.840   5.637   1.700  1.00  0.00           C
ATOM      3  C03  UNK    1       9.763   5.637   1.700  1.00  0.00           C
ATOM      4  C04  UNK    1       7.302   2.843   1.700  1.00  0.00           C
ATOM      5  C05  UNK    1       6.112   4.903   1.700  1.00  0.00           C
ATOM      6  C06  UNK    1       8.491   4.903   1.700  1.00  0.00           C
ATOM      7  N07  UNK    1       1.173   0.678   1.700  1.00  0.00           N
ATOM      8  N08  UNK    1       3.651   4.969   1.700  1.00  0.00           N
ATOM      9  N09  UNK    1       6.129   0.678   1.700  1.00  0.00           N
ATOM     10  N0A  UNK    1       6.129   3.539   1.700  1.00  0.00           N
ATOM     11  N0B  UNK    1       7.302   5.570   1.700  1.00  0.00           N
ATOM     12  N0C  UNK    1       8.475   3.539   1.700  1.00  0.00           N
TER
CONECT    1    9    4    7
CONECT    4    1   10   12
CONECT    7    1    2
CONECT    9    1    3
CONECT    2    8    5    7
CONECT    5   11    2   10
CONECT    8    2    3
CONECT    3    8    9    6
CONECT    6   11    3   12
CONECT   10    4    5
CONECT   12    4    6
CONECT   11    5    6
END
",
    )
    .expect("valid pdb");

    let cell = structure.cell.as_ref().expect("periodic cell");

    assert_eq!(structure.title, "UNK");
    assert_eq!(structure.atoms.len(), 12);
    assert_eq!(structure.bonds.len(), 15);
    assert_eq!(structure.atoms[0].element, "C");
    assert_eq!(structure.atoms[6].element, "N");
    assert!((cell.a - 7.301).abs() < 0.0001);
    assert!((cell.gamma - 60.0).abs() < 0.0001);
    assert!((structure.atoms[0].position.x - 7.302).abs() < 0.0001);
}

#[test]
fn ignores_dummy_cryst1_placeholder_cells() {
    // A `CRYST1 1 1 1 90 90 90` placeholder is what modeling tools write for a
    // non-periodic molecule. It must be ignored for both storage *and* bond
    // inference: under the minimum-image convention a 1 Å cell places every
    // atom within bonding distance of a neighbor's periodic image, which would
    // connect all atom pairs (a C60 buckyball becomes all 1770 pairs). Four
    // carbons in a line must yield exactly the two real C-C bonds (1-2, 2-3),
    // never six. (Earlier this was asserted with only two atoms, where the
    // spurious bond happened to look correct and masked the over-bonding.)
    let structure = parse_pdb(
        "\
TITLE     dummy cell
CRYST1    1.000    1.000    1.000  90.00  90.00  90.00 P 1
ATOM      1  C   UNK A   1       0.000   0.000   0.000  1.00  0.00           C
ATOM      2  C   UNK A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   UNK A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  C   UNK A   1      10.000   0.000   0.000  1.00  0.00           C
END
",
    )
    .expect("valid pdb");

    assert!(
        structure.cell.is_none(),
        "the placeholder cell must be discarded"
    );
    assert_eq!(
        structure.bonds.len(),
        2,
        "the placeholder cell must not connect all atom pairs"
    );
}

#[test]
fn interprets_repeated_conect_entries_as_higher_bond_order() {
    let structure = parse_pdb(
        "\
TITLE     ethene
ATOM      1  C1   ETH     1       0.000   0.000   0.000  1.00  0.00           C
ATOM      2  C2   ETH     1       1.340   0.000   0.000  1.00  0.00           C
CONECT    1    2    2
CONECT    2    1    1
END
",
    )
    .expect("valid pdb");

    assert_eq!(structure.bonds.len(), 1);
    assert_eq!(structure.bonds[0].bond_type, BondType::Double);
}

#[test]
fn parses_link_records_for_biomolecular_contacts() {
    let structure = parse_pdb(
        "\
TITLE     peptide with link
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   GLY A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  O   GLY A   1       4.100   0.000   0.000  1.00  0.00           O
HETATM    5 CA    CA A  10       6.900   0.000   0.000  1.00  0.00          CA
LINK         O   GLY A   1                CA    CA A  10     1555   1555  2.80
END
",
    )
    .expect("valid pdb");

    assert!(
        structure
            .bonds
            .iter()
            .any(|bond| bond.a == 3 && bond.b == 4)
    );
}

#[test]
fn keeps_secondary_structure_metadata() {
    let structure = parse_pdb(
        "\
HELIX    1   1 GLY A    1  ALA A    2  1
SHEET    1   A 1 GLY A   3  ALA A   4  0
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  N   ALA A   2       2.900   0.000   0.000  1.00  0.00           N
ATOM      4  CA  ALA A   2       4.350   0.000   0.000  1.00  0.00           C
ATOM      5  N   GLY A   3       5.800   0.000   0.000  1.00  0.00           N
ATOM      6  CA  GLY A   3       7.250   0.000   0.000  1.00  0.00           C
ATOM      7  N   ALA A   4       8.700   0.000   0.000  1.00  0.00           N
ATOM      8  CA  ALA A   4      10.150   0.000   0.000  1.00  0.00           C
END
",
    )
    .expect("valid pdb");

    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    assert_eq!(biopolymer.secondary_structures.len(), 2);
    assert_eq!(
        biopolymer.secondary_structures[0].kind,
        crate::domain::SecondaryStructureKind::Helix
    );
    assert_eq!(
        biopolymer.secondary_structures[1].kind,
        crate::domain::SecondaryStructureKind::Sheet
    );
}

#[test]
fn to_pdb_preserves_biopolymer_atom_names() {
    // A residue's atom names (CA, C, O, ...) must survive export so an engine
    // preprocessor can match force-field residue templates, rather than being
    // replaced by synthesized element+serial names.
    let structure = parse_pdb(
        "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   GLY A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  O   GLY A   1       4.100   0.000   0.000  1.00  0.00           O
END
",
    )
    .expect("valid pdb");

    let out = to_pdb(&structure).expect("serialize");
    let names: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("ATOM"))
        .map(|l| l[12..16].trim())
        .collect();
    assert_eq!(names, ["N", "CA", "C", "O"]);
}

fn has_bond_between(structure: &crate::domain::Structure, first: usize, second: usize) -> bool {
    structure
        .bonds
        .iter()
        .any(|bond| (bond.a == first && bond.b == second) || (bond.a == second && bond.b == first))
}

fn atom_index(structure: &crate::domain::Structure, residue: &str, atom: &str) -> usize {
    let bio = structure.biopolymer.as_ref().expect("biopolymer overlay");
    let residue_index = bio
        .residues
        .iter()
        .position(|r| r.residue_name == residue)
        .expect("residue present");
    (0..structure.atoms.len())
        .find(|&i| {
            bio.residue_for_atom.get(i).and_then(|r| *r) == Some(residue_index)
                && bio.atom_name(i) == Some(atom)
        })
        .expect("named atom present")
}

#[test]
fn glycan_to_asn_link_round_trips_through_pdb() {
    let source = "\
TITLE     n-glycan junction
ATOM      1  N   ASN A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ASN A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  CG  ASN A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  ND2 ASN A   1       4.000   0.800   0.000  1.00  0.00           N
ATOM      5  C1  NAG B   1       5.400   0.800   0.000  1.00  0.00           C
ATOM      6  C2  NAG B   1       6.100   2.000   0.000  1.00  0.00           C
ATOM      7  O5  NAG B   1       6.100  -0.300   0.000  1.00  0.00           O
LINK         ND2 ASN A   1                 C1  NAG B   1
END
";
    let structure = parse_pdb(source).expect("valid glycoprotein pdb");
    let nd2 = atom_index(&structure, "ASN", "ND2");
    let c1 = atom_index(&structure, "NAG", "C1");
    assert!(
        has_bond_between(&structure, nd2, c1),
        "inbound LINK should create the ND2-C1 bond"
    );

    let serialized = to_pdb(&structure).expect("serialize glycoprotein");
    assert!(
        serialized.lines().any(|line| line.starts_with("LINK")),
        "to_pdb should emit a LINK record for the glycan-protein junction"
    );

    let reparsed = parse_pdb(&serialized).expect("reparse glycoprotein");
    let nd2_again = atom_index(&reparsed, "ASN", "ND2");
    let c1_again = atom_index(&reparsed, "NAG", "C1");
    assert!(
        has_bond_between(&reparsed, nd2_again, c1_again),
        "the cross-residue bond must survive a to_pdb / parse round trip"
    );
}
