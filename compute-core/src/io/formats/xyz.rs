use anyhow::{Context, Result, bail};

use crate::{domain::Structure, io::sdfrust_bridge};

pub fn parse_xyz(input: &str) -> Result<Structure> {
    let molecule = sdfrust_bridge::parse_xyz_string(input)
        .context("failed to parse XYZ input with sdfrust")?;
    let atoms = sdfrust_bridge::atoms_from_molecule(&molecule)?;
    let title = molecule.name.trim().to_string();

    Ok(Structure::new(title, atoms))
}

/// Split a multi-record XYZ file (the usual multi-frame convention: an atom
/// count, a comment, then that many atom lines, repeated) into one structure per
/// record. A plain single-record file yields exactly one.
pub fn parse_xyz_records(input: &str) -> Result<Vec<Structure>> {
    let lines = input.lines().collect::<Vec<_>>();
    let mut structures = Vec::new();
    let mut cursor = 0;

    while cursor < lines.len() {
        if lines[cursor].trim().is_empty() {
            cursor += 1;
            continue;
        }
        let count = lines[cursor].trim().parse::<usize>().with_context(|| {
            format!(
                "XYZ record at line {} must begin with an atom count",
                cursor + 1
            )
        })?;
        let end = cursor + 2 + count;
        if end > lines.len() {
            bail!("XYZ record at line {} is truncated", cursor + 1);
        }
        structures.push(parse_xyz(&format!("{}\n", lines[cursor..end].join("\n")))?);
        cursor = end;
    }

    if structures.is_empty() {
        bail!("XYZ input contains no records");
    }
    Ok(structures)
}

#[cfg(test)]
mod tests {
    use super::{parse_xyz, parse_xyz_records};

    #[test]
    fn parses_every_record_of_a_multi_frame_file() {
        let records = parse_xyz_records(
            "\
1
first
He 0 0 0
2
second
He 1 0 0
He 2 0 0
",
        )
        .expect("multi-record xyz");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].title, "first");
        assert_eq!(records[1].atoms.len(), 2);
    }

    #[test]
    fn single_record_file_yields_one_structure() {
        let records = parse_xyz_records("1\nhelium\nHe 1 2 3\n").expect("xyz");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].title, "helium");
    }

    #[test]
    fn truncated_record_is_an_error_not_a_partial_structure() {
        assert!(parse_xyz_records("3\nwater\nO 0 0 0\n").is_err());
    }

    #[test]
    fn parses_water_and_infers_two_bonds() {
        let structure = parse_xyz(
            "\
3
water
O 0.0 0.0 0.0
H 0.9572 0.0 0.0
H -0.239987 0.927297 0.0
",
        )
        .expect("valid xyz");

        assert_eq!(structure.title, "water");
        assert_eq!(structure.atoms.len(), 3);
        assert_eq!(structure.bonds.len(), 2);
    }
}
