use super::*;

use std::borrow::Cow;
use std::f32::consts::PI;

use eframe::egui::{Pos2, Rect};
use nalgebra::Point3;

use crate::domain::{
    Atom, PdbAtomAnnotation, ResidueId, SecondaryStructureKind, SecondaryStructureSpan, Structure,
    build_biopolymer,
};
use crate::frontend::ViewportVisualState;

use super::super::super::camera::Projector;

/// Single-chain structure with one ALA Cα per residue along an ideal α-helix
/// trace (1.5 Å rise, 100° turn, 2.3 Å radius) and no HELIX/SHEET records.
fn helix_structure(residues: usize) -> Structure {
    let mut atoms = Vec::with_capacity(residues);
    let mut annotations = Vec::with_capacity(residues);
    for i in 0..residues {
        let angle = 100.0 * PI / 180.0 * i as f32;
        atoms.push(Atom {
            element: "C".to_string(),
            position: Point3::new(2.3 * angle.cos(), 2.3 * angle.sin(), 1.5 * i as f32),
            charge: 0.0,
        });
        annotations.push(PdbAtomAnnotation {
            atom_name: "CA".to_string(),
            residue_name: "ALA".to_string(),
            chain_id: 'A',
            residue_seq: i as i32 + 1,
            insertion_code: ' ',
        });
    }
    let mut structure = Structure::with_bonds("helix", atoms, Vec::new());
    structure.biopolymer = build_biopolymer(&annotations, Vec::new());
    structure
}

/// The CPU painter path has no depth buffer, so `build_cartoon_triangles`
/// must back-face cull — otherwise the closed ribbon's hidden underside
/// paints over its visible front and scatters dark triangular slivers down
/// every helix. Guard that every emitted triangle faces the camera, and that
/// culling actually dropped the (roughly half) back-facing ones rather than
/// being a no-op.
#[test]
fn cartoon_triangles_are_back_face_culled() {
    use eframe::egui::Vec2;

    let structure = helix_structure(16);
    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    let visual_state = ViewportVisualState::default();
    let viewport = Projector::new(
        Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)),
        Point3::new(0.0, 0.0, 11.25),
        22.0,
        60.0,
        0.6,
        0.4,
        Vec2::ZERO,
    );
    let segments = visual_state.cartoon.profile_segments.clamp(6, 48);

    let sweeps = cartoon_chain_sweeps(&structure, biopolymer, &visual_state);
    assert!(!sweeps.is_empty(), "helix should produce a ribbon sweep");

    let mut kept = 0usize;
    let mut uncolled = 0usize;
    for samples in &sweeps {
        let triangles = build_cartoon_triangles(&viewport, samples, &visual_state);
        for triangle in &triangles {
            assert!(
                cartoon_triangle_faces_camera(triangle),
                "build_cartoon_triangles emitted a back-facing triangle — \
                 back-face culling regressed; the painter path will show dark \
                 triangular slivers on the ribbon"
            );
        }
        kept += triangles.len();
        // Body quads (two triangles each) plus the two end caps, before culling.
        uncolled += samples.len().saturating_sub(1) * segments * 2 + 2 * segments;
    }

    assert!(kept > 0, "expected front-facing ribbon triangles");
    // A closed convex tube shows ~half its faces, so culling must remove a
    // substantial fraction — a no-op (kept == uncolled) means it regressed.
    assert!(
        kept < uncolled * 3 / 4,
        "back-face culling removed too little ({kept} of {uncolled}); it may be a no-op"
    );
}

#[test]
fn cartoon_derives_secondary_structure_when_records_absent() {
    let structure = helix_structure(12);
    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    assert!(biopolymer.secondary_structures.is_empty());

    let resolved = resolve_secondary_structures(&structure, biopolymer);
    let chain_id = biopolymer.chains[0].id;
    let helix_residues = biopolymer
        .residues
        .iter()
        .filter(|residue| {
            residue_cartoon_kind(residue, resolved.as_ref(), chain_id) == CartoonSegmentKind::Helix
        })
        .count();
    assert!(
        helix_residues >= 8,
        "expected the helix to be drawn as helix ribbon"
    );
}

#[test]
fn cartoon_prefers_explicit_secondary_structure() {
    let mut structure = helix_structure(12);
    let biopolymer = structure.biopolymer.as_mut().expect("biopolymer");
    biopolymer.secondary_structures = vec![SecondaryStructureSpan {
        kind: SecondaryStructureKind::Sheet,
        start: ResidueId::new('A', 1, ' '),
        end: ResidueId::new('A', 12, ' '),
    }];

    let biopolymer = structure.biopolymer.as_ref().expect("biopolymer");
    let resolved = resolve_secondary_structures(&structure, biopolymer);
    // Helical geometry, but the explicit sheet record is used verbatim.
    assert!(matches!(resolved, Cow::Borrowed(_)));
    assert!(
        resolved
            .iter()
            .all(|span| span.kind == SecondaryStructureKind::Sheet)
    );
}
