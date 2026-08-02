use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::chemistry::{infer_bonds_with_cell, normalized_symbol},
    domain::{Atom, PdbAtomAnnotation, Structure, UnitCell, build_biopolymer},
    io::formats::{cif_crystal::parse_classic_cif, cif_syntax::tokenize_cif},
};

pub fn parse_cif(input: &str) -> Result<Structure> {
    // mmCIF (dotted `_atom_site.Cartn_*`) is what we emit on save; try it first
    // and fall back to classic CIF with fractional coordinates.
    if let Ok(structure) = parse_mmcif(input) {
        return Ok(structure);
    }

    parse_classic_cif(&tokenize_cif(input)?)
}

/// Serialize a structure as a minimal mmCIF document with Cartesian
/// coordinates. This is the canonical on-disk form produced on save and read
/// back by the private `parse_mmcif` parser.
pub fn to_cif(structure: &Structure) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!("data_{}\n", sanitize_identifier(&structure.title)));
    output.push_str("#\n");

    if let Some(cell) = &structure.cell {
        output.push_str(&format!("_cell.length_a    {:.3}\n", cell.a));
        output.push_str(&format!("_cell.length_b    {:.3}\n", cell.b));
        output.push_str(&format!("_cell.length_c    {:.3}\n", cell.c));
        output.push_str(&format!("_cell.angle_alpha {:.3}\n", cell.alpha));
        output.push_str(&format!("_cell.angle_beta  {:.3}\n", cell.beta));
        output.push_str(&format!("_cell.angle_gamma {:.3}\n", cell.gamma));
        output.push_str("#\n");
    }

    output.push_str("loop_\n");
    output.push_str("_atom_site.group_PDB\n");
    output.push_str("_atom_site.id\n");
    output.push_str("_atom_site.type_symbol\n");
    output.push_str("_atom_site.Cartn_x\n");
    output.push_str("_atom_site.Cartn_y\n");
    output.push_str("_atom_site.Cartn_z\n");

    for (index, atom) in structure.atoms.iter().enumerate() {
        let symbol = normalized_symbol(atom.element.trim());
        let symbol = if symbol.is_empty() { "X" } else { &symbol };
        output.push_str(&format!(
            "ATOM {} {} {:.3} {:.3} {:.3}\n",
            index + 1,
            symbol,
            atom.position.x,
            atom.position.y,
            atom.position.z,
        ));
    }

    Ok(output)
}

/// Parse an mmCIF document with Cartesian coordinates. Errors (rather than
/// falling back) when the document is not mmCIF, e.g. classic CIF with
/// fractional coordinates, so callers can dispatch on the result.
fn parse_mmcif(input: &str) -> Result<Structure> {
    let tokens = tokenize_cif(input)?;

    let title = tokens
        .iter()
        .find(|token| token.starts_with("data_"))
        .map(|token| token.trim_start_matches("data_").to_string())
        .unwrap_or_else(|| "mmCIF structure".to_string());

    let cell = read_optional_cell(&tokens);
    let parsed = read_cartesian_atom_sites(&tokens)?;
    let atoms = parsed.atoms;
    let biopolymer = parsed
        .annotations
        .and_then(|annotations| build_biopolymer(&annotations, Vec::new()));

    let mut structure = match cell {
        // See the PDB reader: a deposited biomolecule is bonded non-periodically
        // even when it carries a crystallographic cell, which is kept for display
        // and PBC.
        Some(cell) if biopolymer.is_some() => {
            let bonds = infer_bonds_with_cell(&atoms, None);
            Structure::with_cell_and_bonds(title, atoms, bonds, cell)
        }
        Some(cell) => Structure::with_cell(title, atoms, cell),
        None => Structure::new(title, atoms),
    };
    structure.biopolymer = biopolymer;
    Ok(structure)
}

struct ParsedAtomSites {
    atoms: Vec<Atom>,
    annotations: Option<Vec<PdbAtomAnnotation>>,
}

fn read_optional_cell(tokens: &[String]) -> Option<UnitCell> {
    let value = |tag: &str| {
        tokens
            .windows(2)
            .find(|pair| pair[0].eq_ignore_ascii_case(tag))
            .and_then(|pair| parse_cif_number(&pair[1]).ok())
    };

    Some(UnitCell::from_parameters(
        value("_cell.length_a")?,
        value("_cell.length_b")?,
        value("_cell.length_c")?,
        value("_cell.angle_alpha")?,
        value("_cell.angle_beta")?,
        value("_cell.angle_gamma")?,
    ))
}

fn read_cartesian_atom_sites(tokens: &[String]) -> Result<ParsedAtomSites> {
    let mut index = 0;

    while index < tokens.len() {
        if !tokens[index].eq_ignore_ascii_case("loop_") {
            index += 1;
            continue;
        }

        index += 1;
        let header_start = index;
        while index < tokens.len() && tokens[index].starts_with('_') {
            index += 1;
        }

        let headers = &tokens[header_start..index];
        if !headers
            .iter()
            .any(|header| header.eq_ignore_ascii_case("_atom_site.Cartn_x"))
        {
            continue;
        }

        let element_index = find_header(headers, "_atom_site.type_symbol")
            .or_else(|| find_header(headers, "_atom_site.label_atom_id"))
            .ok_or_else(|| anyhow!("mmCIF atom site loop lacks a type symbol"))?;
        let x_index = find_header(headers, "_atom_site.Cartn_x")
            .ok_or_else(|| anyhow!("mmCIF atom site loop lacks Cartn_x"))?;
        let y_index = find_header(headers, "_atom_site.Cartn_y")
            .ok_or_else(|| anyhow!("mmCIF atom site loop lacks Cartn_y"))?;
        let z_index = find_header(headers, "_atom_site.Cartn_z")
            .ok_or_else(|| anyhow!("mmCIF atom site loop lacks Cartn_z"))?;

        let comp_id_index = find_header(headers, "_atom_site.label_comp_id")
            .or_else(|| find_header(headers, "_atom_site.auth_comp_id"));
        let atom_id_index = find_header(headers, "_atom_site.label_atom_id")
            .or_else(|| find_header(headers, "_atom_site.auth_atom_id"));
        let asym_id_index = find_header(headers, "_atom_site.label_asym_id")
            .or_else(|| find_header(headers, "_atom_site.auth_asym_id"));
        let seq_id_index = find_header(headers, "_atom_site.auth_seq_id")
            .or_else(|| find_header(headers, "_atom_site.label_seq_id"));
        let collect_annotations = comp_id_index.is_some() && atom_id_index.is_some();

        let width = headers.len();
        let mut atoms = Vec::new();
        let mut annotations: Vec<PdbAtomAnnotation> = Vec::new();

        while index + width <= tokens.len() {
            if tokens[index].eq_ignore_ascii_case("loop_") || tokens[index].starts_with('_') {
                break;
            }

            let row = &tokens[index..index + width];
            let element = element_from_atom_site(&row[element_index]);
            let x = parse_cif_number(&row[x_index]).context("invalid atom Cartn_x")?;
            let y = parse_cif_number(&row[y_index]).context("invalid atom Cartn_y")?;
            let z = parse_cif_number(&row[z_index]).context("invalid atom Cartn_z")?;

            atoms.push(Atom {
                element,
                position: nalgebra::Point3::new(x, y, z),
                charge: 0.0,
            });

            if collect_annotations {
                annotations.push(atom_site_annotation(
                    row,
                    comp_id_index,
                    atom_id_index,
                    asym_id_index,
                    seq_id_index,
                ));
            }

            index += width;
        }

        if atoms.is_empty() {
            bail!("mmCIF atom site loop did not contain any atoms");
        }

        let annotations =
            (collect_annotations && annotations.len() == atoms.len()).then_some(annotations);
        return Ok(ParsedAtomSites { atoms, annotations });
    }

    bail!("missing mmCIF atom site loop with Cartesian coordinates")
}

fn atom_site_annotation(
    row: &[String],
    comp_id_index: Option<usize>,
    atom_id_index: Option<usize>,
    asym_id_index: Option<usize>,
    seq_id_index: Option<usize>,
) -> PdbAtomAnnotation {
    let cell = |column: Option<usize>| column.and_then(|i| row.get(i)).map(|value| value.trim());
    let residue_name = cell(comp_id_index).unwrap_or("UNK").to_string();
    let atom_name = cell(atom_id_index).unwrap_or("").to_string();
    let chain_id = cell(asym_id_index)
        .and_then(|value| value.chars().next())
        .unwrap_or('A');
    let residue_seq = cell(seq_id_index)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(1);

    PdbAtomAnnotation {
        atom_name,
        residue_name,
        chain_id,
        residue_seq,
        insertion_code: ' ',
    }
}

fn sanitize_identifier(value: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "structure".to_string()
    } else {
        sanitized
    }
}

pub(super) fn find_header(headers: &[String], name: &str) -> Option<usize> {
    headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case(name))
}

pub(super) fn element_from_atom_site(value: &str) -> String {
    let letters = value
        .trim()
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();

    normalized_symbol(&letters)
}

pub(super) fn parse_cif_number(value: &str) -> Result<f32> {
    let trimmed = value.trim();
    let without_uncertainty = trimmed
        .split_once('(')
        .map(|(number, _)| number)
        .unwrap_or(trimmed);

    without_uncertainty
        .parse::<f32>()
        .with_context(|| format!("expected a number, got {value}"))
}

#[cfg(test)]
mod tests {
    use super::parse_cif;
    use crate::domain::AtomCategory;

    #[test]
    fn parses_fractional_atoms_and_unit_cell() {
        let structure = parse_cif(
            "\
data_NaCl
_cell_length_a 5.6402
_cell_length_b 5.6402
_cell_length_c 5.6402
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_
_atom_site_label
_atom_site_type_symbol
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
Na1 Na 0 0 0
Cl1 Cl 0.5 0.5 0.5
",
        )
        .expect("valid cif");

        let cell = structure.cell.as_ref().expect("unit cell");

        assert_eq!(structure.title, "NaCl");
        assert_eq!(structure.atoms.len(), 2);
        assert!((cell.a - 5.6402).abs() < 0.0001);
        assert!((structure.atoms[1].position.x - 2.8201).abs() < 0.0001);
    }

    #[test]
    fn parses_cif_with_symmetry_tags() {
        let structure = parse_cif(
            "\
data_test
_cell_length_a 5.0
_cell_length_b 5.0
_cell_length_c 5.0
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
_symmetry_space_group_name_H-M \"P 1\"
_symmetry_Int_Tables_number 1
loop_
_atom_site_label
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
H1 0.0 0.0 0.0
",
        )
        .expect("valid cif with symmetry tags");

        assert_eq!(structure.atoms.len(), 1);
        assert_eq!(structure.atoms[0].element, "H");
    }

    #[test]
    fn defaults_to_p1_when_symmetry_tags_missing() {
        let structure = parse_cif(
            "\
data_test
_cell_length_a 5.0
_cell_length_b 5.0
_cell_length_c 5.0
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_
_atom_site_label
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
H1 0.0 0.0 0.0
",
        )
        .expect("valid cif without symmetry tags");

        assert_eq!(structure.atoms.len(), 1);
    }

    #[test]
    fn classifies_mmcif_glycoprotein_residues() {
        let structure = parse_cif(
            "\
data_glyco
loop_
_atom_site.group_PDB
_atom_site.id
_atom_site.type_symbol
_atom_site.label_atom_id
_atom_site.label_comp_id
_atom_site.label_asym_id
_atom_site.auth_seq_id
_atom_site.Cartn_x
_atom_site.Cartn_y
_atom_site.Cartn_z
ATOM   1 N  N   ASN A 1 0.000 0.000 0.000
ATOM   2 C  CA  ASN A 1 1.450 0.000 0.000
HETATM 3 C  C1  NAG B 1 5.400 0.800 0.000
HETATM 4 O  O5  NAG B 1 6.100 -0.300 0.000
HETATM 5 C  C1  MAN B 2 8.000 0.800 0.000
",
        )
        .expect("valid mmcif glycoprotein");

        assert_eq!(structure.atoms.len(), 5);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer overlay");
        assert!(biopolymer.is_compatible_with_atom_count(structure.atoms.len()));

        assert_eq!(structure.atom_category(0), AtomCategory::Protein);
        assert_eq!(structure.atom_category(2), AtomCategory::Carbohydrate);
        assert_eq!(structure.atom_category(4), AtomCategory::Carbohydrate);
    }

    #[test]
    fn deposited_protein_mmcif_with_real_cell_is_bonded_nonperiodically() {
        // Same 1HKN regression as the PDB reader, for the mmCIF path: a protein that
        // carries a real cell is bonded non-periodically (the two backbone nitrogens
        // are 9.8 A apart in Cartesian but 0.2 A under minimum image), while the cell
        // is preserved for display/PBC.
        let structure = parse_cif(
            "\
data_peptide
_cell.length_a 10.000
_cell.length_b 10.000
_cell.length_c 10.000
_cell.angle_alpha 90
_cell.angle_beta 90
_cell.angle_gamma 90
loop_
_atom_site.group_PDB
_atom_site.id
_atom_site.type_symbol
_atom_site.label_atom_id
_atom_site.label_comp_id
_atom_site.label_asym_id
_atom_site.auth_seq_id
_atom_site.Cartn_x
_atom_site.Cartn_y
_atom_site.Cartn_z
ATOM 1 N N ALA A 1 0.100 0.000 0.000
ATOM 2 N N ALA A 2 9.900 0.000 0.000
",
        )
        .expect("valid mmcif");

        assert!(
            structure.biopolymer.is_some(),
            "ALA residues form a biopolymer"
        );
        assert!(
            structure.cell.is_some(),
            "the cell is preserved for display/PBC"
        );
        assert!(
            structure.bonds.is_empty(),
            "minimum-image must not fabricate a cross-cell bond for a deposited biomolecule"
        );
    }

    #[test]
    fn rejects_non_p1_space_group() {
        let result = parse_cif(
            "\
data_test
_cell_length_a 5.0
_cell_length_b 5.0
_cell_length_c 5.0
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
_symmetry_space_group_name_H-M \"P 2\"
_symmetry_Int_Tables_number 2
loop_
_atom_site_label
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
H1 0.0 0.0 0.0
",
        );

        assert!(result.is_err());
    }

    #[test]
    fn expands_non_p1_symmetry_and_deduplicates_special_positions() {
        let structure = parse_cif(
            "\
data_inversion
_cell_length_a 10
_cell_length_b 10
_cell_length_c 10
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
_space_group_name_H-M_alt 'P -1'
loop_
_space_group_symop_operation_xyz
'x,y,z'
'-x,-y,-z'
loop_
_atom_site_label
_atom_site_type_symbol
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
C1 C 0.1 0.2 0.3
O1 O 0 0 0
",
        )
        .unwrap();

        assert_eq!(structure.atoms.len(), 3);
        assert_eq!(
            structure
                .atoms
                .iter()
                .filter(|atom| atom.element == "O")
                .count(),
            1
        );
        assert!(structure.atoms.iter().any(|atom| {
            atom.element == "C"
                && (atom.position.x - 9.0).abs() < 1.0e-4
                && (atom.position.y - 8.0).abs() < 1.0e-4
        }));
    }

    #[test]
    fn supports_legacy_symmetry_tag_and_fractional_translation() {
        let structure = parse_cif(
            "\
data_legacy
_cell_length_a 8
_cell_length_b 8
_cell_length_c 8
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
_symmetry_space_group_name_H-M 'P 2'
loop_
_symmetry_equiv_pos_as_xyz
'x,y,z'
'-x,y+1/2,-z'
loop_
_atom_site_label
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
N1 0.25 0.1 0.2
",
        )
        .unwrap();

        assert_eq!(structure.atoms.len(), 2);
        assert!(
            structure
                .atoms
                .iter()
                .any(|atom| (atom.position.y - 4.8).abs() < 1.0e-4)
        );
    }

    #[test]
    fn rejects_partial_occupancy_and_disorder_groups() {
        let base = "\
data_bad
_cell_length_a 5
_cell_length_b 5
_cell_length_c 5
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_\n";
        let partial = format!(
            "{base}_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n_atom_site_occupancy\nC1 0 0 0 0.5\n"
        );
        assert!(
            parse_cif(&partial)
                .unwrap_err()
                .to_string()
                .contains("partial")
        );

        let disorder = format!(
            "{base}_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n_atom_site_disorder_group\nC1 0 0 0 1\n"
        );
        assert!(
            parse_cif(&disorder)
                .unwrap_err()
                .to_string()
                .contains("disordered")
        );
    }

    #[test]
    fn structured_parser_handles_quotes_multiline_text_and_uncertainties() {
        let structure = parse_cif(
            "\
data_metadata
_chemical_name_common 'quoted crystal name'
_publ_section_title
;First line
second line
;
_cell_length_a 5.0(2)
_cell_length_b 5.0(2)
_cell_length_c 5.0(2)
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_
_atom_site_label
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
C1 0.25(1) 0.5 0.75
",
        )
        .unwrap();

        assert_eq!(structure.atoms.len(), 1);
        assert!((structure.atoms[0].position.x - 1.25).abs() < 1.0e-4);
    }
}
