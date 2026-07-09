use anyhow::{Context, Result, anyhow, bail};
use nalgebra::Point3;

use crate::{
    domain::{Atom, Bond, BondType, Structure, UnitCell},
    io::sdfrust_bridge,
};

const DEFAULT_MOL2_SPACE_GROUP_NUMBER: u16 = 1;
const DEFAULT_MOL2_SPACE_GROUP_SETTING: u8 = 1;

#[derive(Debug, Clone)]
pub struct Mol2Document {
    pub title: String,
    pub atoms: Vec<Mol2Atom>,
    pub bonds: Vec<Mol2Bond>,
    pub crysin: Option<Mol2Crysin>,
}

#[derive(Debug, Clone)]
pub struct Mol2Atom {
    pub element: String,
    pub position: Point3<f32>,
    pub charge: f32,
}

#[derive(Debug, Clone)]
pub struct Mol2Bond {
    pub a: usize,
    pub b: usize,
    pub bond_type: BondType,
}

#[derive(Debug, Clone)]
pub struct Mol2Crysin {
    pub cell: UnitCell,
    pub space_group_number: u16,
    pub setting: u8,
}

pub fn parse_mol2(input: &str) -> Result<Structure> {
    let document = parse_mol2_document(input)?;
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

/// Split a multi-molecule MOL2 file into one structure per `@<TRIPOS>MOLECULE`
/// record. A single-molecule file yields exactly one.
pub fn parse_mol2_records(input: &str) -> Result<Vec<Structure>> {
    let lines = input.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with("@<TRIPOS>MOLECULE"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if starts.is_empty() {
        bail!("MOL2 input contains no @<TRIPOS>MOLECULE record");
    }

    starts
        .iter()
        .enumerate()
        .map(|(position, &start)| {
            let end = starts.get(position + 1).copied().unwrap_or(lines.len());
            parse_mol2(&format!("{}\n", lines[start..end].join("\n")))
        })
        .collect()
}

pub fn to_mol2(structure: &Structure) -> String {
    let title = if structure.title.trim().is_empty() {
        "MOL2 structure"
    } else {
        structure.title.trim()
    };
    let mut output = format!(
        "@<TRIPOS>MOLECULE\n{}\n{} {}\nSMALL\nUSER_CHARGES\n\n",
        title,
        structure.atoms.len(),
        structure.bonds.len()
    );

    if let Some(cell) = &structure.cell {
        let crysin = default_crysin_for_cell(cell);
        output.push_str("@<TRIPOS>CRYSIN\n");
        output.push_str(&format!(
            "{:>12.6} {:>12.6} {:>12.6} {:>12.6} {:>12.6} {:>12.6} {} {}\n\n",
            crysin.cell.a,
            crysin.cell.b,
            crysin.cell.c,
            crysin.cell.alpha,
            crysin.cell.beta,
            crysin.cell.gamma,
            crysin.space_group_number,
            crysin.setting
        ));
    }

    output.push_str("@<TRIPOS>ATOM\n");
    for (index, atom) in structure.atoms.iter().enumerate() {
        output.push_str(&format!(
            "{:>6} {:<4} {:>12.6} {:>12.6} {:>12.6} {:<4} 1 MOL {:>10.6}\n",
            index + 1,
            format!("{}{}", atom.element, index + 1),
            atom.position.x,
            atom.position.y,
            atom.position.z,
            mol2_atom_type(&atom.element),
            atom.charge
        ));
    }

    output.push_str("@<TRIPOS>BOND\n");
    for (index, bond) in structure.bonds.iter().enumerate() {
        output.push_str(&format!(
            "{:>6} {:>6} {:>6} {}\n",
            index + 1,
            bond.a + 1,
            bond.b + 1,
            mol2_bond_type(bond.bond_type)
        ));
    }

    output
}

pub fn parse_mol2_document(input: &str) -> Result<Mol2Document> {
    let sections = collect_sections(input);
    let standard_mol2 = standard_tripos_mol2(&sections);
    if standard_mol2.trim().is_empty() {
        bail!("MOL2 input does not contain TRIPOS base sections");
    }

    let molecule = sdfrust_bridge::parse_mol2_string(&standard_mol2)
        .context("failed to parse TRIPOS MOL2 sections with sdfrust")?;
    let partial_charges = sections
        .iter()
        .find(|section| section.name_upper == "@<TRIPOS>ATOM")
        .map(parse_atom_section_partial_charges)
        .transpose()?;
    let atoms = molecule
        .atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| Mol2Atom {
            element: atom.element.clone(),
            position: Point3::new(atom.x as f32, atom.y as f32, atom.z as f32),
            charge: partial_charges
                .as_ref()
                .and_then(|charges| charges.get(index).copied())
                .unwrap_or(atom.formal_charge as f32),
        })
        .collect::<Vec<_>>();
    let bonds = sdfrust_bridge::bonds_from_molecule(&molecule)
        .into_iter()
        .map(|bond| Mol2Bond {
            a: bond.a,
            b: bond.b,
            bond_type: bond.bond_type,
        })
        .collect::<Vec<_>>();

    let crysin = sections
        .iter()
        .find(|section| section.name_upper == "@<TRIPOS>CRYSIN")
        .map(parse_crysin_section)
        .transpose()?;
    Ok(Mol2Document {
        title: default_mol2_title(&molecule.name),
        atoms,
        bonds,
        crysin,
    })
}

#[derive(Debug)]
struct Mol2Section<'a> {
    name_upper: String,
    lines: Vec<&'a str>,
}

fn collect_sections(input: &str) -> Vec<Mol2Section<'_>> {
    let mut sections = Vec::new();
    let mut current_name = None::<String>;
    let mut current_lines = Vec::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@<") {
            if let Some(name_upper) = current_name.take() {
                sections.push(Mol2Section {
                    name_upper,
                    lines: std::mem::take(&mut current_lines),
                });
            }
            current_name = Some(trimmed.to_ascii_uppercase());
        }

        if current_name.is_some() {
            current_lines.push(line);
        }
    }

    if let Some(name_upper) = current_name {
        sections.push(Mol2Section {
            name_upper,
            lines: current_lines,
        });
    }

    sections
}

fn standard_tripos_mol2(sections: &[Mol2Section<'_>]) -> String {
    let mut output = String::new();

    for section in sections {
        if matches!(
            section.name_upper.as_str(),
            "@<TRIPOS>MOLECULE" | "@<TRIPOS>ATOM" | "@<TRIPOS>BOND"
        ) {
            for line in &section.lines {
                output.push_str(line);
                output.push('\n');
            }
            output.push('\n');
        }
    }

    output
}

fn default_mol2_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "MOL2 structure".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_atom_section_partial_charges(section: &Mol2Section<'_>) -> Result<Vec<f32>> {
    let mut charges = Vec::new();

    for line in section.lines.iter().skip(1).map(|line| line.trim()) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 {
            bail!("MOL2 ATOM line must contain at least atom id, name, x, y, z, atom type");
        }

        let charge = fields
            .get(8)
            .map(|value| {
                value
                    .parse::<f32>()
                    .with_context(|| format!("invalid MOL2 partial charge `{value}`"))
            })
            .transpose()?
            .unwrap_or(0.0);
        charges.push(charge);
    }

    Ok(charges)
}

fn parse_crysin_section(section: &Mol2Section<'_>) -> Result<Mol2Crysin> {
    let mut crysin_line = None;
    for line in section.lines.iter().skip(1).map(|line| line.trim()) {
        if !line.is_empty() && !line.starts_with('#') && crysin_line.replace(line).is_some() {
            bail!("MOL2 CRYSIN section must contain exactly one data line");
        }
    }

    let line = crysin_line.ok_or_else(|| anyhow!("MOL2 CRYSIN section is empty"))?;
    parse_crysin_line(line)
}

fn parse_crysin_line(line: &str) -> Result<Mol2Crysin> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 8 {
        bail!("MOL2 CRYSIN line must contain a, b, c, alpha, beta, gamma, space group, setting");
    }

    let a = fields[0]
        .parse::<f32>()
        .context("invalid MOL2 cell length a")?;
    let b = fields[1]
        .parse::<f32>()
        .context("invalid MOL2 cell length b")?;
    let c = fields[2]
        .parse::<f32>()
        .context("invalid MOL2 cell length c")?;
    let alpha = fields[3]
        .parse::<f32>()
        .context("invalid MOL2 cell angle alpha")?;
    let beta = fields[4]
        .parse::<f32>()
        .context("invalid MOL2 cell angle beta")?;
    let gamma = fields[5]
        .parse::<f32>()
        .context("invalid MOL2 cell angle gamma")?;
    let space_group_number = fields[6]
        .parse::<u16>()
        .context("invalid MOL2 space group number")?;
    let setting = fields[7]
        .parse::<u8>()
        .context("invalid MOL2 space group setting")?;

    Ok(Mol2Crysin {
        cell: UnitCell::from_parameters(a, b, c, alpha, beta, gamma),
        space_group_number,
        setting,
    })
}

fn mol2_bond_type(bond_type: BondType) -> &'static str {
    bond_type.to_mol2_token()
}

fn mol2_atom_type(element: &str) -> &'static str {
    match element {
        "C" => "C.3",
        "N" => "N.3",
        "O" => "O.3",
        "H" => "H",
        "F" => "F",
        _ => "Du",
    }
}

fn default_crysin_for_cell(cell: &UnitCell) -> Mol2Crysin {
    // `Structure` only stores metric cell parameters, so emit the least-assuming
    // symmetry metadata when serializing back to MOL2.
    Mol2Crysin {
        cell: cell.clone(),
        space_group_number: DEFAULT_MOL2_SPACE_GROUP_NUMBER,
        setting: DEFAULT_MOL2_SPACE_GROUP_SETTING,
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::Point3;

    use super::{parse_mol2, parse_mol2_document, to_mol2};
    use crate::domain::UnitCell;

    #[test]
    fn parses_atoms_bonds_and_charges() {
        let input = "\
@<TRIPOS>MOLECULE
test
2 1
SMALL
USER_CHARGES
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.ar 1 RES 0.0
2 H1 1.0 0.0 0.0 H 1 RES 0.125
@<TRIPOS>BOND
1 1 2 1
";

        let structure = parse_mol2(input).expect("structure");

        assert_eq!(structure.atoms.len(), 2);
        assert_eq!(structure.bonds.len(), 1);
        assert!((structure.atoms[1].charge - 0.125).abs() < 0.0001);
    }

    #[test]
    fn preserves_aromatic_bonds_from_mol2() {
        let structure = parse_mol2(
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

        assert_eq!(
            structure.bonds[0].bond_type,
            crate::domain::BondType::Aromatic
        );
    }

    #[test]
    fn writes_aromatic_bonds_as_ar() {
        let structure = parse_mol2(
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
        let output = to_mol2(&structure);

        assert!(
            output.contains("1      1      2 ar") || output.contains("     1      1      2 ar")
        );
    }

    #[test]
    fn parses_crysin_section_into_document_and_structure_cell() {
        let input = "\
@<TRIPOS>MOLECULE
cell
1 0
SMALL
USER_CHARGES

@<TRIPOS>CRYSIN
12.312000 4.959000 15.876000 90.000000 99.070000 90.000000 4 1
@<TRIPOS>ATOM
1 C1 0.0 0.0 0.0 C.3 1 RES 0.0
";

        let document = parse_mol2_document(input).expect("document");
        let crysin = document.crysin.as_ref().expect("crysin");
        assert!((crysin.cell.a - 12.312).abs() < 0.0001);
        assert!((crysin.cell.beta - 99.07).abs() < 0.0001);
        assert_eq!(crysin.space_group_number, 4);
        assert_eq!(crysin.setting, 1);

        let structure = parse_mol2(input).expect("structure");
        let cell = structure.cell.as_ref().expect("structure cell");
        assert!((cell.c - 15.876).abs() < 0.0001);
        assert!((cell.gamma - 90.0).abs() < 0.0001);
    }

    #[test]
    fn writes_crysin_for_periodic_structures() {
        let structure = crate::domain::Structure::with_cell(
            "cell",
            vec![crate::domain::Atom {
                element: "C".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            }],
            UnitCell::from_parameters(12.312, 4.959, 15.876, 90.0, 99.07, 90.0),
        );

        let output = to_mol2(&structure);

        assert!(output.contains("@<TRIPOS>CRYSIN"));
        assert!(output.contains("12.312000"));
        assert!(output.contains("99.070000"));
        assert!(output.contains("1 1"));
    }
}
