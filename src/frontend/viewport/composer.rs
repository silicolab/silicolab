use crate::{domain::Structure, frontend::AtomSelection};

use super::{
    SurfaceCache, SurfaceCacheKey, ViewportVisualState,
    camera::Projector,
    render::{
        PickTarget, RenderScene, ScreenDepthBuffer, ViewportGeometry, any_atoms_drawn_as_cartoon,
        any_atoms_drawn_as_surface, build_ball_and_stick_scene,
        build_biopolymer_cartoon_depth_buffer, build_biopolymer_cartoon_scene,
        build_cached_surface_scene, build_cell_scene, build_surface_scene,
    },
};

pub(super) struct ViewportSceneBuildResult {
    pub(super) scene: RenderScene,
    pub(super) pick_targets: Vec<PickTarget>,
}

pub(super) struct RepresentationComposer<'a> {
    structure: &'a Structure,
    geometry: &'a ViewportGeometry,
    viewport: &'a Projector,
    selection: &'a AtomSelection,
    visual_state: &'a ViewportVisualState,
    surface_cache: SurfaceCacheMode<'a>,
}

enum SurfaceCacheMode<'a> {
    Cached(SurfaceCacheContext<'a>),
    Uncached,
}

pub(super) struct SurfaceCacheContext<'a> {
    cache: &'a mut SurfaceCache,
    structure_id: u64,
    structure_revision: u64,
}

impl<'a> SurfaceCacheContext<'a> {
    pub(super) fn new(
        cache: &'a mut SurfaceCache,
        structure_id: u64,
        structure_revision: u64,
    ) -> Self {
        Self {
            cache,
            structure_id,
            structure_revision,
        }
    }
}

impl<'a> RepresentationComposer<'a> {
    pub(super) fn for_viewport(
        structure: &'a Structure,
        geometry: &'a ViewportGeometry,
        viewport: &'a Projector,
        selection: &'a AtomSelection,
        visual_state: &'a ViewportVisualState,
        cache_context: SurfaceCacheContext<'a>,
    ) -> Self {
        Self {
            structure,
            geometry,
            viewport,
            selection,
            visual_state,
            surface_cache: SurfaceCacheMode::Cached(cache_context),
        }
    }

    pub(super) fn for_export(
        structure: &'a Structure,
        geometry: &'a ViewportGeometry,
        viewport: &'a Projector,
        selection: &'a AtomSelection,
        visual_state: &'a ViewportVisualState,
    ) -> Self {
        Self {
            structure,
            geometry,
            viewport,
            selection,
            visual_state,
            surface_cache: SurfaceCacheMode::Uncached,
        }
    }

    pub(super) fn build(self) -> ViewportSceneBuildResult {
        let Self {
            structure,
            geometry,
            viewport,
            selection,
            visual_state,
            mut surface_cache,
        } = self;
        let mut scene = RenderScene::default();

        if visual_state.show_cell
            && let Some(cell) = &structure.cell
        {
            scene.append(build_cell_scene(viewport, cell));
        }

        // The surface (per-chain biopolymer surface and/or the representation
        // overlay) is drawn whenever either is active; the cartoon ribbon only
        // when cartoon atoms are present. Both can be on at once.
        let draw_cartoon = any_atoms_drawn_as_cartoon(structure, visual_state);
        let draw_surface = draw_cartoon || any_atoms_drawn_as_surface(structure, visual_state);
        if draw_surface {
            match visual_state.surface.style {
                super::SurfaceStyle::Fill => {
                    append_surface_scene(
                        &mut scene,
                        structure,
                        viewport,
                        visual_state,
                        None,
                        &mut surface_cache,
                    );
                    if draw_cartoon {
                        scene.append(build_biopolymer_cartoon_scene(
                            structure,
                            viewport,
                            visual_state,
                        ));
                    }
                }
                super::SurfaceStyle::Mesh => {
                    let cartoon_depth = if draw_cartoon {
                        build_biopolymer_cartoon_depth_buffer(structure, viewport, visual_state)
                    } else {
                        None
                    };
                    if draw_cartoon {
                        scene.append(build_biopolymer_cartoon_scene(
                            structure,
                            viewport,
                            visual_state,
                        ));
                    }
                    append_surface_scene(
                        &mut scene,
                        structure,
                        viewport,
                        visual_state,
                        cartoon_depth.as_ref(),
                        &mut surface_cache,
                    );
                }
            }
        }

        scene.append(build_ball_and_stick_scene(
            structure,
            geometry,
            viewport,
            selection,
            visual_state,
        ));

        ViewportSceneBuildResult {
            scene,
            pick_targets: geometry.atoms.clone(),
        }
    }
}

fn append_surface_scene(
    scene: &mut RenderScene,
    structure: &Structure,
    viewport: &Projector,
    visual_state: &ViewportVisualState,
    cartoon_depth: Option<&ScreenDepthBuffer>,
    surface_cache: &mut SurfaceCacheMode<'_>,
) {
    match surface_cache {
        SurfaceCacheMode::Cached(context) => {
            let surface_cache_key = SurfaceCacheKey::new(
                context.structure_id,
                context.structure_revision,
                structure,
                visual_state,
            );
            scene.append(build_cached_surface_scene(
                structure,
                &surface_cache_key,
                viewport,
                visual_state,
                context.cache,
                cartoon_depth,
            ));
        }
        SurfaceCacheMode::Uncached => {
            scene.append(build_surface_scene(
                structure,
                viewport,
                visual_state,
                cartoon_depth,
            ));
        }
    }
}
