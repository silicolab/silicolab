use std::path::Path;

use anyhow::{Result, anyhow};
use eframe::egui::{Color32, Pos2, Rgba as EguiRgba};
use image::{ImageBuffer, Rgba};

use super::{PrimitiveTriangle, edge_function, lerp_pos2};

#[derive(Clone, Copy)]
struct RasterVertex {
    pos: Pos2,
    depth: f32,
    color: Color32,
}

pub(crate) struct HeadlessCanvas {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
    depth: Vec<f32>,
}

impl HeadlessCanvas {
    pub(crate) fn new(width: u32, height: u32, background: Color32) -> Self {
        let pixel = [background.r(), background.g(), background.b(), 255];
        let len = width as usize * height as usize;
        Self {
            width,
            height,
            pixels: vec![pixel; len],
            depth: vec![f32::NEG_INFINITY; len],
        }
    }

    pub(crate) fn save(&self, output_path: &Path) -> Result<()> {
        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let raw = self
            .pixels
            .iter()
            .flat_map(|pixel| pixel.iter().copied())
            .collect::<Vec<_>>();
        let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(self.width, self.height, raw)
            .ok_or_else(|| anyhow!("failed to create PNG buffer"))?;
        image
            .save(output_path)
            .map_err(|error| anyhow!(error.to_string()))
    }

    pub(super) fn draw_opaque_primitive_triangle(&mut self, triangle: &PrimitiveTriangle) {
        self.rasterize_triangle(
            triangle.vertices.map(|vertex| RasterVertex {
                pos: vertex.pos,
                depth: vertex.depth,
                color: vertex.color,
            }),
            true,
        );
    }

    pub(super) fn draw_transparent_primitive_triangle(&mut self, triangle: &PrimitiveTriangle) {
        self.rasterize_triangle(
            triangle.vertices.map(|vertex| RasterVertex {
                pos: vertex.pos,
                depth: vertex.depth,
                color: vertex.color,
            }),
            false,
        );
    }

    pub(super) fn draw_line_segment(&mut self, start: Pos2, end: Pos2, color: Color32, width: f32) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let length = start.distance(end);
        let steps = ((length * 1.5).ceil() as usize).max(1);
        let radius = (width * 0.5).max(0.6);
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            self.stamp_disc(lerp_pos2(start, end, t), radius, color);
        }
    }

    fn rasterize_triangle(&mut self, vertices: [RasterVertex; 3], write_depth: bool) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let area = edge_function(vertices[0].pos, vertices[1].pos, vertices[2].pos);
        if area.abs() <= 0.0001 {
            return;
        }

        let min_x = vertices
            .iter()
            .map(|vertex| vertex.pos.x)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .clamp(0.0, self.width.saturating_sub(1) as f32) as usize;
        let min_y = vertices
            .iter()
            .map(|vertex| vertex.pos.y)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .clamp(0.0, self.height.saturating_sub(1) as f32) as usize;
        let max_x = vertices
            .iter()
            .map(|vertex| vertex.pos.x)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0.0, self.width.saturating_sub(1) as f32) as usize;
        let max_y = vertices
            .iter()
            .map(|vertex| vertex.pos.y)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .clamp(0.0, self.height.saturating_sub(1) as f32) as usize;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sample = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge_function(vertices[1].pos, vertices[2].pos, sample) / area;
                let w1 = edge_function(vertices[2].pos, vertices[0].pos, sample) / area;
                let w2 = edge_function(vertices[0].pos, vertices[1].pos, sample) / area;
                if w0 < -0.0001 || w1 < -0.0001 || w2 < -0.0001 {
                    continue;
                }

                let depth =
                    vertices[0].depth * w0 + vertices[1].depth * w1 + vertices[2].depth * w2;
                let index = y * self.width as usize + x;
                if write_depth && depth <= self.depth[index] {
                    continue;
                }

                let color = Color32::from_rgba_premultiplied(
                    interpolate_channel(vertices, w0, w1, w2, |vertex| vertex.color.r()),
                    interpolate_channel(vertices, w0, w1, w2, |vertex| vertex.color.g()),
                    interpolate_channel(vertices, w0, w1, w2, |vertex| vertex.color.b()),
                    interpolate_channel(vertices, w0, w1, w2, |vertex| vertex.color.a()),
                );
                self.blend_pixel(index, color);
                if write_depth && color.a() > 0 {
                    self.depth[index] = depth;
                }
            }
        }
    }

    fn stamp_disc(&mut self, center: Pos2, radius: f32, color: Color32) {
        let min_x = (center.x - radius)
            .floor()
            .clamp(0.0, self.width.saturating_sub(1) as f32) as usize;
        let min_y = (center.y - radius)
            .floor()
            .clamp(0.0, self.height.saturating_sub(1) as f32) as usize;
        let max_x = (center.x + radius)
            .ceil()
            .clamp(0.0, self.width.saturating_sub(1) as f32) as usize;
        let max_y = (center.y + radius)
            .ceil()
            .clamp(0.0, self.height.saturating_sub(1) as f32) as usize;
        let radius_sq = radius * radius;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - center.x;
                let dy = y as f32 + 0.5 - center.y;
                if dx * dx + dy * dy > radius_sq {
                    continue;
                }
                let index = y * self.width as usize + x;
                self.blend_pixel(index, color);
            }
        }
    }

    fn blend_pixel(&mut self, index: usize, color: Color32) {
        let [src_r, src_g, src_b, src_a] = EguiRgba::from(color).to_srgba_unmultiplied();
        let alpha = src_a as f32 / 255.0;
        if alpha <= 0.0 {
            return;
        }

        let dst = self.pixels[index];
        let blend = |src: u8, dst: u8| -> u8 {
            (src as f32 * alpha + dst as f32 * (1.0 - alpha))
                .round()
                .clamp(0.0, 255.0) as u8
        };
        self.pixels[index] = [
            blend(src_r, dst[0]),
            blend(src_g, dst[1]),
            blend(src_b, dst[2]),
            255,
        ];
    }
}

fn interpolate_channel(
    vertices: [RasterVertex; 3],
    w0: f32,
    w1: f32,
    w2: f32,
    sample: impl Fn(&RasterVertex) -> u8,
) -> u8 {
    (sample(&vertices[0]) as f32 * w0
        + sample(&vertices[1]) as f32 * w1
        + sample(&vertices[2]) as f32 * w2)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blends_unmultiplied_blue_lines_without_turning_them_gray() {
        let mut canvas = HeadlessCanvas::new(1, 1, Color32::WHITE);

        canvas.blend_pixel(0, Color32::from_rgba_unmultiplied(100, 149, 237, 40));

        let pixel = canvas.pixels[0];
        assert!(pixel[2] > pixel[1]);
        assert!(pixel[1] > pixel[0]);
        assert!(pixel[2] - pixel[0] >= 15);
    }

    /// Occlusion must follow depth, not pass-append order. A translucent surface
    /// *in front of* an opaque ribbon has to blend over it even when the surface
    /// pass is appended first (the order the old composer used for Fill+cartoon,
    /// which drew the cartoon flat on top and hid the surface entirely).
    #[test]
    fn nearer_translucent_surface_blends_over_farther_opaque_mesh() {
        use super::super::PrimitiveMeshVertex;
        use super::super::backend::{RenderScene, submit_scene_to_canvas};

        // Larger depth == nearer the camera (see `Projector::project`). A flat
        // triangle covering pixel (0, 0).
        let triangle = |depth: f32, color: Color32| {
            let vertex = |x: f32, y: f32| PrimitiveMeshVertex {
                pos: Pos2::new(x, y),
                depth,
                color,
            };
            super::super::primitive_triangle(
                vertex(-2.0, -2.0),
                vertex(8.0, -2.0),
                vertex(-2.0, 8.0),
            )
        };

        let cartoon = triangle(0.0, Color32::from_rgb(220, 40, 40)); // opaque, far
        let surface = triangle(1.0, Color32::from_rgba_unmultiplied(40, 80, 220, 128)); // near

        let mut scene = RenderScene::default();
        scene.push_transparent_meshes(vec![surface]);
        scene.push_opaque_meshes(vec![cartoon]);

        let mut canvas = HeadlessCanvas::new(2, 2, Color32::WHITE);
        submit_scene_to_canvas(&mut canvas, &scene);

        let pixel = canvas.pixels[0];
        assert!(
            pixel[2] > 90,
            "near translucent surface should show its blue in front, got {pixel:?}"
        );
        assert!(
            pixel[0] < 200,
            "far opaque cartoon should be dimmed by the surface, got {pixel:?}"
        );
    }
}
