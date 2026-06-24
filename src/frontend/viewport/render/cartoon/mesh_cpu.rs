use super::*;

use eframe::egui::Color32;
use nalgebra::Vector3;

use crate::domain::{Biopolymer, Structure};
use crate::frontend::viewport::{
    SecondaryStructureCache, SecondaryStructureCacheKey, ViewportVisualState,
};

use super::super::super::camera::Projector;
use super::super::backend::{LineSegmentPrimitive, RenderScene};
use super::super::{
    PrimitiveMeshVertex, PrimitiveTriangle, darken, edge_function, usable_biopolymer,
};

struct CartoonFragment {
    samples: Vec<CartoonSweepSample>,
    triangles: Vec<PrimitiveTriangle>,
}

pub(crate) fn build_biopolymer_cartoon_scene(
    structure: &Structure,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
) -> RenderScene {
    let Some(biopolymer) = usable_biopolymer(structure) else {
        return RenderScene::default();
    };
    build_cartoon_scene_from_fragments(
        cartoon_fragments(structure, biopolymer, viewport, visual_state),
        viewport,
        visual_state,
    )
}

pub(crate) fn build_cached_biopolymer_cartoon_scene(
    structure: &Structure,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    secondary_cache: &mut SecondaryStructureCache,
    secondary_key: SecondaryStructureCacheKey,
) -> RenderScene {
    let Some(biopolymer) = usable_biopolymer(structure) else {
        return RenderScene::default();
    };
    build_cartoon_scene_from_fragments(
        cached_cartoon_fragments(
            structure,
            biopolymer,
            viewport,
            visual_state,
            secondary_cache,
            secondary_key,
        ),
        viewport,
        visual_state,
    )
}

fn build_cartoon_scene_from_fragments(
    fragments: Vec<CartoonFragment>,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
) -> RenderScene {
    let mut opaque_meshes = Vec::new();
    let mut lines = Vec::new();
    for fragment in fragments {
        if visual_state.lighting.silhouettes && visual_state.lighting.silhouette_width > 0.0 {
            append_cartoon_silhouette(
                &mut lines,
                viewport,
                &fragment.samples,
                visual_state.lighting.silhouette_width,
            );
        }
        opaque_meshes.extend(fragment.triangles);
    }
    let mut scene = RenderScene::default();
    scene.push_opaque_meshes(opaque_meshes);
    scene.push_lines(lines);
    scene.sorted()
}

fn cartoon_fragments(
    structure: &Structure,
    biopolymer: &Biopolymer,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
) -> Vec<CartoonFragment> {
    cartoon_chain_sweeps(structure, biopolymer, visual_state)
        .into_iter()
        .map(|samples| CartoonFragment {
            triangles: build_cartoon_triangles(viewport, &samples, visual_state),
            samples,
        })
        .collect()
}

fn cached_cartoon_fragments(
    structure: &Structure,
    biopolymer: &Biopolymer,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    secondary_cache: &mut SecondaryStructureCache,
    secondary_key: SecondaryStructureCacheKey,
) -> Vec<CartoonFragment> {
    cached_cartoon_chain_sweeps(
        structure,
        biopolymer,
        visual_state,
        secondary_cache,
        secondary_key,
    )
    .into_iter()
    .map(|samples| CartoonFragment {
        triangles: build_cartoon_triangles(viewport, &samples, visual_state),
        samples,
    })
    .collect()
}

pub(crate) fn build_cartoon_triangles(
    viewport: &Projector,
    sweep_samples: &[CartoonSweepSample],
    visual_state: &ViewportVisualState,
) -> Vec<PrimitiveTriangle> {
    let profile_segments = visual_state.cartoon.profile_segments.clamp(6, 48);

    if sweep_samples.len() < 2 {
        return Vec::new();
    }

    let rings = sweep_samples
        .iter()
        .map(|sample| build_cartoon_ring(viewport, sample, profile_segments, visual_state))
        .collect::<Vec<_>>();
    let mut triangles = Vec::with_capacity((rings.len() - 1) * profile_segments * 2 + 28);

    for ring_pair in rings.windows(2) {
        let current = &ring_pair[0];
        let next = &ring_pair[1];
        for index in 0..profile_segments {
            let next_index = (index + 1) % profile_segments;
            triangles.push(cartoon_triangle(
                current[index],
                next[index],
                next[next_index],
            ));
            triangles.push(cartoon_triangle(
                current[index],
                next[next_index],
                current[next_index],
            ));
        }
    }

    append_cartoon_cap(
        &mut triangles,
        viewport,
        sweep_samples[0],
        &rings[0],
        -sweep_samples[0].tangent,
        visual_state,
    );
    append_cartoon_cap(
        &mut triangles,
        viewport,
        *sweep_samples.last().expect("non-empty sweep samples"),
        rings.last().expect("non-empty rings"),
        sweep_samples
            .last()
            .expect("non-empty sweep samples")
            .tangent,
        visual_state,
    );
    // Back-face cull before sorting. The live viewport's CPU path composites
    // through the egui painter, which has no depth buffer, so a closed opaque
    // ribbon would paint its hidden back faces over the visible front and
    // scatter dark slivers down the wide face of every helix — the triangular
    // shadows the GPU depth-buffer path never shows. The swept cross-section is
    // convex, so the camera-facing shell is exactly the front-wound triangles;
    // dropping the back-wound ones leaves a set that no longer self-overlaps, so
    // the depth sort below resolves the remaining across-sweep occlusion without
    // a depth buffer. Edge-on triangles have ~zero projected area and cover no
    // pixels, so dropping them with the back faces is harmless. The GPU path
    // builds its mesh separately (`build_biopolymer_cartoon_world_mesh`) and is
    // unaffected.
    triangles.retain(cartoon_triangle_faces_camera);
    triangles.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    triangles
}

/// Whether a projected cartoon triangle faces the camera, by its screen-space
/// winding. The ribbon mesh is consistently wound, so front faces all share one
/// sign of the projected signed area; [`Projector::project`] flips screen-y,
/// which makes the camera-facing winding negative here.
pub(crate) fn cartoon_triangle_faces_camera(triangle: &PrimitiveTriangle) -> bool {
    edge_function(
        triangle.vertices[0].pos,
        triangle.vertices[1].pos,
        triangle.vertices[2].pos,
    ) < 0.0
}

fn append_cartoon_cap(
    triangles: &mut Vec<PrimitiveTriangle>,
    viewport: &Projector,
    sample: CartoonSweepSample,
    ring: &[PrimitiveMeshVertex],
    cap_normal: Vector3<f32>,
    visual_state: &ViewportVisualState,
) {
    let projected = viewport.project(sample.position);
    let center = PrimitiveMeshVertex {
        pos: projected.pos,
        depth: projected.depth,
        color: shade_cartoon_color(
            viewport,
            darken(sample.color, 0.08),
            cap_normal,
            visual_state.lighting.preset,
        ),
    };

    for index in 0..ring.len() {
        let next_index = (index + 1) % ring.len();
        triangles.push(cartoon_triangle(center, ring[next_index], ring[index]));
    }
}

fn append_cartoon_silhouette(
    lines: &mut Vec<LineSegmentPrimitive>,
    viewport: &Projector,
    samples: &[CartoonSweepSample],
    width: f32,
) {
    for pair in samples.windows(2) {
        let start = viewport.project(pair[0].position).pos;
        let end = viewport.project(pair[1].position).pos;
        let local_width = pair[0].style.half_width.max(pair[0].style.half_thickness)
            + pair[1].style.half_width.max(pair[1].style.half_thickness);
        lines.push(LineSegmentPrimitive {
            start,
            end,
            color: Color32::from_rgba_unmultiplied(25, 28, 32, 90),
            width: width + local_width * 2.0,
        });
    }
}

fn cartoon_triangle(
    first: PrimitiveMeshVertex,
    second: PrimitiveMeshVertex,
    third: PrimitiveMeshVertex,
) -> PrimitiveTriangle {
    super::super::primitive_triangle(first, second, third)
}
