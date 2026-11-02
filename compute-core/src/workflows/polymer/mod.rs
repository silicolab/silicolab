//! Build linear polymer chains from repeat-unit SMILES.
//!
//! A repeat unit is a monomer SMILES with two `[*]` attachment points
//! (`[*]CC[*]` is polyethylene, `[*]CC([*])c1ccccc1` polystyrene). The unit is
//! embedded once, then copied along the chain by self-avoiding growth
//! (`grow`); a copolymer is a sequence of unit labels. Every unit becomes one
//! residue of the result's [`Biopolymer`] so a later topology step can type the
//! chain unit by unit and renderers can colour it per residue.

mod grow;
mod unit;

use anyhow::{Result, bail};
use nalgebra::Point3;

use crate::domain::{
    Structure,
    biopolymer::{Biopolymer, ChainRecord, ResidueId, ResidueRecord},
};

pub use unit::RepeatUnit;

/// Chain id the single built chain is filed under.
const CHAIN_ID: char = 'A';

#[derive(Debug, Clone)]
pub struct PolymerSpec {
    /// Repeat units, labelled `A`, `B`, … in order.
    pub units: Vec<String>,
    /// Degree of polymerization: how many units the chain carries.
    pub degree: usize,
    /// Unit labels in chain order, cycled to `degree`; `None` means the
    /// homopolymer of the single unit.
    pub sequence: Option<String>,
    /// Residue name of every unit (homopolymer only); copolymer units are
    /// named `MOA`, `MOB`, … by label.
    pub residue_name: Option<String>,
    pub seed: u64,
    /// Relax each junction that docks with a hard clash with UFF while growing.
    pub relax: bool,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct PolymerReport {
    pub degree: usize,
    pub atom_count: usize,
    /// Non-bonded atom pairs closer than a hard clash limit after growth.
    pub clashes: usize,
    /// Root-mean-square distance of all atoms from the centroid (Å).
    pub radius_of_gyration: f32,
    /// Distance between the first unit's head atom and the last unit's tail (Å).
    pub end_to_end: f32,
    /// UFF could not relax some junctions (an element it lacks).
    pub relax_skipped: bool,
}

#[derive(Debug, Clone)]
pub struct PolymerResult {
    pub structure: Structure,
    pub report: PolymerReport,
}

pub fn build_polymer(spec: &PolymerSpec) -> Result<PolymerResult> {
    if spec.degree == 0 {
        bail!("degree of polymerization must be at least 1");
    }
    if spec.units.is_empty() {
        bail!("a polymer needs at least one repeat unit");
    }
    if spec.units.len() > 26 {
        bail!("at most 26 repeat units (labels A–Z) are supported");
    }
    let units: Vec<RepeatUnit> = spec
        .units
        .iter()
        .enumerate()
        .map(|(index, smiles)| RepeatUnit::parse((b'A' + index as u8) as char, smiles))
        .collect::<Result<_>>()?;
    let order = unit_order(&units, spec.sequence.as_deref(), spec.degree)?;

    let mut chain = grow::Chain::new(spec.seed, spec.relax);
    for (residue, &unit_index) in order.iter().enumerate() {
        let next = order.get(residue + 1).map(|&index| &units[index]);
        chain.add_unit(&units[unit_index], residue, next)?;
    }
    let end_to_end = chain
        .ends()
        .map(|(head, tail)| (chain.atoms[tail].position - chain.atoms[head].position).norm())
        .unwrap_or(0.0);

    let report = PolymerReport {
        degree: spec.degree,
        atom_count: chain.atoms.len(),
        clashes: chain.clash_count(),
        radius_of_gyration: radius_of_gyration(
            &chain.atoms.iter().map(|a| a.position).collect::<Vec<_>>(),
        ),
        end_to_end,
        relax_skipped: chain.relax_skipped,
    };

    let biopolymer = residue_annotation(&chain, &units, &order, spec.residue_name.as_deref());
    let mut structure = Structure::with_bonds(spec.title.clone(), chain.atoms, chain.bonds);
    structure.biopolymer = Some(biopolymer);
    Ok(PolymerResult { structure, report })
}

/// Unit index per residue: the sequence pattern cycled to `degree`.
fn unit_order(units: &[RepeatUnit], sequence: Option<&str>, degree: usize) -> Result<Vec<usize>> {
    let pattern: Vec<usize> = match sequence {
        None if units.len() == 1 => vec![0],
        None => bail!(
            "a copolymer with {} units needs --sequence (e.g. `AB` for alternating, `AAB`)",
            units.len()
        ),
        Some(text) => {
            let pattern: Vec<usize> = text
                .trim()
                .chars()
                .map(|label| {
                    units
                        .iter()
                        .position(|unit| unit.label == label.to_ascii_uppercase())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "sequence label `{label}` has no unit; units are labelled {}",
                                units
                                    .iter()
                                    .map(|u| u.label.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        })
                })
                .collect::<Result<_>>()?;
            if pattern.is_empty() {
                bail!("--sequence must name at least one unit");
            }
            pattern
        }
    };
    Ok((0..degree).map(|i| pattern[i % pattern.len()]).collect())
}

fn residue_annotation(
    chain: &grow::Chain,
    units: &[RepeatUnit],
    order: &[usize],
    residue_name: Option<&str>,
) -> Biopolymer {
    let residue_count = order.len();
    let mut residues: Vec<ResidueRecord> = (0..residue_count)
        .map(|residue| {
            let unit = &units[order[residue]];
            let name = match residue_name {
                Some(name) if units.len() == 1 => name.to_string(),
                _ => format!("MO{}", unit.label),
            };
            ResidueRecord {
                id: ResidueId::new(CHAIN_ID, residue as i32 + 1, ' '),
                residue_name: name,
                atom_indices: Vec::new(),
                alpha_carbon: None,
                backbone_nitrogen: None,
                backbone_carbon: None,
                backbone_oxygen: None,
                is_standard_amino_acid: false,
            }
        })
        .collect();
    for (atom, &residue) in chain.residue_of_atom.iter().enumerate() {
        residues[residue].atom_indices.push(atom);
    }
    Biopolymer {
        residues,
        chains: vec![ChainRecord {
            id: CHAIN_ID,
            residue_indices: (0..residue_count).collect(),
        }],
        secondary_structures: Vec::new(),
        residue_for_atom: chain.residue_of_atom.iter().map(|r| Some(*r)).collect(),
        atom_name_for_atom: chain.atom_names.iter().map(|n| Some(n.clone())).collect(),
    }
}

fn radius_of_gyration(positions: &[Point3<f32>]) -> f32 {
    if positions.is_empty() {
        return 0.0;
    }
    let centroid = positions
        .iter()
        .fold(nalgebra::Vector3::zeros(), |acc, p| acc + p.coords)
        / positions.len() as f32;
    (positions
        .iter()
        .map(|p| (p.coords - centroid).norm_squared())
        .sum::<f32>()
        / positions.len() as f32)
        .sqrt()
}

#[cfg(test)]
mod tests;
