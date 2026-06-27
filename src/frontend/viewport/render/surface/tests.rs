use std::collections::HashMap;

use eframe::egui::{Rect, Vec2};

use super::*;

/// `Color32` is premultiplied, so the fill colour must fold in its alpha
/// exactly once. The un-premultiplied surface tint therefore has to stay the
/// same bright colour at every opacity — if alpha were premultiplied twice it
/// would scale the RGB down with alpha, darkening the surface toward black
/// (visible as a blue→black fade over a light background).
#[test]
fn fill_colour_does_not_darken_with_opacity() {
    let atoms = [
        SurfaceAtom {
            position: Point3::new(0.0, 0.0, 0.0),
            radius: 2.0,
        },
        SurfaceAtom {
            position: Point3::new(1.6, 0.4, 0.0),
            radius: 2.0,
        },
    ];
    let chain = build_surface_chain_geometry('A', &atoms, SurfaceStyle::Fill).unwrap();

    let unmultiplied_tint = |transparency: f32| {
        let mut visual = ViewportVisualState::default();
        visual.surface.style = SurfaceStyle::Fill;
        visual.surface.transparency = transparency;
        let triangles = build_surface_fill_triangles(
            &chain,
            &test_projector(),
            Color32::from_rgb(120, 150, 210),
            &visual,
        );
        let color = triangles[0].vertices[0].color;
        let [r, g, b, _] = eframe::egui::Rgba::from(color).to_srgba_unmultiplied();
        [r, g, b]
    };

    let opaque = unmultiplied_tint(0.0);
    let faint = unmultiplied_tint(0.5);
    for channel in 0..3 {
        assert!(
            (opaque[channel] as i32 - faint[channel] as i32).abs() <= 4,
            "opacity must not change the underlying tint: opaque {opaque:?} vs faint {faint:?}",
        );
    }
    assert!(
        opaque[2] > 180,
        "surface should read as a light blue tint, not near-black: {opaque:?}"
    );
}

#[test]
fn surface_alpha_spans_completely_transparent_to_opaque() {
    assert_eq!(surface_alpha(0.0), 255);
    assert_eq!(surface_alpha(1.0), 0);
    assert_eq!(surface_alpha(0.5), 128);
}

fn test_projector() -> Projector {
    Projector::new(
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(2000.0)),
        Point3::origin(),
        10.0,
        1000.0,
        0.0,
        0.0,
        Vec2::ZERO,
    )
}

/// The filled style renders the whole closed shell, so every mesh triangle
/// must reach the scene — a back-face cull would drop roughly half and tear
/// holes in the concave necks and pockets that face away from the camera.
#[test]
fn filled_surface_keeps_every_face() {
    let atoms = [
        SurfaceAtom {
            position: Point3::new(0.0, 0.0, 0.0),
            radius: 2.0,
        },
        SurfaceAtom {
            position: Point3::new(1.6, 0.4, 0.0),
            radius: 2.0,
        },
    ];
    let chain = build_surface_chain_geometry('A', &atoms, SurfaceStyle::Fill)
        .expect("two overlapping atoms should produce a surface mesh");

    let triangles = build_surface_fill_triangles(
        &chain,
        &test_projector(),
        Color32::from_rgb(120, 150, 210),
        &ViewportVisualState::default(),
    );

    assert_eq!(triangles.len(), chain.triangles.len());
}

/// The union-of-spheres isosurface is a closed volume that never reaches the
/// padded grid boundary, so a correctly meshed surface is watertight: every
/// undirected edge is shared by exactly two triangles. The 2-in/2-out
/// tetrahedron case used to fan the quad across a boundary edge instead of a
/// diagonal, folding its two triangles over each other and leaving a hole in
/// every such cell — the polygonal fragments and gaps this regression guards
/// against. A single triangle touching an edge (count != 2) is a crack.
fn assert_watertight(atoms: &[SurfaceAtom], spacing: f32) {
    let mesh = build_union_surface_mesh(atoms, spacing)
        .expect("overlapping atoms should produce a surface mesh");
    assert!(!mesh.triangles.is_empty());

    let mut edge_use = HashMap::<(u32, u32), u32>::new();
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.indices;
        for (start, end) in [(a, b), (b, c), (c, a)] {
            let edge = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            *edge_use.entry(edge).or_default() += 1;
        }
    }

    let non_manifold = edge_use.values().filter(|count| **count != 2).count();
    assert_eq!(
        non_manifold,
        0,
        "surface mesh has {non_manifold} non-manifold edges across {} triangles; \
         it should be a closed shell",
        mesh.triangles.len()
    );
}

#[test]
fn two_atom_surface_mesh_is_watertight() {
    let atoms = [
        SurfaceAtom {
            position: Point3::new(0.0, 0.0, 0.0),
            radius: 2.0,
        },
        SurfaceAtom {
            position: Point3::new(1.6, 0.4, 0.0),
            radius: 2.0,
        },
    ];
    assert_watertight(&atoms, SURFACE_FILL_GRID_SPACING);
    assert_watertight(&atoms, SURFACE_MESH_GRID_SPACING);
}

#[test]
fn cluster_surface_mesh_is_watertight() {
    // An off-lattice cluster of differently sized atoms exercises plenty of
    // 2-in/2-out tetrahedra and near-vertex crossings at once.
    let atoms = [
        SurfaceAtom {
            position: Point3::new(0.0, 0.0, 0.0),
            radius: 1.7,
        },
        SurfaceAtom {
            position: Point3::new(1.3, 0.9, 0.2),
            radius: 2.1,
        },
        SurfaceAtom {
            position: Point3::new(-0.7, 1.4, 0.8),
            radius: 1.9,
        },
        SurfaceAtom {
            position: Point3::new(0.5, -1.2, 1.1),
            radius: 1.6,
        },
    ];
    assert_watertight(&atoms, SURFACE_FILL_GRID_SPACING);
}
