//! Reading and writing of the PDB structure format.
//!
//! Parsing is intentionally permissive: real-world `.pdb` files range from
//! strict fixed-column depositions (RCSB) to loosely formatted exports. We read
//! the columns we can rely on (atom name, residue name, element) by fixed width
//! and fall back to whitespace tokenization for the rest, locating the
//! coordinate triple by scanning for three consecutive decimal numbers.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow, bail};
use nalgebra::Point3;

use crate::domain::{
    Atom, Bond, BondLinkage, BondType, PdbAtomAnnotation, ResidueId, SecondaryStructureKind,
    SecondaryStructureSpan, Structure, UnitCell, build_biopolymer,
    chemistry::{infer_bonds_with_cell, normalized_symbol},
};

/// A parsed PDB file. NMR depositions carry many alternative conformers as
/// `MODEL`/`ENDMDL` blocks; each becomes one [`Structure`] in `models`.
pub struct PdbDocument {
    /// The deposition title (from `TITLE`, falling back to the `HEADER`
    /// classification, then a generic placeholder).
    pub title: String,
    /// The four-character PDB identifier from the `HEADER` record, if present.
    pub pdb_id: Option<String>,
    /// One entry per `MODEL` block, or a single entry for a single-model file.
    pub models: Vec<Structure>,
}

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

    let bonds = resolve_bonds(
        &atoms,
        effective_cell.as_ref(),
        &serial_to_index,
        &identity_to_index,
        conect_pairs,
        link_pairs,
    );

    let biopolymer = build_biopolymer(&annotations, secondary_structures.to_vec());

    let mut structure = match effective_cell {
        Some(cell) => Structure::with_cell_and_bonds(title, atoms, bonds, cell),
        None => Structure::with_bonds(title, atoms, bonds),
    };
    structure.biopolymer = biopolymer;
    structure
}

pub fn to_pdb(structure: &Structure) -> Result<String> {
    let mut output = String::new();

    let title = structure.title.trim();
    let title = if title.is_empty() {
        "PDB structure"
    } else {
        title
    };
    output.push_str(&format!("TITLE     {title}\n"));

    if let Some(cell) = &structure.cell {
        output.push_str(&cryst1_line(cell));
        output.push('\n');
    }

    for (index, atom) in structure.atoms.iter().enumerate() {
        let serial = index + 1;
        if serial > 99_999 {
            bail!("cannot save PDB with atom serials above 99999");
        }
        let residue = pdb_residue_identity(structure, index);
        let chain_id = if residue.chain_id == ' ' {
            'A'
        } else {
            residue.chain_id
        };
        let element = normalized_symbol(atom.element.trim());
        let atom_name = pdb_atom_name_for(structure, index, &atom.element, serial);

        output.push_str(&format!(
            "ATOM  {serial:>5} {atom_name:>4} {residue_name:<3} {chain_id:1}{residue_seq:>4}{insertion_code:1}   {x:>8.3}{y:>8.3}{z:>8.3}{occupancy:>6.2}{b_factor:>6.2}          {element:>2}",
            residue_name = residue.name,
            residue_seq = residue.sequence_number,
            insertion_code = residue.insertion_code,
            x = atom.position.x,
            y = atom.position.y,
            z = atom.position.z,
            occupancy = 1.0,
            b_factor = 0.0,
        ));
        output.push('\n');
    }

    for line in pdb_link_lines(structure) {
        output.push_str(&line);
        output.push('\n');
    }

    for line in pdb_conect_lines(structure)? {
        output.push_str(&line);
        output.push('\n');
    }

    output.push_str("END\n");
    Ok(output)
}

fn pdb_link_lines(structure: &Structure) -> Vec<String> {
    let Some(biopolymer) = structure
        .biopolymer
        .as_ref()
        .filter(|biopolymer| biopolymer.is_compatible_with_atom_count(structure.atoms.len()))
    else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    for cross in crate::domain::glycan::cross_residue_linkages(structure, biopolymer) {
        let (first, second) = match cross.linkage {
            BondLinkage::Glycosidic { carbon, oxygen } => (carbon, oxygen),
            BondLinkage::GlycanProtein {
                anomeric_carbon,
                protein_atom,
                ..
            } => (anomeric_carbon, protein_atom),
            BondLinkage::IntraResidue => continue,
        };
        lines.push(pdb_link_line(structure, first, second));
    }
    lines
}

fn pdb_link_line(structure: &Structure, first: usize, second: usize) -> String {
    let first_atom = link_atom_field(structure, first);
    let second_atom = link_atom_field(structure, second);
    format!(
        "LINK        {first_atom}{gap}{second_atom}",
        gap = " ".repeat(15)
    )
}

fn link_atom_field(structure: &Structure, index: usize) -> String {
    let residue = pdb_residue_identity(structure, index);
    let chain_id = if residue.chain_id == ' ' {
        'A'
    } else {
        residue.chain_id
    };
    let element = normalized_symbol(structure.atoms[index].element.trim());
    let atom_name = pdb_atom_name_for(structure, index, &element, index + 1);
    format!(
        "{atom_name:>4} {residue_name:<3} {chain_id:1}{residue_seq:>4}{insertion_code:1}",
        residue_name = residue.name,
        residue_seq = residue.sequence_number,
        insertion_code = residue.insertion_code,
    )
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

struct PdbResidueIdentity {
    name: String,
    chain_id: char,
    sequence_number: i32,
    insertion_code: char,
}

fn pdb_residue_identity(structure: &Structure, index: usize) -> PdbResidueIdentity {
    if let Some(biopolymer) = &structure.biopolymer
        && biopolymer.is_compatible_with_atom_count(structure.atoms.len())
        && let Some(Some(residue_index)) = biopolymer.residue_for_atom.get(index)
        && let Some(residue) = biopolymer.residues.get(*residue_index)
    {
        return PdbResidueIdentity {
            name: residue.residue_name.clone(),
            chain_id: residue.id.chain_id,
            sequence_number: residue.id.sequence_number,
            insertion_code: residue.id.insertion_code,
        };
    }

    PdbResidueIdentity {
        name: "MOL".to_string(),
        chain_id: ' ',
        sequence_number: 1,
        insertion_code: ' ',
    }
}

/// The PDB atom name to write for an atom: the biopolymer's recorded name (e.g.
/// `CA`, `CB`, `HB1`) when present, so an exported structure round-trips and an
/// engine preprocessor can match residue templates; otherwise a synthesized
/// element+serial name.
fn pdb_atom_name_for(structure: &Structure, index: usize, element: &str, serial: usize) -> String {
    if let Some(biopolymer) = &structure.biopolymer
        && biopolymer.is_compatible_with_atom_count(structure.atoms.len())
        && let Some(name) = biopolymer.atom_name(index)
        && !name.trim().is_empty()
    {
        return name.trim().to_string();
    }
    pdb_atom_name(element, serial)
}

fn pdb_atom_name(element: &str, serial: usize) -> String {
    let normalized = if element.trim().is_empty() {
        "X".to_string()
    } else {
        element.trim().to_string()
    };
    let candidate = format!("{normalized}{}", serial % 1000);

    if candidate.len() <= 4 {
        candidate
    } else {
        candidate[candidate.len() - 4..].to_string()
    }
}

fn cryst1_line(cell: &UnitCell) -> String {
    format!(
        "CRYST1{a:>9.3}{b:>9.3}{c:>9.3}{alpha:>7.2}{beta:>7.2}{gamma:>7.2} P 1           1",
        a = cell.a,
        b = cell.b,
        c = cell.c,
        alpha = cell.alpha,
        beta = cell.beta,
        gamma = cell.gamma,
    )
}

fn pdb_conect_lines(structure: &Structure) -> Result<Vec<String>> {
    let mut neighbors = vec![Vec::<usize>::new(); structure.atoms.len()];

    for bond in &structure.bonds {
        let repetitions = conect_repetitions(bond.bond_type);
        let first_serial = bond.a + 1;
        let second_serial = bond.b + 1;

        if first_serial > 99_999 || second_serial > 99_999 {
            bail!("cannot save PDB with atom serials above 99999");
        }

        for _ in 0..repetitions {
            neighbors[bond.a].push(second_serial);
            neighbors[bond.b].push(first_serial);
        }
    }

    let mut lines = Vec::new();
    for (atom_index, bonded_serials) in neighbors.iter().enumerate() {
        if bonded_serials.is_empty() {
            continue;
        }

        for chunk in bonded_serials.chunks(4) {
            let mut line = format!("CONECT{:>5}", atom_index + 1);
            for bonded_serial in chunk {
                line.push_str(&format!("{bonded_serial:>5}"));
            }
            lines.push(line);
        }
    }

    Ok(lines)
}

fn conect_repetitions(bond_type: BondType) -> usize {
    match bond_type {
        BondType::Single => 1,
        BondType::Double => 2,
        BondType::Triple => 3,
        BondType::Aromatic => 1,
    }
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

fn element_from_atom_name(atom_name: &str) -> String {
    let letters = atom_name
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();
    if letters.is_empty() {
        return "X".to_string();
    }

    if letters.len() >= 2 {
        let two_letter = normalized_symbol(&letters[..2]);
        if !two_letter.is_empty() {
            return two_letter;
        }
    }

    let one_letter = normalized_symbol(&letters[..1]);
    if one_letter.is_empty() {
        "X".to_string()
    } else {
        one_letter
    }
}

fn infer_residue_fields<'a>(
    line: &'a str,
    fields: &'a [&'a str],
    coordinate_start: usize,
    line_number: usize,
) -> Result<(char, &'a str)> {
    let metadata = &fields[2..coordinate_start];
    match metadata {
        [_, _, chain_or_residue] => split_chain_and_residue(chain_or_residue)
            .or(Some((' ', *chain_or_residue)))
            .ok_or_else(|| anyhow!("invalid chain/residue token on PDB atom line {line_number}")),
        [_, _, chain_id, residue_seq] => Ok((chain_id.chars().next().unwrap_or(' '), *residue_seq)),
        _ => {
            let chain_id = field(line, 21, 22).chars().next().unwrap_or(' ');
            let residue_seq = field(line, 22, 26).trim();
            if residue_seq.is_empty() {
                bail!("missing residue sequence on PDB atom line {line_number}");
            }
            Ok((chain_id, residue_seq))
        }
    }
}

fn split_chain_and_residue(token: &str) -> Option<(char, &str)> {
    let mut chars = token.char_indices();
    let (_, chain_id) = chars.next()?;
    if !chain_id.is_ascii_alphabetic() {
        return None;
    }

    let residue_start = chars.next().map(|(index, _)| index).unwrap_or(token.len());
    let residue_seq = token[residue_start..].trim();
    if residue_seq.is_empty()
        || !residue_seq
            .chars()
            .all(|ch| ch == '-' || ch.is_ascii_digit())
    {
        return None;
    }

    Some((chain_id, residue_seq))
}

fn find_coordinate_start(fields: &[&str]) -> Option<usize> {
    fields.windows(3).position(|window| {
        window.iter().all(|value| {
            value.parse::<f64>().is_ok()
                && (value.contains('.') || value.contains('e') || value.contains('E'))
        })
    })
}

fn parse_fixed_width_usize_or_fallback(
    line: &str,
    start: usize,
    end: usize,
    fallback: Option<&str>,
    label: &str,
) -> Result<usize> {
    let value = field(line, start, end).trim();
    if !value.is_empty() {
        return value
            .parse::<usize>()
            .with_context(|| format!("invalid PDB {label}"));
    }

    fallback
        .ok_or_else(|| anyhow!("missing PDB {label}"))
        .and_then(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid PDB {label}"))
        })
}

fn parse_fixed_width_i32_or_default(
    line: &str,
    start: usize,
    end: usize,
    label: &str,
    default: i32,
) -> Result<i32> {
    let value = field(line, start, end).trim();
    if value.is_empty() {
        Ok(default)
    } else {
        value
            .parse::<i32>()
            .with_context(|| format!("invalid PDB {label}"))
    }
}

fn field(line: &str, start: usize, end: usize) -> &str {
    // PDB is a fixed-column format, but the columns are byte offsets: a stray
    // multibyte character in a malformed file would put a boundary mid-codepoint
    // and panic the slice. Clamp both ends down to a char boundary so a bad line
    // degrades to a short/empty field instead of crashing the reader.
    let end = floor_char_boundary(line, end.min(line.len()));
    let start = floor_char_boundary(line, start.min(end));
    &line[start..end]
}

/// The largest char boundary `<= index` (stable-Rust stand-in for the unstable
/// `str::floor_char_boundary`). A no-op for the ASCII columns of a well-formed PDB.
fn floor_char_boundary(line: &str, mut index: usize) -> usize {
    if index >= line.len() {
        return line.len();
    }
    while index > 0 && !line.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ordered_pair<T: Ord>(first: T, second: T) -> (T, T) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_pdb, parse_pdb_document, to_pdb};
    use crate::domain::BondType;

    #[test]
    fn splits_nmr_models_into_separate_structures() {
        let document = parse_pdb_document(
            "\
HEADER    ANTIMICROBIAL PROTEIN                   24-JUN-18   6A5J
TITLE     SOLUTION NMR STRUCTURE OF SMALL PEPTIDE
NUMMDL    2
MODEL        1
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ENDMDL
MODEL        2
ATOM      1  N   GLY A   1       0.100   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.550   0.000   0.000  1.00  0.00           C
ENDMDL
END
",
        )
        .expect("valid multi-model pdb");

        assert_eq!(document.pdb_id.as_deref(), Some("6A5J"));
        assert_eq!(document.title, "SOLUTION NMR STRUCTURE OF SMALL PEPTIDE");
        assert_eq!(document.models.len(), 2);
        assert_eq!(document.models[0].atoms.len(), 2);
        assert_eq!(document.models[1].atoms.len(), 2);
        // The two conformers differ in coordinates.
        assert!((document.models[0].atoms[0].position.x - 0.0).abs() < 0.0001);
        assert!((document.models[1].atoms[0].position.x - 0.1).abs() < 0.0001);
    }

    #[test]
    fn single_model_file_yields_one_structure() {
        let document = parse_pdb_document(
            "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
END
",
        )
        .expect("valid pdb");

        assert_eq!(document.models.len(), 1);
        assert_eq!(document.pdb_id, None);
        // parse_pdb returns that single model.
        let structure = parse_pdb(
            "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
END
",
        )
        .expect("valid pdb");
        assert_eq!(structure.atoms.len(), 2);
    }

    #[test]
    fn parses_cryst1_atoms_and_conect_records() {
        let structure = parse_pdb(
            "\
TITLE     UNK
CRYST1    7.301    7.301    3.400  90.00  90.00  60.00 P 1           1
ATOM      1  C01  UNK    1       7.302   1.374   1.700  1.00  0.00           C
ATOM      2  C02  UNK    1       4.840   5.637   1.700  1.00  0.00           C
ATOM      3  C03  UNK    1       9.763   5.637   1.700  1.00  0.00           C
ATOM      4  C04  UNK    1       7.302   2.843   1.700  1.00  0.00           C
ATOM      5  C05  UNK    1       6.112   4.903   1.700  1.00  0.00           C
ATOM      6  C06  UNK    1       8.491   4.903   1.700  1.00  0.00           C
ATOM      7  N07  UNK    1       1.173   0.678   1.700  1.00  0.00           N
ATOM      8  N08  UNK    1       3.651   4.969   1.700  1.00  0.00           N
ATOM      9  N09  UNK    1       6.129   0.678   1.700  1.00  0.00           N
ATOM     10  N0A  UNK    1       6.129   3.539   1.700  1.00  0.00           N
ATOM     11  N0B  UNK    1       7.302   5.570   1.700  1.00  0.00           N
ATOM     12  N0C  UNK    1       8.475   3.539   1.700  1.00  0.00           N
TER
CONECT    1    9    4    7
CONECT    4    1   10   12
CONECT    7    1    2
CONECT    9    1    3
CONECT    2    8    5    7
CONECT    5   11    2   10
CONECT    8    2    3
CONECT    3    8    9    6
CONECT    6   11    3   12
CONECT   10    4    5
CONECT   12    4    6
CONECT   11    5    6
END
",
        )
        .expect("valid pdb");

        let cell = structure.cell.as_ref().expect("periodic cell");

        assert_eq!(structure.title, "UNK");
        assert_eq!(structure.atoms.len(), 12);
        assert_eq!(structure.bonds.len(), 15);
        assert_eq!(structure.atoms[0].element, "C");
        assert_eq!(structure.atoms[6].element, "N");
        assert!((cell.a - 7.301).abs() < 0.0001);
        assert!((cell.gamma - 60.0).abs() < 0.0001);
        assert!((structure.atoms[0].position.x - 7.302).abs() < 0.0001);
    }

    #[test]
    fn ignores_dummy_cryst1_placeholder_cells() {
        // A `CRYST1 1 1 1 90 90 90` placeholder is what modeling tools write for a
        // non-periodic molecule. It must be ignored for both storage *and* bond
        // inference: under the minimum-image convention a 1 Å cell places every
        // atom within bonding distance of a neighbor's periodic image, which would
        // connect all atom pairs (a C60 buckyball becomes all 1770 pairs). Four
        // carbons in a line must yield exactly the two real C-C bonds (1-2, 2-3),
        // never six. (Earlier this was asserted with only two atoms, where the
        // spurious bond happened to look correct and masked the over-bonding.)
        let structure = parse_pdb(
            "\
TITLE     dummy cell
CRYST1    1.000    1.000    1.000  90.00  90.00  90.00 P 1
ATOM      1  C   UNK A   1       0.000   0.000   0.000  1.00  0.00           C
ATOM      2  C   UNK A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   UNK A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  C   UNK A   1      10.000   0.000   0.000  1.00  0.00           C
END
",
        )
        .expect("valid pdb");

        assert!(
            structure.cell.is_none(),
            "the placeholder cell must be discarded"
        );
        assert_eq!(
            structure.bonds.len(),
            2,
            "the placeholder cell must not connect all atom pairs"
        );
    }

    #[test]
    fn interprets_repeated_conect_entries_as_higher_bond_order() {
        let structure = parse_pdb(
            "\
TITLE     ethene
ATOM      1  C1   ETH     1       0.000   0.000   0.000  1.00  0.00           C
ATOM      2  C2   ETH     1       1.340   0.000   0.000  1.00  0.00           C
CONECT    1    2    2
CONECT    2    1    1
END
",
        )
        .expect("valid pdb");

        assert_eq!(structure.bonds.len(), 1);
        assert_eq!(structure.bonds[0].bond_type, BondType::Double);
    }

    #[test]
    fn parses_link_records_for_biomolecular_contacts() {
        let structure = parse_pdb(
            "\
TITLE     peptide with link
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   GLY A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  O   GLY A   1       4.100   0.000   0.000  1.00  0.00           O
HETATM    5 CA    CA A  10       6.900   0.000   0.000  1.00  0.00          CA
LINK         O   GLY A   1                CA    CA A  10     1555   1555  2.80
END
",
        )
        .expect("valid pdb");

        assert!(
            structure
                .bonds
                .iter()
                .any(|bond| bond.a == 3 && bond.b == 4)
        );
    }

    #[test]
    fn keeps_secondary_structure_metadata() {
        let structure = parse_pdb(
            "\
HELIX    1   1 GLY A    1  ALA A    2  1
SHEET    1   A 1 GLY A   3  ALA A   4  0
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  N   ALA A   2       2.900   0.000   0.000  1.00  0.00           N
ATOM      4  CA  ALA A   2       4.350   0.000   0.000  1.00  0.00           C
ATOM      5  N   GLY A   3       5.800   0.000   0.000  1.00  0.00           N
ATOM      6  CA  GLY A   3       7.250   0.000   0.000  1.00  0.00           C
ATOM      7  N   ALA A   4       8.700   0.000   0.000  1.00  0.00           N
ATOM      8  CA  ALA A   4      10.150   0.000   0.000  1.00  0.00           C
END
",
        )
        .expect("valid pdb");

        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
        assert_eq!(biopolymer.secondary_structures.len(), 2);
        assert_eq!(
            biopolymer.secondary_structures[0].kind,
            crate::domain::SecondaryStructureKind::Helix
        );
        assert_eq!(
            biopolymer.secondary_structures[1].kind,
            crate::domain::SecondaryStructureKind::Sheet
        );
    }

    #[test]
    fn to_pdb_preserves_biopolymer_atom_names() {
        // A residue's atom names (CA, C, O, ...) must survive export so an engine
        // preprocessor can match force-field residue templates, rather than being
        // replaced by synthesized element+serial names.
        let structure = parse_pdb(
            "\
TITLE     glycine
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  C   GLY A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  O   GLY A   1       4.100   0.000   0.000  1.00  0.00           O
END
",
        )
        .expect("valid pdb");

        let out = to_pdb(&structure).expect("serialize");
        let names: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("ATOM"))
            .map(|l| l[12..16].trim())
            .collect();
        assert_eq!(names, ["N", "CA", "C", "O"]);
    }

    fn has_bond_between(structure: &crate::domain::Structure, first: usize, second: usize) -> bool {
        structure.bonds.iter().any(|bond| {
            (bond.a == first && bond.b == second) || (bond.a == second && bond.b == first)
        })
    }

    fn atom_index(structure: &crate::domain::Structure, residue: &str, atom: &str) -> usize {
        let bio = structure.biopolymer.as_ref().expect("biopolymer overlay");
        let residue_index = bio
            .residues
            .iter()
            .position(|r| r.residue_name == residue)
            .expect("residue present");
        (0..structure.atoms.len())
            .find(|&i| {
                bio.residue_for_atom.get(i).and_then(|r| *r) == Some(residue_index)
                    && bio.atom_name(i) == Some(atom)
            })
            .expect("named atom present")
    }

    #[test]
    fn glycan_to_asn_link_round_trips_through_pdb() {
        let source = "\
TITLE     n-glycan junction
ATOM      1  N   ASN A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  ASN A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  CG  ASN A   1       2.900   0.000   0.000  1.00  0.00           C
ATOM      4  ND2 ASN A   1       4.000   0.800   0.000  1.00  0.00           N
ATOM      5  C1  NAG B   1       5.400   0.800   0.000  1.00  0.00           C
ATOM      6  C2  NAG B   1       6.100   2.000   0.000  1.00  0.00           C
ATOM      7  O5  NAG B   1       6.100  -0.300   0.000  1.00  0.00           O
LINK         ND2 ASN A   1                 C1  NAG B   1
END
";
        let structure = parse_pdb(source).expect("valid glycoprotein pdb");
        let nd2 = atom_index(&structure, "ASN", "ND2");
        let c1 = atom_index(&structure, "NAG", "C1");
        assert!(
            has_bond_between(&structure, nd2, c1),
            "inbound LINK should create the ND2-C1 bond"
        );

        let serialized = to_pdb(&structure).expect("serialize glycoprotein");
        assert!(
            serialized.lines().any(|line| line.starts_with("LINK")),
            "to_pdb should emit a LINK record for the glycan-protein junction"
        );

        let reparsed = parse_pdb(&serialized).expect("reparse glycoprotein");
        let nd2_again = atom_index(&reparsed, "ASN", "ND2");
        let c1_again = atom_index(&reparsed, "NAG", "C1");
        assert!(
            has_bond_between(&reparsed, nd2_again, c1_again),
            "the cross-residue bond must survive a to_pdb / parse round trip"
        );
    }
}
