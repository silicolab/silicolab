use super::*;
use crate::domain::{PdbAtomAnnotation, build_biopolymer};
use std::f32::consts::PI;

/// Atoms and PDB annotations for one protein chain (chain A, numbered from 1)
/// carrying a single residue per Cα position. Each residue gets a minimal
/// N–CA–C backbone so it reads as peptide-bonded protein; the amide N and
/// carbonyl C are placed on the Cα–Cα midpoints so consecutive C(i)/N(i+1)
/// coincide — a zero-length "bond" that keeps the geometry-based fragmenter
/// from splitting the trace. Only the Cα carries meaningful geometry; the
/// P-SEA assignment reads nothing else.
fn protein_backbone_atoms(ca_positions: &[Point3<f32>]) -> (Vec<Atom>, Vec<PdbAtomAnnotation>) {
    let mut atoms = Vec::new();
    let mut annotations = Vec::new();
    for (index, &ca) in ca_positions.iter().enumerate() {
        let previous = if index > 0 {
            ca_positions[index - 1]
        } else {
            ca
        };
        let next = if index + 1 < ca_positions.len() {
            ca_positions[index + 1]
        } else {
            ca
        };
        let nitrogen = Point3::from((previous.coords + ca.coords) * 0.5);
        let carbon = Point3::from((ca.coords + next.coords) * 0.5);
        let sequence = index as i32 + 1;
        for (atom_name, element, position) in
            [("N", "N", nitrogen), ("CA", "C", ca), ("C", "C", carbon)]
        {
            atoms.push(Atom {
                element: element.to_string(),
                position,
                charge: 0.0,
            });
            annotations.push(PdbAtomAnnotation {
                atom_name: atom_name.to_string(),
                residue_name: "ALA".to_string(),
                chain_id: 'A',
                residue_seq: sequence,
                insertion_code: ' ',
            });
        }
    }
    (atoms, annotations)
}

/// One ALA residue per Cα with a full N/CA/C backbone, single chain numbered
/// from 1 — enough for the Cα-only assignment.
fn ca_only_biopolymer(ca_positions: &[Point3<f32>]) -> (Vec<Atom>, Biopolymer) {
    let atoms: Vec<Atom> = ca_positions
        .iter()
        .map(|position| Atom {
            element: "C".to_string(),
            position: *position,
            charge: 0.0,
        })
        .collect();
    let annotations: Vec<PdbAtomAnnotation> = (0..ca_positions.len())
        .map(|index| PdbAtomAnnotation {
            atom_name: "CA".to_string(),
            residue_name: "ALA".to_string(),
            chain_id: 'A',
            residue_seq: index as i32 + 1,
            insertion_code: ' ',
        })
        .collect();
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("protein biopolymer");
    assert!(
        biopolymer
            .residues
            .iter()
            .all(|residue| !residue.has_peptide_backbone()),
        "test helper must exercise the real Ca-only fallback"
    );
    (atoms, biopolymer)
}

/// Idealized α-helix Cα trace: 1.5 Å rise, 100° turn, 2.3 Å radius.
fn ideal_helix(residues: usize) -> Vec<Point3<f32>> {
    const RADIUS: f32 = 2.3;
    const RISE: f32 = 1.5;
    const TURN: f32 = 100.0 * PI / 180.0;
    (0..residues)
        .map(|i| {
            let angle = TURN * i as f32;
            Point3::new(RADIUS * angle.cos(), RADIUS * angle.sin(), RISE * i as f32)
        })
        .collect()
}

/// Idealized β-strand Cα trace: planar zigzag, 3.8 Å bonds, 120° angle.
fn ideal_strand(residues: usize) -> Vec<Point3<f32>> {
    const BOND: f32 = 3.8;
    const HALF_ANGLE: f32 = 30.0 * PI / 180.0;
    let mut positions = Vec::with_capacity(residues);
    let mut point = Point3::new(0.0, 0.0, 0.0);
    positions.push(point);
    for i in 0..residues - 1 {
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        point +=
            nalgebra::Vector3::new(BOND * HALF_ANGLE.cos(), sign * BOND * HALF_ANGLE.sin(), 0.0);
        positions.push(point);
    }
    positions
}

fn kinds(spans: &[SecondaryStructureSpan]) -> Vec<(SecondaryStructureKind, i32, i32)> {
    spans
        .iter()
        .map(|span| {
            (
                span.kind,
                span.start.sequence_number,
                span.end.sequence_number,
            )
        })
        .collect()
}

#[test]
fn ideal_helix_is_assigned_as_helix() {
    let (atoms, biopolymer) = ca_only_biopolymer(&ideal_helix(12));
    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert_eq!(spans.len(), 1, "one contiguous helix expected: {spans:?}");
    assert_eq!(spans[0].kind, SecondaryStructureKind::Helix);
    // The whole trace is helical; ends may fray by at most a residue.
    assert!(spans[0].start.sequence_number <= 2);
    assert!(spans[0].end.sequence_number >= 11);
}

#[test]
fn ideal_strand_is_assigned_as_sheet() {
    let (atoms, biopolymer) = ca_only_biopolymer(&ideal_strand(10));
    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert_eq!(spans.len(), 1, "one contiguous strand expected: {spans:?}");
    assert_eq!(spans[0].kind, SecondaryStructureKind::Sheet);
}

#[test]
fn short_fragments_stay_coil() {
    // Four residues cannot fill the i..=i+4 window, so nothing is assigned.
    let (atoms, biopolymer) = ca_only_biopolymer(&ideal_helix(4));
    assert!(assign_secondary_structure(&atoms, &biopolymer).is_empty());
}

#[test]
fn helix_and_strand_in_one_chain_are_separated_by_coil() {
    // Helix, then a kink matching neither window, then a strand.
    let mut positions = ideal_helix(12);
    let coil = [
        Point3::new(5.0, 5.0, 20.0),
        Point3::new(8.5, 6.0, 21.5),
        Point3::new(12.0, 5.0, 20.0),
    ];
    positions.extend_from_slice(&coil);
    let last = *positions.last().unwrap();
    for offset in ideal_strand(10) {
        positions.push(last + offset.coords + nalgebra::Vector3::new(3.5, 0.0, 0.0));
    }

    let (atoms, biopolymer) = ca_only_biopolymer(&positions);
    let spans = assign_secondary_structure(&atoms, &biopolymer);
    let assigned = kinds(&spans);
    assert!(
        assigned
            .iter()
            .any(|(kind, ..)| *kind == SecondaryStructureKind::Helix),
        "expected a helix span: {assigned:?}"
    );
    assert!(
        assigned
            .iter()
            .any(|(kind, ..)| *kind == SecondaryStructureKind::Sheet),
        "expected a sheet span: {assigned:?}"
    );
    // Spans must not overlap and must keep file order.
    for window in spans.windows(2) {
        assert!(window[0].end.sequence_number < window[1].start.sequence_number);
    }
}

#[test]
fn non_protein_residues_do_not_break_assignment() {
    // Water after a helix must not extend the helix span. The water residue
    // has no peptide backbone, so topology alone keeps it out of the trace.
    let (mut atoms, mut annotations) = protein_backbone_atoms(&ideal_helix(10));
    atoms.push(Atom {
        element: "O".to_string(),
        position: Point3::new(50.0, 50.0, 50.0),
        charge: 0.0,
    });
    annotations.push(PdbAtomAnnotation {
        atom_name: "OW".to_string(),
        residue_name: "SOL".to_string(),
        chain_id: 'A',
        residue_seq: 11,
        insertion_code: ' ',
    });

    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, SecondaryStructureKind::Helix);
    assert!(
        spans[0].end.sequence_number <= 10,
        "helix must not run into the water"
    );
}

#[test]
fn non_standard_residue_names_do_not_fragment_assignment() {
    // A helix interleaving canonical and force-field/variant residue names
    // must stay one contiguous protein run: fragmentation depends on backbone
    // topology, never on whether each name is a standard amino acid. The old
    // name-gated fragmenter would split at every variant, dropping the helix
    // below its minimum length.
    let (atoms, mut annotations) = protein_backbone_atoms(&ideal_helix(12));
    let variants = ["HID", "HSE", "CYX", "GLH", "LYN", "ASH"];
    for annotation in annotations.iter_mut().filter(|a| a.residue_seq % 2 == 0) {
        let pick = (annotation.residue_seq as usize / 2) % variants.len();
        annotation.residue_name = variants[pick].to_string();
    }
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
    assert!(
        biopolymer
            .residues
            .iter()
            .any(|residue| !residue.is_standard_amino_acid),
        "test must include non-standard residue names"
    );

    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert_eq!(spans.len(), 1, "one contiguous helix expected: {spans:?}");
    assert_eq!(spans[0].kind, SecondaryStructureKind::Helix);
    assert!(spans[0].start.sequence_number <= 2);
    assert!(spans[0].end.sequence_number >= 11);
}

#[test]
fn renumbering_and_insertion_codes_do_not_fragment_assignment() {
    // The Cα geometry is one continuous helix, but the residues are renumbered
    // with large gaps and insertion codes. Contiguity is geometric, so the run
    // stays whole — the old `sequence_number == prev + 1` test would have split
    // it into length-1 runs and assigned nothing.
    let (atoms, mut annotations) = protein_backbone_atoms(&ideal_helix(12));
    for annotation in annotations.iter_mut() {
        let original = annotation.residue_seq - 1; // 0-based original index
        annotation.residue_seq = 10 + original * 5; // 10, 15, 20, … big gaps
        annotation.insertion_code = if original % 2 == 0 { 'A' } else { ' ' };
    }
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");

    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert_eq!(
        spans.len(),
        1,
        "one contiguous helix despite renumbering: {spans:?}"
    );
    assert_eq!(spans[0].kind, SecondaryStructureKind::Helix);
}

// --- Full N/CA/C/O backbones for the DSSP path -------------------------------

/// Place the fourth atom D of a chain A–B–C–D from the C–D bond length, the
/// bond angle ∠B-C-D, and the dihedral A-B-C-D (NeRF, Parsons et al. 2005).
fn place_atom(
    a: Point3<f32>,
    b: Point3<f32>,
    c: Point3<f32>,
    length: f32,
    angle_deg: f32,
    dihedral_deg: f32,
) -> Point3<f32> {
    let theta = angle_deg.to_radians();
    let chi = dihedral_deg.to_radians();
    let bc = (c - b).normalize();
    let n = (b - a).cross(&bc).normalize();
    let m = n.cross(&bc);
    let displacement = bc * (-length * theta.cos())
        + m * (length * theta.sin() * chi.cos())
        + n * (length * theta.sin() * chi.sin());
    c + displacement
}

/// Synthesize one protein chain (chain A, residues numbered from 1) from a
/// per-residue (φ, ψ) list, emitting a full N/CA/C/O backbone so the DSSP
/// hydrogen-bond path runs. ω is held trans (180°); the carbonyl O is placed in
/// the sp² plane along the external bisector of the CA(i)–C(i)–N(i+1) angle.
/// Standard Engh–Huber bond lengths (Å) and angles (degrees).
fn build_protein_backbone(phi_psi: &[(f32, f32)]) -> (Vec<Atom>, Vec<PdbAtomAnnotation>) {
    const N_CA: f32 = 1.458;
    const CA_C: f32 = 1.525;
    const C_N: f32 = 1.329;
    const C_O: f32 = 1.231;
    const ANGLE_N_CA_C: f32 = 111.0;
    const ANGLE_CA_C_N: f32 = 116.6;
    const ANGLE_C_N_CA: f32 = 121.7;
    const OMEGA: f32 = 180.0;

    let count = phi_psi.len();
    assert!(count >= 1, "need at least one residue");

    let mut nitrogen = Vec::with_capacity(count);
    let mut alpha = Vec::with_capacity(count);
    let mut carbon = Vec::with_capacity(count);

    // Seed the first three backbone atoms in the xy-plane. CA→N points along
    // −x; CA→C makes ∠N-CA-C with it, opening into the +y half-plane.
    nitrogen.push(Point3::new(0.0, 0.0, 0.0));
    alpha.push(Point3::new(N_CA, 0.0, 0.0));
    let seed_angle = (180.0 - ANGLE_N_CA_C).to_radians();
    carbon.push(
        alpha[0] + nalgebra::Vector3::new(CA_C * seed_angle.cos(), CA_C * seed_angle.sin(), 0.0),
    );

    for i in 0..count - 1 {
        let psi_i = phi_psi[i].1;
        let phi_next = phi_psi[i + 1].0;
        let n_next = place_atom(nitrogen[i], alpha[i], carbon[i], C_N, ANGLE_CA_C_N, psi_i);
        let ca_next = place_atom(alpha[i], carbon[i], n_next, N_CA, ANGLE_C_N_CA, OMEGA);
        let c_next = place_atom(carbon[i], n_next, ca_next, CA_C, ANGLE_N_CA_C, phi_next);
        nitrogen.push(n_next);
        alpha.push(ca_next);
        carbon.push(c_next);
    }

    let oxygen: Vec<Point3<f32>> = (0..count)
        .map(|i| {
            let to_ca = (alpha[i] - carbon[i]).normalize();
            let direction = if i + 1 < count {
                let to_n = (nitrogen[i + 1] - carbon[i]).normalize();
                -(to_ca + to_n)
            } else {
                // Terminal residue: no next N; point the O away from its CA.
                -to_ca
            };
            carbon[i] + direction.normalize() * C_O
        })
        .collect();

    let mut atoms = Vec::with_capacity(count * 4);
    let mut annotations = Vec::with_capacity(count * 4);
    for i in 0..count {
        for (name, element, position) in [
            ("N", "N", nitrogen[i]),
            ("CA", "C", alpha[i]),
            ("C", "C", carbon[i]),
            ("O", "O", oxygen[i]),
        ] {
            atoms.push(Atom {
                element: element.to_string(),
                position,
                charge: 0.0,
            });
            annotations.push(PdbAtomAnnotation {
                atom_name: name.to_string(),
                residue_name: "ALA".to_string(),
                chain_id: 'A',
                residue_seq: i as i32 + 1,
                insertion_code: ' ',
            });
        }
    }
    (atoms, annotations)
}

fn full_backbone_biopolymer(phi_psi: &[(f32, f32)]) -> (Vec<Atom>, Biopolymer) {
    let (atoms, annotations) = build_protein_backbone(phi_psi);
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("protein biopolymer");
    (atoms, biopolymer)
}

#[test]
fn ideal_alpha_helix_with_full_backbone_is_helix() {
    // φ/ψ ≈ (−57°, −47°) is the canonical right-handed α-helix; its i→i+4
    // backbone H-bonds drive the DSSP helix assignment.
    let phi_psi = vec![(-57.0, -47.0); 14];
    let (atoms, biopolymer) = full_backbone_biopolymer(&phi_psi);
    let spans = assign_secondary_structure(&atoms, &biopolymer);

    assert!(
        spans
            .iter()
            .any(|s| s.kind == SecondaryStructureKind::Helix),
        "expected a helix span: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .all(|s| s.kind == SecondaryStructureKind::Helix),
        "an ideal helix must never be labelled sheet: {spans:?}"
    );
    let helix_residues: i32 = spans
        .iter()
        .map(|s| s.end.sequence_number - s.start.sequence_number + 1)
        .sum();
    assert!(
        helix_residues >= 8,
        "helix should cover most of the chain: {spans:?}"
    );
}

#[test]
fn antiparallel_beta_hairpin_is_sheet() {
    // Two extended strands joined by a two-residue type-II′ turn fold into an
    // antiparallel hairpin whose reciprocal cross-strand H-bonds define a
    // β-sheet (one strand span per arm).
    let strand = (-139.0, 135.0);
    let mut phi_psi = vec![strand; 6];
    phi_psi.push((60.0, -120.0));
    phi_psi.push((-80.0, 0.0));
    phi_psi.extend(std::iter::repeat_n(strand, 6));
    let (atoms, biopolymer) = full_backbone_biopolymer(&phi_psi);
    let spans = assign_secondary_structure(&atoms, &biopolymer);

    assert!(
        spans
            .iter()
            .any(|s| s.kind == SecondaryStructureKind::Sheet),
        "expected a sheet span: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .all(|s| s.kind == SecondaryStructureKind::Sheet),
        "a β-hairpin must never be labelled helix: {spans:?}"
    );
}

#[test]
fn extended_strand_without_partner_is_coil() {
    // A lone fully extended strand carries β backbone dihedrals but has no
    // partner strand to hydrogen-bond with, so DSSP must leave it coil. The
    // Cα-only P-SEA criterion mislabels the whole thing β — this is the key
    // over-assignment regression the H-bond path fixes.
    let phi_psi = vec![(-139.0, 135.0); 12];
    let (atoms, biopolymer) = full_backbone_biopolymer(&phi_psi);
    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert!(
        spans.is_empty(),
        "an isolated extended strand must be coil, got: {spans:?}"
    );
}

#[test]
fn one_renamed_terminal_oxygen_keeps_the_dssp_path() {
    // A full-backbone extended strand must stay on the DSSP path (→ coil) even
    // when a single terminal carbonyl oxygen is renamed, as force fields do at
    // the C terminus. The majority-oxygen gate must not drop the whole fragment
    // back to the Cα-only criterion, which would over-assign it as β-strand.
    let phi_psi = vec![(-139.0, 135.0); 12];
    let (atoms, mut annotations) = build_protein_backbone(&phi_psi);
    let terminal_oxygen = annotations
        .iter_mut()
        .rev()
        .find(|annotation| annotation.atom_name == "O")
        .expect("a carbonyl oxygen to rename");
    terminal_oxygen.atom_name = "OXT".to_string();
    let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("protein biopolymer");

    let spans = assign_secondary_structure(&atoms, &biopolymer);
    assert!(
        spans.is_empty(),
        "DSSP must still run (→ coil) with one renamed terminal O: {spans:?}"
    );
}
