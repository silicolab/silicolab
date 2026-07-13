use super::*;

use std::borrow::Cow;

use eframe::egui::Color32;
use nalgebra::{Point3, Rotation3, Unit, Vector3};

use crate::{
    domain::{
        Atom, Biopolymer, ChainRecord, ResidueRecord, SecondaryStructureKind,
        SecondaryStructureSpan, Structure, assign_secondary_structure, residues_backbone_bonded,
    },
    frontend::viewport::{
        SecondaryStructureCache, SecondaryStructureCacheKey, ViewportVisualState,
    },
};

use super::super::{
    chain_color, initial_cartoon_side, interpolate_orientation_hint, mix_color, normalize_vector3,
    orthogonalize_to_tangent,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CartoonSegmentKind {
    Helix,
    Sheet,
    Coil,
}

#[derive(Clone, Copy)]
pub(crate) struct CartoonStyle {
    pub(crate) half_width: f32,
    pub(crate) half_thickness: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct CartoonControlPoint {
    pub(crate) position: Point3<f32>,
    pub(crate) style: CartoonStyle,
    pub(crate) color: Color32,
    pub(crate) orientation_hint: Option<Vector3<f32>>,
    pub(crate) kind: CartoonSegmentKind,
}

#[derive(Clone, Copy)]
pub(crate) struct CartoonSweepSample {
    pub(crate) position: Point3<f32>,
    pub(crate) tangent: Vector3<f32>,
    pub(crate) side: Vector3<f32>,
    pub(crate) normal: Vector3<f32>,
    pub(crate) style: CartoonStyle,
    pub(crate) color: Color32,
    pub(crate) kind: CartoonSegmentKind,
}

#[derive(Clone, Copy)]
struct ChainResiduePoint {
    residue_index: usize,
    position: Point3<f32>,
}

/// The per-fragment ribbon sweep samples (smoothed spline frames with cross-
/// section styles) for every drawable chain.
pub(crate) fn cartoon_chain_sweeps(
    structure: &Structure,
    biopolymer: &Biopolymer,
    visual_state: &ViewportVisualState,
) -> Vec<Vec<CartoonSweepSample>> {
    let secondary = resolve_secondary_structures(structure, biopolymer);
    cartoon_chain_sweeps_with_secondary(structure, biopolymer, visual_state, secondary.as_ref())
}

pub(crate) fn cached_cartoon_chain_sweeps(
    structure: &Structure,
    biopolymer: &Biopolymer,
    visual_state: &ViewportVisualState,
    secondary_cache: &mut SecondaryStructureCache,
    secondary_key: SecondaryStructureCacheKey,
) -> Vec<Vec<CartoonSweepSample>> {
    let secondary =
        resolve_secondary_structures_cached(structure, biopolymer, secondary_cache, secondary_key);
    cartoon_chain_sweeps_with_secondary(structure, biopolymer, visual_state, secondary.as_ref())
}

fn cartoon_chain_sweeps_with_secondary(
    structure: &Structure,
    biopolymer: &Biopolymer,
    visual_state: &ViewportVisualState,
    secondary: &[SecondaryStructureSpan],
) -> Vec<Vec<CartoonSweepSample>> {
    let mut sweeps = Vec::new();
    for (chain_index, chain) in biopolymer.chains.iter().enumerate() {
        let chain_color = visual_state
            .chain_colors
            .get(&chain.id)
            .copied()
            .unwrap_or_else(|| chain_color(chain, biopolymer, chain_index));
        let residue_trace = chain_residue_trace(structure, biopolymer, chain, visual_state);
        if residue_trace.len() < 2 {
            continue;
        }

        for (start, end) in chain_contiguous_fragments(biopolymer, &structure.atoms, &residue_trace)
        {
            let fragment = &residue_trace[start..end];
            if fragment.len() < 2 {
                continue;
            }

            let controls = fragment
                .iter()
                .enumerate()
                .map(|(fragment_index, entry)| {
                    let residue = &biopolymer.residues[entry.residue_index];
                    let kind = residue_cartoon_kind(residue, secondary, chain.id);
                    CartoonControlPoint {
                        position: entry.position,
                        style: cartoon_style(kind, &visual_state.cartoon),
                        color: cartoon_segment_color(chain_color, kind),
                        orientation_hint: cartoon_orientation_hint(fragment, fragment_index, kind),
                        kind,
                    }
                })
                .collect::<Vec<_>>();

            let smoothed = smooth_cartoon_controls(&controls, visual_state.cartoon.smoothing);
            let mut samples = build_cartoon_sweep_samples(&smoothed);
            apply_sheet_arrows(&mut samples, visual_state.cartoon.smoothing.max(2));
            sweeps.push(samples);
        }
    }
    sweeps
}

/// Widen sheet strands into an arrowhead near their C-terminal end: the ribbon
/// steps out to wide "barbs" at the arrow base then tapers to a point at the
/// tip, the iconic cue for β-strand directionality.
fn apply_sheet_arrows(samples: &mut [CartoonSweepSample], subdivisions: usize) {
    const ARROW_BASE_SCALE: f32 = 1.75;
    const ARROW_TIP_SCALE: f32 = 0.05;
    let arrow_len = ((subdivisions as f32 * 1.6).round() as usize).max(3);

    let mut index = 0;
    while index < samples.len() {
        if samples[index].kind != CartoonSegmentKind::Sheet {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < samples.len() && samples[index].kind == CartoonSegmentKind::Sheet {
            index += 1;
        }
        let run_end = index;
        if run_end - run_start < arrow_len + 1 {
            continue;
        }
        let base = run_end - arrow_len;
        for (step, sample_index) in (base..run_end).enumerate() {
            let t = step as f32 / (arrow_len - 1) as f32;
            let scale = ARROW_BASE_SCALE + (ARROW_TIP_SCALE - ARROW_BASE_SCALE) * t;
            samples[sample_index].style.half_width *= scale;
        }
    }
}

fn chain_residue_trace(
    structure: &Structure,
    biopolymer: &Biopolymer,
    chain: &ChainRecord,
    visual_state: &ViewportVisualState,
) -> Vec<ChainResiduePoint> {
    chain
        .residue_indices
        .iter()
        .filter_map(|&residue_index| {
            let residue = biopolymer.residues.get(residue_index)?;
            let atom_index = residue.alpha_carbon?;
            // Full peptide residues are renderable from N/CA/C topology alone.
            // Standard C-alpha-only residues keep the legacy trace fallback; the
            // overlay toggle then gates whether the user wants the ribbon here.
            let is_cartoon =
                residue.has_cartoon_trace() && visual_state.cartoon_enabled(structure, atom_index);
            is_cartoon.then_some(ChainResiduePoint {
                residue_index,
                position: structure.atoms[atom_index].position,
            })
        })
        .collect()
}

fn chain_contiguous_fragments(
    biopolymer: &Biopolymer,
    atoms: &[Atom],
    residue_trace: &[ChainResiduePoint],
) -> Vec<(usize, usize)> {
    let mut fragments = Vec::new();
    if residue_trace.is_empty() {
        return fragments;
    }

    let mut fragment_start = 0;
    for index in 1..residue_trace.len() {
        // A fragment break is a genuine backbone discontinuity in space, not a
        // jump in residue numbering — so renumbering, insertion codes, and gaps
        // never split a ribbon that is physically one continuous chain.
        let is_contiguous = residues_backbone_bonded(
            &biopolymer.residues[residue_trace[index - 1].residue_index],
            &biopolymer.residues[residue_trace[index].residue_index],
            atoms,
        );

        if !is_contiguous {
            fragments.push((fragment_start, index));
            fragment_start = index;
        }
    }
    fragments.push((fragment_start, residue_trace.len()));
    fragments
}

/// Orient the flat ribbon face using the local backbone curvature: the binormal
/// of the CA trace. Helices coil so this points radially outward; β-strands
/// pleat so it follows the strand's peptide-plane normal. Straight runs give a
/// near-zero binormal (`None`), and the parallel-transported frame carries the
/// previous orientation across them.
fn cartoon_orientation_hint(
    fragment: &[ChainResiduePoint],
    index: usize,
    _kind: CartoonSegmentKind,
) -> Option<Vector3<f32>> {
    if fragment.len() < 3 {
        return None;
    }
    let previous = if index > 0 {
        fragment[index - 1].position
    } else {
        fragment[index].position
    };
    let current = fragment[index].position;
    let next = if index + 1 < fragment.len() {
        fragment[index + 1].position
    } else {
        fragment[index].position
    };
    let incoming = current - previous;
    let outgoing = next - current;
    let normal = incoming.cross(&outgoing);
    (normal.norm_squared() > 0.0001).then(|| normalize_vector3(normal, Vector3::new(0.0, 1.0, 0.0)))
}

/// Secondary-structure spans for the cartoon: the biopolymer's own when present,
/// otherwise derived from the Cα trace so coordinates without HELIX/SHEET records
/// (e.g. a GRO file from an MD run) still render helices and strands.
pub(crate) fn resolve_secondary_structures<'a>(
    structure: &Structure,
    biopolymer: &'a Biopolymer,
) -> Cow<'a, [SecondaryStructureSpan]> {
    if biopolymer.secondary_structures.is_empty() {
        Cow::Owned(assign_secondary_structure(&structure.atoms, biopolymer))
    } else {
        Cow::Borrowed(&biopolymer.secondary_structures)
    }
}

pub(crate) fn resolve_secondary_structures_cached<'a>(
    structure: &Structure,
    biopolymer: &'a Biopolymer,
    cache: &'a mut SecondaryStructureCache,
    key: SecondaryStructureCacheKey,
) -> Cow<'a, [SecondaryStructureSpan]> {
    if !biopolymer.secondary_structures.is_empty() {
        return Cow::Borrowed(&biopolymer.secondary_structures);
    }
    if cache.key != Some(key) {
        cache.spans = assign_secondary_structure(&structure.atoms, biopolymer);
        cache.key = Some(key);
    }
    Cow::Borrowed(&cache.spans)
}

pub(crate) fn residue_cartoon_kind(
    residue: &ResidueRecord,
    secondary_structures: &[SecondaryStructureSpan],
    chain_id: char,
) -> CartoonSegmentKind {
    match residue_secondary_structure_kind(residue, secondary_structures, chain_id) {
        Some(SecondaryStructureKind::Helix) => CartoonSegmentKind::Helix,
        Some(SecondaryStructureKind::Sheet) => CartoonSegmentKind::Sheet,
        None => CartoonSegmentKind::Coil,
    }
}

fn residue_secondary_structure_kind(
    residue: &ResidueRecord,
    secondary_structures: &[SecondaryStructureSpan],
    chain_id: char,
) -> Option<SecondaryStructureKind> {
    secondary_structures
        .iter()
        .find(|span| {
            span.start.chain_id == chain_id
                && residue_in_span(residue, span)
                && matches!(
                    span.kind,
                    SecondaryStructureKind::Helix | SecondaryStructureKind::Sheet
                )
        })
        .map(|span| span.kind)
}

fn residue_in_span(residue: &ResidueRecord, span: &SecondaryStructureSpan) -> bool {
    residue.id.chain_id == span.start.chain_id
        && residue.id.ordering_key() >= span.start.ordering_key()
        && residue.id.ordering_key() <= span.end.ordering_key()
}

fn smooth_cartoon_controls(
    points: &[CartoonControlPoint],
    subdivisions: usize,
) -> Vec<CartoonControlPoint> {
    if points.len() < 3 || subdivisions < 2 {
        return points.to_vec();
    }

    let mut smoothed = Vec::with_capacity((points.len() - 1) * subdivisions + 1);
    for index in 0..points.len() - 1 {
        let p0 = if index == 0 {
            points[0]
        } else {
            points[index - 1]
        };
        let p1 = points[index];
        let p2 = points[index + 1];
        let p3 = if index + 2 < points.len() {
            points[index + 2]
        } else {
            points[points.len() - 1]
        };

        for step in 0..subdivisions {
            let t = step as f32 / subdivisions as f32;
            let eased = smoothstep(t);
            smoothed.push(CartoonControlPoint {
                position: catmull_rom_point(p0.position, p1.position, p2.position, p3.position, t),
                style: lerp_cartoon_style(p1.style, p2.style, eased),
                color: mix_color(p1.color, p2.color, eased),
                orientation_hint: interpolate_orientation_hint(
                    p1.orientation_hint,
                    p2.orientation_hint,
                    eased,
                ),
                kind: if eased < 0.5 { p1.kind } else { p2.kind },
            });
        }
    }
    smoothed.push(*points.last().expect("non-empty point list"));
    smoothed
}

fn catmull_rom_point(
    p0: Point3<f32>,
    p1: Point3<f32>,
    p2: Point3<f32>,
    p3: Point3<f32>,
    t: f32,
) -> Point3<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    let coords = 0.5
        * ((2.0 * p1.coords)
            + (-p0.coords + p2.coords) * t
            + (2.0 * p0.coords - 5.0 * p1.coords + 4.0 * p2.coords - p3.coords) * t2
            + (-p0.coords + 3.0 * p1.coords - 3.0 * p2.coords + p3.coords) * t3);
    Point3::from(coords)
}

fn build_cartoon_sweep_samples(controls: &[CartoonControlPoint]) -> Vec<CartoonSweepSample> {
    if controls.len() < 2 {
        return Vec::new();
    }

    let tangents = controls
        .iter()
        .enumerate()
        .map(|(index, control)| {
            let previous = if index > 0 {
                controls[index - 1].position
            } else {
                control.position
            };
            let next = if index + 1 < controls.len() {
                controls[index + 1].position
            } else {
                control.position
            };
            normalize_vector3(next - previous, Vector3::new(0.0, 0.0, 1.0))
        })
        .collect::<Vec<_>>();

    let mut samples = Vec::with_capacity(controls.len());
    let mut side = controls[0]
        .orientation_hint
        .map(|hint| align_orientation_hint(hint, tangents[0], initial_cartoon_side(tangents[0])))
        .unwrap_or_else(|| initial_cartoon_side(tangents[0]));
    let mut normal = normalize_vector3(tangents[0].cross(&side), Vector3::new(0.0, 1.0, 0.0));
    side = normalize_vector3(normal.cross(&tangents[0]), side);

    samples.push(CartoonSweepSample {
        position: controls[0].position,
        tangent: tangents[0],
        side,
        normal,
        style: controls[0].style,
        color: controls[0].color,
        kind: controls[0].kind,
    });

    for index in 1..controls.len() {
        side = if let Some(hint) = controls[index].orientation_hint {
            let aligned = align_orientation_hint(hint, tangents[index], side);
            if aligned.dot(&side) < 0.0 {
                -aligned
            } else {
                aligned
            }
        } else {
            transport_frame_vector(side, tangents[index - 1], tangents[index])
        };
        side = orthogonalize_to_tangent(side, tangents[index], side);
        normal = normalize_vector3(tangents[index].cross(&side), samples[index - 1].normal);
        side = normalize_vector3(normal.cross(&tangents[index]), side);

        samples.push(CartoonSweepSample {
            position: controls[index].position,
            tangent: tangents[index],
            side,
            normal,
            style: controls[index].style,
            color: controls[index].color,
            kind: controls[index].kind,
        });
    }

    samples
}

fn smoothstep(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn align_orientation_hint(
    hint: Vector3<f32>,
    tangent: Vector3<f32>,
    fallback: Vector3<f32>,
) -> Vector3<f32> {
    let projected = hint - tangent * hint.dot(&tangent);
    normalize_vector3(projected, fallback)
}

fn transport_frame_vector(
    vector: Vector3<f32>,
    previous_tangent: Vector3<f32>,
    tangent: Vector3<f32>,
) -> Vector3<f32> {
    let axis = previous_tangent.cross(&tangent);
    let axis_norm_sq = axis.norm_squared();
    if axis_norm_sq <= 0.000001 {
        return vector;
    }

    let angle = previous_tangent.dot(&tangent).clamp(-1.0, 1.0).acos();
    if angle <= 0.0001 {
        return vector;
    }

    Rotation3::from_axis_angle(&Unit::new_normalize(axis), angle) * vector
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PdbAtomAnnotation, ResidueId, build_biopolymer};

    const CA_STEP: f32 = 3.8;

    /// Atoms + annotations for a straight protein backbone laid along +x: one
    /// residue per `(residue_name, sequence_number, insertion_code)` at `CA_STEP`
    /// Cα spacing, each carrying N/CA/C atoms placed so the C(i)–N(i+1) peptide
    /// bond is short — the residues are physically bonded in file order regardless
    /// of how they are numbered. Returned separately so a test can perturb
    /// coordinates before the structure is assembled.
    fn straight_backbone(residues: &[(&str, i32, char)]) -> (Vec<Atom>, Vec<PdbAtomAnnotation>) {
        let mut atoms = Vec::new();
        let mut annotations = Vec::new();
        for (index, &(name, sequence_number, insertion_code)) in residues.iter().enumerate() {
            let x = index as f32 * CA_STEP;
            for (atom_name, element, offset) in
                [("N", "N", -1.2_f32), ("CA", "C", 0.0), ("C", "C", 1.2)]
            {
                atoms.push(Atom {
                    element: element.to_string(),
                    position: Point3::new(x + offset, 0.0, 0.0),
                    charge: 0.0,
                });
                annotations.push(PdbAtomAnnotation {
                    atom_name: atom_name.to_string(),
                    residue_name: name.to_string(),
                    chain_id: 'A',
                    residue_seq: sequence_number,
                    insertion_code,
                });
            }
        }
        (atoms, annotations)
    }

    /// `straight_backbone` with default consecutive numbering from 1.
    fn backbone_atoms(residue_names: &[&str]) -> (Vec<Atom>, Vec<PdbAtomAnnotation>) {
        let residues: Vec<(&str, i32, char)> = residue_names
            .iter()
            .enumerate()
            .map(|(index, &name)| (name, index as i32 + 1, ' '))
            .collect();
        straight_backbone(&residues)
    }

    /// Wrap atoms + annotations into a `Structure`. A dummy secondary-structure
    /// span is passed so `build_biopolymer` accepts the input as a biopolymer even
    /// when no residue name is a standard amino acid — the parse-level "is this a
    /// protein" gate is out of scope here; these tests exercise the renderer.
    fn structure_from(atoms: Vec<Atom>, annotations: Vec<PdbAtomAnnotation>) -> Structure {
        let span = SecondaryStructureSpan {
            kind: SecondaryStructureKind::Helix,
            start: ResidueId::new('A', 1, ' '),
            end: ResidueId::new('A', 1, ' '),
        };
        let biopolymer = build_biopolymer(&annotations, vec![span]).expect("biopolymer");
        Structure {
            title: "test".to_string(),
            atoms,
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        }
    }

    /// A visual state with the cartoon overlay explicitly on for every atom, i.e.
    /// "cartoon is active" — the precondition the renderer is fixed under.
    fn cartoon_on_all(atom_count: usize) -> ViewportVisualState {
        let mut visual_state = ViewportVisualState::default();
        for atom_index in 0..atom_count {
            visual_state.cartoon_overlay.atoms.insert(atom_index, true);
        }
        visual_state
    }

    #[test]
    fn non_standard_residue_names_form_one_continuous_ribbon() {
        // Force-field protonation states, disulfide variants, selenomethionine,
        // phosphoserine — none recognized by is_standard_amino_acid — must ribbon
        // as one unbroken fragment. This is the regression: the old name gate
        // dropped every one of these residues, shredding the ribbon.
        let residue_names = [
            "HID", "CYX", "HSE", "GLH", "HSD", "LYN", "ASH", "HIP", "HIE", "CYM", "MSE", "SEP",
        ];
        let (atoms, annotations) = backbone_atoms(&residue_names);
        let structure = structure_from(atoms, annotations);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
        assert!(
            biopolymer
                .residues
                .iter()
                .all(|residue| !residue.is_standard_amino_acid),
            "every residue must use a non-standard name for this regression to bite"
        );

        let visual_state = cartoon_on_all(structure.atoms.len());
        let chain = &biopolymer.chains[0];
        let trace = chain_residue_trace(&structure, biopolymer, chain, &visual_state);
        assert_eq!(
            trace.len(),
            residue_names.len(),
            "every backbone residue enters the ribbon trace regardless of name"
        );

        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(
            fragments,
            vec![(0, residue_names.len())],
            "the chain renders as a single continuous ribbon fragment"
        );
    }

    #[test]
    fn non_standard_protein_ribbons_by_default_without_explicit_enabling() {
        // With the *default* visual state (no overlay overrides), a protein whose
        // residues use non-standard names must still default to a cartoon ribbon:
        // enablement is driven by backbone topology, not the residue name nor its
        // name-based atom category (which would classify these as a ligand and,
        // before the fix, default cartoon off — dropping them from the ribbon).
        let residue_names = ["HID", "CYX", "HSE", "GLH", "HSD", "LYN"];
        let (atoms, annotations) = backbone_atoms(&residue_names);
        let structure = structure_from(atoms, annotations);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");

        let visual_state = ViewportVisualState::default(); // nothing enabled by hand
        let chain = &biopolymer.chains[0];
        let trace = chain_residue_trace(&structure, biopolymer, chain, &visual_state);
        assert_eq!(
            trace.len(),
            residue_names.len(),
            "non-standard protein defaults to cartoon and enters the trace"
        );
        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(fragments, vec![(0, residue_names.len())]);
    }

    #[test]
    fn standard_ca_only_trace_ribbons_by_default() {
        let atoms = (0..6)
            .map(|index| Atom {
                element: "C".to_string(),
                position: Point3::new(index as f32 * CA_STEP, 0.0, 0.0),
                charge: 0.0,
            })
            .collect::<Vec<_>>();
        let annotations = (0..6)
            .map(|index| PdbAtomAnnotation {
                atom_name: "CA".to_string(),
                residue_name: "ALA".to_string(),
                chain_id: 'A',
                residue_seq: index + 1,
                insertion_code: ' ',
            })
            .collect::<Vec<_>>();
        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        assert!(
            biopolymer
                .residues
                .iter()
                .all(|residue| !residue.has_peptide_backbone())
        );
        let structure = Structure {
            title: "ca-only".to_string(),
            atoms,
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        };
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");

        let visual_state = ViewportVisualState::default();
        let trace =
            chain_residue_trace(&structure, biopolymer, &biopolymer.chains[0], &visual_state);
        assert_eq!(trace.len(), 6, "standard C-alpha traces still draw ribbons");
        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(fragments, vec![(0, 6)]);
    }
    #[test]
    fn renumbering_and_insertion_codes_keep_one_continuous_ribbon() {
        // Physically one continuous backbone, but with a sequence-number jump and
        // insertion codes — exactly what the old `sequence_number diff <= 1`
        // contiguity test wrongly split. Geometry says one fragment; the numbering
        // must not override it.
        let residues = [
            ("HID", 5, ' '),
            ("CYX", 6, ' '),
            ("HSE", 6, 'A'), // insertion code, same sequence number
            ("GLH", 6, 'B'),
            ("ALA", 41, ' '), // large jump in numbering, still bonded in space
            ("LYN", 42, ' '),
        ];
        let (atoms, annotations) = straight_backbone(&residues);
        let structure = structure_from(atoms, annotations);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");

        let visual_state = cartoon_on_all(structure.atoms.len());
        let chain = &biopolymer.chains[0];
        let trace = chain_residue_trace(&structure, biopolymer, chain, &visual_state);
        assert_eq!(trace.len(), residues.len());

        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(
            fragments,
            vec![(0, residues.len())],
            "bonded backbone stays one fragment despite renumbering and insertion codes"
        );
    }

    #[test]
    fn a_real_backbone_break_splits_the_ribbon_despite_consecutive_numbering() {
        // Contiguity is geometric: a true spatial gap must split the ribbon even
        // though the residue numbers stay 1,2,3,4 with no insertion codes.
        let (mut atoms, annotations) = backbone_atoms(&["HID", "CYX", "HSE", "GLH"]);
        // Push residues 3 and 4 (their nine atoms start at index 6) far away, so
        // the C(2)–N(3) peptide bond is long while everything else stays bonded.
        for atom in atoms.iter_mut().skip(6) {
            atom.position.x += 50.0;
        }
        let structure = structure_from(atoms, annotations);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");

        let visual_state = cartoon_on_all(structure.atoms.len());
        let chain = &biopolymer.chains[0];
        let trace = chain_residue_trace(&structure, biopolymer, chain, &visual_state);
        assert_eq!(trace.len(), 4, "all four residues are renderable");

        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(
            fragments,
            vec![(0, 2), (2, 4)],
            "the spatial break splits the trace into two fragments"
        );
    }

    #[test]
    fn hetero_groups_without_backbone_are_excluded() {
        // A calcium ion (residue and atom both named "CA", so it carries a stray
        // alpha_carbon) and a water are not protein backbone — they lack the
        // N/CA/C triad — so topology keeps them out of the ribbon entirely.
        let (mut atoms, mut annotations) = backbone_atoms(&["HID", "CYX", "HSE"]);
        atoms.push(Atom {
            element: "Ca".to_string(),
            position: Point3::new(100.0, 0.0, 0.0),
            charge: 2.0,
        });
        annotations.push(PdbAtomAnnotation {
            atom_name: "CA".to_string(),
            residue_name: "CA".to_string(),
            chain_id: 'A',
            residue_seq: 100,
            insertion_code: ' ',
        });
        atoms.push(Atom {
            element: "O".to_string(),
            position: Point3::new(110.0, 0.0, 0.0),
            charge: 0.0,
        });
        annotations.push(PdbAtomAnnotation {
            atom_name: "OW".to_string(),
            residue_name: "SOL".to_string(),
            chain_id: 'A',
            residue_seq: 101,
            insertion_code: ' ',
        });
        let structure = structure_from(atoms, annotations);
        let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");

        let visual_state = cartoon_on_all(structure.atoms.len());
        let chain = &biopolymer.chains[0];
        let trace = chain_residue_trace(&structure, biopolymer, chain, &visual_state);
        assert_eq!(
            trace.len(),
            3,
            "only the three backbone residues enter the trace; Ca and water are excluded"
        );

        let fragments = chain_contiguous_fragments(biopolymer, &structure.atoms, &trace);
        assert_eq!(fragments, vec![(0, 3)]);
    }
}
