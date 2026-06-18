//! AutoDock atom typing and PDBQT `ATOM` line formatting.
//!
//! The `docking` crate (a Vina reimplementation) consumes *prepared* PDBQT: each
//! atom carries an AutoDock atom-type token (column 78+) and the crate re-derives
//! bonds and XS scoring types from geometry. Crucially, Vina scoring uses only XS
//! atom types and distances — never the partial-charge column — so charges are
//! emitted as a passthrough (or zero) and never affect a docking result. The only
//! chemistry that matters is the AD type, from which the crate infers
//! donor/acceptor/hydrophobic character (see the crate's `model::initialize`).
//!
//! Typing here is a best-effort derivation from the element + bond graph, so it is
//! flagged "approximate" when the source was not an already-prepared PDBQT. The
//! pieces that are genuinely heuristic are nitrogen acceptor assignment and
//! aromatic-carbon detection; everything else is determined by the element.

use crate::domain::{BondType, Structure, chemistry::normalized_symbol};

/// How an atom maps to the PDBQT output.
pub(super) enum AdAssignment {
    /// Emit the atom with this AutoDock type token (column 78+).
    Emit(&'static str),
    /// A non-polar hydrogen: omit it ("merge non-polar hydrogens", the AutoDock
    /// convention). Charges are irrelevant to Vina, so nothing is folded in.
    DropHydrogen,
}

/// Adjacency built from `structure.bonds`: `(neighbor_index, bond_type)`.
pub(super) type Neighbors = Vec<Vec<(usize, BondType)>>;

pub(super) fn neighbors_of(structure: &Structure) -> Neighbors {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        if bond.a < neighbors.len() && bond.b < neighbors.len() {
            neighbors[bond.a].push((bond.b, bond.bond_type));
            neighbors[bond.b].push((bond.a, bond.bond_type));
        }
    }
    neighbors
}

/// True if the normalized element is a hydrogen (treating deuterium as H).
pub(super) fn is_hydrogen(element: &str) -> bool {
    matches!(normalized_symbol(element).as_str(), "H" | "D")
}

/// Assign the AutoDock type token for atom `index`, or report an unsupported
/// element. Errors (rather than emitting an invalid token) so a malformed ligand
/// fails before the long search instead of being silently mis-typed.
pub(super) fn classify(
    structure: &Structure,
    neighbors: &Neighbors,
    index: usize,
) -> anyhow::Result<AdAssignment> {
    let element = normalized_symbol(&structure.atoms[index].element);
    let bonded = &neighbors[index];

    let aromatic = bonded.iter().any(|(_, t)| *t == BondType::Aromatic);
    let multiple_bond = bonded
        .iter()
        .any(|(_, t)| matches!(t, BondType::Double | BondType::Triple));
    // Total connectivity (heavy + H), used for the nitrogen lone-pair heuristic.
    let degree = bonded.len();

    let token: &'static str = match element.as_str() {
        "H" | "D" => {
            // Polar (bonded to N/O/S) → keep as a donor hydrogen `HD`; otherwise
            // a non-polar hydrogen, which AutoDock merges away.
            let polar = bonded.iter().any(|(j, _)| {
                matches!(
                    normalized_symbol(&structure.atoms[*j].element).as_str(),
                    "N" | "O" | "S"
                )
            });
            return Ok(if polar {
                AdAssignment::Emit("HD")
            } else {
                AdAssignment::DropHydrogen
            });
        }
        "C" => {
            if aromatic {
                "A"
            } else {
                "C"
            }
        }
        // Oxygen is an H-bond acceptor in essentially every organic environment
        // (carbonyl, hydroxyl, ether, carboxylate): type it `OA`. The crate adds
        // the donor flag separately when a polar H is bonded.
        "O" => "OA",
        "N" => {
            // A nitrogen exposes a lone pair (acceptor `NA`) when it is sp2/sp
            // (a multiple bond) or under-coordinated; a fully substituted, all-single
            // nitrogen (amine/amide, degree 3) is a donor/neutral `N`.
            let acceptor = if aromatic {
                degree <= 2
            } else {
                multiple_bond || degree <= 2
            };
            if acceptor { "NA" } else { "N" }
        }
        "S" => "S",
        "P" => "P",
        "F" => "F",
        "Cl" => "Cl",
        "Br" => "Br",
        "I" => "I",
        "Si" => "Si",
        "At" => "At",
        // Metals recognized by the AutoDock4 type table.
        "Mg" => "Mg",
        "Mn" => "Mn",
        "Zn" => "Zn",
        "Ca" => "Ca",
        "Fe" => "Fe",
        // Metals the crate accepts as generic metal donors (`is_non_ad_metal_name`).
        "Cu" => "Cu",
        "Na" => "Na",
        "K" => "K",
        "Hg" => "Hg",
        "Co" => "Co",
        "Cd" => "Cd",
        "Ni" => "Ni",
        "U" => "U",
        // Selenium aliases to sulfur in the crate's type table.
        "Se" => "Se",
        other => anyhow::bail!(
            "element `{other}` is not supported by the Vina atom typing; \
             prepare the molecule with a supported element set first"
        ),
    };
    Ok(AdAssignment::Emit(token))
}

/// Map an AutoDock type token back to an element symbol (for parsing PDBQT poses
/// into a `Structure`). Unknown tokens fall back to `None` so the caller can try
/// the PDB element/name columns instead.
pub(super) fn element_for_ad(token: &str) -> Option<&'static str> {
    Some(match token {
        "C" | "A" | "CG0" | "CG1" | "CG2" | "CG3" => "C",
        "N" | "NA" | "NS" => "N",
        "O" | "OA" | "OS" => "O",
        "S" | "SA" => "S",
        "P" => "P",
        "H" | "HD" | "HS" => "H",
        "F" => "F",
        "Cl" | "CL" => "Cl",
        "Br" | "BR" => "Br",
        "I" => "I",
        "Si" => "Si",
        "At" => "At",
        "Mg" | "MG" => "Mg",
        "Mn" | "MN" => "Mn",
        "Zn" | "ZN" => "Zn",
        "Ca" | "CA" => "Ca",
        "Fe" | "FE" => "Fe",
        "Cu" | "CU" => "Cu",
        "Na" | "NA " => "Na",
        "K" => "K",
        "Hg" | "HG" => "Hg",
        "Co" | "CO" => "Co",
        "Cd" | "CD" => "Cd",
        "Ni" | "NI" => "Ni",
        "U" => "U",
        "Se" => "Se",
        _ => return None,
    })
}

/// Format one PDBQT `ATOM` record. Column layout matches the AutoDock/Vina fixed
/// format the crate's parser reads: serial 7-11, x/y/z 31-54, charge 69-76, and
/// the AutoDock type from column 78. The partial charge is emitted but ignored by
/// Vina scoring.
pub(super) fn atom_line(
    serial: usize,
    element: &str,
    position: [f32; 3],
    charge: f32,
    ad: &str,
) -> String {
    // A short atom name in the PDB name field (columns 13-16); the crate's parser
    // ignores it, so element+serial is sufficient and keeps the field ≤ 4 chars.
    let mut name = format!("{element}{}", serial % 1000);
    if name.len() > 4 {
        name = name[name.len() - 4..].to_string();
    }
    // The charge column (69-76) is read by the parser but ignored by Vina scoring.
    // Keep it within its 6-char field so the fixed columns never shift; a partial
    // charge that does not fit (none should, they are in roughly [-1, 1]) falls
    // back to zero rather than corrupting the line.
    let mut charge_field = format!("{charge:.3}");
    if charge_field.len() > 6 {
        charge_field = "0.000".to_string();
    }
    format!(
        "ATOM  {serial:>5} {name:<4} LIG     1    {x:>8.3}{y:>8.3}{z:>8.3}  1.00  0.00    {charge_field:>6} {ad}",
        x = position[0],
        y = position[1],
        z = position[2],
    )
}
