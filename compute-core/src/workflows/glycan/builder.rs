mod declash;

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use nalgebra::{Point3, Vector3};

use crate::domain::glycan::{
    self, Anomer, GlycanResidue, GlycanTree, Linkage, NodeId, RingTemplate,
};
use crate::domain::{
    Atom, Biopolymer, Bond, BondType, ChainRecord, ResidueId, ResidueRecord, Structure,
};
use crate::engines::forcefield;
use crate::workflows::assembly::stitch::{self, AcceptorSite, DonorSite};

use declash::declash;

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

/// Build a free oligosaccharide. Its reducing end takes the dictionary's default
/// configuration unless the notation states one.
pub fn glycan_to_structure(notation: &str, title: Option<&str>) -> Result<Structure> {
    let mut tree = glycan::parse(notation)?;
    glycan::resolve_root_anomer(&mut tree, None, None)?;
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| notation.to_string());
    tree_to_structure(&tree, title)
}

pub fn tree_to_structure(tree: &GlycanTree, title: String) -> Result<Structure> {
    let mut assembly = Assembly {
        atoms: Vec::new(),
        atom_names: Vec::new(),
        bonds: Vec::new(),
        rings: Vec::new(),
    };

    place_root(&mut assembly, tree)?;
    place_children(&mut assembly, tree, tree.root)?;

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
        glycan::dictionary::anomeric_carbon(residue.mono.kind).unwrap_or(1)
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

/// The dictionary entry realising a residue's exact stereochemistry. Both an
/// unresolved anomer and a configuration the dictionary has no residue for are
/// hard errors: the ring template would otherwise be built as if beta.
fn entry_for(residue: &GlycanResidue) -> Result<glycan::MonosaccharideEntry> {
    let mono = residue.mono;
    if mono.anomer == Anomer::Unknown {
        bail!(
            "the anomeric configuration of {:?} is unspecified; a `?` linkage cannot be built",
            mono.kind
        );
    }
    glycan::entry_for(mono).ok_or_else(|| {
        anyhow!(
            "no {}-{:?} in the monosaccharide dictionary",
            mono.anomer.name(),
            mono.kind
        )
    })
}

fn template_for(residue: &GlycanResidue) -> Result<RingTemplate> {
    entry_for(residue)?;
    glycan::ring_template(residue.mono)
        .ok_or_else(|| anyhow!("no ring template available for residue"))
}

fn residue_name(residue: &GlycanResidue) -> Result<String> {
    Ok(entry_for(residue)?.pdb_ccd.to_string())
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
        let excluded = declash::bonded_exclusions(structure);
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

        // The two arms are alpha-mannose (MAN); only the (b1-4) core mannose is
        // beta (BMA). The anomer is dictated by each residue's own linkage.
        let names: Vec<&str> = bio
            .residues
            .iter()
            .map(|r| r.residue_name.as_str())
            .collect();
        assert_eq!(names, vec!["NAG", "NAG", "BMA", "MAN", "MAN"]);

        assert!(category_counts(&structure) > 0);
    }

    /// The anomer in a linkage must reach the geometry, not merely the torsion
    /// preference: alpha and beta place the glycosidic oxygen on opposite faces.
    #[test]
    fn the_linkage_anomer_selects_the_residue_and_its_geometry() {
        let alpha = glycan_to_structure("Man(a1-3)Gal", None).unwrap();
        let beta = glycan_to_structure("Man(b1-3)Gal", None).unwrap();

        let names = |s: &Structure| -> Vec<String> {
            s.biopolymer
                .as_ref()
                .unwrap()
                .residues
                .iter()
                .map(|r| r.residue_name.clone())
                .collect()
        };
        assert_eq!(names(&alpha), vec!["GAL", "MAN"]);
        assert_eq!(names(&beta), vec!["GAL", "BMA"]);

        assert_eq!(alpha.atoms.len(), beta.atoms.len());
        let moved = (0..alpha.atoms.len())
            .filter(|&i| (alpha.atoms[i].position - beta.atoms[i].position).norm() > 0.1)
            .count();
        assert!(moved > 0, "the two anomers must not be the same structure");
    }

    #[test]
    fn an_unspecified_anomer_is_refused_rather_than_built_as_beta() {
        let err = glycan_to_structure("Man(?1-3)Gal", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unspecified"), "{err}");
    }

    /// The dictionary has no alpha-ManNAc, so ask rather than silently build beta.
    #[test]
    fn an_anomer_absent_from_the_dictionary_is_refused() {
        let err = glycan_to_structure("ManNAc(a1-3)Gal", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("alpha-ManNAc"), "{err}");
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
