use super::*;

use anyhow::Result;
use rusqlite::Connection;

use crate::frontend::{CartoonSectionStyle, ViewportVisualState};

pub(crate) fn save_project_view_settings(
    db: &Connection,
    view: &ProjectViewSettings,
) -> Result<()> {
    db.execute("delete from render_overrides", [])?;
    let default_viewport = ViewportVisualState::default();

    save_viewport_settings(
        db,
        RenderScope::project(),
        &view.viewport,
        &default_viewport,
    )?;
    for (entry_id, viewport) in &view.entry_viewports {
        save_viewport_settings(
            db,
            RenderScope::entry(*entry_id),
            viewport,
            &default_viewport,
        )?;
    }

    Ok(())
}

fn save_viewport_settings(
    db: &Connection,
    scope: RenderScope,
    viewport: &ViewportVisualState,
    default_viewport: &ViewportVisualState,
) -> Result<()> {
    // Project-level category styles live only at project scope; per-atom style
    // overrides live only at entry scope (atom indices belong to a compound).
    if matches!(scope.scope_id, ScopeId::Project) {
        for (category, style) in &viewport.category_styles {
            set_render_override_text(
                db,
                RenderTarget::atom_category(scope, category.token()),
                "style",
                style.token(),
            )?;
        }
    } else {
        for (atom_index, style) in &viewport.atom_styles {
            set_render_override_text(
                db,
                RenderTarget::atom(scope, *atom_index),
                "style",
                style.token(),
            )?;
        }
        // Per-atom visibility override (independent of style; see
        // [`ViewportVisualState::atom_hidden`]).
        for atom_index in &viewport.atom_hidden {
            set_render_override_bool(db, RenderTarget::atom(scope, *atom_index), "hidden", true)?;
        }
    }
    if viewport.background_color != default_viewport.background_color {
        set_render_override_json(
            db,
            RenderTarget::view(scope),
            "background_color",
            color_json(viewport.background_color),
        )?;
    }
    if viewport.show_cell != default_viewport.show_cell {
        set_render_override_bool(
            db,
            RenderTarget::view(scope),
            "show_cell",
            viewport.show_cell,
        )?;
    }
    if viewport.lighting.preset != default_viewport.lighting.preset {
        set_render_override_text(
            db,
            RenderTarget::view(scope),
            "light_preset",
            light_preset_token(viewport.lighting.preset),
        )?;
    }
    if viewport.lighting.silhouettes != default_viewport.lighting.silhouettes {
        set_render_override_bool(
            db,
            RenderTarget::view(scope),
            "silhouettes",
            viewport.lighting.silhouettes,
        )?;
    }
    if viewport.lighting.silhouette_width != default_viewport.lighting.silhouette_width {
        set_render_override_real(
            db,
            RenderTarget::view(scope),
            "silhouette_width",
            viewport.lighting.silhouette_width,
        )?;
    }
    save_cartoon_setting(
        db,
        scope,
        "cartoon_helix",
        viewport.cartoon.helix,
        default_viewport.cartoon.helix,
    )?;
    save_cartoon_setting(
        db,
        scope,
        "cartoon_sheet",
        viewport.cartoon.sheet,
        default_viewport.cartoon.sheet,
    )?;
    save_cartoon_setting(
        db,
        scope,
        "cartoon_coil",
        viewport.cartoon.coil,
        default_viewport.cartoon.coil,
    )?;
    if viewport.cartoon.smoothing != default_viewport.cartoon.smoothing {
        set_render_override_integer(
            db,
            RenderTarget::view(scope),
            "cartoon_smoothing",
            viewport.cartoon.smoothing as i64,
        )?;
    }
    if viewport.cartoon.profile_segments != default_viewport.cartoon.profile_segments {
        set_render_override_integer(
            db,
            RenderTarget::view(scope),
            "cartoon_profile_segments",
            viewport.cartoon.profile_segments as i64,
        )?;
    }
    for (chain, color) in &viewport.chain_colors {
        set_render_override_json(
            db,
            RenderTarget::chain(scope, *chain),
            "color",
            color_json(*color),
        )?;
    }
    for chain in &viewport.surface.chains {
        set_render_override_bool(
            db,
            RenderTarget::chain(scope, *chain),
            "surface_visible",
            true,
        )?;
    }
    if viewport.surface.style != default_viewport.surface.style {
        set_render_override_text(
            db,
            RenderTarget::view(scope),
            "surface_style",
            surface_style_token(viewport.surface.style),
        )?;
    }
    if viewport.surface.transparency != default_viewport.surface.transparency {
        set_render_override_real(
            db,
            RenderTarget::view(scope),
            "surface_transparency",
            viewport.surface.transparency,
        )?;
    }
    if viewport.ions.show_within != default_viewport.ions.show_within {
        match viewport.ions.show_within {
            Some(distance) => set_render_override_real(
                db,
                RenderTarget::atom_category(scope, "ion"),
                "show_within",
                distance,
            )?,
            None => set_render_override_json(
                db,
                RenderTarget::atom_category(scope, "ion"),
                "show_within",
                serde_json::Value::Null,
            )?,
        }
    }
    if viewport.ions.color != default_viewport.ions.color
        && let Some(color) = viewport.ions.color
    {
        set_render_override_json(
            db,
            RenderTarget::atom_category(scope, "ion"),
            "color",
            color_json(color),
        )?;
    }
    if viewport.hetero_atom_colors != default_viewport.hetero_atom_colors {
        set_render_override_bool(
            db,
            RenderTarget::atom_category(scope, "hetero"),
            "auto_color",
            viewport.hetero_atom_colors,
        )?;
    }

    Ok(())
}

fn save_cartoon_setting(
    db: &Connection,
    scope: RenderScope,
    key: &str,
    section: CartoonSectionStyle,
    default_section: CartoonSectionStyle,
) -> Result<()> {
    if section.width != default_section.width || section.thickness != default_section.thickness {
        set_render_override_json(
            db,
            RenderTarget::view(scope),
            key,
            serde_json::json!({
                "width": section.width,
                "thickness": section.thickness,
            }),
        )?;
    }
    Ok(())
}
