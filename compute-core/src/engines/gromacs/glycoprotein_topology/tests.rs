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
        c1_charge > baseline && (c1_charge - (baseline + patch.anomeric_carbon_delta)).abs() < 0.05,
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
