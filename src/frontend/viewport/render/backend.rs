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

/// One mesh triangle tagged with whether it is translucent. Used to composite
/// the opaque (ball-and-stick, cartoon) and translucent (filled surface)
/// geometry of *every* representation into a single depth-sorted draw order.
///
/// The egui painter has no depth buffer — it composites in submission order — so
/// correct occlusion *between* representations depends entirely on drawing
/// back-to-front (painter's algorithm). A single global sort across all
/// representations is what makes nearer geometry occlude farther geometry
/// regardless of which representation emitted it; sorting each representation's
/// pass in isolation (as the per-pass order does) only gets occlusion right
/// *within* a representation, never across.
pub(crate) struct CompositedTriangle<'a> {
    pub(crate) triangle: &'a PrimitiveTriangle,
    pub(crate) translucent: bool,
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

    /// Every mesh triangle across all passes, tagged opaque/translucent and
    /// sorted back-to-front (farthest first, i.e. ascending view-space depth)
    /// for painter's-algorithm compositing. `depth` is larger for nearer
    /// geometry, so ascending order draws far before near.
    fn composited_meshes(&self) -> Vec<CompositedTriangle<'_>> {
        let mut meshes = Vec::with_capacity(self.triangle_count());
        for pass in &self.passes {
            match pass {
                RenderPass::OpaqueMeshes(triangles) => {
                    meshes.extend(triangles.iter().map(|triangle| CompositedTriangle {
                        triangle,
                        translucent: false,
                    }))
                }
                RenderPass::TransparentMeshes(triangles) => {
                    meshes.extend(triangles.iter().map(|triangle| CompositedTriangle {
                        triangle,
                        translucent: true,
                    }))
                }
                RenderPass::Lines(_) => {}
            }
        }
        meshes.sort_by(|a, b| a.triangle.depth.total_cmp(&b.triangle.depth));
        meshes
    }

    /// Opaque mesh triangles across all passes. Seeds the screen-space depth
    /// buffer the wireframe surface is clipped against, so every opaque
    /// representation (cartoon *and* ball-and-stick) occludes the surface.
    pub(crate) fn opaque_triangles(&self) -> impl Iterator<Item = &PrimitiveTriangle> {
        self.passes
            .iter()
            .filter_map(|pass| match pass {
                RenderPass::OpaqueMeshes(triangles) => Some(triangles.as_slice()),
                _ => None,
            })
            .flatten()
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
    /// Draw the frame's mesh triangles. The slice is pre-sorted back-to-front
    /// and spans every representation, so a backend that lacks a depth buffer
    /// (the egui painter) gets correct cross-representation occlusion just by
    /// drawing in order; a backend that has one (the headless canvas) uses the
    /// `translucent` tag to skip depth writes for blended geometry.
    fn draw_meshes(&mut self, meshes: &[CompositedTriangle<'_>]);
    fn draw_line_segments(&mut self, lines: &[LineSegmentPrimitive]);

    fn submit_scene(&mut self, scene: &RenderScene) {
        // Lines emitted *before* any mesh (the unit-cell wireframe) draw behind
        // the molecule; lines emitted after it (surface wireframe, cartoon
        // silhouettes) draw on top. Meshes themselves are composited globally,
        // so their original pass order no longer decides occlusion.
        let first_mesh = scene.passes.iter().position(|pass| {
            matches!(
                pass,
                RenderPass::OpaqueMeshes(_) | RenderPass::TransparentMeshes(_)
            )
        });
        let split = first_mesh.unwrap_or(scene.passes.len());

        for pass in &scene.passes[..split] {
            if let RenderPass::Lines(lines) = pass {
                self.draw_line_segments(lines);
            }
        }

        self.draw_meshes(&scene.composited_meshes());

        for pass in &scene.passes[split..] {
            if let RenderPass::Lines(lines) = pass {
                self.draw_line_segments(lines);
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
    fn draw_meshes(&mut self, meshes: &[CompositedTriangle<'_>]) {
        // One mesh, drawn in the given back-to-front order. egui has no depth
        // buffer and rasterizes in index order with alpha blending, so opaque
        // triangles (alpha 255) overwrite and translucent ones blend over
        // whatever is already there — exactly painter's algorithm.
        add_triangle_mesh(self.painter, meshes);
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
    fn draw_meshes(&mut self, meshes: &[CompositedTriangle<'_>]) {
        // The canvas has a real depth buffer. Drawing the merged stream
        // back-to-front, opaque triangles depth-test *and* write while
        // translucent ones depth-test without writing, so a near opaque atom
        // occludes a far surface wall and a near surface wall blends over the
        // cartoon behind it — both directions correct.
        for mesh in meshes {
            if mesh.translucent {
                self.canvas
                    .draw_transparent_primitive_triangle(mesh.triangle);
            } else {
                self.canvas.draw_opaque_primitive_triangle(mesh.triangle);
            }
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

fn add_triangle_mesh(painter: &Painter, meshes: &[CompositedTriangle<'_>]) {
    if meshes.is_empty() {
        return;
    }

    let mut mesh = Mesh::default();
    mesh.reserve_triangles(meshes.len());
    for composited in meshes {
        let base = mesh.vertices.len() as u32;
        for vertex in composited.triangle.vertices {
            mesh.colored_vertex(vertex.pos, vertex.color);
        }
        mesh.add_triangle(base, base + 1, base + 2);
    }
    painter.add(Shape::mesh(mesh));
}
