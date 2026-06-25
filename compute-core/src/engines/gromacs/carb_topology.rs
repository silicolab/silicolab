use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::domain::glycan::dictionary;
use crate::domain::glycan::linkage_topology::{self, BondLinkage};
use crate::domain::glycan::patches;
use crate::domain::{Biopolymer, Structure};
use crate::md::{
    BondedTerm, MdTopology, MoleculeAtom, MoleculeRun, MoleculeType, TopologyDefaults, bonded_graph,
};

use super::forcefield_assets::{self, AtomTyping, CarbTopologyDatabase, TypingTable};

const GLYCAN_MOLECULE: &str = "GLY";

/// GROMACS interaction function for a 1-4 pair (LJ-14). The carb `[ bondedtypes ]`
/// row has no pair column — 1-4 pairs always use func 1 — so it is fixed here
/// rather than read from the force-field defaults.
const PAIR_FUNC: i32 = 1;

pub(crate) struct AtomTypeAssignment {
    pub(crate) species: String,
    pub(crate) atom_name: String,
    pub(crate) charge: f32,
    pub(crate) residue_name: String,
    pub(crate) residue_number: i32,
}

pub fn build_glycan_topology(structure: &Structure, force_field: &str) -> Result<MdTopology> {
    let database = forcefield_assets::charmm36_carb_database()?;
    build_glycan_topology_with(structure, force_field, &database)
}

pub fn build_glycan_topology_with(
    structure: &Structure,
    force_field: &str,
    database: &CarbTopologyDatabase,
) -> Result<MdTopology> {
    let biopolymer = structure
        .biopolymer
        .as_ref()
        .ok_or_else(|| anyhow!("a glycan topology needs a biopolymer overlay"))?;
    if !biopolymer.is_compatible_with_atom_count(structure.atoms.len()) {
        bail!("the biopolymer overlay does not cover every atom");
    }
    if structure.atoms.is_empty() {
        bail!("cannot build a glycan topology for a structure with no atoms");
    }

    let mut assignments = assign_atoms(structure, biopolymer, &database.typing)?;
    apply_linkage_patches(structure, biopolymer, &mut assignments);
    neutralize_assignment_charges(&mut assignments, 0.0);

    let atoms: Vec<MoleculeAtom> = assignments
        .iter()
        .map(|a| MoleculeAtom {
            species: a.species.clone(),
            atom_name: a.atom_name.clone(),
            charge: a.charge,
            residue_name: Some(a.residue_name.clone()),
            residue_number: Some(a.residue_number),
        })
        .collect();

    // Carbohydrate residues list their bonds (and a few impropers) explicitly,
    // but leave angles and proper dihedrals to be generated from connectivity —
    // exactly what pdb2gmx does and what we bypass by building the topology here.
    // Generating them from the structure's bond graph (which already carries the
    // inter-residue glycosidic bonds) yields the intra-ring bending/torsion terms
    // and the cross-linkage ones in one pass; without them a real trajectory has
    // nothing holding the ring or the glycosidic angles in shape.
    let defaults = database.defaults;
    let adjacency = bonded_graph::bond_adjacency(structure.atoms.len(), &structure.bonds);

    let molecule = MoleculeType {
        name: GLYCAN_MOLECULE.to_string(),
        nrexcl: defaults.nrexcl,
        atoms,
        settle: None,
        bonds: bonded_graph::bonds(&structure.bonds, defaults.bond_func),
        pairs: bonded_graph::one_four_pairs(&adjacency, PAIR_FUNC),
        angles: bonded_graph::angles(&adjacency, defaults.angle_func),
        dihedrals: bonded_graph::proper_dihedrals(&adjacency, defaults.proper_func),
        impropers: glycan_impropers(&assignments, database),
        exclusions: Vec::new(),
    };

    let title = structure.title.lines().next().unwrap_or("").trim();
    Ok(MdTopology {
        title: if title.is_empty() {
            "SilicoLab glycan".to_string()
        } else {
            title.to_string()
        },
        species: Vec::new(),
        molecules: vec![molecule],
        composition: vec![MoleculeRun {
            molecule: GLYCAN_MOLECULE.to_string(),
            count: 1,
        }],
        defaults: Some(charmm_defaults()),
        bonded_params: Vec::new(),
        inline_force_field: Some(forcefield_assets::glycan_force_field_includes(force_field)?),
    })
}

fn charmm_defaults() -> TopologyDefaults {
    TopologyDefaults {
        comb_rule: 2,
        gen_pairs: true,
        fudge_lj: 1.0,
        fudge_qq: 1.0,
    }
}

pub(crate) fn charmm_rtp_for_ccd(ccd: &str) -> Option<&'static str> {
    dictionary::supported_tokens()
        .into_iter()
        .filter_map(dictionary::lookup)
        .find(|entry| entry.pdb_ccd == ccd)
        .map(|entry| entry.charmm_rtp)
}

pub(crate) fn assign_atoms(
    structure: &Structure,
    biopolymer: &Biopolymer,
    typing: &TypingTable,
) -> Result<Vec<AtomTypeAssignment>> {
    let mut assignments = Vec::with_capacity(structure.atoms.len());
    for index in 0..structure.atoms.len() {
        let residue_index = biopolymer
            .residue_for_atom
            .get(index)
            .and_then(|r| *r)
            .ok_or_else(|| anyhow!("atom {index} is not assigned to a residue"))?;
        let residue = biopolymer
            .residues
            .get(residue_index)
            .ok_or_else(|| anyhow!("residue {residue_index} missing"))?;
        let ccd = residue.residue_name.as_str();
        let charmm = charmm_rtp_for_ccd(ccd)
            .ok_or_else(|| anyhow!("no CHARMM rtp name for PDB residue `{ccd}`"))?;
        let atom_name = biopolymer
            .atom_name(index)
            .ok_or_else(|| anyhow!("atom {index} in residue `{ccd}` has no name"))?;
        let typing_entry: &AtomTyping = typing
            .get(&(charmm.to_string(), atom_name.to_string()))
            .ok_or_else(|| anyhow!("no force-field typing for {charmm}.{atom_name} (PDB {ccd})"))?;
        assignments.push(AtomTypeAssignment {
            species: typing_entry.atom_type.clone(),
            atom_name: atom_name.to_string(),
            charge: typing_entry.charge,
            residue_name: ccd.to_string(),
            residue_number: residue_index as i32 + 1,
        });
    }
    Ok(assignments)
}

pub(crate) fn apply_linkage_patches(
    structure: &Structure,
    biopolymer: &Biopolymer,
    assignments: &mut [AtomTypeAssignment],
) {
    let patch = patches::hexopyranose_linkage_patch();
    for cross in linkage_topology::cross_residue_linkages(structure, biopolymer) {
        let BondLinkage::Glycosidic { carbon, oxygen } = cross.linkage else {
            continue;
        };
        if carbon >= assignments.len() || oxygen >= assignments.len() {
            continue;
        }
        assignments[oxygen].species = patch.bridge_oxygen_type.to_string();
        assignments[oxygen].charge = patch.bridge_oxygen_charge;
        assignments[carbon].charge += patch.anomeric_carbon_delta;
    }
}

pub(crate) fn neutralize_assignment_charges(
    assignments: &mut [AtomTypeAssignment],
    external_charge: f32,
) {
    if assignments.is_empty() {
        return;
    }
    let total: f32 = external_charge + assignments.iter().map(|a| a.charge).sum::<f32>();
    let residual = total - total.round();
    if residual.abs() < 1.0e-6 {
        return;
    }
    let correction = residual / assignments.len() as f32;
    for assignment in assignments.iter_mut() {
        assignment.charge -= correction;
    }
}

/// Improper-dihedral terms for every residue instance, mapping each residue's
/// rtp `[ impropers ]` atom *names* onto this molecule's 1-based atom indices.
/// Atom order within a term is preserved so grompp resolves it against the
/// order-specific improper `[ dihedraltypes ]`; a name that does not resolve in
/// its residue (an unexpected roster) drops just that one term.
pub(crate) fn glycan_impropers(
    assignments: &[AtomTypeAssignment],
    database: &CarbTopologyDatabase,
) -> Vec<BondedTerm> {
    // Group atoms by residue instance, keeping first-seen residue order and each
    // residue's atom-name -> 0-based molecule-atom-index map.
    let mut order: Vec<i32> = Vec::new();
    let mut by_residue: HashMap<i32, (String, HashMap<String, usize>)> = HashMap::new();
    for (index, assignment) in assignments.iter().enumerate() {
        let residue = by_residue
            .entry(assignment.residue_number)
            .or_insert_with(|| {
                order.push(assignment.residue_number);
                (assignment.residue_name.clone(), HashMap::new())
            });
        residue.1.insert(assignment.atom_name.clone(), index);
    }

    let mut terms = Vec::new();
    for residue_number in order {
        let (ccd, atom_index) = &by_residue[&residue_number];
        let Some(charmm) = charmm_rtp_for_ccd(ccd) else {
            continue;
        };
        let Some(residue_impropers) = database.impropers.get(charmm) else {
            continue;
        };
        for improper in residue_impropers {
            let resolved: Option<Vec<u32>> = improper
                .iter()
                .map(|name| atom_index.get(name).map(|&i| i as u32 + 1))
                .collect();
            if let Some(atoms) = resolved {
                terms.push(BondedTerm {
                    atoms,
                    func: database.defaults.improper_func,
                });
            }
        }
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::gromacs::topgen::render_top;
    use crate::workflows::glycan::glycan_to_structure;

    fn database() -> CarbTopologyDatabase {
        forcefield_assets::charmm36_carb_database().expect("bundled carb.rtp parses")
    }

    #[test]
    fn single_glcnac_resolves_types_and_charges() {
        let structure = glycan_to_structure("GlcNAc", Some("glcnac")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();

        assert_eq!(topo.molecules.len(), 1);
        let mol = &topo.molecules[0];
        assert_eq!(mol.atoms.len(), structure.atoms.len());

        let c1 = mol.atoms.iter().find(|a| a.atom_name == "C1").unwrap();
        assert_eq!(c1.species, "CC3162");
        let nitrogen = mol.atoms.iter().find(|a| a.atom_name == "N").unwrap();
        assert_eq!(nitrogen.species, "NC2D1");
        assert!((nitrogen.charge + 0.47).abs() < 1e-3);
    }

    #[test]
    fn sialylated_glycan_resolves_types_and_charges() {
        let structure = glycan_to_structure("Neu5Ac(a2-3)Gal", Some("sialyl")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];
        assert_eq!(mol.atoms.len(), structure.atoms.len());
        let bio = structure.biopolymer.as_ref().unwrap();
        let sia_index = bio
            .residues
            .iter()
            .position(|r| r.residue_name == "SIA")
            .unwrap() as i32
            + 1;
        let c2 = mol
            .atoms
            .iter()
            .find(|a| a.atom_name == "C2" && a.residue_number == Some(sia_index))
            .unwrap();
        assert_eq!(c2.species, "CC3062");
    }

    #[test]
    fn uronate_glycan_resolves_types_and_charges() {
        let structure = glycan_to_structure("GlcA(b1-3)Gal", Some("uronate")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];
        assert_eq!(mol.atoms.len(), structure.atoms.len());
        let bio = structure.biopolymer.as_ref().unwrap();
        let gca_index = bio
            .residues
            .iter()
            .position(|r| r.residue_name == "BDP")
            .unwrap() as i32
            + 1;
        let o61 = mol
            .atoms
            .iter()
            .find(|a| a.atom_name == "O61" && a.residue_number == Some(gca_index))
            .unwrap();
        assert_eq!(o61.species, "OC2D2");
    }

    #[test]
    fn topology_atom_count_matches_structure() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        assert_eq!(topo.atom_count(), structure.atoms.len());
    }

    #[test]
    fn glycosidic_linkage_patches_bridge_and_anomeric() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        let bio = structure.biopolymer.as_ref().unwrap();
        let nag_index = bio
            .residues
            .iter()
            .position(|r| r.residue_name == "NAG")
            .unwrap() as i32
            + 1;
        let bridge = mol
            .atoms
            .iter()
            .find(|a| a.atom_name == "O4" && a.residue_number == Some(nag_index))
            .unwrap();
        assert_eq!(
            bridge.species,
            patches::ETHER_BRIDGE_OXYGEN_TYPE,
            "bridge oxygen should be retyped to the ether type"
        );
        assert!(
            (bridge.charge - patches::ETHER_BRIDGE_OXYGEN_CHARGE).abs() < 0.02,
            "bridge oxygen should be near the ether charge after neutralization, got {}",
            bridge.charge
        );
    }

    #[test]
    fn glycan_net_charge_is_integral() {
        let db = database();
        for notation in ["GlcNAc", "Man(b1-4)GlcNAc", "GlcA(b1-3)Gal"] {
            let structure = glycan_to_structure(notation, Some("net")).unwrap();
            let topo =
                build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db)
                    .unwrap();
            let total: f32 = topo.molecules[0].atoms.iter().map(|a| a.charge).sum();
            assert!(
                (total - total.round()).abs() < 1.0e-4,
                "{notation} net charge {total} should be integral"
            );
        }
    }

    #[test]
    fn glycosidic_bond_is_present_in_index_bonds() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        let bio = structure.biopolymer.as_ref().unwrap();
        let c1_man = atom_index_of(&structure, "C1", "BMA");
        let o4_nag = atom_index_of(&structure, "O4", "NAG");
        let (c1, o4) = (c1_man.unwrap(), o4_nag.unwrap());
        let present = mol.bonds.iter().any(|b| {
            let pair = (b.atoms[0] as usize - 1, b.atoms[1] as usize - 1);
            pair == (c1.min(o4), c1.max(o4))
        });
        assert!(present, "glycosidic C1-O4 bond must appear in [bonds]");
        let _ = bio;
    }

    #[test]
    fn n_glycan_core_emits_a_single_moleculetype_with_includes() {
        let structure = glycan_to_structure(
            "Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc",
            Some("n-glycan-core"),
        )
        .unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();

        let top = render_top(&topo);
        assert_eq!(top.matches("[ moleculetype ]").count(), 1);
        assert!(top.contains("[ bonds ]"));
        assert!(top.contains("#include"));
        assert!(top.contains("ffbonded.itp"));
        assert!(!top.contains("forcefield.itp"));
        assert!(top.contains("[ defaults ]"));
        assert!(top.contains("1         2"));
    }

    #[test]
    fn glycan_generates_angles_dihedrals_and_pairs() {
        let structure = glycan_to_structure("GlcNAc", Some("glcnac")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        assert!(!mol.angles.is_empty(), "a sugar ring must carry angles");
        assert!(
            !mol.dihedrals.is_empty(),
            "a sugar ring must carry proper dihedrals"
        );
        assert!(!mol.pairs.is_empty(), "a sugar ring must carry 1-4 pairs");

        // CHARMM function codes: Urey-Bradley angles (5), multiple proper
        // dihedrals (9), LJ-14 pairs (1).
        assert!(mol.angles.iter().all(|t| t.func == 5 && t.atoms.len() == 3));
        assert!(
            mol.dihedrals
                .iter()
                .all(|t| t.func == 9 && t.atoms.len() == 4)
        );
        assert!(mol.pairs.iter().all(|t| t.func == 1 && t.atoms.len() == 2));
    }

    #[test]
    fn amide_improper_is_emitted_for_glcnac() {
        let structure = glycan_to_structure("GlcNAc", Some("glcnac")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        // The N-acetyl amide improper "C CT N O" must appear in rtp order, with
        // the CHARMM harmonic-improper function code (2).
        let expected: Vec<u32> = ["C", "CT", "N", "O"]
            .iter()
            .map(|name| atom_index_of(&structure, name, "NAG").unwrap() as u32 + 1)
            .collect();
        assert_eq!(db.defaults.improper_func, 2);
        assert!(
            mol.impropers
                .iter()
                .any(|t| t.atoms == expected && t.func == 2),
            "expected amide improper {expected:?} among {:?}",
            mol.impropers
        );
    }

    #[test]
    fn glycosidic_linkage_generates_cross_residue_angles() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        // The glycosidic bond C1(Man)-O4(GlcNAc) must bend: the angle centred on
        // the bridging O4, C1(Man)-O4-C4(GlcNAc), spans both residues.
        let c1_man = atom_index_of(&structure, "C1", "BMA").unwrap() as u32 + 1;
        let o4_nag = atom_index_of(&structure, "O4", "NAG").unwrap() as u32 + 1;
        let c4_nag = atom_index_of(&structure, "C4", "NAG").unwrap() as u32 + 1;
        let present = mol.angles.iter().any(|t| {
            let ends = [t.atoms[0], t.atoms[2]];
            t.atoms[1] == o4_nag && ends.contains(&c1_man) && ends.contains(&c4_nag)
        });
        assert!(
            present,
            "expected cross-residue angle C1(Man)-O4-C4(GlcNAc) centred on O4={o4_nag}"
        );
    }

    #[test]
    fn one_four_pairs_skip_one_three_neighbours() {
        let structure = glycan_to_structure("GlcNAc", Some("glcnac")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let mol = &topo.molecules[0];

        // C1 and C3 are 1-3 (bonded through C2), so they must not be a 1-4 pair.
        let c1 = atom_index_of(&structure, "C1", "NAG").unwrap() as u32 + 1;
        let c3 = atom_index_of(&structure, "C3", "NAG").unwrap() as u32 + 1;
        let is_pair = mol
            .pairs
            .iter()
            .any(|t| t.atoms == vec![c1.min(c3), c1.max(c3)]);
        assert!(!is_pair, "C1-C3 are 1-3 and must not appear as a 1-4 pair");
    }

    #[test]
    fn rendered_glycan_top_has_all_bonded_sections() {
        let structure = glycan_to_structure("Man(b1-4)GlcNAc", Some("disacc")).unwrap();
        let db = database();
        let topo =
            build_glycan_topology_with(&structure, forcefield_assets::CHARMM36_TOKEN, &db).unwrap();
        let top = render_top(&topo);
        assert!(top.contains("[ bonds ]"));
        assert!(top.contains("[ pairs ]"));
        assert!(top.contains("[ angles ]"));
        assert!(top.contains("[ dihedrals ]"));
    }

    fn atom_index_of(structure: &Structure, atom_name: &str, ccd: &str) -> Option<usize> {
        let bio = structure.biopolymer.as_ref()?;
        let residue_index = bio.residues.iter().position(|r| r.residue_name == ccd)?;
        (0..structure.atoms.len()).find(|&i| {
            bio.residue_for_atom.get(i).and_then(|r| *r) == Some(residue_index)
                && bio.atom_name(i) == Some(atom_name)
        })
    }
}
