use anyhow::{Context, Result, anyhow, bail};

use crate::domain::{Atom, Structure, UnitCell};

use super::cif::{element_from_atom_site, find_header, parse_cif_number};

pub(super) fn parse_classic_cif(tokens: &[String]) -> Result<Structure> {
    let title = tokens
        .iter()
        .find(|token| token.starts_with("data_"))
        .map(|token| token.trim_start_matches("data_").to_string())
        .unwrap_or_else(|| "CIF structure".to_string());
    let cell = UnitCell::from_parameters(
        read_number_tag(tokens, "_cell_length_a")?,
        read_number_tag(tokens, "_cell_length_b")?,
        read_number_tag(tokens, "_cell_length_c")?,
        read_number_tag(tokens, "_cell_angle_alpha")?,
        read_number_tag(tokens, "_cell_angle_beta")?,
        read_number_tag(tokens, "_cell_angle_gamma")?,
    );
    let sites = read_atom_sites(tokens)?;
    let space_group = read_string_tag(tokens, "_space_group_name_H-M_alt")
        .or_else(|_| read_string_tag(tokens, "_symmetry_space_group_name_H-M"))
        .unwrap_or_else(|_| "P 1".to_string());
    let operations = read_symmetry_operations(tokens)?;
    let operations = if operations.is_empty() {
        if !is_p1(&space_group) {
            bail!(
                "space group `{space_group}` requires explicit CIF symmetry operations, but none were provided"
            );
        }
        vec![SymmetryOperation::identity()]
    } else {
        operations
    };
    Ok(Structure::with_cell(
        title,
        expand_atom_sites(&sites, &operations, &cell),
        cell,
    ))
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

struct FractionalAtomSite {
    element: String,
    coordinates: [f32; 3],
}

fn read_atom_sites(tokens: &[String]) -> Result<Vec<FractionalAtomSite>> {
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
        let occupancy_index = find_header(headers, "_atom_site_occupancy");
        let disorder_group_index = find_header(headers, "_atom_site_disorder_group");
        let width = headers.len();
        let mut atoms = Vec::new();
        while index + width <= tokens.len() {
            if tokens[index].eq_ignore_ascii_case("loop_") || tokens[index].starts_with('_') {
                break;
            }
            let row = &tokens[index..index + width];
            let element = element_from_atom_site(&row[element_index]);
            let coordinates = [
                parse_cif_number(&row[x_index]).context("invalid atom fract_x")?,
                parse_cif_number(&row[y_index]).context("invalid atom fract_y")?,
                parse_cif_number(&row[z_index]).context("invalid atom fract_z")?,
            ];
            if let Some(occupancy_index) = occupancy_index {
                let occupancy = &row[occupancy_index];
                if matches!(occupancy.as_str(), "." | "?") {
                    bail!("atom site has unknown occupancy, which SilicoLab cannot represent");
                }
                if parse_cif_number(occupancy).context("invalid atom occupancy")? < 0.999_99 {
                    bail!("partial atom occupancy is not supported");
                }
            }
            if let Some(disorder_group_index) = disorder_group_index {
                let group = row[disorder_group_index].trim();
                if !matches!(group, "." | "?" | "0") {
                    bail!("disordered atom sites are not supported");
                }
            }
            atoms.push(FractionalAtomSite {
                element,
                coordinates,
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

#[derive(Clone, Copy)]
struct SymmetryOperation {
    matrix: [[f32; 3]; 3],
    offset: [f32; 3],
}

impl SymmetryOperation {
    fn identity() -> Self {
        Self {
            matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            offset: [0.0; 3],
        }
    }

    fn apply(self, coordinates: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|row| {
            (self.matrix[row][0] * coordinates[0]
                + self.matrix[row][1] * coordinates[1]
                + self.matrix[row][2] * coordinates[2]
                + self.offset[row])
                .rem_euclid(1.0)
        })
    }
}

fn read_symmetry_operations(tokens: &[String]) -> Result<Vec<SymmetryOperation>> {
    for name in [
        "_space_group_symop_operation_xyz",
        "_symmetry_equiv_pos_as_xyz",
    ] {
        if let Some(values) = read_loop_column(tokens, name) {
            return values.into_iter().map(parse_symmetry_operation).collect();
        }
    }
    Ok(Vec::new())
}

fn read_loop_column<'a>(tokens: &'a [String], name: &str) -> Option<Vec<&'a str>> {
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
        let Some(column) = find_header(headers, name) else {
            continue;
        };
        let width = headers.len();
        let mut values = Vec::new();
        while width > 0 && index + width <= tokens.len() {
            if tokens[index].eq_ignore_ascii_case("loop_") || tokens[index].starts_with('_') {
                break;
            }
            values.push(tokens[index + column].as_str());
            index += width;
        }
        return Some(values);
    }
    None
}

fn parse_symmetry_operation(input: &str) -> Result<SymmetryOperation> {
    let components = input.split(',').map(str::trim).collect::<Vec<_>>();
    if components.len() != 3 {
        bail!("invalid CIF symmetry operation `{input}`");
    }
    let mut operation = SymmetryOperation {
        matrix: [[0.0; 3]; 3],
        offset: [0.0; 3],
    };
    for (row, component) in components.into_iter().enumerate() {
        let (coefficients, offset) = parse_symmetry_component(component)?;
        operation.matrix[row] = coefficients;
        operation.offset[row] = offset;
    }
    Ok(operation)
}

fn parse_symmetry_component(input: &str) -> Result<([f32; 3], f32)> {
    let compact = input.replace(' ', "").to_ascii_lowercase();
    if compact.is_empty() {
        bail!("empty component in CIF symmetry operation");
    }
    let bytes = compact.as_bytes();
    let mut terms = Vec::new();
    let mut start = 0;
    for index in 1..bytes.len() {
        if bytes[index] == b'+' || bytes[index] == b'-' {
            terms.push(&compact[start..index]);
            start = index;
        }
    }
    terms.push(&compact[start..]);
    let mut coefficients = [0.0; 3];
    let mut offset = 0.0;
    for term in terms {
        let (sign, body) = match term.as_bytes().first() {
            Some(b'+') => (1.0, &term[1..]),
            Some(b'-') => (-1.0, &term[1..]),
            _ => (1.0, term),
        };
        if body.is_empty() {
            bail!("invalid component `{input}` in CIF symmetry operation");
        }
        if let Some((axis, position)) =
            body.char_indices()
                .find_map(|(position, value)| match value {
                    'x' => Some((0, position)),
                    'y' => Some((1, position)),
                    'z' => Some((2, position)),
                    _ => None,
                })
        {
            let factor = &body[..position];
            let factor = if factor.is_empty() {
                1.0
            } else {
                parse_fraction(factor)?
            };
            if position + 1 != body.len() {
                bail!("invalid variable term `{term}` in CIF symmetry operation");
            }
            coefficients[axis] += sign * factor;
        } else {
            offset += sign * parse_fraction(body)?;
        }
    }
    Ok((coefficients, offset))
}

fn parse_fraction(input: &str) -> Result<f32> {
    if let Some((numerator, denominator)) = input.split_once('/') {
        let numerator = numerator
            .parse::<f32>()
            .with_context(|| format!("invalid fraction `{input}`"))?;
        let denominator = denominator
            .parse::<f32>()
            .with_context(|| format!("invalid fraction `{input}`"))?;
        if denominator == 0.0 {
            bail!("invalid zero denominator in fraction `{input}`");
        }
        Ok(numerator / denominator)
    } else {
        input
            .parse::<f32>()
            .with_context(|| format!("invalid number `{input}`"))
    }
}

fn expand_atom_sites(
    sites: &[FractionalAtomSite],
    operations: &[SymmetryOperation],
    cell: &UnitCell,
) -> Vec<Atom> {
    let mut expanded: Vec<(String, [f32; 3])> = Vec::new();
    for site in sites {
        for operation in operations {
            let coordinates = operation.apply(site.coordinates);
            let duplicate = expanded.iter().any(|(element, existing)| {
                element == &site.element
                    && (0..3).all(|axis| {
                        let difference = (coordinates[axis] - existing[axis]).abs();
                        difference.min(1.0 - difference) < 1.0e-4
                    })
            });
            if !duplicate {
                expanded.push((site.element.clone(), coordinates));
            }
        }
    }
    expanded
        .into_iter()
        .map(|(element, [x, y, z])| Atom {
            element,
            position: cell.fractional_to_cartesian(x, y, z),
            charge: 0.0,
        })
        .collect()
}

fn is_p1(space_group: &str) -> bool {
    space_group
        .chars()
        .filter(|value| !value.is_ascii_whitespace())
        .collect::<String>()
        .eq_ignore_ascii_case("P1")
}
