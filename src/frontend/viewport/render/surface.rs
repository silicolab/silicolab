use eframe::egui::Color32;
use nalgebra::{Point3, Vector3};

use crate::{domain::Structure, frontend::SurfaceStyle};

use super::super::gpu::MeshVertex;
use super::{
    SurfaceCache, SurfaceCacheKey, ViewportVisualState, atom_chain_id, atom_is_standard_amino_acid,
};

mod mesh;

use mesh::build_union_surface_mesh;

const SURFACE_FILL_GRID_SPACING: f32 = 0.82;
const SURFACE_MESH_GRID_SPACING: f32 = 1.18;
const REPRESENTATION_SURFACE_CHAIN: char = '\u{0}';

#[derive(Clone, Copy)]
struct SurfaceAtom {
    position: Point3<f32>,
    radius: f32,
}

pub(crate) struct SurfaceSceneGeometry {
    chains: Vec<SurfaceChainGeometry>,
}

struct SurfaceChainGeometry {
    chain_id: char,
    vertices: Vec<SurfaceMeshVertex>,
    triangles: Vec<SurfaceTriangleGeometry>,
}

#[derive(Clone, Copy)]
struct SurfaceMeshVertex {
    position: Point3<f32>,
    normal: Vector3<f32>,
}

#[derive(Clone, Copy)]
struct SurfaceTriangleGeometry {
    indices: [u32; 3],
}

pub(crate) fn build_surface_world_mesh(
    structure: &Structure,
    key: &SurfaceCacheKey,
    visual_state: &ViewportVisualState,
    cache: &mut SurfaceCache,
) -> Vec<MeshVertex> {
    if key.surface_chains.is_empty() && key.surface_atoms.is_empty() {
        return Vec::new();
    }
    let geometry = cached_surface_geometry(cache, structure, key);
    let alpha = (1.0 - visual_state.surface.transparency).clamp(0.0, 1.0);
    let mut mesh = Vec::new();
    for surface in &geometry.chains {
        let base_color = visual_state
            .chain_colors
            .get(&surface.chain_id)
            .copied()
            .unwrap_or(Color32::from_rgb(120, 150, 210));
        let mut color = base_color.to_normalized_gamma_f32();
        color[3] = alpha;
        for triangle in &surface.triangles {
            for &index in &triangle.indices {
                let vertex = surface.vertices[index as usize];
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

fn cached_surface_geometry<'a>(
    cache: &'a mut SurfaceCache,
    structure: &Structure,
    key: &SurfaceCacheKey,
) -> &'a SurfaceSceneGeometry {
    if cache.key.as_ref() != Some(key) {
        cache.geometry = Some(SurfaceSceneGeometry {
            chains: surface_chain_geometries(
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

fn surface_chain_geometries<I>(
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
        let atoms = structure
            .atoms
            .iter()
            .enumerate()
            .filter_map(|(atom_index, atom)| {
                (atom_chain_id(structure, atom_index) == Some(chain_id)
                    && atom_is_standard_amino_acid(structure, atom_index))
                .then_some(surface_atom(atom))
            })
            .collect::<Vec<_>>();
        if let Some(geometry) = build_surface_chain_geometry(chain_id, &atoms, style) {
            chains.push(geometry);
        }
    }

    let atoms = surface_atoms
        .iter()
        .filter_map(|&index| structure.atoms.get(index).map(surface_atom))
        .collect::<Vec<_>>();
    if let Some(geometry) =
        build_surface_chain_geometry(REPRESENTATION_SURFACE_CHAIN, &atoms, style)
    {
        chains.push(geometry);
    }
    chains
}

fn surface_atom(atom: &crate::domain::Atom) -> SurfaceAtom {
    SurfaceAtom {
        position: atom.position,
        radius: crate::domain::chemistry::element_style(&atom.element).display_radius + 1.35,
    }
}

fn build_surface_chain_geometry(
    chain_id: char,
    atoms: &[SurfaceAtom],
    style: SurfaceStyle,
) -> Option<SurfaceChainGeometry> {
    if atoms.is_empty() {
        return None;
    }
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

#[cfg(test)]
mod tests;
