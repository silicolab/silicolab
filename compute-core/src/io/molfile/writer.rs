use std::fmt::Write as _;

use anyhow::{Result, bail};

use super::syntax::{push_record, quote};
use crate::domain::BondType;
use crate::domain::chemistry::is_element_symbol;
use crate::domain::sketch::{Bracket, RepeatConnectivity, RepeatUnit, Sketch, implicit_hydrogens};

const MAX_VALENCE: i32 = 14;
const MAX_CHARGE: i32 = 15;

/// Render a sketch as a V3000 molfile. `title` becomes the header's first line.
pub fn write_molfile(sketch: &Sketch, title: &str) -> Result<String> {
    sketch.validate_repeat_units()?;
    if sketch
        .bonds
        .iter()
        .any(|bond| bond.a >= sketch.atoms.len() || bond.b >= sketch.atoms.len())
    {
        bail!("sketch has a bond to a missing atom");
    }
    if let Some(atom) = sketch
        .atoms
        .iter()
        .find(|atom| atom.charge.abs() > MAX_CHARGE)
    {
        bail!("charge {} is out of range", atom.charge);
    }
    if let Some(atom) = sketch
        .atoms
        .iter()
        .find(|atom| !atom.is_attachment() && !is_element_symbol(&atom.element))
    {
        bail!("`{}` is not an element", atom.element);
    }

    let mut output = String::new();
    writeln!(output, "{}", title.replace(['\r', '\n'], " "))?;
    writeln!(output, "{:20}2D", "")?;
    output.push('\n');
    output.push_str("  0  0  0     0  0            999 V3000\n");
    push_record(&mut output, "BEGIN CTAB");
    push_record(
        &mut output,
        &format!(
            "COUNTS {} {} {} 0 0",
            sketch.atoms.len(),
            sketch.bonds.len(),
            sketch.repeat_units.len()
        ),
    );

    push_record(&mut output, "BEGIN ATOM");
    for index in 0..sketch.atoms.len() {
        push_record(&mut output, &atom_record(sketch, index)?);
    }
    push_record(&mut output, "END ATOM");

    if !sketch.bonds.is_empty() {
        push_record(&mut output, "BEGIN BOND");
        for (index, bond) in sketch.bonds.iter().enumerate() {
            let order = match bond.order {
                BondType::Single => 1,
                BondType::Double => 2,
                BondType::Triple => 3,
                BondType::Aromatic => 4,
            };
            push_record(
                &mut output,
                &format!("{} {order} {} {}", index + 1, bond.a + 1, bond.b + 1),
            );
        }
        push_record(&mut output, "END BOND");
    }

    if !sketch.repeat_units.is_empty() {
        push_record(&mut output, "BEGIN SGROUP");
        for (index, unit) in sketch.repeat_units.iter().enumerate() {
            push_record(&mut output, &sgroup_record(sketch, index + 1, unit)?);
        }
        push_record(&mut output, "END SGROUP");
    }

    push_record(&mut output, "END CTAB");
    output.push_str("M  END\n");
    Ok(output)
}

fn atom_record(sketch: &Sketch, index: usize) -> Result<String> {
    let atom = &sketch.atoms[index];
    let mut record = format!(
        "{} {} {:.4} {:.4} 0 0",
        index + 1,
        atom.element,
        atom.pos.x,
        atom.pos.y
    );
    if atom.charge != 0 {
        write!(record, " CHG={}", atom.charge)?;
    }
    if let Some(valence) = pinned_valence(sketch, index) {
        if valence > MAX_VALENCE {
            bail!("atom {} has too many hydrogens to write", index + 1);
        }
        write!(record, " VAL={valence}")?;
    }
    Ok(record)
}

/// The `VAL` that reproduces a pinned hydrogen count the valence model would not
/// derive by itself. Molfile spells zero total valence `-1`, since `0` means
/// the default.
fn pinned_valence(sketch: &Sketch, index: usize) -> Option<i32> {
    let atom = &sketch.atoms[index];
    let pinned = atom.explicit_hydrogens?;
    let sum = sketch.bond_order_sum(index);
    if atom.is_attachment() || pinned == implicit_hydrogens(&atom.element, atom.charge, sum) {
        return None;
    }
    let hydrogens = i32::try_from(pinned).unwrap_or(i32::MAX);
    match (sum.round() as i32).saturating_add(hydrogens) {
        0 => Some(-1),
        valence => Some(valence),
    }
}

fn sgroup_record(sketch: &Sketch, number: usize, unit: &RepeatUnit) -> Result<String> {
    if unit.label.contains(['\r', '\n']) {
        bail!("repeat unit {number} has a line break in its label");
    }
    let atoms = id_list(&unit.atoms);
    let bonds = id_list(&unit.crossing_bonds);
    let mut record = format!("{number} SRU 0 ATOMS={atoms} XBONDS={bonds}");
    if !unit.label.is_empty() {
        write!(record, " LABEL={}", quote(&unit.label))?;
    }
    let connect = match unit.connectivity {
        RepeatConnectivity::HeadToTail => "HT",
        RepeatConnectivity::HeadToHead => "HH",
        RepeatConnectivity::EitherUnknown => "EU",
    };
    write!(record, " CONNECT={connect}")?;
    let fitted;
    let brackets = if unit.brackets.is_empty() {
        fitted = sketch.fitted_brackets(unit);
        &fitted
    } else {
        &unit.brackets
    };
    for bracket in brackets {
        record.push_str(&bracket_field(bracket));
    }
    Ok(record)
}

fn id_list(indices: &[usize]) -> String {
    let ids: Vec<String> = indices
        .iter()
        .map(|index| (index + 1).to_string())
        .collect();
    format!("({} {})", ids.len(), ids.join(" "))
}

fn bracket_field([first, second]: &Bracket) -> String {
    format!(
        " BRKXYZ=(9 {:.4} {:.4} 0 {:.4} {:.4} 0 0 0 0)",
        first.x, first.y, second.x, second.y
    )
}
