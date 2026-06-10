use eframe::egui::{Color32, Mesh, Painter, Pos2, Shape, Stroke};

use super::{HeadlessCanvas, PrimitiveTriangle};

/// Upper bound on the number of mesh triangles submitted to egui in a single
/// frame. egui-wgpu concatenates every mesh of the frame into one
/// `egui_vertex_buffer`; each triangle contributes three 20-byte vertices, so
/// the buffer must stay below wgpu's 256 MiB (`max_buffer_size`) limit or
/// `Device::create_buffer` panics. 4M triangles ≈ 12M vertices ≈ 240 MB, which
/// leaves headroom for egui's own UI/text vertices in the same buffer.
pub(crate) const MAX_RENDER_TRIANGLES: usize = 4_000_000;

#[derive(Clone, Copy)]
pub(crate) struct LineSegmentPrimitive {
    pub(crate) start: Pos2,
    pub(crate) end: Pos2,
    pub(crate) color: Color32,
    pub(crate) width: f32,
}

#[derive(Default)]
pub(crate) struct RenderScene {
    passes: Vec<RenderPass>,
}

pub(crate) enum RenderPass {
    OpaqueMeshes(Vec<PrimitiveTriangle>),
    TransparentMeshes(Vec<PrimitiveTriangle>),
    Lines(Vec<LineSegmentPrimitive>),
}

impl RenderScene {
    pub(crate) fn is_empty(&self) -> bool {
        self.passes.iter().all(RenderPass::is_empty)
    }

    pub(crate) fn sort_for_raster(&mut self) {
        for pass in &mut self.passes {
            pass.sort_for_raster();
        }
    }

    pub(crate) fn push_opaque_meshes(&mut self, triangles: Vec<PrimitiveTriangle>) {
        if triangles.is_empty() {
            return;
        }
        self.passes.push(RenderPass::OpaqueMeshes(triangles));
    }

    pub(crate) fn push_transparent_meshes(&mut self, triangles: Vec<PrimitiveTriangle>) {
        if triangles.is_empty() {
            return;
        }
        self.passes.push(RenderPass::TransparentMeshes(triangles));
    }

    pub(crate) fn push_lines(&mut self, lines: Vec<LineSegmentPrimitive>) {
        if lines.is_empty() {
            return;
        }
        self.passes.push(RenderPass::Lines(lines));
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.passes.append(&mut other.passes);
    }

    pub(crate) fn sorted(mut self) -> Self {
        self.sort_for_raster();
        self
    }

    /// Total mesh triangles across every opaque/transparent pass. Line passes
    /// are excluded — they are drawn as egui strokes, not into the shared mesh
    /// buffer, so they do not count against [`MAX_RENDER_TRIANGLES`].
    pub(crate) fn triangle_count(&self) -> usize {
        self.passes
            .iter()
            .map(|pass| match pass {
                RenderPass::OpaqueMeshes(triangles) | RenderPass::TransparentMeshes(triangles) => {
                    triangles.len()
                }
                RenderPass::Lines(_) => 0,
            })
            .sum()
    }

    #[cfg(test)]
    pub(crate) fn line_segments(&self) -> Vec<LineSegmentPrimitive> {
        self.passes
            .iter()
            .flat_map(|pass| match pass {
                RenderPass::Lines(lines) => lines.clone(),
                _ => Vec::new(),
            })
            .collect()
    }
}

impl RenderPass {
    fn is_empty(&self) -> bool {
        match self {
            Self::OpaqueMeshes(triangles) | Self::TransparentMeshes(triangles) => {
                triangles.is_empty()
            }
            Self::Lines(lines) => lines.is_empty(),
        }
    }

    fn sort_for_raster(&mut self) {
        match self {
            Self::OpaqueMeshes(triangles) | Self::TransparentMeshes(triangles) => {
                triangles.sort_by(|a, b| a.depth.total_cmp(&b.depth));
            }
            Self::Lines(_) => {}
        }
    }
}

pub(crate) trait RenderBackend {
    fn draw_opaque_triangles(&mut self, triangles: &[PrimitiveTriangle]);
    fn draw_transparent_triangles(&mut self, triangles: &[PrimitiveTriangle]);
    fn draw_line_segments(&mut self, lines: &[LineSegmentPrimitive]);

    fn submit_scene(&mut self, scene: &RenderScene) {
        for pass in &scene.passes {
            match pass {
                RenderPass::OpaqueMeshes(triangles) => self.draw_opaque_triangles(triangles),
                RenderPass::TransparentMeshes(triangles) => {
                    self.draw_transparent_triangles(triangles)
                }
                RenderPass::Lines(lines) => self.draw_line_segments(lines),
            }
        }
    }
}

pub(crate) struct EguiRenderBackend<'a> {
    painter: &'a Painter,
}

impl<'a> EguiRenderBackend<'a> {
    pub(crate) fn new(painter: &'a Painter) -> Self {
        Self { painter }
    }
}

impl RenderBackend for EguiRenderBackend<'_> {
    fn draw_opaque_triangles(&mut self, triangles: &[PrimitiveTriangle]) {
        add_triangle_mesh(self.painter, triangles);
    }

    fn draw_transparent_triangles(&mut self, triangles: &[PrimitiveTriangle]) {
        add_triangle_mesh(self.painter, triangles);
    }

    fn draw_line_segments(&mut self, lines: &[LineSegmentPrimitive]) {
        for line in lines {
            self.painter
                .line_segment([line.start, line.end], Stroke::new(line.width, line.color));
        }
    }
}

pub(crate) struct CanvasRenderBackend<'a> {
    canvas: &'a mut HeadlessCanvas,
}

impl<'a> CanvasRenderBackend<'a> {
    pub(crate) fn new(canvas: &'a mut HeadlessCanvas) -> Self {
        Self { canvas }
    }
}

impl RenderBackend for CanvasRenderBackend<'_> {
    fn draw_opaque_triangles(&mut self, triangles: &[PrimitiveTriangle]) {
        for triangle in triangles {
            self.canvas.draw_opaque_primitive_triangle(triangle);
        }
    }

    fn draw_transparent_triangles(&mut self, triangles: &[PrimitiveTriangle]) {
        for triangle in triangles {
            self.canvas.draw_transparent_primitive_triangle(triangle);
        }
    }

    fn draw_line_segments(&mut self, lines: &[LineSegmentPrimitive]) {
        for line in lines {
            self.canvas
                .draw_line_segment(line.start, line.end, line.color, line.width);
        }
    }
}

pub(crate) fn submit_scene_to_painter(painter: &Painter, scene: &RenderScene) {
    if scene.is_empty() {
        return;
    }
    let mut backend = EguiRenderBackend::new(painter);
    backend.submit_scene(scene);
}

/// Submit a scene to the egui painter, but refuse to emit the mesh passes when
/// their combined triangle count would overflow the shared vertex buffer (see
/// [`MAX_RENDER_TRIANGLES`]). Returns `true` when the full scene was drawn and
/// `false` when the meshes were suppressed. On suppression the cheap line
/// passes (e.g. the unit-cell outline) are still drawn so the user keeps the
/// box for context, and the caller is expected to surface a warning.
///
/// This is the last-resort guard against the wgpu validation panic; callers
/// should also keep per-frame geometry within budget via level-of-detail so
/// this path is rarely taken.
pub(crate) fn submit_scene_to_painter_within_budget(
    painter: &Painter,
    scene: &RenderScene,
    max_triangles: usize,
) -> bool {
    if scene.is_empty() {
        return true;
    }
    if scene.triangle_count() <= max_triangles {
        submit_scene_to_painter(painter, scene);
        return true;
    }
    let mut backend = EguiRenderBackend::new(painter);
    for pass in &scene.passes {
        if let RenderPass::Lines(lines) = pass {
            backend.draw_line_segments(lines);
        }
    }
    false
}

pub(crate) fn submit_scene_to_canvas(canvas: &mut HeadlessCanvas, scene: &RenderScene) {
    if scene.is_empty() {
        return;
    }
    let mut backend = CanvasRenderBackend::new(canvas);
    backend.submit_scene(scene);
}

fn add_triangle_mesh(painter: &Painter, triangles: &[PrimitiveTriangle]) {
    if triangles.is_empty() {
        return;
    }

    let mut mesh = Mesh::default();
    mesh.reserve_triangles(triangles.len());
    for triangle in triangles {
        let base = mesh.vertices.len() as u32;
        for vertex in triangle.vertices {
            mesh.colored_vertex(vertex.pos, vertex.color);
        }
        mesh.add_triangle(base, base + 1, base + 2);
    }
    painter.add(Shape::mesh(mesh));
}
