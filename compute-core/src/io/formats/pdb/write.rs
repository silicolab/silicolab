use anyhow::{Result, bail};

use crate::domain::{BondLinkage, BondType, Structure, UnitCell, chemistry::normalized_symbol};

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
