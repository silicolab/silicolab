mod protein_top;

use anyhow::{Result, anyhow, bail};

use protein_top::ProteinTopology;

use crate::domain::biopolymer::{Biopolymer, ChainRecord, ResidueRecord, is_carbohydrate_residue};
use crate::domain::glycan::linkage_topology::{self, BondLinkage};
use crate::domain::glycan::patches::{self, JunctionPatch};
use crate::domain::structure::{Atom, Bond, Structure};
use crate::md::{BondedTerm, MdTopology, MoleculeType};

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

/// The modification-specific behavior the otherwise-generic protein-topology
/// merge is parameterized over: which atoms form the modifying group, how its
/// self-contained sub-topology is built, where it joins the protein, and the
/// junction bond force constants. The glycan path is one instance of this; any
/// future modification welded on as its own moleculetype reuses the same merge.
pub struct ModificationMerge<'a> {
    pub modifying_atoms: &'a dyn Fn(&Structure, &Biopolymer) -> Vec<usize>,
    pub junction_sites: &'a dyn Fn(&Structure, &Biopolymer) -> Result<Vec<JunctionSite>>,
    pub build_sub_topology: &'a dyn Fn(&Structure) -> Result<MdTopology>,
    pub junction_bond_params: &'a dyn Fn(&str) -> (f32, f32),
    pub empty_error: &'static str,
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
    let build = |sub: &Structure| build_glycan_topology_with(sub, force_field, database);
    let merge = ModificationMerge {
        modifying_atoms: &carbohydrate_atom_indices,
        junction_sites: &junction_sites,
        build_sub_topology: &build,
        junction_bond_params: &junction_bond_params,
        empty_error: "no carbohydrate atoms to merge into the protein topology",
    };
    merge_modification_into_protein_topology_with(protein_top, structure, &merge)
}

/// Merge a modification's self-contained moleculetype into a single-moleculetype
/// protein topology: append its atoms/bonds/bonded terms re-indexed onto the
/// protein numbering, patch the anchor/junction charges, and add the junction
/// bond(s). Modification-agnostic — see [`ModificationMerge`].
pub fn merge_modification_into_protein_topology_with(
    protein_top: &str,
    structure: &Structure,
    merge: &ModificationMerge,
) -> Result<String> {
    let biopolymer = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a modified-protein topology needs a biopolymer overlay"))?;
    if !biopolymer.is_compatible_with_atom_count(structure.atoms.len()) {
        bail!("the biopolymer overlay does not cover every atom");
    }

    let glycan_atoms = (merge.modifying_atoms)(structure, biopolymer);
    if glycan_atoms.is_empty() {
        bail!("{}", merge.empty_error);
    }

    // Every modifying-group-to-protein linkage gets its own junction bond and
    // charge patch. All modifying atoms (across every site) live in one
    // moleculetype; handling only the first junction would leave the other
    // sites' groups topologically free-floating in the MD system.
    let junctions = (merge.junction_sites)(structure, biopolymer)?;
    let (glycan_structure, local_for_global) =
        extract_substructure(structure, biopolymer, &glycan_atoms, 'G');
    let glycan_topology = (merge.build_sub_topology)(&glycan_structure)?;
    let glycan_molecule = glycan_topology
        .molecules
        .first()
        .ok_or_else(|| anyhow!("modification topology produced no molecule"))?;

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
        let (junction_b0, junction_kb) = (merge.junction_bond_params)(&junction.anchor_atom_name);
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
            other => bail!("no glycan junction topology for anchor {other:?}"),
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
mod tests;
