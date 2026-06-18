//! Reading PDBQT text into [`Structure`]s.
//!
//! PDBQT atom records are PDB `ATOM`/`HETATM` lines with two extra trailing
//! columns: a partial charge (69-76) and an AutoDock atom type (78+). We read the
//! coordinate triple, the charge, and recover the element from the AutoDock type
//! (falling back to the PDB name columns). The torsion-tree keywords
//! (`ROOT`/`BRANCH`/`TORSDOF`/…) carry no atoms and are skipped. `MODEL`/`ENDMDL`
//! split a multi-pose file (e.g. a docking result) into one structure per pose.

use anyhow::{Result, bail};
use nalgebra::Point3;

use crate::domain::{Atom, Structure, chemistry::normalized_symbol};

use super::typing::element_for_ad;

/// Parse a PDBQT file into a single [`Structure`] (the first `MODEL` for a
/// multi-pose file).
pub fn parse_pdbqt(input: &str) -> Result<Structure> {
    let mut structures = parse_pdbqt_document(input)?;
    if structures.is_empty() {
        bail!("PDBQT file contains no atoms");
    }
    Ok(structures.swap_remove(0))
}

/// Parse a PDBQT file, preserving each `MODEL` block (Vina writes one per pose) as
/// a separate [`Structure`]. A file without `MODEL` records yields a single
/// structure.
pub fn parse_pdbqt_document(input: &str) -> Result<Vec<Structure>> {
    let mut models: Vec<Vec<Atom>> = Vec::new();
    let mut current: Vec<Atom> = Vec::new();
    let mut current_remark: Option<String> = None;
    let mut remarks: Vec<Option<String>> = Vec::new();

    for (line_index, raw) in input.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let tag = line.get(0..6).unwrap_or("").trim();
        match tag {
            "MODEL" => {
                // A new pose block begins; flush any atoms accumulated outside a
                // MODEL (defensive — well-formed Vina output opens MODEL first).
                if !current.is_empty() {
                    models.push(std::mem::take(&mut current));
                    remarks.push(current_remark.take());
                }
                current_remark = None;
            }
            "ENDMDL" => {
                models.push(std::mem::take(&mut current));
                remarks.push(current_remark.take());
            }
            "ATOM" | "HETATM" => {
                current.push(parse_atom_line(line, line_index + 1)?);
            }
            // Keep the first VINA RESULT remark as the model title hint.
            "REMARK" if current_remark.is_none() && line.contains("VINA RESULT") => {
                current_remark = Some(line["REMARK".len()..].trim().to_string());
            }
            _ => {}
        }
    }
    if !current.is_empty() {
        models.push(current);
        remarks.push(current_remark);
    }

    if models.is_empty() {
        bail!("PDBQT file contains no atom records");
    }

    Ok(models
        .into_iter()
        .enumerate()
        .map(|(index, atoms)| {
            let title = remarks
                .get(index)
                .and_then(|r| r.clone())
                .unwrap_or_else(|| "PDBQT structure".to_string());
            // Bonds are not stored in PDBQT; infer them from geometry for display
            // (the same approach the other readers rely on via `Structure::new`).
            Structure::new(title, atoms)
        })
        .collect())
}

/// Parse one PDBQT `ATOM`/`HETATM` line. Columns are 1-based inclusive (matching
/// the AutoDock/Vina fixed format); `line.get` keeps a short line from panicking.
fn parse_atom_line(line: &str, line_number: usize) -> Result<Atom> {
    let coord = |start: usize, end: usize, what: &str| -> Result<f32> {
        let field = line.get(start - 1..end).map(str::trim).unwrap_or("");
        field
            .parse::<f32>()
            .map_err(|_| anyhow::anyhow!("invalid {what} on PDBQT line {line_number}: {field:?}"))
    };
    let x = coord(31, 38, "x coordinate")?;
    let y = coord(39, 46, "y coordinate")?;
    let z = coord(47, 54, "z coordinate")?;

    let charge = line
        .get(69 - 1..76)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);

    let ad_token = line.get(78 - 1..).map(str::trim).unwrap_or("");
    let element = element_for_ad(ad_token)
        .map(str::to_string)
        .unwrap_or_else(|| element_from_name(line));

    Ok(Atom {
        element,
        position: Point3::new(x, y, z),
        charge,
    })
}

/// Fallback element recovery when the AutoDock type column is missing or unknown:
/// read the PDB element columns (77-78), else the leading letters of the atom name.
fn element_from_name(line: &str) -> String {
    let from_element_cols = line.get(77 - 1..78).map(str::trim).unwrap_or("");
    if !from_element_cols.is_empty() {
        let normalized = normalized_symbol(from_element_cols);
        if !normalized.is_empty() {
            return normalized;
        }
    }
    let name = line.get(13 - 1..16).map(str::trim).unwrap_or("");
    let letters: String = name.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if letters.is_empty() {
        "C".to_string()
    } else {
        normalized_symbol(&letters[..letters.len().min(2)])
    }
}
