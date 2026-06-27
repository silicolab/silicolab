//! Secondary-structure assignment for coordinates that arrive without HELIX/SHEET
//! records (e.g. files from an MD run) and would otherwise render as bare coil.
//!
//! Two methods, chosen per contiguous protein fragment by what its backbone
//! carries:
//!
//! * **DSSP** (Kabsch & Sander, 1983) when every residue has a full N/CA/C/O
//!   backbone: helices and strands are read from the backbone hydrogen-bond
//!   network, so an extended loop is not mistaken for a β-strand.
//! * **P-SEA** (Labesse & Mornon, 1997) as the Cα-only fallback: a residue is
//!   helical or extended when the Cα(i)→Cα(i+2/i+3/i+4) distances all fall in the
//!   matching reference window. Used when the carbonyl O is absent (a Cα-only
//!   trace), where no hydrogen-bond network can be computed. Cα-only keeps it
//!   robust to added/removed hydrogens across an engine round-trip.
//!
//! Full peptide backbones and contiguity are recognized from atoms and coordinates.
//! Legacy C-alpha-only traces keep the standard amino-acid name gate because they
//! have no N/C topology to distinguish them from hetero atoms.

use nalgebra::Point3;

use crate::domain::{
    Atom, Biopolymer, ChainRecord, ResidueId, SecondaryStructureKind, SecondaryStructureSpan,
    residues_backbone_bonded,
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
/// contiguous run of cartoon-trace residues is assigned independently; solvent,
/// ions, ligands, and genuine backbone breaks split a run. Full peptide residues
/// are recognized from N/CA/C topology; C-alpha-only residues keep the legacy
/// standard-amino-acid gate.
pub fn assign_secondary_structure(
    atoms: &[Atom],
    biopolymer: &Biopolymer,
) -> Vec<SecondaryStructureSpan> {
    let mut spans = Vec::new();
    for chain in &biopolymer.chains {
        for fragment in protein_fragments(atoms, biopolymer, chain) {
            assign_fragment(atoms, biopolymer, &fragment, &mut spans);
        }
    }
    spans
}

/// Split a chain into maximal runs of cartoon-trace residues whose backbones are
/// actually bonded in sequence (geometric contiguity). Returns residue indices
/// into [`Biopolymer::residues`].
fn protein_fragments(
    atoms: &[Atom],
    biopolymer: &Biopolymer,
    chain: &ChainRecord,
) -> Vec<Vec<usize>> {
    let mut fragments = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut previous: Option<usize> = None;

    for &residue_index in &chain.residue_indices {
        let residue = &biopolymer.residues[residue_index];
        if !residue.has_cartoon_trace() {
            // Solvent, ions, ligands, or residues without either a full peptide
            // backbone or a legacy standard-residue C-alpha trace break the run.
            if !current.is_empty() {
                fragments.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }

        let bonded = previous.is_none_or(|prev_index| {
            residues_backbone_bonded(&biopolymer.residues[prev_index], residue, atoms)
        });
        if !bonded && !current.is_empty() {
            fragments.push(std::mem::take(&mut current));
        }
        current.push(residue_index);
        previous = Some(residue_index);
    }

    if !current.is_empty() {
        fragments.push(current);
    }
    fragments
}

/// Assign one contiguous protein fragment and append its helix/strand spans. When
/// the fragment carries carbonyl oxygens (a full backbone) the DSSP hydrogen-bond
/// analysis is authoritative — it separates a genuine β-strand from a merely
/// extended loop, which the Cα-only criterion systematically over-assigns. The
/// P-SEA path is the fallback for backbone-incomplete inputs (a Cα-only trace).
fn assign_fragment(
    atoms: &[Atom],
    biopolymer: &Biopolymer,
    fragment: &[usize],
    spans: &mut Vec<SecondaryStructureSpan>,
) {
    let labels = dssp::assign_fragment(atoms, biopolymer, fragment)
        .unwrap_or_else(|| psea_labels(atoms, biopolymer, fragment));
    emit_spans(biopolymer, fragment, &labels, spans);
}

/// Cα-only P-SEA labelling: the fallback used when a fragment lacks the carbonyl
/// oxygens the DSSP path needs. Seeds helix/strand from the Cα(i)→Cα(i+2/3/4)
/// distance windows, then drops runs too short to be real.
fn psea_labels(atoms: &[Atom], biopolymer: &Biopolymer, fragment: &[usize]) -> Vec<Label> {
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
    labels
}

/// DSSP-style assignment from the full backbone hydrogen-bond network.
///
/// Used when every residue of a fragment carries an N/CA/C/O backbone. Unlike the
/// Cα-only P-SEA criterion it tells a real β-strand (cross-strand H-bonds) from a
/// merely extended loop, so it does not over-assign strand. All geometry is read
/// from atom coordinates — never residue names — so force-field protonation,
/// disulfide, and modified-residue renaming leave the assignment unchanged.
mod dssp {
    use nalgebra::Point3;

    use super::Label;
    use crate::domain::{Atom, Biopolymer};

    /// Kabsch–Sander electrostatic prefactor q1·q2·f, in kcal·Å/mol.
    const ENERGY_PREFACTOR: f32 = 27.888;
    /// A hydrogen bond is counted when its Kabsch–Sander energy is below this
    /// cutoff (kcal/mol) — the standard DSSP threshold.
    const HBOND_ENERGY_CUTOFF: f32 = -0.5;
    /// Donor/acceptor atoms closer than this (Å) are treated as a definite bond at
    /// a fixed strong energy, avoiding a singular 1/r term (mirrors DSSP).
    const MIN_ATOM_DISTANCE: f32 = 0.5;
    /// The energy assigned when two interacting atoms are implausibly close.
    const STRONG_BOND_ENERGY: f32 = -9.9;
    /// Minimum sequence separation for a hydrogen bond to count, in residues.
    const MIN_HBOND_SEPARATION: usize = 2;
    /// Minimum sequence separation between the two partners of a β-bridge.
    const MIN_BRIDGE_SEPARATION: i64 = 3;

    /// The hydrogen-bonding backbone atom positions of one residue. The α-carbon is
    /// not needed: the H-bond network is defined entirely by N, C, and O. N and C
    /// are always present (the fragment is pre-filtered to peptide backbones); the
    /// carbonyl O is optional, because some force fields rename the C-terminal
    /// carbonyl oxygen (OT1/OT2/OXT…). A residue without O simply cannot donate a
    /// hydrogen bond and its successor cannot place an amide H — a local effect,
    /// not a reason to abandon DSSP for the whole fragment.
    struct Backbone {
        nitrogen: Point3<f32>,
        carbon: Point3<f32>,
        oxygen: Option<Point3<f32>>,
    }

    /// Assign a Helix/Strand/Coil label to every residue of one fragment, or `None`
    /// when the fragment is not a full backbone (a Cα/N-CA-C-only trace with no
    /// carbonyl oxygens) — the caller then falls back to P-SEA. Indices are local
    /// to the fragment; "adjacent" means consecutive within it.
    pub fn assign_fragment(
        atoms: &[Atom],
        biopolymer: &Biopolymer,
        fragment: &[usize],
    ) -> Option<Vec<Label>> {
        let backbone: Vec<Backbone> = fragment
            .iter()
            .map(|&residue_index| {
                let residue = &biopolymer.residues[residue_index];
                Some(Backbone {
                    nitrogen: atoms[residue.backbone_nitrogen?].position,
                    carbon: atoms[residue.backbone_carbon?].position,
                    oxygen: residue.backbone_oxygen.map(|index| atoms[index].position),
                })
            })
            .collect::<Option<Vec<_>>>()?;

        let count = backbone.len();

        // The DSSP path needs the carbonyl hydrogen-bond network. With oxygens on
        // only a minority of residues this is a Cα/N-CA-C-only trace, so defer to
        // the P-SEA fallback; a full backbone missing a renamed terminal O or two
        // still clears the bar and is assigned by H-bonds.
        let with_oxygen = backbone.iter().filter(|b| b.oxygen.is_some()).count();
        if with_oxygen * 2 <= count {
            return None;
        }

        // Amide H of residue i (i ≥ 1) sits 1 Å from N along the preceding
        // residue's carbonyl C→O direction. The first residue has no preceding
        // carbonyl, and a residue whose predecessor lacks O gets no virtual H; such
        // residues can never accept a hydrogen bond.
        let amide_hydrogen: Vec<Option<Point3<f32>>> = (0..count)
            .map(|i| {
                if i == 0 {
                    return None;
                }
                let previous = &backbone[i - 1];
                let oxygen = previous.oxygen?;
                let direction = previous.carbon - oxygen;
                let length = direction.norm();
                (length > 1e-6).then(|| backbone[i].nitrogen + direction / length)
            })
            .collect();

        // hbond(donor → acceptor): does the donor's C=O accept the acceptor's N–H?
        // Indices are signed and fully bounds-checked so the β-bridge formulas can
        // probe i±1 / j±1 without separate guards. A donor without O cannot bond.
        let hbond = |donor: i64, acceptor: i64| -> bool {
            if donor < 0 || acceptor < 0 {
                return false;
            }
            let (donor, acceptor) = (donor as usize, acceptor as usize);
            if donor >= count || acceptor >= count {
                return false;
            }
            if donor.abs_diff(acceptor) < MIN_HBOND_SEPARATION {
                return false;
            }
            let Some(oxygen) = backbone[donor].oxygen else {
                return false;
            };
            let Some(hydrogen) = amide_hydrogen[acceptor] else {
                return false;
            };
            kabsch_sander_energy(
                backbone[donor].carbon,
                oxygen,
                backbone[acceptor].nitrogen,
                hydrogen,
            ) < HBOND_ENERGY_CUTOFF
        };

        let mut labels = vec![Label::Coil; count];

        // α-helix: two consecutive backbone 4-turns (an i→i+4 H-bond opening at
        // i−1 and again at i) define a helix spanning residues i..=i+3.
        for i in 1..count {
            let turn_at_previous = hbond((i - 1) as i64, (i + 3) as i64);
            let turn_at_current = hbond(i as i64, (i + 4) as i64);
            if turn_at_previous && turn_at_current {
                let end = (i + 3).min(count - 1);
                for label in &mut labels[i..=end] {
                    *label = Label::Helix;
                }
            }
        }

        // β-bridges: two residues ≥3 apart are bridge partners when a characteristic
        // pair of H-bonds links them, in either the antiparallel or parallel motif.
        let mut in_bridge = vec![false; count];
        for i in 0..count as i64 {
            for j in (i + MIN_BRIDGE_SEPARATION)..count as i64 {
                let antiparallel =
                    (hbond(i, j) && hbond(j, i)) || (hbond(i - 1, j + 1) && hbond(j - 1, i + 1));
                let parallel =
                    (hbond(i - 1, j) && hbond(j, i + 1)) || (hbond(j - 1, i) && hbond(i, j + 1));
                if antiparallel || parallel {
                    in_bridge[i as usize] = true;
                    in_bridge[j as usize] = true;
                }
            }
        }

        // A residue in any bridge is a strand candidate; a lone residue framed by
        // two candidates is a β-bulge and is filled in (read from the pre-fill
        // bridge state so the fill cannot cascade).
        let mut candidate = in_bridge.clone();
        for i in 1..count.saturating_sub(1) {
            if !in_bridge[i] && in_bridge[i - 1] && in_bridge[i + 1] {
                candidate[i] = true;
            }
        }

        // Runs of ≥2 consecutive candidates become a strand; an isolated bridge is
        // coil. A residue already in a helix wins on overlap and breaks the run.
        let mut i = 0;
        while i < count {
            if !candidate[i] || labels[i] == Label::Helix {
                i += 1;
                continue;
            }
            let start = i;
            while i < count && candidate[i] && labels[i] != Label::Helix {
                i += 1;
            }
            if i - start >= 2 {
                for label in &mut labels[start..i] {
                    *label = Label::Strand;
                }
            }
        }

        Some(labels)
    }

    /// Kabsch–Sander electrostatic hydrogen-bond energy (kcal/mol) for the carbonyl
    /// C=O of the donor residue interacting with the amide N–H of the acceptor.
    fn kabsch_sander_energy(
        carbon: Point3<f32>,
        oxygen: Point3<f32>,
        nitrogen: Point3<f32>,
        hydrogen: Point3<f32>,
    ) -> f32 {
        let r_on = (oxygen - nitrogen).norm();
        let r_ch = (carbon - hydrogen).norm();
        let r_oh = (oxygen - hydrogen).norm();
        let r_cn = (carbon - nitrogen).norm();
        if r_on < MIN_ATOM_DISTANCE
            || r_ch < MIN_ATOM_DISTANCE
            || r_oh < MIN_ATOM_DISTANCE
            || r_cn < MIN_ATOM_DISTANCE
        {
            return STRONG_BOND_ENERGY;
        }
        ENERGY_PREFACTOR * (1.0 / r_on + 1.0 / r_ch - 1.0 / r_oh - 1.0 / r_cn)
    }
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
mod tests;
