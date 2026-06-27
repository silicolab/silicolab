use std::collections::HashMap;

use nalgebra::{Point3, Vector3};

use super::super::normalize_vector3;
use super::{SurfaceAtom, SurfaceMeshVertex, SurfaceTriangleGeometry};

const SURFACE_VERTEX_QUANTIZATION: f32 = 1024.0;

#[derive(Clone, Copy)]
struct SurfaceGridPoint {
    position: Point3<f32>,
    value: f32,
    normal: Vector3<f32>,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct SurfaceVertexKey([i32; 3]);

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

pub(super) struct SurfaceMeshGeometry {
    pub(super) vertices: Vec<SurfaceMeshVertex>,
    pub(super) triangles: Vec<SurfaceTriangleGeometry>,
}

pub(super) fn build_union_surface_mesh(
    atoms: &[SurfaceAtom],
    spacing: f32,
) -> Option<SurfaceMeshGeometry> {
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
