use std::collections::HashMap;

use anyhow::{Result, anyhow};
use nalgebra::{Point3, Rotation3, Unit, Vector3};

use crate::domain::glycan::{self, GlycanResidue, GlycanTree, Linkage, NodeId, RingTemplate};
use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord, Structure,
};
use crate::engines::forcefield;
use crate::workflows::assembly::stitch::{self, AcceptorSite, DonorSite};

use super::torsions;

const GLYCAN_CHAIN_ID: char = 'A';

struct PlacedRing {
    node: NodeId,
    residue_name: String,
    name_to_global: HashMap<String, usize>,
}

struct Assembly {
    atoms: Vec<Atom>,
    atom_names: Vec<Option<String>>,
    bonds: Vec<Bond>,
    rings: Vec<PlacedRing>,
}

pub fn glycan_to_structure(notation: &str, title: Option<&str>) -> Result<Structure> {
    let tree = glycan::parse(notation)?;
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| notation.to_string());

    let mut assembly = Assembly {
        atoms: Vec::new(),
        atom_names: Vec::new(),
        bonds: Vec::new(),
        rings: Vec::new(),
    };

    place_root(&mut assembly, &tree)?;
    place_children(&mut assembly, &tree, tree.root)?;

    let (atoms, atom_names, bonds, rings) = compact(assembly);

    let mut structure = Structure::with_bonds(title, atoms, bonds);
    declash(&mut structure);
    structure.biopolymer = Some(build_overlay(&rings, &atom_names, structure.atoms.len()));

    Ok(structure)
}

fn place_root(assembly: &mut Assembly, tree: &GlycanTree) -> Result<()> {
    let template = template_for(&tree.nodes[tree.root])?;
    let offset = assembly.atoms.len();
    let mut name_to_global = HashMap::new();
    for atom in &template.atoms {
        let index = assembly.atoms.len();
        assembly.atoms.push(Atom {
            element: atom.element.clone(),
            position: atom.position,
            charge: 0.0,
        });
        assembly.atom_names.push(Some(atom.name.clone()));
        name_to_global.insert(atom.name.clone(), index);
    }
    for bond in &template.bonds {
        assembly.bonds.push(Bond::with_type(
            offset + bond.a,
            offset + bond.b,
            bond.bond_type,
        ));
    }
    assembly.rings.push(PlacedRing {
        node: tree.root,
        residue_name: residue_name(&tree.nodes[tree.root])?,
        name_to_global,
    });
    Ok(())
}

fn place_children(assembly: &mut Assembly, tree: &GlycanTree, parent: NodeId) -> Result<()> {
    let children = tree.nodes[parent].children.clone();
    for (linkage, child) in children {
        place_edge(assembly, tree, parent, child, &linkage)?;
        place_children(assembly, tree, child)?;
    }
    Ok(())
}

fn place_edge(
    assembly: &mut Assembly,
    tree: &GlycanTree,
    parent: NodeId,
    child: NodeId,
    linkage: &Linkage,
) -> Result<()> {
    let template = template_for(&tree.nodes[child])?;
    let acceptor = acceptor_for(assembly, parent, linkage.parent_pos)?;
    let donor = donor_for(&template, &tree.nodes[child], linkage)?;

    let bond_length = forcefield::equilibrium_bond_length("C", "O", BondType::Single)?;
    let preference = torsions::preferred_torsion(linkage);
    let reference_normal = reference_normal(&preference);

    let child_atoms = &template.atoms;
    let child_bonds: Vec<(usize, usize)> = template.bonds.iter().map(|b| (b.a, b.b)).collect();

    let placement = stitch::place_fragment(
        child_atoms,
        &child_bonds,
        DonorSite {
            anomeric_atom: donor.binding_atom,
            outward: donor.outward,
        },
        acceptor.site,
        acceptor.position,
        bond_length,
        reference_normal,
    );

    let child_anomeric = format!("C{}", linkage.child_pos);
    let child_leaving_o = format!("O{}", donor.position_label);
    let child_leaving_h = format!("HO{}", donor.position_label);
    let parent_leaving_h = format!("HO{}", linkage.parent_pos);

    let mut name_to_global = HashMap::new();
    let mut anomeric_global = None;
    for atom in &placement.atoms {
        if atom.name == child_leaving_o || atom.name == child_leaving_h {
            continue;
        }
        let index = assembly.atoms.len();
        assembly.atoms.push(Atom {
            element: atom.element.clone(),
            position: atom.position,
            charge: 0.0,
        });
        assembly.atom_names.push(Some(atom.name.clone()));
        name_to_global.insert(atom.name.clone(), index);
        if atom.name == child_anomeric {
            anomeric_global = Some(index);
        }
    }

    for bond in &placement.bonds {
        let a_name = &placement.atoms[bond.a].name;
        let b_name = &placement.atoms[bond.b].name;
        let (Some(&a), Some(&b)) = (name_to_global.get(a_name), name_to_global.get(b_name)) else {
            continue;
        };
        assembly.bonds.push(Bond::with_type(a, b, BondType::Single));
    }

    remove_parent_hydrogen(assembly, parent, &parent_leaving_h);

    let anomeric_global =
        anomeric_global.ok_or_else(|| anyhow!("child anomeric carbon not placed"))?;
    let bridging_oxygen = acceptor.site.oxygen_atom;
    assembly.bonds.push(Bond::with_type(
        anomeric_global,
        bridging_oxygen,
        BondType::Single,
    ));

    assembly.rings.push(PlacedRing {
        node: child,
        residue_name: residue_name(&tree.nodes[child])?,
        name_to_global,
    });
    Ok(())
}

struct ResolvedAcceptor {
    site: AcceptorSite,
    position: Point3<f32>,
}

fn acceptor_for(assembly: &Assembly, parent: NodeId, parent_pos: u8) -> Result<ResolvedAcceptor> {
    let ring = assembly
        .rings
        .iter()
        .find(|ring| ring.node == parent)
        .ok_or_else(|| anyhow!("parent ring not placed"))?;
    let oxygen_name = format!("O{parent_pos}");
    let carbon_name = format!("C{parent_pos}");
    let oxygen = *ring
        .name_to_global
        .get(&oxygen_name)
        .ok_or_else(|| anyhow!("parent has no acceptor {oxygen_name}"))?;
    let outward = match ring.name_to_global.get(&carbon_name) {
        Some(&carbon) => (assembly.atoms[oxygen].position - assembly.atoms[carbon].position)
            .try_normalize(1.0e-4)
            .unwrap_or_else(Vector3::z),
        None => Vector3::z(),
    };
    Ok(ResolvedAcceptor {
        site: AcceptorSite {
            oxygen_atom: oxygen,
            outward,
        },
        position: assembly.atoms[oxygen].position,
    })
}

struct ResolvedDonor {
    binding_atom: usize,
    outward: Vector3<f32>,
    position_label: u8,
}

fn donor_for(
    template: &RingTemplate,
    residue: &GlycanResidue,
    linkage: &Linkage,
) -> Result<ResolvedDonor> {
    let anomeric_label = anomeric_label(residue, linkage);
    let carbon_name = format!("C{anomeric_label}");
    let binding_atom = template
        .atoms
        .iter()
        .position(|atom| atom.name == carbon_name)
        .ok_or_else(|| anyhow!("child has no anomeric carbon {carbon_name}"))?;
    let oxygen_name = format!("O{anomeric_label}");
    let outward = template
        .atoms
        .iter()
        .find(|atom| atom.name == oxygen_name)
        .map(|oxygen| {
            (oxygen.position - template.atoms[binding_atom].position)
                .try_normalize(1.0e-4)
                .unwrap_or(template.donor_site.coordination_position)
        })
        .unwrap_or(template.donor_site.coordination_position);
    Ok(ResolvedDonor {
        binding_atom,
        outward,
        position_label: anomeric_label,
    })
}

fn anomeric_label(residue: &GlycanResidue, linkage: &Linkage) -> u8 {
    if linkage.child_pos != 0 {
        linkage.child_pos
    } else {
        glycan::dictionary::supported_tokens()
            .into_iter()
            .filter_map(glycan::dictionary::lookup)
            .find(|entry| entry.mono == residue.mono)
            .map(|entry| entry.anomeric_carbon)
            .unwrap_or(1)
    }
}

fn remove_parent_hydrogen(assembly: &mut Assembly, parent: NodeId, hydrogen_name: &str) {
    let target = assembly
        .rings
        .iter()
        .find(|ring| ring.node == parent)
        .and_then(|ring| ring.name_to_global.get(hydrogen_name).copied());
    if let Some(index) = target {
        mark_removed(assembly, index);
    }
}

const REMOVED: &str = "\u{0}removed";

fn mark_removed(assembly: &mut Assembly, index: usize) {
    assembly.atoms[index].element = REMOVED.to_string();
    assembly
        .bonds
        .retain(|bond| bond.a != index && bond.b != index);
    for ring in &mut assembly.rings {
        ring.name_to_global.retain(|_, &mut v| v != index);
    }
}

fn compact(assembly: Assembly) -> (Vec<Atom>, Vec<Option<String>>, Vec<Bond>, Vec<CompactRing>) {
    let mut remap = vec![usize::MAX; assembly.atoms.len()];
    let mut atoms = Vec::new();
    let mut atom_names = Vec::new();
    for (old, atom) in assembly.atoms.iter().enumerate() {
        if atom.element == REMOVED {
            continue;
        }
        remap[old] = atoms.len();
        atoms.push(atom.clone());
        atom_names.push(assembly.atom_names[old].clone());
    }

    let mut bonds = Vec::new();
    for bond in &assembly.bonds {
        let (a, b) = (remap[bond.a], remap[bond.b]);
        if a != usize::MAX && b != usize::MAX {
            bonds.push(Bond::with_type(a, b, bond.bond_type));
        }
    }

    let rings = assembly
        .rings
        .iter()
        .map(|ring| CompactRing {
            residue_name: ring.residue_name.clone(),
            atom_indices: ring
                .name_to_global
                .values()
                .map(|&old| remap[old])
                .filter(|&new| new != usize::MAX)
                .collect(),
        })
        .collect();

    (atoms, atom_names, bonds, rings)
}

struct CompactRing {
    residue_name: String,
    atom_indices: Vec<usize>,
}

/// Resolve the residual steric clashes that stitching and idealized templates can
/// leave — the inter-residue overlap of branched arms at a glycosidic junction,
/// or the intra-residue overlap of two exocyclic arms (the sialic-acid acetamido
/// and glycerol tails). The subsystem's contract is *declash, not energy-
/// minimize*: this only rotates rigid fragments about rotatable single bonds —
/// every bond length, angle and ring stays exactly as built — until nothing
/// overlaps. A no-op for already-clean single residues and linear chains.
fn declash(structure: &mut Structure) {
    let axes = rotatable_axes(structure);
    if axes.is_empty() {
        return;
    }
    let excluded = bonded_exclusions(structure);
    for _ in 0..MAX_DECLASH_ROUNDS {
        let mut improved = false;
        for axis in &axes {
            if relieve_axis(structure, axis, &excluded) {
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
}

const MAX_DECLASH_ROUNDS: usize = 8;
const DECLASH_STEPS: usize = 24;

/// A rigid rotation degree of freedom: spin `subtree` about the line through
/// `pivot` (an on-axis pinned atom not in the subtree) and `axis_partner`. Both
/// the bond's atoms lie on the axis, so every bond — the rotated one included — is
/// length-preserved, and intra-subtree geometry is rigid: only the fragment's
/// orientation relative to the rest of the molecule changes.
struct TorsionAxis {
    pivot: usize,
    axis_partner: usize,
    subtree: Vec<usize>,
    in_subtree: Vec<bool>,
}

/// One rotation axis per rotatable single bond: every bond that is a graph bridge
/// (cutting it splits the molecule, so ring bonds are excluded) with a non-terminal
/// atom on each side. The smaller fragment is the one rotated. This covers the
/// glycosidic φ/ψ torsions and the exocyclic chain torsions alike.
fn rotatable_axes(structure: &Structure) -> Vec<TorsionAxis> {
    let atom_count = structure.atoms.len();
    let neighbors = neighbor_lists(structure);

    let mut axes = Vec::new();
    for bond in &structure.bonds {
        let (a, b) = (bond.a, bond.b);
        // A terminal atom (only this bond) has nothing to swing.
        if neighbors[a].len() < 2 || neighbors[b].len() < 2 {
            continue;
        }
        let Some(b_side) = subtree_excluding(&neighbors, b, a) else {
            continue; // ring bond: not a bridge
        };
        // Rotate the smaller fragment about the bond; its pivot is the bond atom on
        // the larger side, which stays put.
        let (pivot, partner, subtree) = if b_side.len() * 2 <= atom_count {
            (a, b, b_side)
        } else {
            let a_side = subtree_excluding(&neighbors, a, b).expect("bridge from a");
            (b, a, a_side)
        };
        axes.push(TorsionAxis::new(pivot, partner, subtree, atom_count));
    }
    axes
}

impl TorsionAxis {
    fn new(pivot: usize, axis_partner: usize, subtree: Vec<usize>, atom_count: usize) -> Self {
        let mut in_subtree = vec![false; atom_count];
        for &atom in &subtree {
            in_subtree[atom] = true;
        }
        Self {
            pivot,
            axis_partner,
            subtree,
            in_subtree,
        }
    }
}

/// Rotate `axis.subtree` to the multiple of 15° that most relieves its clashes
/// with the rest of the molecule. Returns whether it moved.
fn relieve_axis(
    structure: &mut Structure,
    axis: &TorsionAxis,
    excluded: &std::collections::HashSet<(usize, usize)>,
) -> bool {
    let pivot = structure.atoms[axis.pivot].position;
    let Some(direction) =
        (structure.atoms[axis.axis_partner].position - pivot).try_normalize(1.0e-5)
    else {
        return false;
    };
    let unit = Unit::new_normalize(direction);

    let base_penalty = axis_penalty(structure, axis, excluded);
    if base_penalty <= 1.0e-3 {
        return false;
    }

    let original: Vec<Point3<f32>> = axis
        .subtree
        .iter()
        .map(|&atom| structure.atoms[atom].position)
        .collect();

    let mut best_angle = 0.0_f32;
    let mut best_penalty = base_penalty;
    for step in 1..DECLASH_STEPS {
        let angle = step as f32 * std::f32::consts::TAU / DECLASH_STEPS as f32;
        let rotation = Rotation3::from_axis_angle(&unit, angle);
        apply_rotation(structure, axis, &original, pivot, &rotation);
        let penalty = axis_penalty(structure, axis, excluded);
        if penalty < best_penalty - 1.0e-3 {
            best_penalty = penalty;
            best_angle = angle;
        }
    }

    let rotation = Rotation3::from_axis_angle(&unit, best_angle);
    apply_rotation(structure, axis, &original, pivot, &rotation);
    best_angle != 0.0
}

fn apply_rotation(
    structure: &mut Structure,
    axis: &TorsionAxis,
    original: &[Point3<f32>],
    pivot: Point3<f32>,
    rotation: &Rotation3<f32>,
) {
    for (slot, &atom) in axis.subtree.iter().enumerate() {
        structure.atoms[atom].position = pivot + rotation * (original[slot] - pivot);
    }
}

/// Sum of squared steric overlaps between the rotated subtree and the rest of the
/// molecule (1–2 and 1–3 bonded pairs excluded). Subtree-internal distances are
/// rigid, so they are skipped.
fn axis_penalty(
    structure: &Structure,
    axis: &TorsionAxis,
    excluded: &std::collections::HashSet<(usize, usize)>,
) -> f32 {
    let mut penalty = 0.0;
    for &i in &axis.subtree {
        for j in 0..structure.atoms.len() {
            if axis.in_subtree[j] {
                continue;
            }
            let key = if i < j { (i, j) } else { (j, i) };
            if excluded.contains(&key) {
                continue;
            }
            let distance = (structure.atoms[i].position - structure.atoms[j].position).norm();
            let target = clash_target(&structure.atoms[i].element, &structure.atoms[j].element);
            if distance < target {
                let overlap = target - distance;
                penalty += overlap * overlap;
            }
        }
    }
    penalty
}

/// Minimum acceptable non-bonded contact distance (Å) — below the van der Waals
/// sum so ordinary close packing is not treated as a clash, but well above a
/// fused overlap.
fn clash_target(first: &str, second: &str) -> f32 {
    match (first == "H", second == "H") {
        (true, true) => 1.6,
        (false, false) => 2.4,
        _ => 1.9,
    }
}

fn neighbor_lists(structure: &Structure) -> Vec<Vec<usize>> {
    let mut neighbors = vec![Vec::new(); structure.atoms.len()];
    for bond in &structure.bonds {
        neighbors[bond.a].push(bond.b);
        neighbors[bond.b].push(bond.a);
    }
    neighbors
}

/// Atoms reachable from `start` without crossing the `start`–`blocked` bond.
/// Returns `None` when `blocked` is reachable by another path, i.e. the bond is
/// part of a cycle and rotating about it would tear the molecule.
fn subtree_excluding(neighbors: &[Vec<usize>], start: usize, blocked: usize) -> Option<Vec<usize>> {
    let mut visited = vec![false; neighbors.len()];
    visited[start] = true;
    let mut stack = vec![start];
    let mut subtree = vec![start];
    while let Some(atom) = stack.pop() {
        for &next in &neighbors[atom] {
            if next == blocked {
                if atom == start {
                    continue; // the cut bond itself
                }
                return None; // a cycle reaches the pinned partner
            }
            if !visited[next] {
                visited[next] = true;
                subtree.push(next);
                stack.push(next);
            }
        }
    }
    Some(subtree)
}

/// 1–2 and 1–3 bonded atom pairs, which are never steric clashes.
fn bonded_exclusions(structure: &Structure) -> std::collections::HashSet<(usize, usize)> {
    let neighbors = neighbor_lists(structure);
    let ordered = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
    let mut excluded = std::collections::HashSet::new();
    for (atom, bonded) in neighbors.iter().enumerate() {
        for &near in bonded {
            excluded.insert(ordered(atom, near));
            for &far in &neighbors[near] {
                if far != atom {
                    excluded.insert(ordered(atom, far));
                }
            }
        }
    }
    excluded
}

fn build_overlay(
    rings: &[CompactRing],
    atom_names: &[Option<String>],
    atom_count: usize,
) -> Biopolymer {
    let mut residues = Vec::new();
    let mut residue_for_atom = vec![None; atom_count];
    let mut residue_indices = Vec::new();
    for (sequence, ring) in rings.iter().enumerate() {
        let mut atom_indices = ring.atom_indices.clone();
        atom_indices.sort_unstable();
        let residue_index = residues.len();
        for &atom in &atom_indices {
            if atom < atom_count {
                residue_for_atom[atom] = Some(residue_index);
            }
        }
        residues.push(ResidueRecord {
            id: ResidueId::new(GLYCAN_CHAIN_ID, sequence as i32 + 1, ' '),
            residue_name: ring.residue_name.clone(),
            atom_indices,
            alpha_carbon: None,
            backbone_nitrogen: None,
            backbone_carbon: None,
            backbone_oxygen: None,
            is_standard_amino_acid: false,
        });
        residue_indices.push(residue_index);
    }

    let mut atom_name_for_atom = vec![None; atom_count];
    for (index, name) in atom_names.iter().enumerate() {
        if index < atom_count {
            atom_name_for_atom[index] = name.clone();
        }
    }

    Biopolymer {
        residues,
        chains: vec![ChainRecord {
            id: GLYCAN_CHAIN_ID,
            residue_indices,
        }],
        secondary_structures: Vec::new(),
        residue_for_atom,
        atom_name_for_atom,
    }
}

fn template_for(residue: &GlycanResidue) -> Result<RingTemplate> {
    glycan::ring_template(residue.mono)
        .ok_or_else(|| anyhow!("no ring template available for residue"))
}

fn residue_name(residue: &GlycanResidue) -> Result<String> {
    glycan::dictionary::supported_tokens()
        .into_iter()
        .filter_map(glycan::dictionary::lookup)
        .find(|entry| entry.mono == residue.mono)
        .map(|entry| entry.pdb_ccd.to_string())
        .ok_or_else(|| anyhow!("no PDB CCD code for residue"))
}

fn reference_normal(preference: &torsions::TorsionPreference) -> Vector3<f32> {
    let phi = preference.phi.to_radians();
    let psi = preference.psi.to_radians();
    Vector3::new(phi.cos() * psi.cos(), phi.sin(), psi.sin())
        .try_normalize(1.0e-4)
        .unwrap_or_else(Vector3::z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AtomCategory;

    fn category_counts(structure: &Structure) -> usize {
        (0..structure.atoms.len())
            .filter(|&i| structure.atom_category(i) == AtomCategory::Carbohydrate)
            .count()
    }

    fn has_bond_between_names(structure: &Structure, name_a: &str, name_b: &str) -> bool {
        let bio = structure.biopolymer.as_ref().unwrap();
        structure.bonds.iter().any(|bond| {
            let an = bio.atom_name(bond.a);
            let bn = bio.atom_name(bond.b);
            (an == Some(name_a) && bn == Some(name_b)) || (an == Some(name_b) && bn == Some(name_a))
        })
    }

    fn min_pairwise_distance(structure: &Structure) -> f32 {
        let mut min = f32::INFINITY;
        for i in 0..structure.atoms.len() {
            for j in (i + 1)..structure.atoms.len() {
                let d = (structure.atoms[i].position - structure.atoms[j].position).norm();
                if d < min {
                    min = d;
                }
            }
        }
        min
    }

    fn min_nonbonded_distance(structure: &Structure) -> f32 {
        let excluded = bonded_exclusions(structure);
        let mut min = f32::INFINITY;
        for i in 0..structure.atoms.len() {
            for j in (i + 1)..structure.atoms.len() {
                if excluded.contains(&(i, j)) {
                    continue;
                }
                min = min.min((structure.atoms[i].position - structure.atoms[j].position).norm());
            }
        }
        min
    }

    #[test]
    fn relaxed_glycan_has_no_overlapping_atoms() {
        let structure = glycan_to_structure(
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc",
            Some("n-glycan-core"),
        )
        .unwrap();
        assert!(
            min_pairwise_distance(&structure) > 0.5,
            "declashed glycan must not stack atoms"
        );
    }

    /// Idealized templates plus the rigid-fragment declash must leave bonds exactly
    /// as built and no atom pair in a steric clash — across the hexoses, the N-/O-
    /// acetylated sugars, the multi-tailed sialic acids (whose acetamido and
    /// glycerol arms once folded together) and a branched core.
    #[test]
    fn declashed_glycans_have_intact_bonds_and_no_steric_clash() {
        for notation in [
            "GlcNAc",
            "GalNAc",
            "Xyl", // pentose: C5 carries two H's, no C6/O6
            "Fuc", // L-config, C6 deoxymethyl
            "Neu5Ac",
            "Neu5Gc",
            "GlcA",
            "IdoA", // L-config uronate with the trigonal carboxylate
            "Neu5Ac(a2-3)Gal",
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc",
        ] {
            let structure = glycan_to_structure(notation, None).unwrap();

            for bond in &structure.bonds {
                let length =
                    (structure.atoms[bond.a].position - structure.atoms[bond.b].position).norm();
                assert!(
                    (0.90..=1.60).contains(&length),
                    "{notation}: bond length {length:.3} Å outside the ideal template range — \
                     the declash must not distort intra-residue geometry"
                );
            }

            let min_nonbonded = min_nonbonded_distance(&structure);
            assert!(
                min_nonbonded > 1.4,
                "{notation}: non-bonded atoms approach {min_nonbonded:.3} Å — a steric clash"
            );
        }
    }

    #[test]
    fn builds_a_single_monosaccharide() {
        let structure = glycan_to_structure("GlcNAc", Some("glcnac")).unwrap();
        let bio = structure.biopolymer.as_ref().unwrap();
        assert_eq!(bio.residues.len(), 1);
        assert_eq!(bio.residues[0].residue_name, "NAG");
        assert!(bio.is_compatible_with_atom_count(structure.atoms.len()));
        assert!(category_counts(&structure) > 0);
    }

    #[test]
    fn builds_the_n_glycan_core_pentasaccharide() {
        let structure = glycan_to_structure(
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc",
            Some("n-glycan-core"),
        )
        .unwrap();
        let bio = structure.biopolymer.as_ref().unwrap();
        assert_eq!(bio.residues.len(), 5);
        assert!(bio.is_compatible_with_atom_count(structure.atoms.len()));

        let names: Vec<&str> = bio
            .residues
            .iter()
            .map(|r| r.residue_name.as_str())
            .collect();
        assert_eq!(names.iter().filter(|n| **n == "NAG").count(), 2);
        assert!(
            names
                .iter()
                .filter(|n| **n == "MAN" || **n == "BMA")
                .count()
                >= 3
        );

        assert!(category_counts(&structure) > 0);
    }

    #[test]
    fn glycosidic_linkage_bonds_anomeric_carbon_to_acceptor_oxygen() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        assert!(has_bond_between_names(&structure, "C1", "O4"));
    }

    #[test]
    fn leaving_atoms_are_removed() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let bio = structure.biopolymer.as_ref().unwrap();
        let man_residue = bio
            .residues
            .iter()
            .find(|r| r.residue_name == "BMA")
            .unwrap();
        let man_has_o1 = man_residue
            .atom_indices
            .iter()
            .any(|&i| bio.atom_name(i) == Some("O1"));
        assert!(!man_has_o1);
    }
}
