//! Cα-only secondary-structure assignment, for coordinates without HELIX/SHEET
//! records (e.g. GRO files from an MD run) that would otherwise render as coil.
//!
//! Uses the P-SEA criterion (Labesse & Mornon, 1997): a residue is helical or
//! extended when the Cα(i)→Cα(i+2/i+3/i+4) distances all fall in the matching
//! reference window. Cα-only keeps it robust to backbone atom naming and to
//! added/removed hydrogens across an engine round-trip.

use nalgebra::Point3;

use crate::domain::{
    Atom, Biopolymer, ChainRecord, ResidueId, SecondaryStructureKind, SecondaryStructureSpan,
};

/// Inclusive (min, max) reference window, in angstroms, for one Cα–Cα distance.
struct DistanceWindow {
    min: f32,
    max: f32,
}

impl DistanceWindow {
    const fn new(center: f32, tolerance: f32) -> Self {
        Self {
            min: center - tolerance,
            max: center + tolerance,
        }
    }

    fn contains(&self, value: f32) -> bool {
        value >= self.min && value <= self.max
    }
}

/// P-SEA Cα–Cα distance windows for an α-helix: `d2` = Cα(i)–Cα(i+2), and so on.
const HELIX_D2: DistanceWindow = DistanceWindow::new(5.5, 0.5);
const HELIX_D3: DistanceWindow = DistanceWindow::new(5.3, 0.5);
const HELIX_D4: DistanceWindow = DistanceWindow::new(6.4, 0.6);

/// P-SEA Cα–Cα distance windows for an extended β-strand.
const STRAND_D2: DistanceWindow = DistanceWindow::new(6.7, 0.6);
const STRAND_D3: DistanceWindow = DistanceWindow::new(9.9, 0.9);
const STRAND_D4: DistanceWindow = DistanceWindow::new(12.4, 1.1);

/// Minimum consecutive residues for a run to survive as a helix / strand;
/// shorter runs are geometric noise and revert to coil.
const MIN_HELIX_LEN: usize = 5;
const MIN_STRAND_LEN: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Label {
    Coil,
    Helix,
    Strand,
}

/// Derive helix/strand spans from the Cα trace of every protein chain. Each
/// contiguous run of Cα-bearing amino acids is assigned independently; solvent,
/// ions, and sequence gaps break a run.
pub fn assign_secondary_structure(
    atoms: &[Atom],
    biopolymer: &Biopolymer,
) -> Vec<SecondaryStructureSpan> {
    let mut spans = Vec::new();
    for chain in &biopolymer.chains {
        for fragment in protein_fragments(biopolymer, chain) {
            assign_fragment(atoms, biopolymer, &fragment, &mut spans);
        }
    }
    spans
}

/// Split a chain into maximal runs of consecutive, Cα-bearing amino-acid
/// residues. Returns residue indices into [`Biopolymer::residues`].
fn protein_fragments(biopolymer: &Biopolymer, chain: &ChainRecord) -> Vec<Vec<usize>> {
    let mut fragments = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut previous_seq: Option<i32> = None;

    for &residue_index in &chain.residue_indices {
        let residue = &biopolymer.residues[residue_index];
        let usable = residue.is_standard_amino_acid && residue.alpha_carbon.is_some();
        if !usable {
            // Solvent/ion/ligand, or a residue with no Cα, breaks the trace.
            if !current.is_empty() {
                fragments.push(std::mem::take(&mut current));
            }
            previous_seq = None;
            continue;
        }

        let contiguous = previous_seq.is_none_or(|seq| residue.id.sequence_number == seq + 1);
        if !contiguous && !current.is_empty() {
            fragments.push(std::mem::take(&mut current));
        }
        current.push(residue_index);
        previous_seq = Some(residue.id.sequence_number);
    }

    if !current.is_empty() {
        fragments.push(current);
    }
    fragments
}

/// Assign one contiguous protein fragment and append its helix/strand spans.
fn assign_fragment(
    atoms: &[Atom],
    biopolymer: &Biopolymer,
    fragment: &[usize],
    spans: &mut Vec<SecondaryStructureSpan>,
) {
    let ca: Vec<Point3<f32>> = fragment
        .iter()
        .map(|&residue_index| {
            let ca_index = biopolymer.residues[residue_index]
                .alpha_carbon
                .expect("fragment residues are filtered to those with a Cα");
            atoms[ca_index].position
        })
        .collect();

    let mut labels = vec![Label::Coil; ca.len()];

    // Each matching residue i seeds the five residues i..=i+4 its distances
    // constrain; helix wins over strand where the two overlap.
    for i in 0..ca.len().saturating_sub(4) {
        let d2 = (ca[i + 2] - ca[i]).norm();
        let d3 = (ca[i + 3] - ca[i]).norm();
        let d4 = (ca[i + 4] - ca[i]).norm();

        let seed = if HELIX_D2.contains(d2) && HELIX_D3.contains(d3) && HELIX_D4.contains(d4) {
            Label::Helix
        } else if STRAND_D2.contains(d2) && STRAND_D3.contains(d3) && STRAND_D4.contains(d4) {
            Label::Strand
        } else {
            continue;
        };

        for label in &mut labels[i..=i + 4] {
            if seed == Label::Helix || *label == Label::Coil {
                *label = seed;
            }
        }
    }

    enforce_min_length(&mut labels, Label::Helix, MIN_HELIX_LEN);
    enforce_min_length(&mut labels, Label::Strand, MIN_STRAND_LEN);
    emit_spans(biopolymer, fragment, &labels, spans);
}

/// Revert any run of `target` shorter than `min_len` back to coil.
fn enforce_min_length(labels: &mut [Label], target: Label, min_len: usize) {
    let mut start = 0;
    while start < labels.len() {
        if labels[start] != target {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < labels.len() && labels[end] == target {
            end += 1;
        }
        if end - start < min_len {
            for label in &mut labels[start..end] {
                *label = Label::Coil;
            }
        }
        start = end;
    }
}

/// Turn consecutive runs of helix/strand labels into [`SecondaryStructureSpan`]s.
fn emit_spans(
    biopolymer: &Biopolymer,
    fragment: &[usize],
    labels: &[Label],
    spans: &mut Vec<SecondaryStructureSpan>,
) {
    let mut index = 0;
    while index < labels.len() {
        let kind = match labels[index] {
            Label::Helix => SecondaryStructureKind::Helix,
            Label::Strand => SecondaryStructureKind::Sheet,
            Label::Coil => {
                index += 1;
                continue;
            }
        };
        let start = index;
        while index < labels.len() && labels[index] == labels[start] {
            index += 1;
        }
        spans.push(SecondaryStructureSpan {
            kind,
            start: residue_id(biopolymer, fragment[start]),
            end: residue_id(biopolymer, fragment[index - 1]),
        });
    }
}

fn residue_id(biopolymer: &Biopolymer, residue_index: usize) -> ResidueId {
    biopolymer.residues[residue_index].id.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PdbAtomAnnotation, build_biopolymer};
    use std::f32::consts::PI;

    /// One ALA residue per Cα, single chain numbered from 1 — enough for the
    /// Cα-only assignment.
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
            point += nalgebra::Vector3::new(
                BOND * HALF_ANGLE.cos(),
                sign * BOND * HALF_ANGLE.sin(),
                0.0,
            );
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
        // Water after a helix must not extend the helix span.
        let mut atoms: Vec<Atom> = ideal_helix(10)
            .iter()
            .map(|position| Atom {
                element: "C".to_string(),
                position: *position,
                charge: 0.0,
            })
            .collect();
        let mut annotations: Vec<PdbAtomAnnotation> = (0..10)
            .map(|index| PdbAtomAnnotation {
                atom_name: "CA".to_string(),
                residue_name: "ALA".to_string(),
                chain_id: 'A',
                residue_seq: index + 1,
                insertion_code: ' ',
            })
            .collect();
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
}
