use super::*;

use nalgebra::{Point3, Vector3};

use crate::domain::Structure;
use crate::frontend::ViewportVisualState;

use super::super::super::gpu::MeshVertex;
use super::super::usable_biopolymer;

/// Build the world-space cartoon mesh (position, normal, color triangle soup)
/// for the GPU mesh pipeline. Camera-independent.
pub(crate) fn build_biopolymer_cartoon_world_mesh(
    structure: &Structure,
    visual_state: &ViewportVisualState,
) -> Vec<MeshVertex> {
    let Some(biopolymer) = usable_biopolymer(structure) else {
        return Vec::new();
    };
    let segments = visual_state.cartoon.profile_segments.clamp(6, 48);
    let mut mesh = Vec::new();
    for samples in cartoon_chain_sweeps(structure, biopolymer, visual_state) {
        append_cartoon_world_fragment(&mut mesh, &samples, segments);
    }
    mesh
}

fn mesh_vertex(position: Point3<f32>, normal: Vector3<f32>, color: [f32; 4]) -> MeshVertex {
    MeshVertex {
        position: [position.x, position.y, position.z],
        normal: [normal.x, normal.y, normal.z],
        color,
    }
}

/// Append one ribbon fragment to the GPU world mesh: a tube/ribbon swept along
/// the spline plus flat end caps.
fn append_cartoon_world_fragment(
    mesh: &mut Vec<MeshVertex>,
    samples: &[CartoonSweepSample],
    segments: usize,
) {
    if samples.len() < 2 {
        return;
    }
    let rings = samples
        .iter()
        .map(|sample| cartoon_ring_geometry(sample, segments))
        .collect::<Vec<_>>();
    let colors = samples
        .iter()
        .map(|sample| sample.color.to_normalized_gamma_f32())
        .collect::<Vec<_>>();

    for ring_index in 0..rings.len() - 1 {
        let current = &rings[ring_index];
        let next = &rings[ring_index + 1];
        let color_current = colors[ring_index];
        let color_next = colors[ring_index + 1];
        for index in 0..segments {
            let next_index = (index + 1) % segments;
            let a = mesh_vertex(current[index].0, current[index].1, color_current);
            let b = mesh_vertex(next[index].0, next[index].1, color_next);
            let c = mesh_vertex(next[next_index].0, next[next_index].1, color_next);
            let d = mesh_vertex(current[next_index].0, current[next_index].1, color_current);
            mesh.extend([a, b, c, a, c, d]);
        }
    }

    append_cartoon_world_cap(
        mesh,
        &rings[0],
        samples[0].position,
        -samples[0].tangent,
        colors[0],
    );
    let last = rings.len() - 1;
    append_cartoon_world_cap(
        mesh,
        &rings[last],
        samples[last].position,
        samples[last].tangent,
        colors[last],
    );
}

fn append_cartoon_world_cap(
    mesh: &mut Vec<MeshVertex>,
    ring: &[(Point3<f32>, Vector3<f32>)],
    center: Point3<f32>,
    cap_normal: Vector3<f32>,
    color: [f32; 4],
) {
    let center_vertex = mesh_vertex(center, cap_normal, color);
    for index in 0..ring.len() {
        let next_index = (index + 1) % ring.len();
        mesh.extend([
            center_vertex,
            mesh_vertex(ring[next_index].0, cap_normal, color),
            mesh_vertex(ring[index].0, cap_normal, color),
        ]);
    }
}
