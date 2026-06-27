use std::collections::HashSet;

use eframe::egui::{Color32, Pos2};
use nalgebra::{Point3, Vector3};

use crate::{domain::Structure, frontend::SurfaceStyle};

use super::super::camera::{Projector, camera_forward_world};
use super::super::gpu::MeshVertex;
use super::backend::{LineSegmentPrimitive, RenderScene};
use super::cartoon::{ScreenDepthBuffer, mesh_sample_visible};
use super::{
    MESH_VISIBILITY_SAMPLE_PIXELS, PrimitiveMeshVertex, PrimitiveTriangle, SurfaceCache,
    SurfaceCacheKey, ViewportVisualState, atom_chain_id, atom_is_standard_amino_acid, lerp_pos2,
    normalize_vector3, surface_atom_indices,
};

mod color;
mod mesh;

use color::{mesh_stroke_color, shade_union_surface_color, surface_alpha, surface_fill_color};
use mesh::build_union_surface_mesh;

const SURFACE_FILL_GRID_SPACING: f32 = 0.82;
const SURFACE_MESH_GRID_SPACING: f32 = 1.18;

/// Sentinel "chain id" for the representation surface — the molecular surface
/// over atoms with the Surface overlay enabled, which has no real chain. It
/// carries no `chain_colors` entry, so it falls back to the default surface
/// tint.
const REPRESENTATION_SURFACE_CHAIN: char = '\u{0}';

#[derive(Clone, Copy)]
struct SurfaceAtom {
    position: Point3<f32>,
    radius: f32,
}

pub(crate) struct SurfaceSceneGeometry {
    pub(super) chains: Vec<SurfaceChainGeometry>,
}

pub(crate) struct SurfaceChainGeometry {
    pub(super) chain_id: char,
    pub(super) vertices: Vec<SurfaceMeshVertex>,
    pub(super) triangles: Vec<SurfaceTriangleGeometry>,
}

#[derive(Clone, Copy)]
pub(super) struct SurfaceMeshVertex {
    position: Point3<f32>,
    normal: Vector3<f32>,
}

#[derive(Clone, Copy)]
pub(super) struct SurfaceTriangleGeometry {
    pub(super) indices: [u32; 3],
}

pub(crate) fn build_cached_surface_scene(
    structure: &Structure,
    surface_cache_key: &SurfaceCacheKey,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    cache: &mut SurfaceCache,
    occluder_depth: Option<&ScreenDepthBuffer>,
) -> RenderScene {
    if surface_cache_key.surface_chains.is_empty() && surface_cache_key.surface_atoms.is_empty() {
        return RenderScene::default();
    }

    let surface_geometry = cached_surface_geometry(cache, structure, surface_cache_key);
    build_surface_scene_from_geometry(surface_geometry, viewport, visual_state, occluder_depth)
}

/// World-space surface mesh (position, normal, translucent color) for the GPU
/// transparent mesh pipeline. The expensive contoured geometry is cached
/// (selection-independent); only the cheap per-chain coloring runs each build.
pub(crate) fn build_surface_world_mesh(
    structure: &Structure,
    surface_cache_key: &SurfaceCacheKey,
    visual_state: &ViewportVisualState,
    cache: &mut SurfaceCache,
) -> Vec<MeshVertex> {
    if visual_state.surface.chains.is_empty() {
        return Vec::new();
    }
    let geometry = cached_surface_geometry(cache, structure, surface_cache_key);
    let alpha = (1.0 - visual_state.surface.transparency).clamp(0.0, 1.0);
    let mut mesh = Vec::new();
    for chain_surface in &geometry.chains {
        let base_color = visual_state
            .chain_colors
            .get(&chain_surface.chain_id)
            .copied()
            .unwrap_or(Color32::from_rgb(120, 150, 210));
        let mut color = base_color.to_normalized_gamma_f32();
        color[3] = alpha;
        for triangle in &chain_surface.triangles {
            for &index in &triangle.indices {
                let vertex = chain_surface.vertices[index as usize];
                mesh.push(MeshVertex {
                    position: [vertex.position.x, vertex.position.y, vertex.position.z],
                    normal: [vertex.normal.x, vertex.normal.y, vertex.normal.z],
                    color,
                });
            }
        }
    }
    mesh
}

pub(crate) fn build_surface_scene(
    structure: &Structure,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    occluder_depth: Option<&ScreenDepthBuffer>,
) -> RenderScene {
    let surface_geometry = SurfaceSceneGeometry {
        chains: surface_chain_geometries(structure, visual_state),
    };
    build_surface_scene_from_geometry(&surface_geometry, viewport, visual_state, occluder_depth)
}

fn build_surface_scene_from_geometry(
    surface_geometry: &SurfaceSceneGeometry,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    occluder_depth: Option<&ScreenDepthBuffer>,
) -> RenderScene {
    let mut transparent_meshes = Vec::new();
    let mut lines = Vec::new();

    for chain_surface in &surface_geometry.chains {
        if chain_surface.vertices.is_empty() || chain_surface.triangles.is_empty() {
            continue;
        }
        let base_color = visual_state
            .chain_colors
            .get(&chain_surface.chain_id)
            .copied()
            .unwrap_or(Color32::from_rgb(120, 150, 210));
        match visual_state.surface.style {
            SurfaceStyle::Fill => {
                transparent_meshes.extend(build_surface_fill_triangles(
                    chain_surface,
                    viewport,
                    base_color,
                    visual_state,
                ));
            }
            SurfaceStyle::Mesh => {
                lines.extend(build_surface_mesh_lines(
                    viewport,
                    chain_surface,
                    base_color,
                    visual_state.surface.transparency,
                    occluder_depth,
                ));
            }
        }
    }

    let mut scene = RenderScene::default();
    scene.push_transparent_meshes(transparent_meshes);
    scene.push_lines(lines);
    scene.sorted()
}

fn cached_surface_geometry<'a>(
    cache: &'a mut SurfaceCache,
    structure: &Structure,
    key: &SurfaceCacheKey,
) -> &'a SurfaceSceneGeometry {
    if cache.key.as_ref() != Some(key) {
        cache.geometry = Some(SurfaceSceneGeometry {
            chains: surface_chain_geometries_with_style(
                structure,
                key.surface_chains.iter().copied(),
                &key.surface_atoms,
                key.style,
            ),
        });
        cache.key = Some(key.clone());
    }

    cache
        .geometry
        .as_ref()
        .expect("surface geometry cache must be initialized")
}

fn surface_chain_geometries(
    structure: &Structure,
    visual_state: &ViewportVisualState,
) -> Vec<SurfaceChainGeometry> {
    surface_chain_geometries_with_style(
        structure,
        visual_state.surface.chains.iter().copied(),
        &surface_atom_indices(structure, visual_state),
        visual_state.surface.style,
    )
}

fn surface_chain_geometries_with_style<I>(
    structure: &Structure,
    surface_chains: I,
    surface_atoms: &[usize],
    style: SurfaceStyle,
) -> Vec<SurfaceChainGeometry>
where
    I: IntoIterator<Item = char>,
{
    let mut chains = Vec::new();
    for chain_id in surface_chains {
        let chain_atoms = structure
            .atoms
            .iter()
            .enumerate()
            .filter_map(|(atom_index, atom)| {
                (atom_chain_id(structure, atom_index) == Some(chain_id)
                    && atom_is_standard_amino_acid(structure, atom_index))
                .then_some(SurfaceAtom {
                    position: atom.position,
                    radius: crate::domain::chemistry::element_style(&atom.element).display_radius
                        + 1.35,
                })
            })
            .collect::<Vec<_>>();
        if chain_atoms.is_empty() {
            continue;
        }
        if let Some(geometry) = build_surface_chain_geometry(chain_id, &chain_atoms, style) {
            chains.push(geometry);
        }
    }

    // Representation surface: a molecular surface over the overlay atom set
    // (any molecule), grouped under a sentinel id so it works without chains.
    if !surface_atoms.is_empty() {
        let atoms = surface_atoms
            .iter()
            .filter_map(|&atom_index| {
                structure.atoms.get(atom_index).map(|atom| SurfaceAtom {
                    position: atom.position,
                    radius: crate::domain::chemistry::element_style(&atom.element).display_radius
                        + 1.35,
                })
            })
            .collect::<Vec<_>>();
        if !atoms.is_empty()
            && let Some(geometry) =
                build_surface_chain_geometry(REPRESENTATION_SURFACE_CHAIN, &atoms, style)
        {
            chains.push(geometry);
        }
    }

    chains
}

/// Project every surface triangle for the filled style. The mesh is a closed,
/// watertight shell, so — unlike the wireframe — we keep *all* faces instead of
/// back-face culling. The compositor sorts the translucent triangles
/// back-to-front and alpha-blends them, drawing each far wall before the near
/// wall in front of it; that fills the concave necks and pockets where a
/// normal-only cull would punch holes (the wall there is front-most yet faces
/// away from the camera, and with no depth buffer in this pass a cull cannot
/// tell "hidden" from "visible but angled away"). The GPU surface pipeline
/// renders the same shell with `cull_mode: None`, so both paths now match.
fn build_projected_surface_triangles(
    projected_vertices: &[PrimitiveMeshVertex],
    triangles: &[SurfaceTriangleGeometry],
) -> Vec<PrimitiveTriangle> {
    triangles
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = triangle.indices;
            let first = *projected_vertices.get(a as usize)?;
            let second = *projected_vertices.get(b as usize)?;
            let third = *projected_vertices.get(c as usize)?;
            Some(super::primitive_triangle(first, second, third))
        })
        .collect()
}

fn build_surface_fill_triangles(
    chain_surface: &SurfaceChainGeometry,
    viewport: &Projector,
    base_color: Color32,
    visual_state: &ViewportVisualState,
) -> Vec<PrimitiveTriangle> {
    // Shade the surface as an opaque colour and premultiply the transparency
    // exactly once, here at the end. `Color32` stores premultiplied alpha, so
    // baking the alpha into `surface_fill_color` and then rebuilding the colour
    // in `shade_union_surface_color` premultiplied it twice — darkening the
    // surface toward black through the midrange, which is invisible over a dark
    // background but glaring over a light one.
    let tint = surface_fill_color(base_color);
    let alpha = surface_alpha(visual_state.surface.transparency);
    let projected_vertices = chain_surface
        .vertices
        .iter()
        .map(|vertex| {
            let projected = viewport.project(vertex.position);
            let shaded = shade_union_surface_color(
                viewport,
                tint,
                vertex.normal,
                visual_state.lighting.preset,
            );
            PrimitiveMeshVertex {
                pos: projected.pos,
                depth: projected.depth,
                color: Color32::from_rgba_unmultiplied(shaded.r(), shaded.g(), shaded.b(), alpha),
            }
        })
        .collect::<Vec<_>>();
    build_projected_surface_triangles(&projected_vertices, &chain_surface.triangles)
}

fn build_surface_chain_geometry(
    chain_id: char,
    atoms: &[SurfaceAtom],
    style: SurfaceStyle,
) -> Option<SurfaceChainGeometry> {
    let spacing = match style {
        SurfaceStyle::Fill => SURFACE_FILL_GRID_SPACING,
        SurfaceStyle::Mesh => SURFACE_MESH_GRID_SPACING,
    };
    let mesh = build_union_surface_mesh(atoms, spacing)?;
    Some(SurfaceChainGeometry {
        chain_id,
        vertices: mesh.vertices,
        triangles: mesh.triangles,
    })
}

fn build_surface_mesh_lines(
    viewport: &Projector,
    chain_surface: &SurfaceChainGeometry,
    base_color: Color32,
    transparency: f32,
    occluder_depth: Option<&ScreenDepthBuffer>,
) -> Vec<LineSegmentPrimitive> {
    let view_direction = camera_forward_world(viewport);
    let stroke_color = mesh_stroke_color(base_color, transparency);
    let mut edges = HashSet::<(u32, u32)>::new();
    let mut lines = Vec::new();
    let projected = chain_surface
        .vertices
        .iter()
        .map(|vertex| viewport.project(vertex.position))
        .collect::<Vec<_>>();

    for triangle in &chain_surface.triangles {
        let [a, b, c] = triangle.indices;
        let normal = normalize_vector3(
            chain_surface.vertices[a as usize].normal
                + chain_surface.vertices[b as usize].normal
                + chain_surface.vertices[c as usize].normal,
            chain_surface.vertices[a as usize].normal,
        );
        if normal.dot(&view_direction) <= 0.0 {
            continue;
        }
        for (start, end) in [(a, b), (b, c), (c, a)] {
            let edge = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            if !edges.insert(edge) {
                continue;
            }
            for visible_run in visible_mesh_line_runs(
                projected[start as usize].pos,
                projected[start as usize].depth,
                projected[end as usize].pos,
                projected[end as usize].depth,
                occluder_depth,
            ) {
                lines.push(LineSegmentPrimitive {
                    start: visible_run.start,
                    end: visible_run.end,
                    color: stroke_color,
                    width: 0.9,
                });
            }
        }
    }

    lines
}

struct VisibleLineRun {
    start: Pos2,
    end: Pos2,
}

fn visible_mesh_line_runs(
    start: Pos2,
    start_depth: f32,
    end: Pos2,
    end_depth: f32,
    occluder_depth: Option<&ScreenDepthBuffer>,
) -> Vec<VisibleLineRun> {
    let Some(depth_buffer) = occluder_depth else {
        return vec![VisibleLineRun { start, end }];
    };

    let delta = end - start;
    let length = delta.length();
    let segments = ((length / MESH_VISIBILITY_SAMPLE_PIXELS).ceil() as usize).clamp(1, 64);
    let mut visible_run_start = None;
    let mut runs = Vec::new();

    for step in 0..segments {
        let t0 = step as f32 / segments as f32;
        let t1 = (step + 1) as f32 / segments as f32;
        let midpoint = lerp_pos2(start, end, (t0 + t1) * 0.5);
        let midpoint_depth = start_depth + (end_depth - start_depth) * ((t0 + t1) * 0.5);
        let visible = mesh_sample_visible(depth_buffer, midpoint, midpoint_depth);

        if visible {
            visible_run_start.get_or_insert(t0);
        } else if let Some(run_start) = visible_run_start.take() {
            runs.push(VisibleLineRun {
                start: lerp_pos2(start, end, run_start),
                end: lerp_pos2(start, end, t0),
            });
        }
    }

    if let Some(run_start) = visible_run_start {
        runs.push(VisibleLineRun {
            start: lerp_pos2(start, end, run_start),
            end,
        });
    }
    runs
}

#[cfg(test)]
mod tests;
