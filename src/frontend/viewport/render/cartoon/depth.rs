use eframe::egui::{Pos2, Rect};

use super::super::{PrimitiveTriangle, edge_function};

const CARTOON_DEPTH_BUFFER_RESOLUTION: usize = 384;

pub(crate) struct ScreenDepthBuffer {
    pub(super) bounds: Rect,
    pub(super) scale: f32,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) depths: Vec<f32>,
}

pub(super) fn sample_depth_buffer(depth_buffer: &ScreenDepthBuffer, pos: Pos2) -> Option<f32> {
    if !depth_buffer.bounds.contains(pos) {
        return None;
    }

    let sample = depth_buffer_pos(pos, depth_buffer.bounds, depth_buffer.scale);
    let x = sample.x.floor() as isize;
    let y = sample.y.floor() as isize;
    if x < 0 || y < 0 || x >= depth_buffer.width as isize || y >= depth_buffer.height as isize {
        return None;
    }

    let depth = depth_buffer.depths[y as usize * depth_buffer.width + x as usize];
    (depth > f32::NEG_INFINITY).then_some(depth)
}

/// Whether a surface-wireframe sample at `pos`/`depth` is in front of (or within
/// an epsilon of) the opaque geometry recorded in `depth_buffer` — i.e. visible
/// rather than occluded. `depth` is larger for nearer geometry, so the sample
/// shows when it is at least as near as the stored opaque depth.
pub(crate) fn mesh_sample_visible(depth_buffer: &ScreenDepthBuffer, pos: Pos2, depth: f32) -> bool {
    match sample_depth_buffer(depth_buffer, pos) {
        Some(occluder_depth) => {
            depth >= occluder_depth - super::super::MESH_OCCLUSION_DEPTH_EPSILON
        }
        None => true,
    }
}

/// Rasterize a low-resolution screen-space depth buffer from opaque mesh
/// triangles (cartoon ribbons and/or the ball-and-stick base).
///
/// The wireframe ("mesh") surface can't join the triangle depth sort — it is
/// drawn as screen-space line runs, not triangles — so it is clipped against
/// this buffer instead. Seeding it with *all* opaque geometry (not just the
/// cartoon) is what lets a ball-and-stick atom occlude the surface wireframe in
/// front of it, the same way the cartoon already did.
pub(crate) fn build_opaque_depth_buffer<'a>(
    rect: Rect,
    triangles: impl IntoIterator<Item = &'a PrimitiveTriangle>,
) -> Option<ScreenDepthBuffer> {
    let triangles = triangles.into_iter().collect::<Vec<_>>();
    if triangles.is_empty() || rect.width() <= 1.0 || rect.height() <= 1.0 {
        return None;
    }

    let max_dimension = rect.width().max(rect.height()).max(1.0);
    let scale = (CARTOON_DEPTH_BUFFER_RESOLUTION as f32 / max_dimension).min(1.0);
    let width = (rect.width() * scale).ceil().max(2.0) as usize;
    let height = (rect.height() * scale).ceil().max(2.0) as usize;
    let mut depths = vec![f32::NEG_INFINITY; width * height];

    for triangle in triangles {
        rasterize_cartoon_triangle_depth(&mut depths, width, height, rect, scale, triangle);
    }

    Some(ScreenDepthBuffer {
        bounds: rect,
        scale,
        width,
        height,
        depths,
    })
}

fn rasterize_cartoon_triangle_depth(
    depth_buffer: &mut [f32],
    width: usize,
    height: usize,
    rect: Rect,
    scale: f32,
    triangle: &PrimitiveTriangle,
) {
    let a = depth_buffer_pos(triangle.vertices[0].pos, rect, scale);
    let b = depth_buffer_pos(triangle.vertices[1].pos, rect, scale);
    let c = depth_buffer_pos(triangle.vertices[2].pos, rect, scale);
    let area = edge_function(a, b, c);
    if area.abs() <= 0.0001 {
        return;
    }

    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as usize;
    let max_x = a.x.max(b.x).max(c.x).ceil().min((width - 1) as f32) as usize;
    let max_y = a.y.max(b.y).max(c.y).ceil().min((height - 1) as f32) as usize;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let sample = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge_function(b, c, sample) / area;
            let w1 = edge_function(c, a, sample) / area;
            let w2 = edge_function(a, b, sample) / area;
            if w0 < -0.0001 || w1 < -0.0001 || w2 < -0.0001 {
                continue;
            }

            let depth = triangle.vertices[0].depth * w0
                + triangle.vertices[1].depth * w1
                + triangle.vertices[2].depth * w2;
            let index = y * width + x;
            if depth > depth_buffer[index] {
                depth_buffer[index] = depth;
            }
        }
    }
}

fn depth_buffer_pos(pos: Pos2, rect: Rect, scale: f32) -> Pos2 {
    Pos2::new((pos.x - rect.min.x) * scale, (pos.y - rect.min.y) * scale)
}
