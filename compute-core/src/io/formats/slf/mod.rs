mod document;
mod reticular;
mod sectioned;

use anyhow::Result;

use crate::{
    domain::{Atom, Bond, Structure},
    io::formats::mol2::{parse_mol2_document, to_mol2},
};

pub use document::SlfDocument;
pub use reticular::{SlfReticular, SlfSubstitutionSite};
pub use sectioned::{
    SlfExtensionBlock, SlfExtensionPayload, SlfKeyBlock, SlfKeyEntry, SlfSection,
    SlfSectionedBlock, SlfTableBlock,
};

use reticular::{parse_reticular_block, serialize_reticular_block};
use sectioned::parse_extension_block;

pub fn to_slf(structure: &Structure, reticular: Option<&SlfReticular>) -> String {
    let mut output = to_mol2(structure);

    if let Some(reticular) = reticular {
        output.push_str(&serialize_reticular_block(reticular));
    }

    output
}

pub fn parse_slf(input: &str) -> Result<Structure> {
    let document = parse_slf_document(input)?;
    let atoms = document
        .atoms
        .iter()
        .map(|atom| Atom {
            element: atom.element.clone(),
            position: atom.position,
            charge: atom.charge,
        })
        .collect::<Vec<_>>();
    let bonds = document
        .bonds
        .iter()
        .map(|bond| Bond::with_type(bond.a, bond.b, bond.bond_type))
        .collect::<Vec<_>>();

    if let Some(crysin) = document.crysin {
        Ok(Structure::with_cell_and_bonds(
            document.title,
            atoms,
            bonds,
            crysin.cell,
        ))
    } else {
        Ok(Structure::with_bonds(document.title, atoms, bonds))
    }
}

pub fn parse_slf_document(input: &str) -> Result<SlfDocument> {
    let base = parse_mol2_document(input)?;

    let mut extensions = Vec::new();
    let lines = input.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim();
        if !line.starts_with("@<") {
            index += 1;
            continue;
        }

        let section = line.to_ascii_uppercase();
        index += 1;

        if let Some(name) = section.strip_prefix("@<SILICOLAB>") {
            let block = parse_extension_block(name, &lines, &mut index)?;
            extensions.push(block);
        } else {
            while index < lines.len() && !lines[index].trim().starts_with("@<") {
                index += 1;
            }
        }
    }

    let mut document = SlfDocument {
        title: base.title,
        atoms: base.atoms,
        bonds: base.bonds,
        crysin: base.crysin,
        extensions,
        reticular: None,
    };
    document.reticular = document
        .extension_block("RETICULAR")
        .map(parse_reticular_block)
        .transpose()?;

    Ok(document)
}

#[cfg(test)]
mod tests {
    use crate::domain::BondType;

    use super::{
        SlfExtensionPayload, SlfReticular, SlfSection, SlfSubstitutionSite, parse_slf,
        parse_slf_document, to_slf,
    };

    #[test]
    fn parses_sectioned_extensions() {
        let input = "\
@<TRIPOS>MOLECULE
test
2 1
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.ar 1 RES 0.0
2 Du1 1.4 0.0 0.0 Du 1 RES 0.0
@<TRIPOS>BOND
1 1 2 1
@<SILICOLAB>RETICULAR
#KEY
class core
label TEST
#TABLE
leaving_atom binding_atom
2 1
@<SILICOLAB>ATOM_SITE
#TABLE
atom_id occupancy b_factor
1 0.50 13.37
";

        let document = parse_slf_document(input).expect("document");
        let reticular = document.reticular.expect("reticular");
        assert_eq!(reticular.class, "core");
        assert_eq!(reticular.label.as_deref(), Some("TEST"));
        assert_eq!(reticular.substitution_sites.len(), 1);
        assert_eq!(reticular.substitution_sites[0].leaving_atom, 1);
        assert_eq!(reticular.substitution_sites[0].binding_atom, 0);
        assert_eq!(document.extensions.len(), 2);

        match &document.extensions[1].payload {
            SlfExtensionPayload::Sectioned(sectioned) => {
                let table = sectioned.first_table().expect("atom-site table");
                assert_eq!(table.columns, vec!["atom_id", "occupancy", "b_factor"]);
                assert_eq!(table.rows.len(), 1);
                assert!(matches!(sectioned.sections[0], SlfSection::Table(_)));
            }
        }
    }

    #[test]
    fn reuses_mol2_crysin_parsing_for_slf_base_sections() {
        let document = parse_slf_document(
            "\
@<TRIPOS>MOLECULE
cell
1 0
SMALL
USER_CHARGES
@<TRIPOS>CRYSIN
12.312000 4.959000 15.876000 90.000000 99.070000 90.000000 4 1
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.3 1 RES 0.0
",
        )
        .expect("document");

        let crysin = document.crysin.expect("crysin");
        assert!((crysin.cell.a - 12.312).abs() < 0.0001);
        assert_eq!(crysin.space_group_number, 4);
    }

    #[test]
    fn parses_structure_and_preserves_aromatic_bonds() {
        let structure = parse_slf(
            "\
@<TRIPOS>MOLECULE
benzene
2 1
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.ar 1 RES 0.0
2 C2 1.4 0.0 0.0 C.ar 1 RES 0.0
@<TRIPOS>BOND
1 1 2 ar
",
        )
        .expect("structure");

        assert_eq!(structure.atoms.len(), 2);
        assert_eq!(structure.bonds.len(), 1);
        assert_eq!(structure.bonds[0].bond_type, BondType::Aromatic);
    }

    #[test]
    fn writes_reticular_extension_block() {
        let structure = parse_slf(
            "\
@<TRIPOS>MOLECULE
test
2 1
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.ar 1 RES 0.0
2 Du1 1.4 0.0 0.0 Du 1 RES 0.0
@<TRIPOS>BOND
1 1 2 1
",
        )
        .expect("structure");
        let output = to_slf(
            &structure,
            Some(&SlfReticular {
                class: "core".to_string(),
                label: Some("TEST".to_string()),
                substitution_sites: vec![SlfSubstitutionSite {
                    leaving_atom: 1,
                    binding_atom: 0,
                }],
            }),
        );

        assert!(output.contains("@<SILICOLAB>RETICULAR"));
        assert!(output.contains("#KEY"));
        assert!(output.contains("class core"));
        assert!(output.contains("label TEST"));
        assert!(output.contains("#TABLE"));
        assert!(output.contains("leaving_atom binding_atom"));
        assert!(output.contains("2 1"));
    }

    #[test]
    fn rejects_malformed_table_rows() {
        let error = parse_slf_document(
            "\
@<TRIPOS>MOLECULE
bad
1 0
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.3 1 RES 0.0
@<SILICOLAB>RETICULAR
#KEY
class core
label TEST
#TABLE
leaving_atom binding_atom
2
",
        )
        .expect_err("malformed table should fail");

        assert!(
            error
                .to_string()
                .contains("#TABLE row has 1 fields but header defines 2 columns")
        );
    }

    #[test]
    fn rejects_non_sectioned_extension_blocks() {
        let error = parse_slf_document(
            "\
@<TRIPOS>MOLECULE
bad
1 0
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.3 1 RES 0.0
@<SILICOLAB>ATOM_SITE
TABLE
atom_id occupancy
1 0.5
",
        )
        .expect_err("non-sectioned block should fail");

        assert!(
            error
                .to_string()
                .contains("must use section tags like #KEY or #TABLE")
        );
    }
}
