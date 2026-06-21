use anyhow::{Context, Result, anyhow};
use nalgebra::Point3;
use sdfrust::{BondOrder, Molecule};

use crate::domain::{Atom, Bond, BondType};

pub fn parse_mol2_string(input: &str) -> Result<Molecule> {
    sdfrust::parse_mol2_string(input).map_err(|error| anyhow!(error))
}

pub fn parse_xyz_string(input: &str) -> Result<Molecule> {
    sdfrust::parse_xyz_string(input).map_err(|error| anyhow!(error))
}

pub fn atoms_from_molecule(molecule: &Molecule) -> Result<Vec<Atom>> {
    molecule
        .atoms
        .iter()
        .map(|atom| {
            Ok(Atom {
                element: atom.element.clone(),
                position: Point3::new(
                    f32_from_f64(atom.x, "atom x")?,
                    f32_from_f64(atom.y, "atom y")?,
                    f32_from_f64(atom.z, "atom z")?,
                ),
                charge: atom.formal_charge as f32,
            })
        })
        .collect()
}

pub fn bonds_from_molecule(molecule: &Molecule) -> Vec<Bond> {
    molecule
        .bonds
        .iter()
        .map(|bond| Bond::with_type(bond.atom1, bond.atom2, bond_type_from_sdfrust(bond.order)))
        .collect()
}

fn bond_type_from_sdfrust(order: BondOrder) -> BondType {
    match order {
        BondOrder::Single => BondType::Single,
        BondOrder::Double => BondType::Double,
        BondOrder::Triple => BondType::Triple,
        BondOrder::Aromatic => BondType::Aromatic,
        BondOrder::SingleOrDouble
        | BondOrder::SingleOrAromatic
        | BondOrder::DoubleOrAromatic
        | BondOrder::Any
        | BondOrder::Coordination
        | BondOrder::Hydrogen => BondType::Single,
    }
}

fn f32_from_f64(value: f64, label: &str) -> Result<f32> {
    if value.is_finite() && value >= f32::MIN as f64 && value <= f32::MAX as f64 {
        Ok(value as f32)
    } else {
        Err(anyhow!("sdfrust {label} is out of range for f32"))
    }
    .with_context(|| format!("failed to convert {label}"))
}
