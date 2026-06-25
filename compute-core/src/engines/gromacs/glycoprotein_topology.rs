mod protein_top;

use anyhow::{Result, anyhow, bail};

use protein_top::ProteinTopology;

use crate::domain::biopolymer::{Biopolymer, ChainRecord, ResidueRecord, is_carbohydrate_residue};
use crate::domain::glycan::linkage_topology::{self, BondLinkage};
use crate::domain::glycan::patches::{self, JunctionPatch};
use crate::domain::structure::{Atom, Bond, Structure};
use crate::md::{BondedTerm, MoleculeType};

use super::carb_topology::build_glycan_topology_with;
use super::forcefield_assets::{self, CarbTopologyDatabase};

const ANGSTROM_TO_NM: f32 = 0.1;

pub struct JunctionSite {
    pub anchor_residue_name: String,
    pub anchor_atom_name: String,
    pub anchor_sequence_number: i32,
    pub anomeric_carbon_global: usize,
    pub patch: JunctionPatch,
}

pub fn merge_glycan_into_protein_topology(
    protein_top: &str,
    structure: &Structure,
    force_field: &str,
) -> Result<String> {
    let database = forcefield_assets::charmm36_carb_database()?;
    merge_glycan_into_protein_topology_with(protein_top, structure, force_field, &database)
}

pub fn merge_glycan_into_protein_topology_with(
    protein_top: &str,
    structure: &Structure,
    force_field: &str,
    database: &CarbTopologyDatabase,
) -> Result<String> {
    let biopolymer = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a glycoprotein topology needs a biopolymer overlay"))?;
    if !biopolymer.is_compatible_with_atom_count(structure.atoms.len()) {
        bail!("the biopolymer overlay does not cover every atom");
    }

    let glycan_atoms = carbohydrate_atom_indices(structure, biopolymer);
    if glycan_atoms.is_empty() {
        bail!("no carbohydrate atoms to merge into the protein topology");
    }

    // Every glycan-to-protein linkage gets its own junction bond and charge
    // patch. All carbohydrate atoms (across every glycosylation site) live in one
    // glycan moleculetype; handling only the first junction would leave the other
    // sites' sugars topologically free-floating in the MD system.
    let junctions = junction_sites(structure, biopolymer)?;
    let (glycan_structure, local_for_global) =
        extract_substructure(structure, biopolymer, &glycan_atoms, 'G');
    let glycan_topology = build_glycan_topology_with(&glycan_structure, force_field, database)?;
    let glycan_molecule = glycan_topology
        .molecules
        .first()
        .ok_or_else(|| anyhow!("glycan topology produced no molecule"))?;

    let mut parsed = ProteinTopology::parse(protein_top)?;

    let protein_atom_count = parsed.atom_count;
    let protein_residue_count = parsed.max_residue_number;

    // Resolve each junction's anomeric carbon to its local glycan index up front,
    // so a missing one aborts before any topology is mutated.
    let anomeric_locals = junctions
        .iter()
        .map(|junction| {
            local_for_global
                .get(&junction.anomeric_carbon_global)
                .copied()
                .ok_or_else(|| anyhow!("the anomeric carbon is not a carbohydrate atom"))
        })
        .collect::<Result<Vec<usize>>>()?;

    let mut glycan_molecule = glycan_molecule.clone();
    for (junction, &anomeric_local) in junctions.iter().zip(&anomeric_locals) {
        apply_junction_patch_to_protein(&mut parsed, junction)?;
        apply_junction_patch_to_anomeric(&mut glycan_molecule, anomeric_local, &junction.patch);
    }
    neutralize_molecule_charges(&mut glycan_molecule.atoms, parsed.total_charge());

    let atom_lines =
        render_glycan_atom_lines(&glycan_molecule, protein_atom_count, protein_residue_count);
    let mut bond_lines = render_glycan_bond_lines(&glycan_molecule, protein_atom_count);

    for (junction, &anomeric_local) in junctions.iter().zip(&anomeric_locals) {
        let anomeric_topology_index = protein_atom_count + anomeric_local + 1;
        let anchor_topology_index = parsed.anchor_atom_index(
            junction.anchor_sequence_number,
            &junction.anchor_residue_name,
            &junction.anchor_atom_name,
        )?;
        let (junction_b0, junction_kb) = junction_bond_params(&junction.anchor_atom_name);
        bond_lines.push(format!(
            "  {:>4} {:>4} 1 {:.5} {:.1}\n",
            anchor_topology_index, anomeric_topology_index, junction_b0, junction_kb
        ));
    }

    parsed.append_atoms(&atom_lines);
    parsed.append_bonds(&bond_lines);

    // The glycan's own bending/torsion terms (intra-ring and across each
    // glycosidic linkage) carry over from its molecule type, re-indexed onto the
    // merged atom numbering. Without them the appended sugar would be held by
    // bonds alone and deform. The few terms spanning the glycan-protein junction
    // bond itself are not emitted here — that linkage needs force-field patch
    // parameters, a separate concern from carrying over the glycan's interior.
    let bonded_blocks = render_glycan_bonded_blocks(&glycan_molecule, protein_atom_count);
    parsed.append_after_bonds(&bonded_blocks);

    Ok(parsed.render())
}

fn carbohydrate_atom_indices(structure: &Structure, biopolymer: &Biopolymer) -> Vec<usize> {
    (0..structure.atoms.len())
        .filter(|&index| {
            residue_name_of(biopolymer, index)
                .map(is_carbohydrate_residue)
                .unwrap_or(false)
        })
        .collect()
}

pub fn protein_only_structure(structure: &Structure) -> Result<Structure> {
    let biopolymer = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a glycoprotein needs a biopolymer overlay"))?;
    let glycan: std::collections::HashSet<usize> = carbohydrate_atom_indices(structure, biopolymer)
        .into_iter()
        .collect();
    let protein_atoms: Vec<usize> = (0..structure.atoms.len())
        .filter(|index| !glycan.contains(index))
        .collect();
    let (sub, _) = extract_substructure(structure, biopolymer, &protein_atoms, 'A');
    Ok(sub)
}

pub fn append_glycan_coordinates(processed_gro: &str, structure: &Structure) -> Result<String> {
    let biopolymer = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a glycoprotein needs a biopolymer overlay"))?;
    let glycan_atoms = carbohydrate_atom_indices(structure, biopolymer);
    if glycan_atoms.is_empty() {
        return Ok(processed_gro.to_string());
    }

    let mut lines: Vec<&str> = processed_gro.lines().collect();
    if lines.len() < 3 {
        bail!("processed.gro is too short to carry coordinates");
    }
    let box_line = lines
        .pop()
        .ok_or_else(|| anyhow!("processed.gro has no box line"))?;
    let existing_count: usize = lines[1]
        .trim()
        .parse()
        .map_err(|_| anyhow!("processed.gro has an unparseable atom count"))?;

    let residue_offset = lines[2..]
        .iter()
        .filter_map(|line| line.get(0..5).and_then(|c| c.trim().parse::<i32>().ok()))
        .max()
        .unwrap_or(0);

    let mut residue_numbers = std::collections::HashMap::new();
    let mut next_residue = residue_offset;
    let mut body = String::new();
    for (local, &global) in glycan_atoms.iter().enumerate() {
        let residue_index = biopolymer
            .residue_for_atom
            .get(global)
            .and_then(|r| *r)
            .ok_or_else(|| anyhow!("a glycan atom is not assigned to a residue"))?;
        let resnum = *residue_numbers.entry(residue_index).or_insert_with(|| {
            next_residue += 1;
            next_residue
        });
        let residue_name = biopolymer
            .residues
            .get(residue_index)
            .map(|residue| residue.residue_name.as_str())
            .unwrap_or("UNK");
        let atom_name = biopolymer.atom_name(global).unwrap_or("X");
        let serial = (existing_count + local + 1) % 100_000;
        let position = structure.atoms[global].position;
        body.push_str(&format!(
            "{:>5}{:<5}{:>5}{:>5}{:>8.3}{:>8.3}{:>8.3}\n",
            resnum % 100_000,
            truncate_field(residue_name, 5),
            truncate_field(atom_name, 5),
            serial,
            position.x * ANGSTROM_TO_NM,
            position.y * ANGSTROM_TO_NM,
            position.z * ANGSTROM_TO_NM,
        ));
    }

    let mut out = String::new();
    out.push_str(lines[0]);
    out.push('\n');
    out.push_str(&format!("{:>5}\n", existing_count + glycan_atoms.len()));
    for line in &lines[2..] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&body);
    out.push_str(box_line);
    out.push('\n');
    Ok(out)
}

fn truncate_field(value: &str, width: usize) -> &str {
    if value.len() > width {
        &value[..width]
    } else {
        value
    }
}

fn residue_name_of(biopolymer: &Biopolymer, atom_index: usize) -> Option<&str> {
    let residue_index = (*biopolymer.residue_for_atom.get(atom_index)?)?;
    Some(
        biopolymer
            .residues
            .get(residue_index)?
            .residue_name
            .as_str(),
    )
}

fn junction_sites(structure: &Structure, biopolymer: &Biopolymer) -> Result<Vec<JunctionSite>> {
    let mut sites = Vec::new();
    for cross in linkage_topology::cross_residue_linkages(structure, biopolymer) {
        let BondLinkage::GlycanProtein {
            anomeric_carbon,
            protein_atom,
            anchor,
        } = cross.linkage
        else {
            continue;
        };
        let residue_index = biopolymer
            .residue_for_atom
            .get(protein_atom)
            .and_then(|r| *r)
            .ok_or_else(|| anyhow!("the protein anchor atom is not assigned to a residue"))?;
        let residue = biopolymer
            .residues
            .get(residue_index)
            .ok_or_else(|| anyhow!("the protein anchor residue is missing"))?;
        let anchor_atom_name = biopolymer
            .atom_name(protein_atom)
            .unwrap_or(anchor.atom_name())
            .to_string();
        let patch = match anchor {
            linkage_topology::ProteinAnchor::AsnNd2 => patches::n_linked_junction_patch(),
            linkage_topology::ProteinAnchor::SerOg | linkage_topology::ProteinAnchor::ThrOg1 => {
                patches::o_linked_junction_patch(&anchor_atom_name)
            }
        };
        sites.push(JunctionSite {
            anchor_residue_name: residue.residue_name.clone(),
            anchor_atom_name,
            anchor_sequence_number: residue.id.sequence_number,
            anomeric_carbon_global: anomeric_carbon,
            patch,
        });
    }
    if sites.is_empty() {
        bail!("no glycan-to-protein junction bond was found");
    }
    Ok(sites)
}

fn extract_substructure(
    structure: &Structure,
    biopolymer: &Biopolymer,
    glycan_atoms: &[usize],
    chain_id: char,
) -> (Structure, std::collections::HashMap<usize, usize>) {
    let mut local_for_global = std::collections::HashMap::new();
    for (local, &global) in glycan_atoms.iter().enumerate() {
        local_for_global.insert(global, local);
    }

    let atoms: Vec<Atom> = glycan_atoms
        .iter()
        .map(|&global| structure.atoms[global].clone())
        .collect();

    let bonds: Vec<Bond> = structure
        .bonds
        .iter()
        .filter_map(|bond| {
            let a = *local_for_global.get(&bond.a)?;
            let b = *local_for_global.get(&bond.b)?;
            Some(Bond::with_type(a, b, bond.bond_type))
        })
        .collect();

    let mut residue_indices = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for &global in glycan_atoms {
        if let Some(Some(residue_index)) = biopolymer.residue_for_atom.get(global)
            && seen.insert(*residue_index)
        {
            residue_indices.push(*residue_index);
        }
    }

    let mut residue_local_for_global = std::collections::HashMap::new();
    for (local, &global) in residue_indices.iter().enumerate() {
        residue_local_for_global.insert(global, local);
    }

    let residues: Vec<ResidueRecord> = residue_indices
        .iter()
        .map(|&global| {
            let source = &biopolymer.residues[global];
            ResidueRecord {
                id: source.id.clone(),
                residue_name: source.residue_name.clone(),
                atom_indices: source
                    .atom_indices
                    .iter()
                    .filter_map(|gi| local_for_global.get(gi).copied())
                    .collect(),
                alpha_carbon: source
                    .alpha_carbon
                    .and_then(|gi| local_for_global.get(&gi).copied()),
                backbone_nitrogen: source
                    .backbone_nitrogen
                    .and_then(|gi| local_for_global.get(&gi).copied()),
                backbone_carbon: source
                    .backbone_carbon
                    .and_then(|gi| local_for_global.get(&gi).copied()),
                backbone_oxygen: source
                    .backbone_oxygen
                    .and_then(|gi| local_for_global.get(&gi).copied()),
                is_standard_amino_acid: source.is_standard_amino_acid,
            }
        })
        .collect();

    let residue_for_atom: Vec<Option<usize>> = glycan_atoms
        .iter()
        .map(|&global| {
            biopolymer
                .residue_for_atom
                .get(global)
                .and_then(|r| *r)
                .and_then(|residue_index| residue_local_for_global.get(&residue_index).copied())
        })
        .collect();

    let atom_name_for_atom: Vec<Option<String>> = glycan_atoms
        .iter()
        .map(|&global| biopolymer.atom_name(global).map(|name| name.to_string()))
        .collect();

    let chains = vec![ChainRecord {
        id: chain_id,
        residue_indices: (0..residues.len()).collect(),
    }];

    let sub_biopolymer = Biopolymer {
        residues,
        chains,
        secondary_structures: Vec::new(),
        residue_for_atom,
        atom_name_for_atom,
    };

    let mut sub_structure = Structure::with_bonds(structure.title.clone(), atoms, bonds);
    sub_structure.biopolymer = Some(sub_biopolymer);
    (sub_structure, local_for_global)
}

fn junction_bond_params(anchor_atom_name: &str) -> (f32, f32) {
    if anchor_atom_name.starts_with('O') {
        (0.1420, 267776.0)
    } else {
        (0.1448, 251040.0)
    }
}

fn neutralize_molecule_charges(atoms: &mut [crate::md::MoleculeAtom], external_charge: f32) {
    if atoms.is_empty() {
        return;
    }
    let total: f32 = external_charge + atoms.iter().map(|a| a.charge).sum::<f32>();
    let residual = total - total.round();
    if residual.abs() < 1.0e-6 {
        return;
    }
    let correction = residual / atoms.len() as f32;
    for atom in atoms.iter_mut() {
        atom.charge -= correction;
    }
}

fn apply_junction_patch_to_anomeric(
    glycan_molecule: &mut MoleculeType,
    anomeric_local: usize,
    patch: &JunctionPatch,
) {
    if let Some(atom) = glycan_molecule.atoms.get_mut(anomeric_local) {
        atom.charge += patch.anomeric_carbon_delta;
    }
}

fn apply_junction_patch_to_protein(
    parsed: &mut ProteinTopology,
    junction: &JunctionSite,
) -> Result<()> {
    for delta in &junction.patch.protein_deltas {
        parsed.adjust_atom_charge(
            junction.anchor_sequence_number,
            &junction.anchor_residue_name,
            delta.atom_name,
            delta.delta,
        );
    }
    Ok(())
}

fn render_glycan_atom_lines(
    glycan_molecule: &MoleculeType,
    atom_offset: usize,
    residue_offset: i32,
) -> Vec<String> {
    glycan_molecule
        .atoms
        .iter()
        .enumerate()
        .map(|(local, atom)| {
            let nr = atom_offset + local + 1;
            let resnr = residue_offset + atom.residue_number.unwrap_or(1);
            let residue = atom
                .residue_name
                .as_deref()
                .unwrap_or(&glycan_molecule.name);
            format!(
                "  {:<5} {:<6} {:<4} {:<6} {:<5} {:<5} {:>9.4}\n",
                nr, atom.species, resnr, residue, atom.atom_name, nr, atom.charge
            )
        })
        .collect()
}

fn render_glycan_bond_lines(glycan_molecule: &MoleculeType, atom_offset: usize) -> Vec<String> {
    glycan_molecule
        .bonds
        .iter()
        .map(|bond| {
            let a = atom_offset as u32 + bond.atoms[0];
            let b = atom_offset as u32 + bond.atoms[1];
            format!("  {:>4} {:>4} {}\n", a, b, bond.func)
        })
        .collect()
}

/// Render the glycan molecule's `[ pairs ]`, `[ angles ]` and `[ dihedrals ]`
/// (proper dihedrals then impropers, as the renderer for a standalone glycan
/// does) as directive blocks, each local atom index shifted by `atom_offset`
/// onto the merged numbering. Returns the lines (with their headers); empty when
/// the glycan carries no such terms.
fn render_glycan_bonded_blocks(glycan_molecule: &MoleculeType, atom_offset: usize) -> Vec<String> {
    let mut lines = Vec::new();
    push_term_section(&mut lines, "pairs", &glycan_molecule.pairs, atom_offset);
    push_term_section(&mut lines, "angles", &glycan_molecule.angles, atom_offset);
    if !glycan_molecule.dihedrals.is_empty() || !glycan_molecule.impropers.is_empty() {
        lines.push(String::new());
        lines.push("[ dihedrals ]".to_string());
        for term in glycan_molecule
            .dihedrals
            .iter()
            .chain(&glycan_molecule.impropers)
        {
            lines.push(render_term_line(term, atom_offset));
        }
    }
    lines
}

fn push_term_section(
    lines: &mut Vec<String>,
    name: &str,
    terms: &[BondedTerm],
    atom_offset: usize,
) {
    if terms.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("[ {name} ]"));
    for term in terms {
        lines.push(render_term_line(term, atom_offset));
    }
}

fn render_term_line(term: &BondedTerm, atom_offset: usize) -> String {
    let mut line = String::from(" ");
    for atom in &term.atoms {
        line.push_str(&format!(" {:>4}", atom_offset as u32 + atom));
    }
    line.push_str(&format!(" {}", term.func));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ResidueId;
    use crate::domain::biopolymer::{Biopolymer, ChainRecord, ResidueRecord};
    use crate::domain::structure::{Atom, Bond, BondType, Structure};
    use crate::workflows::glycan::glycoprotein::{GlycosylationKind, glycosylate_protein};
    use nalgebra::Point3;

    fn database() -> CarbTopologyDatabase {
        forcefield_assets::charmm36_carb_database().expect("bundled carb.rtp parses")
    }

    fn atom(element: &str, x: f32, y: f32, z: f32) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::new(x, y, z),
            charge: 0.0,
        }
    }

    fn asn_residue(seq: i32, atom_indices: Vec<usize>) -> ResidueRecord {
        ResidueRecord {
            id: ResidueId::new('A', seq, ' '),
            residue_name: "ASN".to_string(),
            atom_indices,
            alpha_carbon: None,
            backbone_nitrogen: None,
            backbone_carbon: None,
            backbone_oxygen: None,
            is_standard_amino_acid: true,
        }
    }

    fn protein_with_asn() -> Structure {
        let atoms = vec![
            atom("N", 0.0, 0.0, 0.0),
            atom("C", 1.5, 0.0, 0.0),
            atom("C", 2.5, 1.0, 0.0),
            atom("C", 3.0, 2.0, 0.0),
            atom("O", 2.5, 3.0, 0.0),
            atom("N", 4.3, 2.0, 0.0),
            atom("H", 4.8, 1.2, 0.0),
            atom("H", 4.8, 2.8, 0.0),
        ];
        let bonds = vec![
            Bond::with_type(0, 1, BondType::Single),
            Bond::with_type(1, 2, BondType::Single),
            Bond::with_type(2, 3, BondType::Single),
            Bond::with_type(3, 4, BondType::Double),
            Bond::with_type(3, 5, BondType::Single),
            Bond::with_type(5, 6, BondType::Single),
            Bond::with_type(5, 7, BondType::Single),
        ];
        let names = ["N", "CA", "CB", "CG", "OD1", "ND2", "HD21", "HD22"];
        let biopolymer = Biopolymer {
            residues: vec![asn_residue(1, (0..8).collect())],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom: vec![Some(0); 8],
            atom_name_for_atom: names.iter().map(|n| Some(n.to_string())).collect(),
        };
        let mut structure = Structure::with_bonds("asn".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    const SYNTHETIC_PROTEIN_TOP: &str = "\
; SilicoLab-generated topology

#include \"charmm36.ff/forcefield.itp\"

[ moleculetype ]
; name  nrexcl
Protein_chain_A  3

[ atoms ]
;   nr  type  resnr residue  atom  cgnr  charge   mass
     1  NH1    1     ASN      N     1    -0.4700  14.0070
     2  CT1    1     ASN      CA    2     0.0700  12.0110
     3  CT2    1     ASN      CB    3    -0.1800  12.0110
     4  CC     1     ASN      CG    4     0.5500  12.0110
     5  O      1     ASN      OD1   5    -0.5500  15.9994
     6  NH2    1     ASN      ND2   6    -0.6200  14.0070
     7  H      1     ASN      HD21  7     0.3200   1.0080
     8  H      1     ASN      HD22  8     0.3000   1.0080

[ bonds ]
;   ai    aj funct
     1     2 1
     2     3 1
     3     4 1
     4     5 1
     4     6 1
     6     7 1
     6     8 1

#include \"posre.itp\"

[ system ]
ASN

[ molecules ]
Protein_chain_A  1
";

    fn n_linked_glycoprotein() -> Structure {
        let protein = protein_with_asn();
        glycosylate_protein(
            &protein,
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            GlycosylationKind::NLinked,
        )
        .expect("glycosylation succeeds")
    }

    fn protein_with_two_asn() -> Structure {
        // Two ASN residues 20 Å apart in z so each glycan de-clashes independently.
        let names = ["N", "CA", "CB", "CG", "OD1", "ND2", "HD21", "HD22"];
        let residue_atoms = |dz: f32| {
            vec![
                atom("N", 0.0, 0.0, dz),
                atom("C", 1.5, 0.0, dz),
                atom("C", 2.5, 1.0, dz),
                atom("C", 3.0, 2.0, dz),
                atom("O", 2.5, 3.0, dz),
                atom("N", 4.3, 2.0, dz),
                atom("H", 4.8, 1.2, dz),
                atom("H", 4.8, 2.8, dz),
            ]
        };
        let residue_bonds = |off: usize| {
            vec![
                Bond::with_type(off, off + 1, BondType::Single),
                Bond::with_type(off + 1, off + 2, BondType::Single),
                Bond::with_type(off + 2, off + 3, BondType::Single),
                Bond::with_type(off + 3, off + 4, BondType::Double),
                Bond::with_type(off + 3, off + 5, BondType::Single),
                Bond::with_type(off + 5, off + 6, BondType::Single),
                Bond::with_type(off + 5, off + 7, BondType::Single),
            ]
        };
        let mut atoms = residue_atoms(0.0);
        atoms.extend(residue_atoms(20.0));
        let mut bonds = residue_bonds(0);
        bonds.extend(residue_bonds(8));
        let mut residue_for_atom = vec![Some(0); 8];
        residue_for_atom.extend(vec![Some(1); 8]);
        let atom_name_for_atom: Vec<Option<String>> = names
            .iter()
            .chain(names.iter())
            .map(|n| Some(n.to_string()))
            .collect();
        let biopolymer = Biopolymer {
            residues: vec![
                asn_residue(1, (0..8).collect()),
                asn_residue(2, (8..16).collect()),
            ],
            chains: vec![ChainRecord {
                id: 'A',
                residue_indices: vec![0, 1],
            }],
            secondary_structures: Vec::new(),
            residue_for_atom,
            atom_name_for_atom,
        };
        let mut structure = Structure::with_bonds("asn2".to_string(), atoms, bonds);
        structure.biopolymer = Some(biopolymer);
        structure
    }

    fn two_site_glycoprotein() -> Structure {
        let protein = protein_with_two_asn();
        let first = glycosylate_protein(
            &protein,
            "GlcNAc",
            ResidueId::new('A', 1, ' '),
            GlycosylationKind::NLinked,
        )
        .expect("first glycosylation succeeds");
        glycosylate_protein(
            &first,
            "GlcNAc",
            ResidueId::new('A', 2, ' '),
            GlycosylationKind::NLinked,
        )
        .expect("second glycosylation succeeds")
    }

    const TWO_ASN_PROTEIN_TOP: &str = "\
; SilicoLab-generated topology

#include \"charmm36.ff/forcefield.itp\"

[ moleculetype ]
; name  nrexcl
Protein_chain_A  3

[ atoms ]
;   nr  type  resnr residue  atom  cgnr  charge   mass
     1  NH1    1     ASN      N     1    -0.4700  14.0070
     2  CT1    1     ASN      CA    2     0.0700  12.0110
     3  CT2    1     ASN      CB    3    -0.1800  12.0110
     4  CC     1     ASN      CG    4     0.5500  12.0110
     5  O      1     ASN      OD1   5    -0.5500  15.9994
     6  NH2    1     ASN      ND2   6    -0.6200  14.0070
     7  H      1     ASN      HD21  7     0.3200   1.0080
     8  H      1     ASN      HD22  8     0.3000   1.0080
     9  NH1    2     ASN      N     9    -0.4700  14.0070
    10  CT1    2     ASN      CA   10     0.0700  12.0110
    11  CT2    2     ASN      CB   11    -0.1800  12.0110
    12  CC     2     ASN      CG   12     0.5500  12.0110
    13  O      2     ASN      OD1  13    -0.5500  15.9994
    14  NH2    2     ASN      ND2  14    -0.6200  14.0070
    15  H      2     ASN      HD21 15     0.3200   1.0080
    16  H      2     ASN      HD22 16     0.3000   1.0080

[ bonds ]
;   ai    aj funct
     1     2 1
     2     3 1
     3     4 1
     4     5 1
     4     6 1
     6     7 1
     6     8 1
     9    10 1
    10    11 1
    11    12 1
    12    13 1
    12    14 1
    14    15 1
    14    16 1

#include \"posre.itp\"

[ system ]
ASN

[ molecules ]
Protein_chain_A  1
";

    #[test]
    fn glycan_atoms_are_appended_with_reindexing() {
        let structure = n_linked_glycoprotein();
        let merged = merge_glycan_into_protein_topology_with(
            SYNTHETIC_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .expect("merge succeeds");

        let bio = structure.biopolymer.as_ref().unwrap();
        let glycan_atom_count = (0..structure.atoms.len())
            .filter(|&i| {
                bio.residue_for_atom
                    .get(i)
                    .and_then(|r| *r)
                    .and_then(|ri| bio.residues.get(ri))
                    .map(|r| is_carbohydrate_residue(&r.residue_name))
                    .unwrap_or(false)
            })
            .count();

        let atoms_block = section_text(&merged, "[ atoms ]");
        let nag_rows: Vec<&str> = atoms_block
            .lines()
            .filter(|line| line.contains("NAG"))
            .collect();
        assert_eq!(
            nag_rows.len(),
            glycan_atom_count,
            "every glycan atom is appended"
        );

        let first_glycan_nr: usize = nag_rows[0]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            first_glycan_nr, 9,
            "glycan atoms continue after protein nr 8"
        );

        let c1_row = nag_rows
            .iter()
            .find(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                cols.get(4) == Some(&"C1")
            })
            .expect("C1 present");
        let c1_resnr: i32 = c1_row.split_whitespace().nth(2).unwrap().parse().unwrap();
        assert_eq!(
            c1_resnr, 2,
            "glycan residue numbered after the protein residue"
        );
    }

    #[test]
    fn junction_bond_is_present() {
        let structure = n_linked_glycoprotein();
        let merged = merge_glycan_into_protein_topology_with(
            SYNTHETIC_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .unwrap();

        let bonds_block = section_text(&merged, "[ bonds ]");
        let atoms_block = section_text(&merged, "[ atoms ]");
        let c1_nr = glycan_atom_nr(&atoms_block, "C1").expect("C1 nr");

        let nd2_present = bonds_block.lines().any(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            cols.len() >= 2
                && ((cols[0] == "6" && cols[1] == c1_nr.to_string())
                    || (cols[1] == "6" && cols[0] == c1_nr.to_string()))
        });
        assert!(
            nd2_present,
            "the ND2(6)-C1({c1_nr}) junction bond must be present in:\n{bonds_block}"
        );
    }

    #[test]
    fn junction_patch_adjusts_anchor_and_anomeric_charges() {
        let structure = n_linked_glycoprotein();
        let merged = merge_glycan_into_protein_topology_with(
            SYNTHETIC_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .unwrap();

        let atoms_block = section_text(&merged, "[ atoms ]");
        let nd2_charge = atom_charge(&atoms_block, "ND2").expect("ND2 charge");
        let patch = patches::n_linked_junction_patch();
        let nd2_delta = patch
            .protein_deltas
            .iter()
            .find(|d| d.atom_name == "ND2")
            .unwrap()
            .delta;
        assert!(
            (nd2_charge - (-0.6200 + nd2_delta)).abs() < 1e-3,
            "ND2 charge should shift by the junction patch, got {nd2_charge}"
        );

        let baseline = baseline_anomeric_charge(&structure);
        let c1_charge = atom_charge(&atoms_block, "C1").expect("C1 charge");
        assert!(
            c1_charge > baseline
                && (c1_charge - (baseline + patch.anomeric_carbon_delta)).abs() < 0.05,
            "C1 charge should shift up by ~the anomeric junction delta, got {c1_charge} (baseline {baseline})"
        );
    }

    #[test]
    fn merged_glycoprotein_net_charge_is_integral() {
        let structure = n_linked_glycoprotein();
        let merged = merge_glycan_into_protein_topology_with(
            SYNTHETIC_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .unwrap();
        let atoms_block = section_text(&merged, "[ atoms ]");
        let total: f32 = atoms_block
            .lines()
            .filter_map(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                (cols.len() >= 7)
                    .then(|| cols[6].parse::<f32>().ok())
                    .flatten()
            })
            .sum();
        assert!(
            (total - total.round()).abs() < 1.0e-3,
            "merged glycoprotein net charge {total} should be integral"
        );
    }

    #[test]
    fn multi_site_glycoprotein_bonds_every_junction() {
        let structure = two_site_glycoprotein();
        let bio = structure.biopolymer.as_ref().unwrap();
        let junction_count = linkage_topology::cross_residue_linkages(&structure, bio)
            .iter()
            .filter(|cross| matches!(cross.linkage, BondLinkage::GlycanProtein { .. }))
            .count();
        assert_eq!(junction_count, 2, "the fixture has two glycosylation sites");

        let merged = merge_glycan_into_protein_topology_with(
            TWO_ASN_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .expect("merge succeeds");

        // Each ASN ND2 (protein atoms 6 and 14) must be bonded to a glycan
        // anomeric carbon, numbered past the 16 protein atoms.
        let bonds_block = section_text(&merged, "[ bonds ]");
        let junctions_from = |nd2: &str| {
            bonds_block
                .lines()
                .filter(|line| {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    if cols.len() < 2 {
                        return false;
                    }
                    let glycan = |s: &str| s.parse::<usize>().map(|n| n > 16).unwrap_or(false);
                    (cols[0] == nd2 && glycan(cols[1])) || (cols[1] == nd2 && glycan(cols[0]))
                })
                .count()
        };
        assert_eq!(
            junctions_from("6"),
            1,
            "ASN 1 ND2 must bond to its glycan:\n{bonds_block}"
        );
        assert_eq!(
            junctions_from("14"),
            1,
            "ASN 2 ND2 must bond to its glycan:\n{bonds_block}"
        );

        // Both anchors received the N-linked charge patch.
        let atoms_block = section_text(&merged, "[ atoms ]");
        let patch = patches::n_linked_junction_patch();
        let nd2_delta = patch
            .protein_deltas
            .iter()
            .find(|d| d.atom_name == "ND2")
            .unwrap()
            .delta;
        for resnr in ["1", "2"] {
            let charge = atoms_block
                .lines()
                .find_map(|line| {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    (cols.len() >= 7 && cols[2] == resnr && cols[3] == "ASN" && cols[4] == "ND2")
                        .then(|| cols[6].parse::<f32>().ok())
                        .flatten()
                })
                .expect("ND2 charge present");
            assert!(
                (charge - (-0.6200 + nd2_delta)).abs() < 1e-3,
                "ASN {resnr} ND2 should be patched, got {charge}"
            );
        }

        // Net charge stays integral with both sites merged. The summed total is
        // off from a whole number only by the 4-decimal rounding of each rendered
        // charge column, which accumulates with atom count — tolerance scales with
        // it rather than the single-site 1e-3.
        let charges: Vec<f32> = atoms_block
            .lines()
            .filter_map(|line| {
                let cols: Vec<&str> = line.split_whitespace().collect();
                (cols.len() >= 7)
                    .then(|| cols[6].parse::<f32>().ok())
                    .flatten()
            })
            .collect();
        let total: f32 = charges.iter().sum();
        let tolerance = 5.0e-5 * charges.len() as f32;
        assert!(
            (total - total.round()).abs() < tolerance,
            "merged two-site glycoprotein net charge {total} should be integral within {tolerance}"
        );
    }

    #[test]
    fn merged_top_carries_glycan_angles_dihedrals_and_pairs() {
        let structure = n_linked_glycoprotein();
        let merged = merge_glycan_into_protein_topology_with(
            SYNTHETIC_PROTEIN_TOP,
            &structure,
            forcefield_assets::CHARMM36_TOKEN,
            &database(),
        )
        .unwrap();

        // The synthetic protein top declares only [ atoms ] and [ bonds ], so any
        // [ angles ]/[ pairs ]/[ dihedrals ] that appear are the glycan's, and
        // every atom they reference must be a glycan atom (numbered past the 8
        // protein atoms).
        assert!(merged.contains("[ pairs ]"), "glycan 1-4 pairs missing");
        assert!(merged.contains("[ dihedrals ]"), "glycan dihedrals missing");

        let angles = section_text(&merged, "[ angles ]");
        let angle_rows: Vec<&str> = angles.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(!angle_rows.is_empty(), "glycan angles missing:\n{merged}");
        for row in angle_rows {
            let cols: Vec<&str> = row.split_whitespace().collect();
            assert_eq!(cols.len(), 4, "an angle is three atoms + func: {row}");
            for atom in &cols[..3] {
                assert!(
                    atom.parse::<usize>().unwrap() > 8,
                    "merged angle must reference glycan atoms only: {row}"
                );
            }
        }
    }

    fn baseline_anomeric_charge(structure: &Structure) -> f32 {
        let db = database();
        let bio = structure.biopolymer.as_ref().unwrap();
        let glycan_atoms = carbohydrate_atom_indices(structure, bio);
        let (glycan_structure, _) = extract_substructure(structure, bio, &glycan_atoms, 'G');
        let topology =
            build_glycan_topology_with(&glycan_structure, forcefield_assets::CHARMM36_TOKEN, &db)
                .expect("glycan typing");
        topology
            .molecules
            .first()
            .unwrap()
            .atoms
            .iter()
            .find(|a| a.atom_name == "C1")
            .unwrap()
            .charge
    }

    fn section_text(top: &str, header: &str) -> String {
        let lines: Vec<&str> = top.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("section header present");
        let mut out = String::new();
        for line in &lines[start + 1..] {
            if line.trim_start().starts_with('[') || line.trim_start().starts_with("#include") {
                break;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    fn glycan_atom_nr(atoms_block: &str, atom_name: &str) -> Option<usize> {
        atoms_block.lines().find_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.get(4) == Some(&atom_name) && cols.get(3) == Some(&"NAG") {
                cols[0].parse().ok()
            } else {
                None
            }
        })
    }

    fn atom_charge(atoms_block: &str, atom_name: &str) -> Option<f32> {
        atoms_block.lines().find_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.get(4) == Some(&atom_name) {
                cols.get(6).and_then(|c| c.parse().ok())
            } else {
                None
            }
        })
    }

    #[test]
    fn protein_only_structure_drops_glycan_atoms() {
        let structure = n_linked_glycoprotein();
        let bio = structure.biopolymer.as_ref().unwrap();
        let glycan_count = carbohydrate_atom_indices(&structure, bio).len();
        assert!(glycan_count > 0, "fixture has glycan atoms");

        let protein_only = protein_only_structure(&structure).expect("split succeeds");
        assert_eq!(
            protein_only.atoms.len(),
            structure.atoms.len() - glycan_count
        );
        let protein_bio = protein_only.biopolymer.as_ref().unwrap();
        let remaining_glycan = carbohydrate_atom_indices(&protein_only, protein_bio).len();
        assert_eq!(remaining_glycan, 0, "no carbohydrate atoms remain");
    }

    #[test]
    fn append_glycan_coordinates_matches_topology_order() {
        let structure = n_linked_glycoprotein();
        let bio = structure.biopolymer.as_ref().unwrap();
        let glycan_atoms = carbohydrate_atom_indices(&structure, bio);
        let protein_count = structure.atoms.len() - glycan_atoms.len();

        let mut gro = String::from("glycoprotein\n");
        gro.push_str(&format!("{protein_count:>5}\n"));
        for i in 0..protein_count {
            gro.push_str(&format!(
                "{:>5}{:<5}{:>5}{:>5}{:>8.3}{:>8.3}{:>8.3}\n",
                1,
                "ASN",
                "X",
                i + 1,
                0.0,
                0.0,
                0.0
            ));
        }
        gro.push_str("   5.00000   5.00000   5.00000\n");

        let appended = append_glycan_coordinates(&gro, &structure).expect("append succeeds");
        let lines: Vec<&str> = appended.lines().collect();
        let count: usize = lines[1].trim().parse().unwrap();
        assert_eq!(count, structure.atoms.len(), "count includes glycan atoms");
        assert_eq!(
            lines.len(),
            structure.atoms.len() + 3,
            "title + count + atoms + box line"
        );

        let first_glycan_global = glycan_atoms[0];
        let first_glycan_name = bio.atom_name(first_glycan_global).unwrap();
        let first_glycan_line = lines[2 + protein_count];
        assert_eq!(
            first_glycan_line.get(10..15).map(str::trim),
            Some(first_glycan_name),
            "first appended atom is the first carbohydrate atom"
        );
        assert!(
            appended.trim_end().ends_with("5.00000"),
            "box line is preserved as the last line"
        );
    }
}
