use eframe::egui::{self, Color32, FontId, Rect, Vec2};
use nalgebra::{Point3, Vector3};

use crate::{
    domain::chemistry::element_style,
    domain::{Biopolymer, ChainRecord, ResidueRecord, Structure},
    frontend::{AtomSelection, state::AtomStyle},
};

use super::{SurfaceCache, SurfaceCacheKey, ViewportVisualState};

mod cartoon;
mod cell;
mod instances;
mod scene;
mod surface;

pub(super) use cell::draw_cell_labels;
pub(super) use instances::build_cached_molecule_instances;
pub(super) use instances::build_molecule_instances;
pub(super) use scene::{PickTarget, pick_atom, project_pick_targets};
pub(super) use surface::{SurfaceSceneGeometry, build_surface_world_mesh};

const BALL_RADIUS_SCALE: f32 = 0.78;
const SINGLE_BOND_RADIUS: f32 = 0.115;
/// Clear spacing between adjacent rods in a multiple-bond bundle. The bundle's
/// full envelope stays equal to the single-bond diameter.
const STICK_MULTI_BOND_GAP: f32 = SINGLE_BOND_RADIUS * 0.12;
const STICK_DOUBLE_BOND_RADIUS: f32 = SINGLE_BOND_RADIUS * 0.5 - STICK_MULTI_BOND_GAP * 0.25;
const STICK_DOUBLE_BOND_OFFSET: f32 = SINGLE_BOND_RADIUS - STICK_DOUBLE_BOND_RADIUS;
const STICK_TRIPLE_BOND_RADIUS: f32 = (SINGLE_BOND_RADIUS - STICK_MULTI_BOND_GAP) / 3.0;
const STICK_TRIPLE_BOND_OFFSET: f32 = SINGLE_BOND_RADIUS - STICK_TRIPLE_BOND_RADIUS;
const MULTI_BOND_RADIUS: f32 = 0.082;
const MULTI_BOND_OFFSET: f32 = 0.18;
/// Radius of the thin rod a Wireframe (line) bond becomes on the GPU path, which
/// has no screen-space line primitive. Wireframe atoms draw no node, so the bond
/// carries the whole representation; kept well under [`SINGLE_BOND_RADIUS`] so
/// wireframe still reads as light lines next to solid sticks.
const WIREFRAME_BOND_RADIUS: f32 = 0.05;
const AROMATIC_DASH_RADIUS: f32 = 0.052;
const AROMATIC_DASH_OFFSET: f32 = 0.17;
const AROMATIC_DASH_LENGTH: f32 = 0.26;
const AROMATIC_GAP_LENGTH: f32 = 0.14;

pub(super) fn draw_hovered_atom_label(
    painter: &egui::Painter,
    rect: Rect,
    structure: &Structure,
    atom_index: usize,
    pal: crate::frontend::theme::Palette,
) {
    let atom = &structure.atoms[atom_index];
    let top_right = rect.right_top() + Vec2::new(-20.0, 20.0);
    let label = format!("#{}", atom_index + 1);

    let element_font = FontId::new(42.0, egui::FontFamily::Proportional);
    let number_font = FontId::new(18.0, egui::FontFamily::Proportional);

    // HUD overlay drawn directly on the viewport with no backing box, so the
    // text must follow the theme or it vanishes on the dark viewport.
    let element_color = pal.text_strong;
    let number_color = pal.text_muted;

    let element_galley = painter.layout(
        atom.element.clone(),
        element_font.clone(),
        element_color,
        f32::INFINITY,
    );
    let number_galley = painter.layout(
        label.clone(),
        number_font.clone(),
        number_color,
        f32::INFINITY,
    );

    let element_width = element_galley.size().x;
    let number_width = number_galley.size().x;

    let element_pos = top_right - Vec2::new(element_width, 0.0);
    let number_pos = element_pos - Vec2::new(number_width + 8.0, 0.0);

    painter.galley(element_pos, element_galley, element_color);
    painter.galley(number_pos, number_galley, number_color);
}

fn atom_screen_radius(display_radius: f32, scale: f32, projection_scale: f32) -> f32 {
    (display_radius * BALL_RADIUS_SCALE * scale * projection_scale).clamp(5.0, 30.0)
}

fn atom_ball_radius(element: &str) -> f32 {
    element_style(element).display_radius * BALL_RADIUS_SCALE
}

/// World-space radius of the solid atom marker drawn for `style`, before any
/// selection-highlight scaling. Stick joints are topology-dependent connectors,
/// not atom markers, and are emitted separately by the molecule builders.
fn atom_marker_radius(element: &str, style: AtomStyle) -> Option<f32> {
    match style {
        AtomStyle::Sphere => Some(atom_ball_radius(element) / BALL_RADIUS_SCALE),
        AtomStyle::BallAndStick => Some(atom_ball_radius(element)),
        _ => None,
    }
}

fn atom_render_color(
    structure: &Structure,
    atom_index: usize,
    selection: &AtomSelection,
) -> Color32 {
    let base = color32(element_style(&structure.atoms[atom_index].element).color);
    if selection.primary() == Some(atom_index) {
        mix_color(base, Color32::from_rgb(100, 180, 255), 0.72)
    } else if selection.contains(atom_index) {
        mix_color(base, Color32::from_rgb(160, 210, 255), 0.52)
    } else {
        base
    }
}

fn atom_render_color_with_settings(
    structure: &Structure,
    atom_index: usize,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
) -> Color32 {
    if is_ion_atom(structure, atom_index)
        && let Some(color) = visual_state.ions.color
    {
        return color;
    }
    atom_render_color(structure, atom_index, selection)
}

fn usable_biopolymer(structure: &Structure) -> Option<&Biopolymer> {
    structure
        .biopolymer
        .as_ref()
        .filter(|biopolymer| biopolymer.is_compatible_with_atom_count(structure.atoms.len()))
}

/// Whether an atom participates in the ball-and-stick scene (and is individually
/// pickable). Driven entirely by the atom's resolved [`AtomStyle`]: `Hidden` and
/// `Cartoon` atoms are excluded (cartoon atoms are drawn by the cartoon path).
/// Water visibility is just the solvent category style — hidden by setting the
/// `Solvent` category (or a per-atom override) to [`AtomStyle::Hidden`].
pub(super) fn atom_visible(
    structure: &Structure,
    visual_state: &ViewportVisualState,
    atom_index: usize,
) -> bool {
    // The explicit visibility override wins over everything: an atom hidden from
    // the Style panel draws no geometry whatever its style.
    if visual_state.atom_is_hidden(atom_index) {
        return false;
    }
    // Ball-and-stick visibility is driven by the *base* style; the cartoon and
    // surface overlays draw in their own passes and never hide the base. A base
    // of `Hidden` (including the legacy cartoon default, whose base resolves to
    // Hidden) draws no per-atom geometry here.
    if visual_state.resolved_base_style(structure, atom_index) == AtomStyle::Hidden {
        return false;
    }
    if let Some(distance) = visual_state.ions.show_within
        && is_ion_atom(structure, atom_index)
    {
        return ion_within_protein_distance(structure, atom_index, distance);
    }
    true
}

fn ion_within_protein_distance(structure: &Structure, atom_index: usize, distance: f32) -> bool {
    let Some(biopolymer) = usable_biopolymer(structure) else {
        return false;
    };
    let cutoff_sq = distance * distance;
    let ion_position = structure.atoms[atom_index].position;
    biopolymer
        .residues
        .iter()
        .filter(|residue| residue.is_standard_amino_acid)
        .flat_map(|residue| residue.atom_indices.iter())
        .any(|&protein_atom_index| {
            (structure.atoms[protein_atom_index].position - ion_position).norm_squared()
                <= cutoff_sq
        })
}

/// Sorted indices of atoms with the surface overlay enabled. Feeds the
/// representation-surface geometry and the surface cache key, so the cached mesh
/// rebuilds when the overlaid set changes.
pub(super) fn surface_atom_indices(
    structure: &Structure,
    visual_state: &ViewportVisualState,
) -> Vec<usize> {
    if visual_state.surface_overlay.is_empty() {
        return Vec::new();
    }
    (0..structure.atoms.len())
        .filter(|&atom_index| visual_state.surface_enabled(structure, atom_index))
        .collect()
}

fn atom_chain_id(structure: &Structure, atom_index: usize) -> Option<char> {
    residue_for_atom(structure, atom_index).map(|residue| residue.id.chain_id)
}

fn atom_is_standard_amino_acid(structure: &Structure, atom_index: usize) -> bool {
    residue_for_atom(structure, atom_index).is_some_and(|residue| residue.is_standard_amino_acid)
}

fn residue_for_atom(structure: &Structure, atom_index: usize) -> Option<&ResidueRecord> {
    let biopolymer = usable_biopolymer(structure)?;
    let residue_index = biopolymer
        .residue_for_atom
        .get(atom_index)
        .copied()
        .flatten()?;
    biopolymer.residues.get(residue_index)
}

fn is_ion_atom(structure: &Structure, atom_index: usize) -> bool {
    let element = structure.atoms[atom_index].element.as_str();
    matches!(
        element,
        "Li" | "Na"
            | "K"
            | "Rb"
            | "Cs"
            | "Mg"
            | "Ca"
            | "Sr"
            | "Ba"
            | "Zn"
            | "Fe"
            | "Cu"
            | "Mn"
            | "Cl"
    )
}

fn normalize_vector3(vector: Vector3<f32>, fallback: Vector3<f32>) -> Vector3<f32> {
    let norm_sq = vector.norm_squared();
    if norm_sq <= 0.000001 {
        let fallback_norm_sq = fallback.norm_squared();
        if fallback_norm_sq <= 0.000001 {
            Vector3::new(0.0, 0.0, 1.0)
        } else {
            fallback / fallback.norm()
        }
    } else {
        vector / norm_sq.sqrt()
    }
}

fn initial_cartoon_side(tangent: Vector3<f32>) -> Vector3<f32> {
    let up = Vector3::new(0.0, 1.0, 0.0);
    let alternate = Vector3::new(1.0, 0.0, 0.0);
    let candidate = tangent.cross(&up);
    if candidate.norm_squared() > 0.0001 {
        normalize_vector3(candidate, up)
    } else {
        normalize_vector3(tangent.cross(&alternate), alternate)
    }
}

fn orthogonalize_to_tangent(
    vector: Vector3<f32>,
    tangent: Vector3<f32>,
    fallback: Vector3<f32>,
) -> Vector3<f32> {
    let projected = vector - tangent * vector.dot(&tangent);
    if projected.norm_squared() > 0.0001 {
        normalize_vector3(projected, fallback)
    } else {
        let alternate = initial_cartoon_side(tangent);
        normalize_vector3(alternate - tangent * alternate.dot(&tangent), fallback)
    }
}

fn interpolate_orientation_hint(
    start: Option<Vector3<f32>>,
    end: Option<Vector3<f32>>,
    t: f32,
) -> Option<Vector3<f32>> {
    match (start, end) {
        (Some(start), Some(end)) => {
            let end = if start.dot(&end) < 0.0 { -end } else { end };
            Some(normalize_vector3(start + (end - start) * t, start))
        }
        (Some(vector), None) | (None, Some(vector)) => Some(vector),
        (None, None) => None,
    }
}

fn chain_color(chain: &ChainRecord, biopolymer: &Biopolymer, chain_index: usize) -> Color32 {
    let protein_residue_count = chain
        .residue_indices
        .iter()
        .filter_map(|&residue_index| biopolymer.residues.get(residue_index))
        .filter(|residue| residue.is_standard_amino_acid)
        .count();

    let long_chain_palette = [
        Color32::from_rgb(111, 164, 242),
        Color32::from_rgb(137, 184, 248),
        Color32::from_rgb(101, 145, 226),
    ];
    let short_chain_palette = [
        Color32::from_rgb(235, 170, 136),
        Color32::from_rgb(244, 191, 157),
        Color32::from_rgb(222, 151, 120),
    ];

    if protein_residue_count >= 40 {
        long_chain_palette[chain_index % long_chain_palette.len()]
    } else {
        short_chain_palette[chain_index % short_chain_palette.len()]
    }
}

fn lighten(color: Color32, factor: f32) -> Color32 {
    mix_color(color, Color32::WHITE, factor)
}

fn darken(color: Color32, factor: f32) -> Color32 {
    mix_color(color, Color32::BLACK, factor)
}

fn mix_color(base: Color32, target: Color32, factor: f32) -> Color32 {
    let clamped = factor.clamp(0.0, 1.0);
    let mix = |start: u8, end: u8| -> u8 {
        (start as f32 + (end as f32 - start as f32) * clamped).round() as u8
    };

    Color32::from_rgba_unmultiplied(
        mix(base.r(), target.r()),
        mix(base.g(), target.g()),
        mix(base.b(), target.b()),
        base.a(),
    )
}

fn color32(point: Point3<f32>) -> Color32 {
    Color32::from_rgb(
        (point.x.clamp(0.0, 1.0) * 255.0) as u8,
        (point.y.clamp(0.0, 1.0) * 255.0) as u8,
        (point.z.clamp(0.0, 1.0) * 255.0) as u8,
    )
}
