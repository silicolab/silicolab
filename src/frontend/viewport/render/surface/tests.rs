use std::collections::HashMap;

use nalgebra::Point3;

use super::*;

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
        "surface mesh has {non_manifold} non-manifold edges across {} triangles",
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

#[test]
fn representation_overlay_builds_gpu_mesh_without_chain_surface() {
    let structure = Structure::new(
        "overlay",
        vec![
            crate::domain::Atom {
                element: "C".to_string(),
                position: Point3::origin(),
                charge: 0.0,
            },
            crate::domain::Atom {
                element: "O".to_string(),
                position: Point3::new(1.2, 0.0, 0.0),
                charge: 0.0,
            },
        ],
    );
    let mut visual = ViewportVisualState::default();
    visual.surface_overlay.atoms.insert(0, true);
    visual.surface_overlay.atoms.insert(1, true);
    let key = SurfaceCacheKey::new(1, 1, &structure, &visual);
    let mesh = build_surface_world_mesh(&structure, &key, &visual, &mut SurfaceCache::default());
    assert!(!mesh.is_empty());
}
