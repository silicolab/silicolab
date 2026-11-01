use nalgebra::Point2;

use super::syntax::{parse_list, push_record, quote, split_fields, unquote};
use super::{read_molfile, write_molfile};
use crate::domain::sketch::{RepeatConnectivity, RepeatUnit, Sketch};
use crate::domain::smiles;

const SMALL_MOLECULE: &str = include_str!("../../../tests/fixtures/molfile/small_molecule.mol");
const POLYSTYRENE: &str = include_str!("../../../tests/fixtures/molfile/polystyrene.mol");
const QUATERNIZED_PPO: &str = include_str!("../../../tests/fixtures/molfile/quaternized_ppo.mol");
const PIPERIDINIUM: &str = include_str!("../../../tests/fixtures/molfile/piperidinium_polymer.mol");

fn assert_same_sketch(first: &Sketch, second: &Sketch) {
    assert_eq!(first.atoms.len(), second.atoms.len());
    for (a, b) in first.atoms.iter().zip(&second.atoms) {
        assert_eq!(a.element, b.element);
        assert_eq!(a.charge, b.charge);
        assert_eq!(a.explicit_hydrogens, b.explicit_hydrogens);
        assert!((a.pos - b.pos).norm() < 1.0e-3);
    }
    assert_eq!(first.bonds, second.bonds);
    assert_eq!(first.repeat_units.len(), second.repeat_units.len());
    for (a, b) in first.repeat_units.iter().zip(&second.repeat_units) {
        assert_eq!(a.atoms, b.atoms);
        assert_eq!(a.crossing_bonds, b.crossing_bonds);
        assert_eq!(a.label, b.label);
        assert_eq!(a.connectivity, b.connectivity);
        assert_eq!(a.brackets.len(), b.brackets.len());
        for (x, y) in a.brackets.iter().zip(&b.brackets) {
            assert!((x[0] - y[0]).norm() < 1.0e-3 && (x[1] - y[1]).norm() < 1.0e-3);
        }
    }
}

fn round_trip(text: &str) -> Sketch {
    let first = read_molfile(text).unwrap();
    let written = write_molfile(&first.sketch, &first.title).unwrap();
    assert!(written.lines().all(|line| line.chars().count() <= 80));
    let second = read_molfile(&written).unwrap();
    assert_eq!(first.title, second.title);
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
    assert_same_sketch(&first.sketch, &second.sketch);
    first.sketch
}

#[test]
fn small_molecule_round_trips() {
    let sketch = round_trip(SMALL_MOLECULE);
    assert_eq!(sketch.atoms.len(), 7);
    assert_eq!(sketch.bonds.len(), 7);
    assert_eq!(sketch.attachment_count(), 0);
    assert!(sketch.repeat_units.is_empty());
    let nitrogen = sketch.atoms.iter().position(|a| a.element == "N").unwrap();
    assert_eq!(sketch.implicit_hydrogens(nitrogen), 0);
}

#[test]
fn homopolymer_round_trips() {
    let sketch = round_trip(POLYSTYRENE);
    assert_eq!(sketch.attachment_count(), 2);
    let [unit] = &sketch.repeat_units[..] else {
        panic!("expected one repeat unit");
    };
    assert_eq!(unit.atoms.len(), 8);
    assert_eq!(unit.label, "n");
    assert_eq!(unit.connectivity, RepeatConnectivity::HeadToTail);
    assert_eq!(unit.brackets.len(), 2);
}

#[test]
fn random_copolymer_round_trips() {
    let sketch = round_trip(QUATERNIZED_PPO);
    let [x, y] = &sketch.repeat_units[..] else {
        panic!("expected two repeat units");
    };
    assert_eq!((x.label.as_str(), y.label.as_str()), ("x", "y"));
    assert_eq!(x.crossing_bonds[1], y.crossing_bonds[0]);
    let middle = |bracket: &[Point2<f32>; 2]| nalgebra::center(&bracket[0], &bracket[1]);
    let gap = (middle(&x.brackets[1]) - middle(&y.brackets[0])).norm();
    assert!((gap - 0.5).abs() < 0.01, "{gap}");
    let charged = |element: &str, charge: i32| {
        sketch
            .atoms
            .iter()
            .position(|a| a.element == element && a.charge == charge)
            .unwrap()
    };
    assert!(y.atoms.contains(&charged("N", 1)));
    let bromide = charged("Br", -1);
    assert!(sketch.neighbors(bromide).is_empty());
    assert!(!x.atoms.contains(&bromide) && !y.atoms.contains(&bromide));
}

#[test]
fn ring_cation_polymer_round_trips() {
    let sketch = round_trip(PIPERIDINIUM);
    let nitrogen = sketch.atoms.iter().position(|a| a.charge == 1).unwrap();
    assert_eq!(sketch.atoms[nitrogen].element, "N");
    assert_eq!(sketch.neighbors(nitrogen).len(), 4);
    assert_eq!(sketch.implicit_hydrogens(nitrogen), 0);
    assert!(sketch.repeat_units[0].atoms.contains(&nitrogen));
}

#[test]
fn rdkit_r_atoms_warn_and_become_attachments() {
    let read = read_molfile(POLYSTYRENE).unwrap();
    assert_eq!(read.warnings, ["read R atoms as attachment points"]);
    assert_eq!(read.title, "polystyrene");
}

fn with_atoms(atoms: &str, tail: &str) -> String {
    format!(
        "t\n\n\n  0  0  0     0  0            999 V3000\nM  V30 BEGIN CTAB\n\
         {atoms}{tail}M  V30 END CTAB\nM  END\n"
    )
}

fn atoms_only(line: &str) -> String {
    with_atoms(
        &format!("M  V30 COUNTS 1 0 0 0 0\nM  V30 BEGIN ATOM\nM  V30 {line}\nM  V30 END ATOM\n"),
        "",
    )
}

/// `C-C-C` with bonds 1 and 2 and the given SGROUP records.
fn chain_with_sgroups(count: usize, sgroups: &str) -> String {
    with_atoms(
        &format!(
            "M  V30 COUNTS 3 2 {count} 0 0\nM  V30 BEGIN ATOM\nM  V30 1 C 0 0 0 0\n\
             M  V30 2 C 1.5 0 0 0\nM  V30 3 C 3 0 0 0\nM  V30 END ATOM\n\
             M  V30 BEGIN BOND\nM  V30 1 1 1 2\nM  V30 2 1 2 3\nM  V30 END BOND\n"
        ),
        &format!("M  V30 BEGIN SGROUP\n{sgroups}M  V30 END SGROUP\n"),
    )
}

fn read_error(text: &str) -> String {
    format!("{:#}", read_molfile(text).unwrap_err())
}

#[test]
fn unsupported_content_is_a_useful_error() {
    assert!(read_error("").contains("header"));
    assert!(read_error("a\n\n\n  0  0  0     0  0            999 V2000\n").contains("V2000"));
    assert!(read_error(&atoms_only("1 A 0 0 0 0")).contains("query atom `A`"));
    assert!(read_error(&atoms_only("1 Q 0 0 0 0")).contains("query atom `Q`"));
    assert!(read_error(&atoms_only("1 [C,N] 0 0 0 0")).contains("unsupported atom type"));
    assert!(read_error(&atoms_only("1 Xx 0 0 0 0")).contains("unsupported atom type"));
    assert!(read_error(&atoms_only("1 C NaN 0 0 0")).contains("not finite"));
    assert!(read_error(&atoms_only("1 C 0 0 0 0 CHG=99")).contains("out of range"));
    assert!(read_error(&atoms_only("1 C 0 0")).contains("atom record"));
    let unterminated = "t\n\n\n  0  0  0     0  0            999 V3000\nM  V30 BEGIN CTAB\n";
    assert!(read_error(unterminated).contains("M  END"));
    let rgroup = with_atoms("M  V30 BEGIN RGROUP 1\n", "");
    assert!(read_error(&rgroup).contains("R-group"));
    let mon = chain_with_sgroups(1, "M  V30 1 MON 0 ATOMS=(1 2)\n");
    assert!(read_error(&mon).contains("MON is not supported"));
    let not_crossing = chain_with_sgroups(1, "M  V30 1 SRU 0 ATOMS=(3 1 2 3) XBONDS=(2 1 2)\n");
    assert!(read_error(&not_crossing).contains("does not cross"));
    let one = chain_with_sgroups(1, "M  V30 1 SRU 0 ATOMS=(1 2) XBONDS=(1 1)\n");
    assert!(read_error(&one).contains("exactly 2"));
    let three = chain_with_sgroups(1, "M  V30 1 SRU 0 ATOMS=(1 2) XBONDS=(3 1 2 2)\n");
    assert!(read_error(&three).contains("exactly 2"));
    for pseudo in ["Ala", "Pol", "CL", "D"] {
        let line = format!("1 {pseudo} 0 0 0 0");
        assert!(read_error(&atoms_only(&line)).contains("unsupported atom type"));
    }
    assert!(read_error(&atoms_only("1 C 0 0 0 0 VAL=2000000000")).contains("out of range"));
    assert!(read_error(&atoms_only("1 C 0 0 0 0 VAL=-5")).contains("out of range"));
    let bad_list = chain_with_sgroups(1, "M  V30 1 SRU 0 ATOMS=(5 2) XBONDS=(2 1 2)\n");
    assert!(read_error(&bad_list).contains("declares 5"));
    let bad_counts = chain_with_sgroups(2, "M  V30 1 SRU 0 ATOMS=(1 2) XBONDS=(2 1 2)\n");
    assert!(read_error(&bad_counts).contains("COUNTS declares"));
}

#[test]
fn copolymer_wrapper_is_dropped_only_around_repeat_units() {
    let wrapped = chain_with_sgroups(
        2,
        "M  V30 1 SRU 0 ATOMS=(1 2) XBONDS=(2 1 2) LABEL=x PARENT=2\n\
         M  V30 2 COP 0 ATOMS=(1 2) XBONDS=(2 1 2) SUBTYPE=RAN SUBTYPE=RAN\n",
    );
    let read = read_molfile(&wrapped).unwrap();
    assert_eq!(read.sketch.repeat_units.len(), 1);
    assert_eq!(
        read.warnings,
        ["dropped the COP wrapper and kept its SRU groups"]
    );

    let bare = chain_with_sgroups(1, "M  V30 1 COP 0 ATOMS=(1 2) XBONDS=(2 1 2)\n");
    assert!(read_error(&bare).contains("no SRU inside"));
}

#[test]
fn nested_repeat_units_warn() {
    let text = with_atoms(
        "M  V30 COUNTS 4 3 2 0 0\nM  V30 BEGIN ATOM\nM  V30 1 C 0 0 0 0\nM  V30 2 C 1 0 0 0\n\
         M  V30 3 C 2 0 0 0\nM  V30 4 C 3 0 0 0\nM  V30 END ATOM\nM  V30 BEGIN BOND\n\
         M  V30 1 1 1 2\nM  V30 2 1 2 3\nM  V30 3 1 3 4\nM  V30 END BOND\n",
        "M  V30 BEGIN SGROUP\nM  V30 1 SRU 0 ATOMS=(2 2 3) XBONDS=(2 1 3)\n\
         M  V30 2 SRU 0 ATOMS=(1 2) XBONDS=(2 1 2) PARENT=1\nM  V30 END SGROUP\n",
    );
    let warnings = read_molfile(&text).unwrap().warnings;
    assert!(
        warnings.iter().any(|w| w.contains("nested")),
        "{warnings:?}"
    );
}

#[test]
fn missing_connect_means_either_and_missing_brackets_are_generated() {
    let read = read_molfile(&chain_with_sgroups(
        1,
        "M  V30 1 SRU 0 ATOMS=(1 2) XBONDS=(2 1 2)\n",
    ))
    .unwrap();
    let unit = &read.sketch.repeat_units[0];
    assert_eq!(unit.connectivity, RepeatConnectivity::EitherUnknown);
    assert_eq!(unit.brackets.len(), 2);
    assert_eq!(unit.label, "");
}

#[test]
fn unsupported_features_warn() {
    let text = with_atoms(
        "M  V30 COUNTS 2 1 0 0 0\nM  V30 BEGIN ATOM\nM  V30 1 C 0 0 1 3 MASS=13 CFG=1\n\
         M  V30 2 C 1.5 0 0 0 RAD=2 FOO=1\nM  V30 END ATOM\n",
        "M  V30 BEGIN BOND\nM  V30 1 1 1 2 CFG=1\nM  V30 END BOND\n\
         M  V30 BEGIN COLLECTION\nM  V30 MDLV30/STEABS ATOMS=(1 1)\nM  V30 END COLLECTION\n",
    );
    let warnings = read_molfile(&text).unwrap().warnings.join("\n");
    for expected in [
        "z coordinates",
        "mapping",
        "MASS",
        "atom stereo",
        "RAD",
        "FOO",
        "bond stereo",
        "COLLECTION",
    ] {
        assert!(warnings.contains(expected), "{expected}: {warnings}");
    }
}

#[test]
fn coincident_atoms_get_a_generated_layout() {
    let text = with_atoms(
        "M  V30 COUNTS 2 1 0 0 0\nM  V30 BEGIN ATOM\nM  V30 1 C 0 0 0 0\n\
         M  V30 2 O 0 0 0 0\nM  V30 END ATOM\n",
        "M  V30 BEGIN BOND\nM  V30 1 1 1 2\nM  V30 END BOND\n",
    );
    let read = read_molfile(&text).unwrap();
    assert!(read.warnings[0].contains("no 2D coordinates"));
    assert!((read.sketch.atoms[0].pos - read.sketch.atoms[1].pos).norm() > 0.5);
}

#[test]
fn sketches_from_smiles_write_attachments_and_pinned_hydrogens() {
    let mut sketch = smiles::parse("[*]CC([*])c1cc[nH]c1").unwrap();
    let atoms = (0..sketch.atoms.len())
        .filter(|&atom| !sketch.atoms[atom].is_attachment())
        .collect();
    let crossing_bonds = [0, 2];
    sketch.repeat_units.push(RepeatUnit {
        atoms,
        crossing_bonds,
        label: "degree of polymerization".to_string(),
        connectivity: RepeatConnectivity::HeadToHead,
        brackets: Vec::new(),
    });

    let written = write_molfile(&sketch, "line\nbreak").unwrap();
    assert!(written.starts_with("line break\n"));
    assert!(written.contains(" * "));
    assert!(written.contains("VAL=4"));
    let read = read_molfile(&written).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.sketch.bonds, sketch.bonds);
    let nitrogen = read
        .sketch
        .atoms
        .iter()
        .position(|a| a.element == "N")
        .unwrap();
    assert_eq!(read.sketch.implicit_hydrogens(nitrogen), 1);
    assert_eq!(
        read.sketch.repeat_units[0].label,
        sketch.repeat_units[0].label
    );
    assert_eq!(read.sketch.repeat_units[0].brackets.len(), 2);
}

#[test]
fn inconsistent_sketches_are_write_errors() {
    let mut sketch = Sketch::new();
    sketch.add_atom("C", Point2::origin());
    sketch.repeat_units.push(RepeatUnit {
        atoms: vec![7],
        crossing_bonds: [0, 1],
        label: String::new(),
        connectivity: RepeatConnectivity::default(),
        brackets: Vec::new(),
    });
    assert!(write_molfile(&sketch, "t").is_err());

    let mut unknown = Sketch::new();
    unknown.add_atom("Xx", Point2::origin());
    assert!(write_molfile(&unknown, "t").is_err());

    let mut charged = smiles::parse("C").unwrap();
    charged.atoms[0].charge = 20;
    assert!(write_molfile(&charged, "t").is_err());

    let mut crowded = smiles::parse("C").unwrap();
    crowded.atoms[0].explicit_hydrogens = Some(15);
    assert!(write_molfile(&crowded, "t").is_err());
}

#[test]
fn long_records_wrap_and_rejoin() {
    let long: String = (1..=60)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let mut output = String::new();
    push_record(&mut output, &long);
    assert!(output.lines().count() > 1);
    assert!(output.lines().all(|line| line.len() <= 80));
    let lines: Vec<&str> = output.lines().chain(["M  END"]).collect();
    let records = super::syntax::collect_records(&lines, 0).unwrap().records;
    assert_eq!(records[0].text, long);
}

#[test]
fn field_syntax_handles_quotes_and_lists() {
    assert_eq!(
        split_fields("1 SRU LABEL=\"a b\" ATOMS=(2 1 2)").unwrap(),
        ["1", "SRU", "LABEL=\"a b\"", "ATOMS=(2 1 2)"]
    );
    assert!(split_fields("LABEL=\"open").is_err());
    assert!(split_fields("ATOMS=(2 1").is_err());
    assert!(split_fields("ATOMS=1)").is_err());
    assert_eq!(unquote(&quote("say \"hi\"")), "say \"hi\"");
    assert_eq!(quote("n"), "n");
    assert_eq!(parse_list("(2 4 5)").unwrap(), ["4", "5"]);
    assert!(parse_list("()").is_err());
    assert!(parse_list("(x 1)").is_err());
}

#[test]
fn arbitrary_input_never_panics() {
    let base = POLYSTYRENE.as_bytes();
    let mut state = 0x2545_f491_u32;
    for _ in 0..4000 {
        let mut bytes = base.to_vec();
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let index = state as usize % bytes.len();
            bytes[index] = b" (=\"-0x\n"[(state >> 24) as usize % 8];
        }
        let _ = read_molfile(&String::from_utf8_lossy(&bytes));
    }
    for cut in (0..POLYSTYRENE.len()).step_by(7) {
        let _ = read_molfile(&POLYSTYRENE[..cut]);
    }
}
