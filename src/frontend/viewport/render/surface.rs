use std::collections::{HashMap, HashSet};

use eframe::egui::{Color32, Pos2};
use nalgebra::{Point3, Vector3};

use crate::{
    domain::Structure,
    frontend::{LightPreset, SurfaceStyle},
};

use super::super::camera::{Projector, camera_forward_world};
use super::super::gpu::MeshVertex;
use super::backend::{LineSegmentPrimitive, RenderScene};
use super::cartoon::{ScreenDepthBuffer, mesh_sample_visible};
use super::{
    MESH_VISIBILITY_SAMPLE_PIXELS, PrimitiveMeshVertex, PrimitiveTriangle, SurfaceCache,
    SurfaceCacheKey, ViewportVisualState, atom_chain_id, atom_is_standard_amino_acid, darken,
    lerp_pos2, lighten, mix_color, normalize_vector3, surface_atom_indices,
};

const SURFACE_FILL_GRID_SPACING: f32 = 0.82;
const SURFACE_MESH_GRID_SPACING: f32 = 1.18;
const SURFACE_VERTEX_QUANTIZATION: f32 = 1024.0;

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

#[derive(Clone, Copy)]
struct SurfaceGridPoint {
    position: Point3<f32>,
    value: f32,
    normal: Vector3<f32>,
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

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct SurfaceVertexKey([i32; 3]);

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

struct SurfaceMeshBuilder {
    vertices: Vec<SurfaceMeshVertex>,
    vertex_lookup: HashMap<SurfaceVertexKey, u32>,
    triangles: Vec<SurfaceTriangleGeometry>,
}

impl SurfaceMeshBuilder {
    fn new() -> Self {
        Self {
            vertices: Vec::new(),
            vertex_lookup: HashMap::new(),
            triangles: Vec::new(),
        }
    }

    fn vertex_index(&mut self, vertex: SurfaceMeshVertex) -> u32 {
        let key = quantize_surface_vertex(vertex.position);
        if let Some(&index) = self.vertex_lookup.get(&key) {
            let entry = &mut self.vertices[index as usize];
            entry.normal = normalize_vector3(entry.normal + vertex.normal, entry.normal);
            index
        } else {
            let index = self.vertices.len() as u32;
            self.vertices.push(vertex);
            self.vertex_lookup.insert(key, index);
            index
        }
    }

    fn push_triangle(
        &mut self,
        first: SurfaceMeshVertex,
        second: SurfaceMeshVertex,
        third: SurfaceMeshVertex,
    ) {
        // Reject only triangles that collapse once their corners are welded —
        // i.e. two corners land on the same grid vertex. Dropping those leaves
        // no crack, because the face they would have covered is a degenerate
        // point that the neighbouring cell omits too. A thin-but-real sliver
        // (three distinct welded vertices) must be kept: each of its edges is
        // shared with a neighbour, so culling it on area instead would tear a
        // one-triangle hole in the surface.
        let a = self.vertex_index(first);
        let b = self.vertex_index(second);
        let c = self.vertex_index(third);
        if a == b || b == c || a == c {
            return;
        }
        self.triangles
            .push(SurfaceTriangleGeometry { indices: [a, b, c] });
    }
}

struct SurfaceMeshGeometry {
    vertices: Vec<SurfaceMeshVertex>,
    triangles: Vec<SurfaceTriangleGeometry>,
}

fn build_union_surface_mesh(atoms: &[SurfaceAtom], spacing: f32) -> Option<SurfaceMeshGeometry> {
    if atoms.is_empty() {
        return None;
    }

    let padding = atoms.iter().map(|atom| atom.radius).fold(0.0_f32, f32::max) + 0.8;
    let mut min = atoms[0].position.coords;
    let mut max = atoms[0].position.coords;
    for atom in atoms {
        min = min.inf(&atom.position.coords);
        max = max.sup(&atom.position.coords);
    }
    min -= Vector3::repeat(padding);
    max += Vector3::repeat(padding);

    let dims = [
        ((max.x - min.x) / spacing).ceil() as usize + 1,
        ((max.y - min.y) / spacing).ceil() as usize + 1,
        ((max.z - min.z) / spacing).ceil() as usize + 1,
    ];
    if dims.iter().any(|dim| *dim < 2 || *dim > 96) {
        return None;
    }

    let value_at_grid = |x: usize, y: usize, z: usize| -> SurfaceGridPoint {
        let position = Point3::new(
            min.x + x as f32 * spacing,
            min.y + y as f32 * spacing,
            min.z + z as f32 * spacing,
        );
        let mut best_value = f32::INFINITY;
        let mut best_normal = Vector3::new(0.0, 0.0, 1.0);
        for atom in atoms {
            let delta = position - atom.position;
            let distance = delta.norm().max(0.0001);
            let value = distance - atom.radius;
            if value < best_value {
                best_value = value;
                best_normal = delta / distance;
            }
        }
        SurfaceGridPoint {
            position,
            value: best_value,
            normal: best_normal,
        }
    };

    let mut values = Vec::with_capacity(dims[0] * dims[1] * dims[2]);
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                values.push(value_at_grid(x, y, z));
            }
        }
    }

    let index = |x: usize, y: usize, z: usize| -> usize { (z * dims[1] + y) * dims[0] + x };
    let cube_corners = [
        [0, 0, 0],
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [1, 1, 1],
        [0, 1, 1],
    ];
    let tetrahedra = [
        [0, 5, 1, 6],
        [0, 1, 2, 6],
        [0, 2, 3, 6],
        [0, 3, 7, 6],
        [0, 7, 4, 6],
        [0, 4, 5, 6],
    ];

    let mut builder = SurfaceMeshBuilder::new();
    for z in 0..dims[2] - 1 {
        for y in 0..dims[1] - 1 {
            for x in 0..dims[0] - 1 {
                let mut cube = [values[0]; 8];
                for (corner_index, [dx, dy, dz]) in cube_corners.iter().enumerate() {
                    cube[corner_index] = values[index(x + dx, y + dy, z + dz)];
                }
                for tetra in tetrahedra {
                    polygonize_surface_tetra_mesh(&cube, tetra, &mut builder);
                }
            }
        }
    }

    if builder.triangles.is_empty() {
        return None;
    }

    Some(SurfaceMeshGeometry {
        vertices: builder.vertices,
        triangles: builder.triangles,
    })
}

fn polygonize_surface_tetra_mesh(
    cube: &[SurfaceGridPoint; 8],
    tetra: [usize; 4],
    builder: &mut SurfaceMeshBuilder,
) {
    let points = [
        cube[tetra[0]],
        cube[tetra[1]],
        cube[tetra[2]],
        cube[tetra[3]],
    ];
    let inside = [
        points[0].value <= 0.0,
        points[1].value <= 0.0,
        points[2].value <= 0.0,
        points[3].value <= 0.0,
    ];
    let inside_count = inside.iter().filter(|value| **value).count();
    if inside_count == 0 || inside_count == 4 {
        return;
    }

    let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    let mut vertices = Vec::with_capacity(4);
    for (a, b) in edges {
        if inside[a] == inside[b] {
            continue;
        }
        vertices.push(interpolate_surface_mesh_vertex(points[a], points[b]));
    }

    match vertices.len() {
        3 => {
            if inside_count == 1 {
                builder.push_triangle(vertices[0], vertices[1], vertices[2]);
            } else {
                builder.push_triangle(vertices[0], vertices[2], vertices[1]);
            }
        }
        4 => {
            // The 2-in/2-out case crosses four edges, forming a quad. The
            // crossing points are collected in edge-list order, not in order
            // around the quad's boundary: walking the four tetra faces shows
            // the cyclic order is always vertices[0,1,3,2], so the only real
            // diagonal is vertices[0]-vertices[3]. Fanning on vertices[0]-[2]
            // (a boundary edge) folds the two triangles over each other and
            // leaves a hole in every such cell — the gaps that broke the fill.
            builder.push_triangle(vertices[0], vertices[1], vertices[3]);
            builder.push_triangle(vertices[0], vertices[3], vertices[2]);
        }
        _ => {}
    }
}

fn interpolate_surface_mesh_vertex(a: SurfaceGridPoint, b: SurfaceGridPoint) -> SurfaceMeshVertex {
    let t = (a.value / (a.value - b.value)).clamp(0.0, 1.0);
    let position = Point3::from(a.position.coords + (b.position - a.position) * t);
    let normal = normalize_vector3(a.normal + (b.normal - a.normal) * t, a.normal);
    SurfaceMeshVertex { position, normal }
}

fn quantize_surface_vertex(point: Point3<f32>) -> SurfaceVertexKey {
    SurfaceVertexKey([
        (point.x * SURFACE_VERTEX_QUANTIZATION).round() as i32,
        (point.y * SURFACE_VERTEX_QUANTIZATION).round() as i32,
        (point.z * SURFACE_VERTEX_QUANTIZATION).round() as i32,
    ])
}

fn shade_union_surface_color(
    viewport: &Projector,
    base_color: Color32,
    surface_normal: Vector3<f32>,
    light_preset: LightPreset,
) -> Color32 {
    let view_normal = normalize_vector3(
        viewport.rotate_to_view(surface_normal),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let light_direction =
        normalize_vector3(Vector3::new(-0.30, 0.42, 1.0), Vector3::new(0.0, 0.0, 1.0));
    let half_vector = normalize_vector3(
        light_direction + Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let diffuse = view_normal.dot(&light_direction).max(0.0);
    let rim = (1.0 - view_normal.z.abs()).powi(2);
    let specular = view_normal.dot(&half_vector).max(0.0).powf(7.5);
    let (ambient, diffuse_strength, rim_strength, specular_strength) = match light_preset {
        LightPreset::Soft => (0.78, 0.16, 0.10, 0.05),
        LightPreset::Gentle => (0.82, 0.11, 0.07, 0.03),
        LightPreset::Studio => (0.70, 0.24, 0.12, 0.08),
    };
    let brightness =
        (ambient + diffuse * diffuse_strength + rim * rim_strength + specular * specular_strength)
            .clamp(0.0, 1.0);
    let lit = if brightness >= 0.72 {
        lighten(base_color, (brightness - 0.72) * 0.55)
    } else {
        darken(base_color, (0.72 - brightness) * 0.32)
    };
    Color32::from_rgba_unmultiplied(lit.r(), lit.g(), lit.b(), base_color.a())
}

/// Opaque base tint for the filled surface. The transparency is applied once,
/// after shading, by the caller — folding it in here as well would premultiply
/// the colour twice (see [`build_surface_fill_triangles`]).
fn surface_fill_color(base_color: Color32) -> Color32 {
    mix_color(base_color, Color32::WHITE, 0.18)
}

fn mesh_stroke_color(base_color: Color32, transparency: f32) -> Color32 {
    let tinted = darken(base_color, 0.12);
    let alpha = ((1.0 - transparency.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(tinted.r(), tinted.g(), tinted.b(), alpha)
}

fn surface_alpha(transparency: f32) -> u8 {
    // Linear over the full range so the slider spans completely transparent
    // (alpha 0) to completely opaque (alpha 255), matching the GPU surface.
    let opacity = 1.0 - transparency.clamp(0.0, 1.0);
    (opacity * 255.0).round() as u8
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
mod tests {
    use super::*;
    use eframe::egui::{Rect, Vec2};

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
}
