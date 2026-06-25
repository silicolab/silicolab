use anyhow::{Context, Result, anyhow, bail};

use crate::{
    domain::chemistry::normalized_symbol,
    domain::{Atom, PdbAtomAnnotation, Structure, UnitCell, build_biopolymer},
};

pub fn parse_cif(input: &str) -> Result<Structure> {
    // mmCIF (dotted `_atom_site.Cartn_*`) is what we emit on save; try it first
    // and fall back to classic CIF with fractional coordinates.
    if let Ok(structure) = parse_mmcif(input) {
        return Ok(structure);
    }

    let tokens = tokenize_cif(input);
    let title = tokens
        .iter()
        .find(|token| token.starts_with("data_"))
        .map(|token| token.trim_start_matches("data_").to_string())
        .unwrap_or_else(|| "CIF structure".to_string());

    let a = read_number_tag(&tokens, "_cell_length_a")?;
    let b = read_number_tag(&tokens, "_cell_length_b")?;
    let c = read_number_tag(&tokens, "_cell_length_c")?;
    let alpha = read_number_tag(&tokens, "_cell_angle_alpha")?;
    let beta = read_number_tag(&tokens, "_cell_angle_beta")?;
    let gamma = read_number_tag(&tokens, "_cell_angle_gamma")?;
    let cell = UnitCell::from_parameters(a, b, c, alpha, beta, gamma);
    let atoms = read_atom_sites(&tokens, &cell)?;

    let space_group = read_string_tag(&tokens, "_symmetry_space_group_name_H-M")
        .unwrap_or_else(|_| "P 1".to_string());
    let _tables_number = read_number_tag(&tokens, "_symmetry_Int_Tables_number").unwrap_or(1.0);

    if !space_group.trim().eq_ignore_ascii_case("P 1") {
        bail!("non-P1 space groups are not yet supported");
    }

    Ok(Structure::with_cell(title, atoms, cell))
}

/// Serialize a structure as a minimal mmCIF document with Cartesian
/// coordinates. This is the canonical on-disk form produced on save and read
/// back by [`parse_mmcif`].
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
    let tokens = tokenize_cif(input);

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

fn read_number_tag(tokens: &[String], tag: &str) -> Result<f32> {
    let value = tokens
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case(tag))
        .map(|pair| pair[1].as_str())
        .ok_or_else(|| anyhow!("missing required CIF tag {tag}"))?;

    parse_cif_number(value).with_context(|| format!("invalid value for {tag}"))
}

fn read_string_tag(tokens: &[String], tag: &str) -> Result<String> {
    let value = tokens
        .windows(2)
        .find(|pair| pair[0].eq_ignore_ascii_case(tag))
        .map(|pair| pair[1].as_str())
        .ok_or_else(|| anyhow!("missing required CIF tag {tag}"))?;

    Ok(value.trim_matches('\'').trim_matches('"').to_string())
}

fn read_atom_sites(tokens: &[String], cell: &UnitCell) -> Result<Vec<Atom>> {
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
            .any(|header| header.eq_ignore_ascii_case("_atom_site_fract_x"))
        {
            continue;
        }

        let element_index = find_header(headers, "_atom_site_type_symbol")
            .or_else(|| find_header(headers, "_atom_site_label"))
            .ok_or_else(|| anyhow!("atom site loop lacks type symbol or label"))?;
        let x_index = find_header(headers, "_atom_site_fract_x")
            .ok_or_else(|| anyhow!("atom site loop lacks fract_x"))?;
        let y_index = find_header(headers, "_atom_site_fract_y")
            .ok_or_else(|| anyhow!("atom site loop lacks fract_y"))?;
        let z_index = find_header(headers, "_atom_site_fract_z")
            .ok_or_else(|| anyhow!("atom site loop lacks fract_z"))?;

        let width = headers.len();
        let mut atoms = Vec::new();

        while index + width <= tokens.len() {
            if tokens[index].eq_ignore_ascii_case("loop_") || tokens[index].starts_with('_') {
                break;
            }

            let row = &tokens[index..index + width];
            let element = element_from_atom_site(&row[element_index]);
            let x = parse_cif_number(&row[x_index]).context("invalid atom fract_x")?;
            let y = parse_cif_number(&row[y_index]).context("invalid atom fract_y")?;
            let z = parse_cif_number(&row[z_index]).context("invalid atom fract_z")?;

            atoms.push(Atom {
                element,
                position: cell.fractional_to_cartesian(x, y, z),
                charge: 0.0,
            });

            index += width;
        }

        if atoms.is_empty() {
            bail!("atom site loop did not contain any atoms");
        }

        return Ok(atoms);
    }

    bail!("missing CIF atom site loop with fractional coordinates")
}

fn find_header(headers: &[String], name: &str) -> Option<usize> {
    headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case(name))
}

fn element_from_atom_site(value: &str) -> String {
    let letters = value
        .trim()
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();

    normalized_symbol(&letters)
}

fn parse_cif_number(value: &str) -> Result<f32> {
    let trimmed = value.trim();
    let without_uncertainty = trimmed
        .split_once('(')
        .map(|(number, _)| number)
        .unwrap_or(trimmed);

    without_uncertainty
        .parse::<f32>()
        .with_context(|| format!("expected a number, got {value}"))
}

fn tokenize_cif(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    for line in input.lines() {
        let without_comment = line.split_once('#').map(|(head, _)| head).unwrap_or(line);
        let mut current = String::new();
        let mut quote = None;

        for ch in without_comment.chars() {
            match quote {
                Some(active) if ch == active => {
                    quote = None;
                }
                Some(_) => current.push(ch),
                None if ch == '\'' || ch == '"' => {
                    quote = Some(ch);
                }
                None if ch.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                None => current.push(ch),
            }
        }

        if !current.is_empty() {
            tokens.push(current);
        }
    }

    tokens
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
}
