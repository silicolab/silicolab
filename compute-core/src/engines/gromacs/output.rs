//! Parsers for GROMACS output files.
//!
//! Coordinates from `gmx mdrun -c` are lifted into a [`Structure`] through the
//! GRO parser while preserving the bond topology of the original input.

use std::{fs, path::Path};

use anyhow::{Context, Result};

use crate::{domain::Structure, io::formats::gro::parse_gro};

/// Load `out.gro`-style coordinates produced by GROMACS, then graft the bond
/// topology from `original` so chemistry (bond orders, biopolymer metadata) is
/// preserved across the engine round-trip.
pub fn load_minimized_structure(path: &Path, original: &Structure) -> Result<Structure> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read GROMACS output {}", path.display()))?;
    let mut updated = parse_gro(&source)
        .with_context(|| format!("failed to parse GROMACS output {}", path.display()))?;
    graft_topology(&mut updated, original);
    Ok(updated)
}

fn graft_topology(updated: &mut Structure, original: &Structure) {
    if updated.atoms.len() == original.atoms.len() {
        for (new_atom, old_atom) in updated.atoms.iter_mut().zip(original.atoms.iter()) {
            new_atom.element = old_atom.element.clone();
            new_atom.charge = old_atom.charge;
        }
        updated.bonds = original.bonds.clone();
        updated.biopolymer = original.biopolymer.clone();
    }
    if !original.title.trim().is_empty() {
        updated.title = original.title.clone();
    }
}

/// Extract the final potential energy (kJ/mol) from a `mdrun` log.
///
/// GROMACS' steepest-descent block prints `Potential Energy  =  ...` lines
/// during and at the end of minimization. Returns the last occurrence, or
/// `None` if no match is found.
pub fn parse_final_potential_energy(mdrun_log: &str) -> Option<f64> {
    let mut latest = None;
    for line in mdrun_log.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("Potential Energy") else {
            continue;
        };
        let value = rest
            .trim_start_matches(|c: char| c == '=' || c.is_whitespace())
            .split_whitespace()
            .next()?;
        if let Ok(parsed) = value.parse::<f64>() {
            latest = Some(parsed);
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::Point3;

    use crate::domain::{Atom, Structure, UnitCell};

    #[test]
    fn graft_topology_preserves_bonds_when_counts_match() {
        let original = Structure::with_cell_and_bonds(
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
            vec![crate::domain::Bond::with_type(
                0,
                1,
                crate::domain::BondType::Double,
            )],
            UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0),
        );

        let mut updated = Structure::with_cell(
            "after-em",
            vec![
                Atom {
                    element: "X".to_string(),
                    position: Point3::new(0.01, 0.0, 0.0),
                    charge: 0.0,
                },
                Atom {
                    element: "X".to_string(),
                    position: Point3::new(1.33, 0.0, 0.0),
                    charge: 0.0,
                },
            ],
            UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0),
        );

        graft_topology(&mut updated, &original);

        assert_eq!(updated.atoms[0].element, "C");
        assert_eq!(updated.bonds.len(), 1);
        assert_eq!(updated.title, "ethene");
    }

    #[test]
    fn parses_potential_energy_lines() {
        let log = "\
Step           Time
   100         1.0
   Potential Energy  =  -1.2345e+03
   Step           Time
   200         2.0
   Potential Energy  =  -2.3456e+03
";
        let energy = parse_final_potential_energy(log).expect("energy");
        assert!((energy + 2345.6).abs() < 1.0e-3);
    }
}
