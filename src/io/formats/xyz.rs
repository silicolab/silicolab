use anyhow::{Context, Result};

use crate::{domain::Structure, io::sdfrust_bridge};

pub fn parse_xyz(input: &str) -> Result<Structure> {
    let molecule = sdfrust_bridge::parse_xyz_string(input)
        .context("failed to parse XYZ input with sdfrust")?;
    let atoms = sdfrust_bridge::atoms_from_molecule(&molecule)?;
    let title = molecule.name.trim().to_string();

    Ok(Structure::new(title, atoms))
}

#[cfg(test)]
mod tests {
    use super::parse_xyz;

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
