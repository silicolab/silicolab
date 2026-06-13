use std::time::Duration;

use eframe::egui::{self, Align2, FontId, Pos2, Sense, Vec2};

use crate::{domain::Structure, frontend::AtomSelection};

mod camera;
mod composer;
mod export;
mod gpu;
mod interaction;
mod render;
mod visual_state;

pub use camera::ViewCamera;
pub(crate) use camera::view_center_and_radius;
pub(crate) use export::{ViewportPngExport, export_viewport_png};
pub(crate) use gpu::init as init_gpu_renderer;
pub use visual_state::{
    CartoonSectionStyle, LightPreset, SurfaceStyle, ViewportCartoonState, ViewportIonState,
    ViewportLightingState, ViewportSurfaceState, ViewportVisualState, software_default_style,
};

use camera::Projector;
use composer::{RepresentationComposer, SurfaceCacheContext};
use interaction::{InteractionSystem, ViewportInteraction};
use render::*;

pub const STRUCTURE_INTERACTION_FRAME: Duration = Duration::from_millis(16);
pub const HOVER_FRAME: Duration = Duration::from_millis(100);

/// Per-viewport render caches. The projected ball-and-stick geometry and the
/// (much more expensive) molecular surface are cached independently so a frame
/// can borrow the geometry immutably while still updating the surface cache —
/// avoiding a full clone of the geometry every frame.
#[derive(Default)]
pub struct ViewportCache {
    geometry: GeometryCache,
    surface: SurfaceCache,
    gpu: GpuViewCache,
}

#[derive(Default)]
pub(super) struct GeometryCache {
    key: Option<ViewportCacheKey>,
    geometry: Option<ViewportGeometry>,
}

#[derive(Default)]
pub(super) struct SurfaceCache {
    key: Option<SurfaceCacheKey>,
    geometry: Option<SurfaceSceneGeometry>,
}

/// State for the GPU ball-and-stick path. The instance set is camera-independent
/// (rebuilt only when `instance_key` changes), while pick targets are projected
/// atom centers cached per camera so hover/click picking stays on the CPU.
#[derive(Default)]
pub(super) struct GpuViewCache {
    instance_key: Option<GpuInstanceKey>,
    pick_key: Option<ViewportCacheKey>,
    pick_targets: Vec<PickTarget>,
}

impl ViewportCache {
    pub fn clear(&mut self) {
        self.geometry = GeometryCache::default();
        self.surface = SurfaceCache::default();
        self.gpu = GpuViewCache::default();
    }
}

/// Identifies the camera-independent inputs to the GPU instance set: atom
/// positions (via the structure revision) plus everything that affects an atom's
/// sphere radius, color, or visibility (styling and selection). Camera changes
/// are deliberately excluded so rotation never rebuilds instances.
#[derive(Clone, Copy, PartialEq)]
struct GpuInstanceKey {
    structure_id: u64,
    structure_revision: u64,
    visual_hash: u64,
    selection_hash: u64,
}

impl GpuInstanceKey {
    fn new(
        structure_id: u64,
        structure_revision: u64,
        visual_state: &ViewportVisualState,
        selection: &AtomSelection,
    ) -> Self {
        Self {
            structure_id,
            structure_revision,
            visual_hash: hash_visual_state(visual_state),
            selection_hash: hash_selection(selection),
        }
    }
}

/// Hash the styling that changes the GPU scene: per-category and per-atom style
/// overrides, ion visibility/color, cartoon ribbon geometry, surface settings,
/// and chain colors. Only the lighting preset and background are excluded (they
/// do not change geometry). Camera state is excluded by construction.
fn hash_visual_state(visual_state: &ViewportVisualState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (category, style) in &visual_state.category_styles {
        category.hash(&mut hasher);
        style.hash(&mut hasher);
    }
    for (index, style) in &visual_state.atom_styles {
        index.hash(&mut hasher);
        style.hash(&mut hasher);
    }
    visual_state
        .ions
        .show_within
        .map(f32::to_bits)
        .hash(&mut hasher);
    visual_state
        .ions
        .color
        .map(|color| color.to_array())
        .hash(&mut hasher);

    let cartoon = &visual_state.cartoon;
    for section in [cartoon.helix, cartoon.sheet, cartoon.coil] {
        section.width.to_bits().hash(&mut hasher);
        section.thickness.to_bits().hash(&mut hasher);
    }
    cartoon.smoothing.hash(&mut hasher);
    cartoon.profile_segments.hash(&mut hasher);

    visual_state.surface.style.hash(&mut hasher);
    visual_state
        .surface
        .transparency
        .to_bits()
        .hash(&mut hasher);
    for chain in &visual_state.surface.chains {
        chain.hash(&mut hasher);
    }
    for overlay in [&visual_state.cartoon_overlay, &visual_state.surface_overlay] {
        for (category, on) in &overlay.categories {
            category.hash(&mut hasher);
            on.hash(&mut hasher);
        }
        for (atom, on) in &overlay.atoms {
            atom.hash(&mut hasher);
            on.hash(&mut hasher);
        }
    }
    for (chain, color) in &visual_state.chain_colors {
        chain.hash(&mut hasher);
        color.to_array().hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_selection(selection: &AtomSelection) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    selection.primary().hash(&mut hasher);
    selection.ordered_indices().hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, PartialEq)]
struct ViewportCacheKey {
    structure_id: u64,
    structure_revision: u64,
    rect_min: Pos2,
    rect_max: Pos2,
    camera: ViewCamera,
    show_cell: bool,
}

#[derive(Clone, PartialEq)]
pub(super) struct SurfaceCacheKey {
    pub(super) structure_id: u64,
    pub(super) structure_revision: u64,
    pub(super) style: SurfaceStyle,
    pub(super) surface_chains: Vec<char>,
    /// Atoms with the Surface overlay enabled (the representation surface). Part
    /// of the key so the cached mesh rebuilds when the overlaid set changes.
    pub(super) surface_atoms: Vec<usize>,
}

impl SurfaceCacheKey {
    pub(super) fn new(
        structure_id: u64,
        structure_revision: u64,
        structure: &Structure,
        visual_state: &ViewportVisualState,
    ) -> Self {
        Self {
            structure_id,
            structure_revision,
            style: visual_state.surface.style,
            surface_chains: visual_state.surface.chains.iter().copied().collect(),
            surface_atoms: surface_atom_indices(structure, visual_state),
        }
    }
}

pub struct ViewportDrawArgs<'a> {
    pub structure: &'a Structure,
    pub structure_id: u64,
    pub structure_revision: u64,
    pub camera: &'a mut ViewCamera,
    pub selection: &'a AtomSelection,
    pub visual_state: &'a ViewportVisualState,
    pub previous_hovered_atom: Option<usize>,
    pub cache: &'a mut ViewportCache,
    pub empty_state_hint: Option<&'a str>,
    /// Whether the GPU molecule renderer initialized successfully at startup.
    /// When false (or for representations the GPU path doesn't cover), the CPU
    /// rasterizer is used instead.
    pub gpu_ready: bool,
    /// Fixed camera framing `(center, radius)` to use instead of deriving it
    /// from `structure`. Set during trajectory playback so the view does not
    /// drift/zoom as atoms move between frames; `None` recomputes per frame.
    pub view_override: Option<(nalgebra::Point3<f32>, f32)>,
    /// Corner radius for the viewport background fill. The workspace uses this
    /// to keep its inset surface rounded while the renderer still owns the
    /// scene background color.
    pub background_corner_radius: u8,
}

/// Whether the GPU path can render this scene. It covers spheres, stick bonds,
/// cartoon ribbons, and filled per-chain molecular surfaces. The wireframe
/// ("mesh") surface style still uses the CPU line rasterizer, and the
/// representation-surface overlay is only wired through the CPU path, so those
/// scenes fall back.
fn gpu_path_supported(visual_state: &ViewportVisualState) -> bool {
    let mesh_surface =
        !visual_state.surface.chains.is_empty() && visual_state.surface.style == SurfaceStyle::Mesh;
    let representation_surface = !visual_state.surface_overlay.is_empty();
    !mesh_surface && !representation_surface
}

pub fn draw_viewport(ui: &mut egui::Ui, args: ViewportDrawArgs<'_>) -> ViewportInteraction {
    let ViewportDrawArgs {
        structure,
        structure_id,
        structure_revision,
        camera,
        selection,
        visual_state,
        previous_hovered_atom,
        cache,
        empty_state_hint,
        gpu_ready,
        view_override,
        background_corner_radius,
    } = args;
    let available = ui.available_size();
    // The default background follows the app theme (dark in dark mode); an
    // explicit user-chosen color in settings is left untouched.
    let pal = crate::frontend::theme::palette(ui);
    let background = if visual_state.background_follows_theme() {
        pal.viewport_bg
    } else {
        visual_state.background_color
    };
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, f32::from(background_corner_radius), background);

    if structure.atoms.is_empty() {
        if let Some(hint) = empty_state_hint {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                hint,
                FontId::proportional(18.0),
                pal.text_muted,
            );
        }

        cache.clear();
        return ViewportInteraction::default();
    }

    let (center, radius) =
        view_override.unwrap_or_else(|| view_center_and_radius(structure, visual_state.show_cell));
    let viewport = Projector::new(
        rect,
        center,
        rect.width().min(rect.height()) * 0.35 * (1.0 + camera.zoom) / radius,
        radius * 3.2,
        camera.yaw,
        camera.pitch,
        camera.pan,
    );
    let cache_key = ViewportCacheKey {
        structure_id,
        structure_revision,
        rect_min: rect.min,
        rect_max: rect.max,
        camera: *camera,
        show_cell: visual_state.show_cell,
    };
    let pick_targets = if gpu_ready && gpu_path_supported(visual_state) {
        render_molecules_gpu(
            &painter,
            rect,
            &viewport,
            cache_key,
            structure,
            structure_id,
            structure_revision,
            selection,
            visual_state,
            cache,
        )
    } else {
        render_molecules_cpu(
            &painter,
            rect,
            &viewport,
            cache_key,
            structure,
            structure_id,
            structure_revision,
            selection,
            visual_state,
            cache,
            pal,
        )
    };

    if visual_state.show_cell
        && let Some(cell) = &structure.cell
    {
        draw_cell_labels(&painter, &viewport, cell);
    }

    let interaction = InteractionSystem::new(
        structure,
        &pick_targets,
        previous_hovered_atom,
        visual_state,
    )
    .run(ui, &response, camera);

    for atom_projection in &pick_targets {
        if !visual_state.show_atom_labels
            || !atom_visible(structure, visual_state, atom_projection.index)
        {
            continue;
        }
        let atom = &structure.atoms[atom_projection.index];
        // On-atom element label: follow the theme's text color, but draw a
        // thin contrasting halo (the window backing) underneath so it stays
        // legible over any atom-sphere color in either light or dark mode.
        let font = FontId::proportional(12.0);
        for offset in [
            Vec2::new(1.0, 0.0),
            Vec2::new(-1.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(0.0, -1.0),
        ] {
            painter.text(
                atom_projection.pos + offset,
                Align2::CENTER_CENTER,
                &atom.element,
                font.clone(),
                pal.window_backing,
            );
        }
        painter.text(
            atom_projection.pos,
            Align2::CENTER_CENTER,
            &atom.element,
            font,
            pal.text_strong,
        );
    }

    if let Some(index) = interaction.hovered_atom {
        draw_hovered_atom_label(&painter, rect, structure, index, pal);
    }

    interaction
}

/// CPU rasterizer path: build the full scene (ball-and-stick, cartoon, surface,
/// cell) and submit it to the egui painter. Returns the projected pick targets.
#[allow(clippy::too_many_arguments)]
fn render_molecules_cpu(
    painter: &egui::Painter,
    rect: egui::Rect,
    viewport: &Projector,
    cache_key: ViewportCacheKey,
    structure: &Structure,
    structure_id: u64,
    structure_revision: u64,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
    cache: &mut ViewportCache,
    pal: crate::frontend::theme::Palette,
) -> Vec<PickTarget> {
    let geometry = cached_geometry(&mut cache.geometry, cache_key, structure, viewport);
    let scene_result = RepresentationComposer::for_viewport(
        structure,
        geometry,
        viewport,
        selection,
        visual_state,
        SurfaceCacheContext::new(&mut cache.surface, structure_id, structure_revision),
    )
    .build();
    let rendered_in_full =
        submit_scene_to_painter_within_budget(painter, &scene_result.scene, MAX_RENDER_TRIANGLES);

    if !rendered_in_full {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            format!(
                "Structure too large to render ({} atoms).\nThe view is simplified; reduce the system or hide water to see more.",
                structure.atoms.len()
            ),
            FontId::proportional(16.0),
            pal.status_red,
        );
    }

    scene_result.pick_targets
}

/// GPU path: rebuild the (camera-independent) instance set only when styling or
/// selection changed, then queue a single paint callback. The unit-cell box is
/// still drawn through the painter. Returns projected atom centers for picking.
#[allow(clippy::too_many_arguments)]
fn render_molecules_gpu(
    painter: &egui::Painter,
    rect: egui::Rect,
    viewport: &Projector,
    cache_key: ViewportCacheKey,
    structure: &Structure,
    structure_id: u64,
    structure_revision: u64,
    selection: &AtomSelection,
    visual_state: &ViewportVisualState,
    cache: &mut ViewportCache,
) -> Vec<PickTarget> {
    if visual_state.show_cell
        && let Some(cell) = &structure.cell
    {
        submit_scene_to_painter_within_budget(
            painter,
            &build_cell_scene(viewport, cell),
            MAX_RENDER_TRIANGLES,
        );
    }

    let instance_key =
        GpuInstanceKey::new(structure_id, structure_revision, visual_state, selection);
    let upload = if cache.gpu.instance_key == Some(instance_key) {
        None
    } else {
        cache.gpu.instance_key = Some(instance_key);
        let mut scene = build_molecule_instances(structure, selection, visual_state);
        let surface_key =
            SurfaceCacheKey::new(structure_id, structure_revision, structure, visual_state);
        scene.surface =
            build_surface_world_mesh(structure, &surface_key, visual_state, &mut cache.surface);
        Some(scene)
    };
    gpu::emit(painter, rect, viewport, upload);

    if cache.gpu.pick_key.as_ref() != Some(&cache_key) {
        cache.gpu.pick_targets = project_pick_targets(structure, viewport);
        cache.gpu.pick_key = Some(cache_key);
    }
    cache.gpu.pick_targets.clone()
}
