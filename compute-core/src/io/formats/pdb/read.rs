use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow, bail};
use nalgebra::Point3;

use crate::domain::{
    Atom, Bond, BondType, PdbAtomAnnotation, ResidueId, SecondaryStructureKind,
    SecondaryStructureSpan, Structure, UnitCell, build_biopolymer,
    chemistry::{infer_bonds_with_cell, normalized_symbol},
};

use super::PdbDocument;
use super::fields::{
    element_from_atom_name, field, find_coordinate_start, infer_residue_fields, ordered_pair,
    parse_fixed_width_i32_or_default, parse_fixed_width_usize_or_fallback,
};

/// Parse a PDB file into a single [`Structure`]. For multi-model (NMR)
/// depositions only the first model is returned; use [`parse_pdb_document`] to
/// access every conformer.
pub fn parse_pdb(input: &str) -> Result<Structure> {
    let mut document = parse_pdb_document(input)?;
    Ok(document.models.swap_remove(0))
}

/// Parse a PDB file, preserving each `MODEL` block as a separate structure
/// along with the deposition title and PDB identifier.
pub fn parse_pdb_document(input: &str) -> Result<PdbDocument> {
    let mut title_lines = Vec::new();
    let mut classification = None;
    let mut pdb_id = None;
    let mut inferred_cell = None;
    let mut conect_pairs = BTreeMap::new();
    let mut link_pairs = Vec::new();
    let mut secondary_structures = Vec::new();

    // Atoms are partitioned by `MODEL`/`ENDMDL`. Files without `MODEL` records
    // accumulate every atom into a single implicit model.
    let mut models: Vec<Vec<ParsedAtom>> = Vec::new();
    let mut current_model: Vec<ParsedAtom> = Vec::new();

    for (line_index, line) in input.lines().enumerate() {
        let line_number = line_index + 1;
        match field(line, 0, 6).trim() {
            "HEADER" => {
                let class = field(line, 10, 50).trim();
                if !class.is_empty() {
                    classification = Some(class.to_string());
                }
                let id = field(line, 62, 66).trim();
                if !id.is_empty() {
                    pdb_id = Some(id.to_string());
                }
            }
            "TITLE" => {
                let title = field(line, 10, 80).trim();
                if !title.is_empty() {
                    title_lines.push(title.to_string());
                }
            }
            "CRYST1" => inferred_cell = parse_cryst1_line(line),
            // Flush the accumulated atoms at each model boundary; empty flushes
            // (e.g. records between `ENDMDL` and the next `MODEL`) are dropped
            // afterwards.
            "MODEL" | "ENDMDL" => models.push(std::mem::take(&mut current_model)),
            "ATOM" | "HETATM" => {
                let Some(parsed) = parse_atom_record(line, line_number)? else {
                    continue;
                };
                current_model.push(parsed);
            }
            "CONECT" => parse_conect_line(line, line_number, &mut conect_pairs)?,
            "LINK" => parse_link_line(line, line_number, &mut link_pairs)?,
            "HELIX" => secondary_structures.push(parse_helix_line(line, line_number)?),
            "SHEET" => secondary_structures.push(parse_sheet_line(line, line_number)?),
            _ => {}
        }
    }

    // Tolerate a trailing model with no closing `ENDMDL`, and the single-model
    // case where no `MODEL` records appear at all.
    if !current_model.is_empty() {
        models.push(current_model);
    }
    models.retain(|model| !model.is_empty());

    if models.is_empty() {
        bail!("structure does not contain any atoms after filtering supported conformers");
    }

    let title = if !title_lines.is_empty() {
        title_lines.join(" ")
    } else if let Some(classification) = &classification {
        classification.clone()
    } else {
        "PDB structure".to_string()
    };

    let built = models
        .into_iter()
        .map(|atoms| {
            build_model_structure(
                atoms,
                title.clone(),
                inferred_cell.as_ref(),
                &conect_pairs,
                &link_pairs,
                &secondary_structures,
            )
        })
        .collect();

    Ok(PdbDocument {
        title,
        pdb_id,
        models: built,
    })
}

/// Assemble one model's parsed atoms into a [`Structure`], applying the
/// file-level bonding (`CONECT`/`LINK`) and secondary-structure records that
/// are shared across every model.
fn build_model_structure(
    parsed_atoms: Vec<ParsedAtom>,
    title: String,
    inferred_cell: Option<&UnitCell>,
    conect_pairs: &BTreeMap<(usize, usize), usize>,
    link_pairs: &[(PdbAtomIdentity, PdbAtomIdentity)],
    secondary_structures: &[SecondaryStructureSpan],
) -> Structure {
    let mut atoms = Vec::with_capacity(parsed_atoms.len());
    let mut annotations = Vec::with_capacity(parsed_atoms.len());
    let mut serial_to_index = HashMap::new();
    let mut identity_to_index = HashMap::new();

    for parsed in parsed_atoms {
        let atom_index = atoms.len();
        let identity = parsed.identity();
        atoms.push(parsed.atom);
        annotations.push(parsed.annotation);
        serial_to_index.entry(parsed.serial).or_insert(atom_index);
        identity_to_index.entry(identity).or_insert(atom_index);
    }

    // A placeholder cell (e.g. a `CRYST1 1 1 1 ... P1` written by modeling tools
    // for a non-periodic molecule) must not drive periodic bond inference: its
    // minimum-image convention would place every atom within bonding distance of
    // a neighbor's image and connect everything. Use the same dummy-cell guard
    // here as for storage, so such files fall back to non-periodic inference.
    let effective_cell = match inferred_cell {
        Some(cell) if cell.is_placeholder() => None,
        other => other.cloned(),
    };

    let biopolymer = build_biopolymer(&annotations, secondary_structures.to_vec());

    // A deposited biomolecule's covalent graph is intramolecular (plus the
    // explicit CONECT/LINK records). Its crystallographic cell tiles space by
    // symmetry operators, not the pure translation that minimum-image bonding
    // assumes, so periodic inference across that cell discovers no real bonds and
    // only fabricates false ones — and on a large structure its O(n^2) form
    // stalls the load. Bond such structures non-periodically; the cell is still
    // stored below for display and PBC.
    let bonding_cell = effective_cell.as_ref().filter(|_| biopolymer.is_none());

    let bonds = resolve_bonds(
        &atoms,
        bonding_cell,
        &serial_to_index,
        &identity_to_index,
        conect_pairs,
        link_pairs,
    );

    let mut structure = match effective_cell {
        Some(cell) => Structure::with_cell_and_bonds(title, atoms, bonds, cell),
        None => Structure::with_bonds(title, atoms, bonds),
    };
    structure.biopolymer = biopolymer;
    structure
}

struct ParsedAtom {
    serial: usize,
    atom: Atom,
    annotation: PdbAtomAnnotation,
}

impl ParsedAtom {
    fn identity(&self) -> PdbAtomIdentity {
        PdbAtomIdentity {
            atom_name: self.annotation.atom_name.clone(),
            residue_name: self.annotation.residue_name.clone(),
            chain_id: self.annotation.chain_id,
            residue_seq: self.annotation.residue_seq,
            insertion_code: self.annotation.insertion_code,
        }
    }
}

/// Parse a single `ATOM`/`HETATM` record. Returns `Ok(None)` when the atom uses
/// an alternate location we do not keep (anything other than blank or "A").
fn parse_atom_record(line: &str, line_number: usize) -> Result<Option<ParsedAtom>> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 8 {
        bail!("PDB atom line {line_number} does not contain enough fields");
    }

    let alt_loc = field(line, 16, 17).chars().next().unwrap_or(' ');
    if alt_loc != ' ' && alt_loc != 'A' {
        return Ok(None);
    }

    let serial =
        parse_fixed_width_usize_or_fallback(line, 6, 11, fields.get(1).copied(), "serial")?;

    let atom_name = field(line, 12, 16).trim().to_string();
    let atom_name = if atom_name.is_empty() {
        fields.get(2).copied().unwrap_or("X").to_string()
    } else {
        atom_name
    };

    let residue_name = field(line, 17, 20).trim().to_string();
    let residue_name = if residue_name.is_empty() {
        fields.get(3).copied().unwrap_or("MOL").to_string()
    } else {
        residue_name
    };

    let coordinate_start = find_coordinate_start(&fields)
        .ok_or_else(|| anyhow!("could not find coordinates on PDB atom line {line_number}"))?;
    let x = fields[coordinate_start]
        .parse::<f32>()
        .with_context(|| format!("invalid x coordinate on PDB atom line {line_number}"))?;
    let y = fields[coordinate_start + 1]
        .parse::<f32>()
        .with_context(|| format!("invalid y coordinate on PDB atom line {line_number}"))?;
    let z = fields[coordinate_start + 2]
        .parse::<f32>()
        .with_context(|| format!("invalid z coordinate on PDB atom line {line_number}"))?;

    let (chain_id, residue_seq_token) =
        infer_residue_fields(line, &fields, coordinate_start, line_number)?;
    let residue_seq = residue_seq_token
        .parse::<i32>()
        .with_context(|| format!("invalid residue sequence on PDB atom line {line_number}"))?;
    let insertion_code = field(line, 26, 27).chars().next().unwrap_or(' ');

    let element = field(line, 76, 78).trim().to_string();
    let element = if element.is_empty() {
        fields
            .get(coordinate_start + 5)
            .copied()
            .filter(|token| token.chars().all(|ch| ch.is_ascii_alphabetic()))
            .map(str::to_string)
            .unwrap_or_else(|| element_from_atom_name(&atom_name))
    } else {
        element
    };
    let element = normalized_symbol(&element);
    let element = if element.is_empty() {
        element_from_atom_name(&atom_name)
    } else {
        element
    };

    Ok(Some(ParsedAtom {
        serial,
        atom: Atom {
            element,
            position: Point3::new(x, y, z),
            charge: 0.0,
        },
        annotation: PdbAtomAnnotation {
            atom_name,
            residue_name,
            chain_id,
            residue_seq,
            insertion_code,
        },
    }))
}

fn parse_cryst1_line(line: &str) -> Option<UnitCell> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 7 {
        return None;
    }
    let value = |index: usize| fields[index].parse::<f32>().ok();
    Some(UnitCell::from_parameters(
        value(1)?,
        value(2)?,
        value(3)?,
        value(4)?,
        value(5)?,
        value(6)?,
    ))
}

fn resolve_bonds(
    atoms: &[Atom],
    cell: Option<&UnitCell>,
    serial_to_index: &HashMap<usize, usize>,
    identity_to_index: &HashMap<PdbAtomIdentity, usize>,
    conect_pairs: &BTreeMap<(usize, usize), usize>,
    link_pairs: &[(PdbAtomIdentity, PdbAtomIdentity)],
) -> Vec<Bond> {
    let mut merged = infer_bonds_with_cell(atoms, cell)
        .into_iter()
        .map(|bond| (ordered_pair(bond.a, bond.b), bond.bond_type))
        .collect::<BTreeMap<_, _>>();

    for ((first_serial, second_serial), occurrences) in conect_pairs {
        let Some(first) = serial_to_index.get(first_serial) else {
            continue;
        };
        let Some(second) = serial_to_index.get(second_serial) else {
            continue;
        };
        merged.insert(
            ordered_pair(*first, *second),
            bond_type_from_conect_occurrences(*occurrences),
        );
    }

    for (first_identity, second_identity) in link_pairs {
        let Some(first) = identity_to_index.get(first_identity) else {
            continue;
        };
        let Some(second) = identity_to_index.get(second_identity) else {
            continue;
        };
        merged.insert(ordered_pair(*first, *second), BondType::Single);
    }

    merged
        .into_iter()
        .map(|((first, second), bond_type)| Bond::with_type(first, second, bond_type))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PdbAtomIdentity {
    atom_name: String,
    residue_name: String,
    chain_id: char,
    residue_seq: i32,
    insertion_code: char,
}

fn parse_conect_line(
    line: &str,
    line_number: usize,
    conect_pairs: &mut BTreeMap<(usize, usize), usize>,
) -> Result<()> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 2 {
        bail!("PDB CONECT line {line_number} does not contain an atom serial");
    }

    let origin = fields[1]
        .parse::<usize>()
        .with_context(|| format!("invalid atom serial on PDB CONECT line {line_number}"))?;

    for neighbor in &fields[2..] {
        let target = neighbor.parse::<usize>().with_context(|| {
            format!("invalid bonded atom serial on PDB CONECT line {line_number}")
        })?;

        if origin == target {
            continue;
        }

        *conect_pairs
            .entry(ordered_pair(origin, target))
            .or_insert(0) += 1;
    }

    Ok(())
}

fn parse_link_line(
    line: &str,
    line_number: usize,
    link_pairs: &mut Vec<(PdbAtomIdentity, PdbAtomIdentity)>,
) -> Result<()> {
    let Some(first) = parse_link_atom_identity(line, line_number, "first", 12, 22)? else {
        return Ok(());
    };
    let Some(second) = parse_link_atom_identity(line, line_number, "second", 42, 52)? else {
        return Ok(());
    };

    if first != second {
        link_pairs.push((first, second));
    }

    Ok(())
}

/// Parse one of the two atom identities on a `LINK` record. `base` is the start
/// column of the atom name; all remaining fields are at fixed offsets from it as
/// laid out by the PDB format.
fn parse_link_atom_identity(
    line: &str,
    line_number: usize,
    label: &str,
    base: usize,
    residue_seq_start: usize,
) -> Result<Option<PdbAtomIdentity>> {
    let alt_loc = field(line, base + 4, base + 5)
        .chars()
        .next()
        .unwrap_or(' ');
    if alt_loc != ' ' && alt_loc != 'A' {
        return Ok(None);
    }

    let atom_name = field(line, base, base + 4).trim().to_string();
    if atom_name.is_empty() {
        bail!("PDB LINK line {line_number} has an empty {label} atom name");
    }

    Ok(Some(PdbAtomIdentity {
        atom_name,
        residue_name: field(line, base + 5, base + 8).trim().to_string(),
        chain_id: field(line, base + 9, base + 10)
            .chars()
            .next()
            .unwrap_or(' '),
        residue_seq: parse_fixed_width_i32_or_default(
            line,
            residue_seq_start,
            residue_seq_start + 4,
            &format!("{label} LINK residue sequence"),
            0,
        )?,
        insertion_code: field(line, residue_seq_start + 4, residue_seq_start + 5)
            .chars()
            .next()
            .unwrap_or(' '),
    }))
}

fn parse_helix_line(line: &str, line_number: usize) -> Result<SecondaryStructureSpan> {
    let start_chain_id = field(line, 19, 20).chars().next().unwrap_or(' ');
    let start_sequence_number =
        parse_fixed_width_i32_or_default(line, 21, 25, "HELIX start residue sequence", 0)?;
    let start_insertion_code = field(line, 25, 26).chars().next().unwrap_or(' ');
    let end_chain_id = field(line, 31, 32).chars().next().unwrap_or(' ');
    let end_sequence_number =
        parse_fixed_width_i32_or_default(line, 33, 37, "HELIX end residue sequence", 0)?;
    let end_insertion_code = field(line, 37, 38).chars().next().unwrap_or(' ');

    if start_chain_id != end_chain_id {
        bail!("HELIX line {line_number} spans multiple chains, which is unsupported");
    }

    Ok(SecondaryStructureSpan {
        kind: SecondaryStructureKind::Helix,
        start: ResidueId::new(start_chain_id, start_sequence_number, start_insertion_code),
        end: ResidueId::new(end_chain_id, end_sequence_number, end_insertion_code),
    })
}

fn parse_sheet_line(line: &str, line_number: usize) -> Result<SecondaryStructureSpan> {
    let start_chain_id = field(line, 21, 22).chars().next().unwrap_or(' ');
    let start_sequence_number =
        parse_fixed_width_i32_or_default(line, 22, 26, "SHEET start residue sequence", 0)?;
    let start_insertion_code = field(line, 26, 27).chars().next().unwrap_or(' ');
    let end_chain_id = field(line, 32, 33).chars().next().unwrap_or(' ');
    let end_sequence_number =
        parse_fixed_width_i32_or_default(line, 33, 37, "SHEET end residue sequence", 0)?;
    let end_insertion_code = field(line, 37, 38).chars().next().unwrap_or(' ');

    if start_chain_id != end_chain_id {
        bail!("SHEET line {line_number} spans multiple chains, which is unsupported");
    }

    Ok(SecondaryStructureSpan {
        kind: SecondaryStructureKind::Sheet,
        start: ResidueId::new(start_chain_id, start_sequence_number, start_insertion_code),
        end: ResidueId::new(end_chain_id, end_sequence_number, end_insertion_code),
    })
}

fn bond_type_from_conect_occurrences(occurrences: usize) -> BondType {
    let normalized = if occurrences >= 2 && occurrences.is_multiple_of(2) {
        occurrences / 2
    } else {
        occurrences
    };

    match normalized {
        0 | 1 => BondType::Single,
        2 => BondType::Double,
        _ => BondType::Triple,
    }
}
