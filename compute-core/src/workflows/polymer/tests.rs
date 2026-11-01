use super::*;

fn spec(units: &[&str], degree: usize) -> PolymerSpec {
    PolymerSpec {
        units: units.iter().map(|u| u.to_string()).collect(),
        degree,
        sequence: None,
        residue_name: None,
        seed: 7,
        relax: true,
        title: "test".to_string(),
    }
}

fn bonded_pairs(structure: &Structure) -> Vec<(usize, usize)> {
    structure
        .bonds
        .iter()
        .map(|b| (b.a.min(b.b), b.a.max(b.b)))
        .collect()
}

#[test]
fn polyethylene_decamer_has_expected_formula() {
    let result = build_polymer(&spec(&["[*]CC[*]"], 10)).unwrap();
    let structure = &result.structure;
    // C20H42: an H-capped chain of ten CH2–CH2 units.
    let carbons = structure.atoms.iter().filter(|a| a.element == "C").count();
    let hydrogens = structure.atoms.iter().filter(|a| a.element == "H").count();
    assert_eq!(carbons, 20);
    assert_eq!(hydrogens, 42);
    assert_eq!(result.report.atom_count, 62);
    // Every carbon carries four bonds, every hydrogen one.
    let mut degree = vec![0usize; structure.atoms.len()];
    for bond in &structure.bonds {
        degree[bond.a] += 1;
        degree[bond.b] += 1;
    }
    for (atom, degree) in structure.atoms.iter().zip(degree) {
        let expected = if atom.element == "C" { 4 } else { 1 };
        assert_eq!(degree, expected, "{} has {degree} bonds", atom.element);
    }
    assert_eq!(result.report.clashes, 0);
    assert!(
        result.report.end_to_end > 8.0,
        "end-to-end {}",
        result.report.end_to_end
    );
}

#[test]
fn residues_cover_every_atom_once_in_chain_order() {
    let result = build_polymer(&spec(&["[*]CC[*]"], 4)).unwrap();
    let biopolymer = result.structure.biopolymer.as_ref().unwrap();
    assert_eq!(biopolymer.residues.len(), 4);
    assert_eq!(biopolymer.chains.len(), 1);
    let mut covered = vec![0usize; result.structure.atoms.len()];
    for (index, residue) in biopolymer.residues.iter().enumerate() {
        assert_eq!(residue.id.sequence_number, index as i32 + 1);
        assert_eq!(residue.residue_name, "MOA");
        for &atom in &residue.atom_indices {
            covered[atom] += 1;
            assert_eq!(biopolymer.residue_for_atom[atom], Some(index));
        }
    }
    assert!(covered.iter().all(|c| *c == 1));
    // Interior units lose both caps; the end units keep one.
    assert_eq!(biopolymer.residues[0].atom_indices.len(), 7);
    assert_eq!(biopolymer.residues[1].atom_indices.len(), 6);
    assert_eq!(biopolymer.residues[3].atom_indices.len(), 7);
    assert!(biopolymer.atom_name(0).is_some());
}

#[test]
fn consecutive_residues_are_bonded() {
    let result = build_polymer(&spec(&["[*]CC([*])c1ccccc1"], 5)).unwrap();
    let biopolymer = result.structure.biopolymer.as_ref().unwrap();
    let pairs = bonded_pairs(&result.structure);
    for window in biopolymer.residues.windows(2) {
        let linked = pairs.iter().any(|(a, b)| {
            window[0].atom_indices.contains(a) && window[1].atom_indices.contains(b)
                || window[0].atom_indices.contains(b) && window[1].atom_indices.contains(a)
        });
        assert!(
            linked,
            "residues {:?} not bonded",
            window[0].id.sequence_number
        );
    }
}

#[test]
fn copolymer_sequence_cycles_and_names_units() {
    let mut spec = spec(&["[*]CC[*]", "[*]CC([*])c1ccccc1"], 5);
    spec.sequence = Some("AB".to_string());
    let result = build_polymer(&spec).unwrap();
    let names: Vec<_> = result
        .structure
        .biopolymer
        .as_ref()
        .unwrap()
        .residues
        .iter()
        .map(|r| r.residue_name.as_str())
        .collect();
    assert_eq!(names, ["MOA", "MOB", "MOA", "MOB", "MOA"]);
}

#[test]
fn copolymer_without_sequence_is_an_error() {
    let err = build_polymer(&spec(&["[*]CC[*]", "[*]CO[*]"], 4))
        .unwrap_err()
        .to_string();
    assert!(err.contains("--sequence"), "{err}");
}

#[test]
fn homopolymer_residue_name_is_honoured() {
    let mut spec = spec(&["[*]CC[*]"], 2);
    spec.residue_name = Some("PE".to_string());
    let result = build_polymer(&spec).unwrap();
    let biopolymer = result.structure.biopolymer.as_ref().unwrap();
    assert!(biopolymer.residues.iter().all(|r| r.residue_name == "PE"));
}

#[test]
fn different_seeds_give_different_conformations() {
    let a = build_polymer(&spec(&["[*]CC[*]"], 8)).unwrap();
    let mut other = spec(&["[*]CC[*]"], 8);
    other.seed = 99;
    let b = build_polymer(&other).unwrap();
    assert!((a.report.end_to_end - b.report.end_to_end).abs() > 1.0e-3);
}

#[test]
fn unrelaxed_growth_is_still_bonded_and_clash_free() {
    let mut spec = spec(&["[*]CC[*]"], 12);
    spec.relax = false;
    let result = build_polymer(&spec).unwrap();
    assert_eq!(result.report.atom_count, 12 * 6 + 2);
    assert_eq!(result.report.clashes, 0);
    assert!(!result.report.relax_skipped);
}

#[test]
fn the_same_seed_rebuilds_the_same_chain() {
    let spec = spec(&["[*]CC([*])c1ccccc1"], 6);
    let a = build_polymer(&spec).unwrap();
    let b = build_polymer(&spec).unwrap();
    let positions = |r: &PolymerResult| -> Vec<_> {
        r.structure.atoms.iter().map(|atom| atom.position).collect()
    };
    assert_eq!(positions(&a), positions(&b));
}
