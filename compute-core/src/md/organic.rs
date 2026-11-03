//! An MD topology for a bonded organic system (packed polymer chains, guests,
//! fullerenes) assembled from typed molecule templates: each molecule in the
//! structure is matched to the template with the same atom sequence, and the
//! template's typed atoms and parameterised bonded terms become the molecule
//! type. Nothing is re-typed here; the templates come from a typer such as acpype.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

use crate::domain::{Structure, chemistry::normalized_symbol};
use crate::md::topology::{
    BondedTerm, MdTopology, MoleculeAtom, MoleculeRun, MoleculeType, Species, TopologyDefaults,
};
use crate::md::typed::{BondedInteraction, TypedMolecule};

/// A typed molecule the structure's molecules may be instances of.
#[derive(Debug, Clone)]
pub struct OrganicTemplate {
    /// Library name; becomes the molecule-type name.
    pub label: String,
    pub molecule: TypedMolecule,
}

#[derive(Debug, Clone)]
pub struct OrganicReport {
    /// Molecule-type name and how many copies, in first-seen order.
    pub counts: Vec<(String, usize)>,
    pub atom_count: usize,
    pub net_charge: f64,
}

/// GAFF/AMBER nonbonded conventions: Lorentz–Berthelot with scaled 1-4 terms.
pub const GAFF_DEFAULTS: TopologyDefaults = TopologyDefaults {
    comb_rule: 2,
    gen_pairs: true,
    fudge_lj: 0.5,
    fudge_qq: 0.8333,
};

/// Build the topology of `structure` from `templates`.
///
/// Molecules are the connected components of the structure's bonds and must be
/// contiguous in atom order (as `disorder` and `polymer build` lay them out). A
/// molecule matches a template when the element sequence agrees atom for atom;
/// `preferred` maps a residue name to the template to use when several match.
pub fn organic_topology(
    structure: &Structure,
    templates: &[OrganicTemplate],
    preferred: &BTreeMap<String, String>,
) -> Result<(MdTopology, OrganicReport)> {
    if structure.atoms.is_empty() {
        bail!("cannot build a topology for a structure with no atoms");
    }
    if templates.is_empty() {
        bail!(
            "no typed molecules to build from; type each distinct molecule with `forcefield type` \
             first"
        );
    }
    let elements: Vec<String> = structure
        .atoms
        .iter()
        .map(|atom| normalized_symbol(&atom.element))
        .collect();
    let signatures: Vec<Signature> = templates
        .iter()
        .map(|template| {
            Ok(Signature {
                elements: template_elements(&template.molecule)?,
                bonds: template_bond_graph(&template.molecule),
            })
        })
        .collect::<Result<_>>()?;

    let mut species: Vec<Species> = Vec::new();
    let mut molecules: Vec<MoleculeType> = Vec::new();
    let mut composition: Vec<MoleculeRun> = Vec::new();
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    let mut order: Vec<usize> = Vec::new();

    for (start, end) in molecule_ranges(structure)? {
        let residue = structure
            .biopolymer
            .as_ref()
            .and_then(|b| b.residue_for_atom.get(start).copied().flatten())
            .and_then(|r| b_residue_name(structure, r));
        let molecule = Signature {
            elements: elements[start..end].to_vec(),
            bonds: bond_graph(structure, start, end),
        };
        let index = match_template(
            &molecule,
            templates,
            &signatures,
            residue.as_deref(),
            preferred,
            start,
        )?;
        if !order.contains(&index) {
            order.push(index);
            let template = &templates[index];
            merge_species(&mut species, &template.molecule)?;
            molecules.push(molecule_type(template, residue.as_deref()));
        }
        *counts.entry(index).or_insert(0) += 1;
        let name = templates[index].label.clone();
        match composition.last_mut() {
            Some(run) if run.molecule == name => run.count += 1,
            _ => composition.push(MoleculeRun {
                molecule: name,
                count: 1,
            }),
        }
    }

    let title = structure.title.lines().next().unwrap_or("").trim();
    let topology = MdTopology {
        title: if title.is_empty() {
            "SilicoLab organic system".to_string()
        } else {
            title.to_string()
        },
        species,
        molecules,
        composition,
        defaults: Some(GAFF_DEFAULTS),
        bonded_params: Vec::new(),
        inline_force_field: None,
    };
    let report = OrganicReport {
        counts: order
            .iter()
            .map(|&index| (templates[index].label.clone(), counts[&index]))
            .collect(),
        atom_count: structure.atoms.len(),
        net_charge: topology.net_charge() as f64,
    };
    Ok((topology, report))
}

fn b_residue_name(structure: &Structure, residue: usize) -> Option<String> {
    structure
        .biopolymer
        .as_ref()?
        .residues
        .get(residue)
        .map(|r| r.residue_name.clone())
}

/// Contiguous `[start, end)` atom ranges of the structure's molecules, in order.
fn molecule_ranges(structure: &Structure) -> Result<Vec<(usize, usize)>> {
    let n = structure.atoms.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for bond in &structure.bonds {
        if bond.a >= n || bond.b >= n {
            bail!(
                "bond {}–{} refers to an atom the structure lacks",
                bond.a,
                bond.b
            );
        }
        let (a, b) = (find(&mut parent, bond.a), find(&mut parent, bond.b));
        if a != b {
            parent[a] = b;
        }
    }
    let mut bounds: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for atom in 0..n {
        let root = find(&mut parent, atom);
        let entry = bounds.entry(root).or_insert((atom, atom));
        entry.1 = atom;
    }
    let mut ranges: Vec<(usize, usize)> = bounds.values().map(|&(lo, hi)| (lo, hi + 1)).collect();
    ranges.sort_unstable();
    let mut cursor = 0;
    for &(start, end) in &ranges {
        if start != cursor {
            bail!(
                "atoms {}–{} belong to a molecule interleaved with another; molecules must be \
                 contiguous in atom order (re-pack the system)",
                start + 1,
                end
            );
        }
        cursor = end;
    }
    Ok(ranges)
}

/// Bonds among atoms `start..end`, as sorted molecule-local index pairs.
fn bond_graph(structure: &Structure, start: usize, end: usize) -> BTreeSet<(usize, usize)> {
    structure
        .bonds
        .iter()
        .filter(|bond| (start..end).contains(&bond.a) && (start..end).contains(&bond.b))
        .map(|bond| {
            let (a, b) = (bond.a - start, bond.b - start);
            (a.min(b), a.max(b))
        })
        .collect()
}

/// The template's `[ bonds ]` as sorted 0-based index pairs.
fn template_bond_graph(molecule: &TypedMolecule) -> BTreeSet<(usize, usize)> {
    molecule
        .bonds
        .iter()
        .filter_map(|bond| match bond.atoms.as_slice() {
            &[a, b] if a > 0 && b > 0 => {
                let (a, b) = (a as usize - 1, b as usize - 1);
                Some((a.min(b), a.max(b)))
            }
            _ => None,
        })
        .collect()
}

/// Element sequence and bond graph (molecule-local indices) of a molecule or
/// a template.
struct Signature {
    elements: Vec<String>,
    bonds: BTreeSet<(usize, usize)>,
}

/// A molecule matches a template when the element sequence agrees atom for
/// atom and, under that same index mapping, the bonds are the same: an
/// isomer packed in the same element order (1- vs 2-propanol) must not get
/// the other's bonded terms.
fn match_template(
    molecule: &Signature,
    templates: &[OrganicTemplate],
    signatures: &[Signature],
    residue: Option<&str>,
    preferred: &BTreeMap<String, String>,
    start: usize,
) -> Result<usize> {
    let elements = &molecule.elements;
    let same_atoms: Vec<usize> = signatures
        .iter()
        .enumerate()
        .filter(|(_, template)| template.elements == *elements)
        .map(|(index, _)| index)
        .collect();
    let candidates: Vec<usize> = same_atoms
        .iter()
        .copied()
        .filter(|&index| signatures[index].bonds == molecule.bonds)
        .collect();
    let describe = || {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for element in elements {
            *counts.entry(element).or_insert(0) += 1;
        }
        let formula: String = counts
            .iter()
            .map(|(element, count)| format!("{element}{count}"))
            .collect();
        match residue {
            Some(residue) => format!("molecule {residue} at atom {} ({formula})", start + 1),
            None => format!("molecule at atom {} ({formula})", start + 1),
        }
    };
    if let Some(residue) = residue
        && let Some(wanted) = preferred.get(residue)
    {
        return match templates
            .iter()
            .position(|t| t.label.eq_ignore_ascii_case(wanted))
        {
            Some(index) if candidates.contains(&index) => Ok(index),
            Some(index) if same_atoms.contains(&index) => bail!(
                "typed molecule `{wanted}` has the atoms of {} but different bonds (an isomer?); \
                 type the exact molecule that was packed",
                describe()
            ),
            Some(_) => bail!(
                "typed molecule `{wanted}` does not have the atoms of {} (same atoms in the same \
                 order are required; type the exact molecule that was packed)",
                describe()
            ),
            None => bail!("no typed molecule named `{wanted}` for residue {residue}"),
        };
    }
    match candidates.as_slice() {
        [] if !same_atoms.is_empty() => {
            let names: Vec<&str> = same_atoms
                .iter()
                .map(|&i| templates[i].label.as_str())
                .collect();
            bail!(
                "{} has the same atoms in the same order as {} but different bonds (an isomer?); \
                 type it with `forcefield type` and build again",
                describe(),
                names.join(", ")
            )
        }
        [] => bail!(
            "no typed molecule matches {}; type it with `forcefield type` and build again",
            describe()
        ),
        [index] => Ok(*index),
        several => {
            if let Some(residue) = residue
                && let Some(index) = several
                    .iter()
                    .copied()
                    .find(|&i| templates[i].label.eq_ignore_ascii_case(residue))
            {
                return Ok(index);
            }
            let names: Vec<&str> = several
                .iter()
                .map(|&i| templates[i].label.as_str())
                .collect();
            bail!(
                "{} matches several typed molecules ({}); pick one with `--typed {}=<name>`",
                describe(),
                names.join(", "),
                residue.unwrap_or("RES")
            )
        }
    }
}

fn template_elements(molecule: &TypedMolecule) -> Result<Vec<String>> {
    molecule
        .atoms
        .iter()
        .map(|atom| {
            element_from_mass(atom.mass_u)
                .or_else(|| element_from_name(&atom.name))
                .map(str::to_string)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cannot tell the element of typed atom `{}` (mass {})",
                        atom.name,
                        atom.mass_u
                    )
                })
        })
        .collect()
}

const ELEMENT_MASSES: &[(&str, f64, u32)] = &[
    ("H", 1.008, 1),
    ("B", 10.81, 5),
    ("C", 12.011, 6),
    ("N", 14.007, 7),
    ("O", 15.999, 8),
    ("F", 18.998, 9),
    ("Na", 22.990, 11),
    ("Mg", 24.305, 12),
    ("Si", 28.086, 14),
    ("P", 30.974, 15),
    ("S", 32.06, 16),
    ("Cl", 35.45, 17),
    ("K", 39.098, 19),
    ("Ca", 40.078, 20),
    ("Br", 79.904, 35),
    ("I", 126.904, 53),
];

fn element_from_mass(mass: f64) -> Option<&'static str> {
    ELEMENT_MASSES
        .iter()
        .find(|(_, reference, _)| (mass - reference).abs() < 0.4)
        .map(|(symbol, _, _)| *symbol)
}

fn element_from_name(name: &str) -> Option<&'static str> {
    let letters: String = name
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let symbol = normalized_symbol(&letters);
    ELEMENT_MASSES
        .iter()
        .find(|(candidate, _, _)| *candidate == symbol)
        .map(|(symbol, _, _)| *symbol)
}

fn atomic_number(mass: f64) -> u32 {
    ELEMENT_MASSES
        .iter()
        .find(|(_, reference, _)| (mass - reference).abs() < 0.4)
        .map(|(_, _, z)| *z)
        .unwrap_or(0)
}

/// Add the template's atom types to `species`, refusing a type that another
/// template already defined with different parameters. The mass comes from the
/// atoms carrying the type: acpype leaves the type table's mass column at zero.
fn merge_species(species: &mut Vec<Species>, molecule: &TypedMolecule) -> Result<()> {
    for atom_type in &molecule.atom_types {
        let mass = molecule
            .atoms
            .iter()
            .find(|atom| atom.type_name == atom_type.name)
            .map(|atom| atom.mass_u)
            .filter(|mass| *mass > 0.0)
            .unwrap_or(atom_type.mass_u);
        let candidate = Species {
            element: atom_type.name.clone(),
            atomic_number: atomic_number(mass),
            mass_u: mass as f32,
            charge: 0.0,
            sigma_angstrom: (atom_type.sigma_nm * 10.0) as f32,
            epsilon_kj_mol: atom_type.epsilon_kj_mol as f32,
        };
        match species.iter().find(|s| s.element == candidate.element) {
            None => species.push(candidate),
            Some(existing)
                if (existing.sigma_angstrom - candidate.sigma_angstrom).abs() > 1.0e-4
                    || (existing.epsilon_kj_mol - candidate.epsilon_kj_mol).abs() > 1.0e-4 =>
            {
                bail!(
                    "atom type `{}` is defined with different parameters by two typed molecules \
                     (σ {} vs {} Å); retype them with the same atom-type set",
                    candidate.element,
                    existing.sigma_angstrom,
                    candidate.sigma_angstrom
                )
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn molecule_type(template: &OrganicTemplate, residue: Option<&str>) -> MoleculeType {
    let molecule = &template.molecule;
    let residue_name = residue
        .map(str::to_string)
        .unwrap_or_else(|| residue_label(&template.label));
    let terms = |interactions: &[BondedInteraction]| -> Vec<BondedTerm> {
        interactions
            .iter()
            .map(|term| BondedTerm {
                atoms: term.atoms.clone(),
                func: term.func,
                params: term.params.clone(),
            })
            .collect()
    };
    let (impropers, dihedrals): (Vec<_>, Vec<_>) = molecule
        .dihedrals
        .iter()
        .partition(|term| term.func == 4 || term.func == 2);
    MoleculeType {
        name: template.label.clone(),
        nrexcl: molecule.nrexcl,
        atoms: molecule
            .atoms
            .iter()
            .map(|atom| MoleculeAtom {
                species: atom.type_name.clone(),
                atom_name: atom.name.clone(),
                charge: atom.charge as f32,
                residue_name: Some(residue_name.clone()),
                residue_number: Some(1),
            })
            .collect(),
        settle: None,
        bonds: terms(&molecule.bonds),
        pairs: terms(&molecule.pairs),
        angles: terms(&molecule.angles),
        dihedrals: terms(&dihedrals.into_iter().cloned().collect::<Vec<_>>()),
        impropers: terms(&impropers.into_iter().cloned().collect::<Vec<_>>()),
        exclusions: Vec::new(),
    }
}

fn residue_label(label: &str) -> String {
    let short: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    if short.is_empty() {
        "MOL".to_string()
    } else {
        short
    }
}

#[cfg(test)]
mod tests;
