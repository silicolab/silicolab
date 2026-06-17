use crate::{
    backend::representation::{BaseStyle, CartoonPrefs, RepresentationPrefs},
    domain::{AtomCategory, Structure},
    frontend::{CartoonSectionStyle, ViewportCartoonState, ViewportVisualState, state::AtomStyle},
};

/// Past this many solvent (water) atoms in an entry, every water is shown as
/// wireframe lines instead of ball-and-stick spheres. Thousands of full water
/// spheres are slow to tessellate every interaction frame; lines are cheap and
/// keep the box readable. The decision is made once, here, at the entry's first
/// load and baked into per-atom config (below), rather than re-derived from atom
/// counts inside the renderer.
const WATER_WIREFRAME_THRESHOLD: usize = 64;

/// Set per-entry view defaults when a structure is first shown, then persisted
/// with the entry. Most styles resolve lazily from the category tiers (software
/// default → project override → atom override) and are not materialized here.
/// The exception is bulk solvent: a heavily solvated entry bakes an explicit
/// per-atom wireframe style for each water, so the choice is stable config (not
/// a render-time heuristic) and a user can still promote individual waters — an
/// active-site water, say — back to a fuller representation.
pub fn apply_entry_render_defaults(
    viewport: &mut ViewportVisualState,
    structure: &Structure,
    prefs: &RepresentationPrefs,
) {
    // Periodic structures show their box by default, but biopolymers hide it: a
    // protein's crystallographic cell is usually noise, whereas an MD/crystal box
    // is essential context.
    viewport.show_cell = structure.cell.is_some() && structure.biopolymer.is_none();

    // Seed the global base-style default for non-polymer categories. Biopolymer
    // chains (Protein/NucleicAcid) are intentionally untouched so they keep their
    // Cartoon default and follow the cartoon geometry below. `Other` is included
    // so freshly *built* structures (which classify as `Other`) inherit the
    // default too. `set_category_style` clears the override when it equals the
    // software default, so the map stays sparse.
    let base = base_style_to_atom_style(prefs.base.default_style);
    for category in [
        AtomCategory::Ligand,
        AtomCategory::Ion,
        AtomCategory::Solvent,
        AtomCategory::Other,
    ] {
        viewport.set_category_style(category, base);
    }

    // Seed the cartoon-ribbon geometry from the global default.
    viewport.cartoon = cartoon_prefs_to_viewport(&prefs.cartoon);

    apply_solvent_render_default(viewport, structure);
}

/// Map a [`BaseStyle`] default 1:1 onto the renderer's [`AtomStyle`]. A faithful
/// pairing — never a downgrade — so picking a base style stores exactly what the
/// renderer draws.
fn base_style_to_atom_style(style: BaseStyle) -> AtomStyle {
    match style {
        BaseStyle::Wire => AtomStyle::Wireframe,
        BaseStyle::Stick => AtomStyle::Stick,
        BaseStyle::BallAndStick => AtomStyle::BallAndStick,
        BaseStyle::Sphere => AtomStyle::Sphere,
    }
}

/// Translate the persisted [`CartoonPrefs`] into the viewport's live cartoon
/// state. The integer counts widen to `usize`; the section widths/thicknesses
/// pass through unchanged.
fn cartoon_prefs_to_viewport(prefs: &CartoonPrefs) -> ViewportCartoonState {
    ViewportCartoonState {
        helix: CartoonSectionStyle::new(prefs.helix_width, prefs.helix_thickness),
        sheet: CartoonSectionStyle::new(prefs.sheet_width, prefs.sheet_thickness),
        coil: CartoonSectionStyle::new(prefs.coil_width, prefs.coil_thickness),
        smoothing: prefs.smoothing as usize,
        profile_segments: prefs.profile as usize,
    }
}

/// Bake the bulk-solvent display choice into per-atom config: past
/// [`WATER_WIREFRAME_THRESHOLD`] waters, every water becomes wireframe lines.
/// Split out from [`apply_entry_render_defaults`] so it can also migrate entries
/// saved before this default existed without disturbing their other view state
/// (e.g. the cell toggle).
pub fn apply_solvent_render_default(viewport: &mut ViewportVisualState, structure: &Structure) {
    let waters: Vec<(usize, AtomCategory)> = (0..structure.atoms.len())
        .map(|index| (index, structure.atom_category(index)))
        .filter(|(_, category)| *category == AtomCategory::Solvent)
        .collect();
    if waters.len() > WATER_WIREFRAME_THRESHOLD {
        viewport.apply_atom_styles(waters, AtomStyle::Wireframe);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Atom, PdbAtomAnnotation, build_biopolymer};
    use crate::frontend::state::AtomStyle;
    use nalgebra::Point3;

    fn atom(element: &str) -> Atom {
        Atom {
            element: element.to_string(),
            position: Point3::origin(),
            charge: 0.0,
        }
    }

    fn annotation(atom_name: &str, residue_name: &str, seq: i32) -> PdbAtomAnnotation {
        PdbAtomAnnotation {
            atom_name: atom_name.to_string(),
            residue_name: residue_name.to_string(),
            chain_id: 'A',
            residue_seq: seq,
            insertion_code: ' ',
        }
    }

    #[test]
    fn category_resolution_cartoons_protein_seeds_nonpolymer_base_style() {
        // Protein CA, water O, and a sodium ion — no per-atom overrides stored.
        let annotations = vec![
            annotation("CA", "ALA", 1),
            annotation("OW", "SOL", 2),
            annotation("NA", "NA", 3),
        ];
        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        let structure = Structure {
            title: "t".to_string(),
            atoms: vec![atom("C"), atom("O"), atom("Na")],
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        };

        let mut viewport = ViewportVisualState::default();
        apply_entry_render_defaults(&mut viewport, &structure, &RepresentationPrefs::default());

        // No *per-atom* overrides are materialized; non-polymer base styles come
        // from the project-level category tier the defaults just seeded.
        assert!(viewport.atom_styles.is_empty());
        // Biopolymer chains are untouched and keep their Cartoon default.
        assert_eq!(
            viewport.resolved_atom_style(&structure, 0),
            AtomStyle::Cartoon
        );
        // Solvent inherits the default base style (ball-and-stick by default).
        assert_eq!(
            viewport.resolved_atom_style(&structure, 1),
            AtomStyle::BallAndStick
        );
        // The ion now inherits the global base-style default too (ball-and-stick),
        // seeded over its Sphere software default.
        assert_eq!(
            viewport.resolved_atom_style(&structure, 2),
            AtomStyle::BallAndStick
        );
    }

    /// One protein anchor (so a biopolymer exists and SOL classifies as Solvent)
    /// plus `water` SOL oxygens.
    fn solvated(water: usize) -> Structure {
        let mut atoms = vec![atom("C")];
        let mut annotations = vec![annotation("CA", "ALA", 1)];
        for i in 0..water {
            atoms.push(atom("O"));
            annotations.push(annotation("OW", "SOL", 2 + i as i32));
        }
        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        Structure {
            title: "s".to_string(),
            atoms,
            bonds: Vec::new(),
            cell: None,
            biopolymer: Some(biopolymer),
        }
    }

    #[test]
    fn heavily_solvated_entry_bakes_per_atom_wireframe_water() {
        let structure = solvated(WATER_WIREFRAME_THRESHOLD + 1);
        let mut viewport = ViewportVisualState::default();
        apply_entry_render_defaults(&mut viewport, &structure, &RepresentationPrefs::default());

        // Every water (indices 1..) carries an explicit wireframe override, baked
        // in once at load rather than recomputed in the renderer.
        for index in 1..structure.atoms.len() {
            assert_eq!(
                viewport.atom_styles.get(&index),
                Some(&AtomStyle::Wireframe),
                "water atom {index} should be stored as wireframe"
            );
        }
        // The protein anchor is untouched (resolves to cartoon, no stored row).
        assert!(!viewport.atom_styles.contains_key(&0));
        assert_eq!(
            viewport.resolved_atom_style(&structure, 0),
            AtomStyle::Cartoon
        );

        // A user can still promote a specific (e.g. active-site) water back to a
        // fuller representation on top of the baked default.
        viewport.apply_atom_styles([(1usize, AtomCategory::Solvent)], AtomStyle::BallAndStick);
        assert_eq!(
            viewport.resolved_atom_style(&structure, 1),
            AtomStyle::BallAndStick
        );
        assert_eq!(
            viewport.resolved_atom_style(&structure, 2),
            AtomStyle::Wireframe
        );
    }

    #[test]
    fn lightly_solvated_entry_keeps_default_water() {
        // At the threshold (not above it), nothing is materialized; water resolves
        // to its category default.
        let structure = solvated(WATER_WIREFRAME_THRESHOLD);
        let mut viewport = ViewportVisualState::default();
        apply_entry_render_defaults(&mut viewport, &structure, &RepresentationPrefs::default());
        assert!(viewport.atom_styles.is_empty());
        assert_eq!(
            viewport.resolved_atom_style(&structure, 1),
            AtomStyle::BallAndStick
        );
    }

    #[test]
    fn periodic_structure_shows_cell_by_default() {
        use crate::domain::UnitCell;

        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let structure = Structure {
            title: "md".to_string(),
            atoms: vec![atom("O")],
            bonds: Vec::new(),
            cell: Some(cell),
            biopolymer: None,
        };

        let mut viewport = ViewportVisualState {
            show_cell: false,
            ..Default::default()
        };
        apply_entry_render_defaults(&mut viewport, &structure, &RepresentationPrefs::default());
        assert!(viewport.show_cell);
    }

    #[test]
    fn biopolymer_with_cell_hides_cell_by_default() {
        use crate::domain::UnitCell;

        // A protein anchor (so the structure carries a biopolymer) plus a cell:
        // the crystallographic box is hidden by default to keep the molecule clear.
        let annotations = vec![annotation("CA", "ALA", 1)];
        let biopolymer = build_biopolymer(&annotations, Vec::new()).expect("biopolymer");
        let cell = UnitCell::from_parameters(10.0, 10.0, 10.0, 90.0, 90.0, 90.0);
        let structure = Structure {
            title: "protein".to_string(),
            atoms: vec![atom("C")],
            bonds: Vec::new(),
            cell: Some(cell),
            biopolymer: Some(biopolymer),
        };

        let mut viewport = ViewportVisualState {
            show_cell: true,
            ..Default::default()
        };
        apply_entry_render_defaults(&mut viewport, &structure, &RepresentationPrefs::default());
        assert!(
            !viewport.show_cell,
            "a biopolymer's crystallographic cell should be hidden by default"
        );
    }
}
