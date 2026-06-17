use super::*;

use anyhow::Result;
use rusqlite::Connection;

use crate::{
    domain::AtomCategory,
    frontend::{AtomStyle, ViewportVisualState},
};

pub(crate) fn load_project_view_settings(db: &Connection) -> Result<ProjectViewSettings> {
    let mut view = ProjectViewSettings::default();
    let mut statement = db.prepare(
        "select scope_type, scope_id, target_type, target_id, property, value_type, value_text, value_real, value_integer, value_json from render_overrides order by priority, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(RenderOverrideRow {
            scope_type: row.get(0)?,
            scope_id: row.get(1)?,
            target_type: row.get(2)?,
            target_id: row.get(3)?,
            property: row.get(4)?,
            value_type: row.get(5)?,
            value_text: row.get(6)?,
            value_real: row.get(7)?,
            value_integer: row.get(8)?,
            value_json: row.get(9)?,
        })
    })?;
    for row in rows {
        let row = row?;
        apply_render_override_row(&mut view, row)?;
    }
    Ok(view)
}

fn apply_render_override_row(view: &mut ProjectViewSettings, row: RenderOverrideRow) -> Result<()> {
    let viewport = match row.scope_type.as_str() {
        "project" if row.scope_id == "project" => &mut view.viewport,
        "entry" => {
            let Ok(entry_id) = row.scope_id.parse::<u64>() else {
                return Ok(());
            };
            view.entry_viewports.entry(entry_id).or_default()
        }
        _ => return Ok(()),
    };
    match row.target_type.as_str() {
        "view" => apply_view_override(viewport, &row)?,
        "chain" => apply_chain_override(viewport, &row)?,
        "atom_category" => apply_atom_category_override(viewport, &row)?,
        "atom" => apply_atom_override(viewport, &row),
        _ => {}
    }
    Ok(())
}

fn apply_atom_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) {
    let Ok(index) = row.target_id.parse::<usize>() else {
        return;
    };
    match row.property.as_str() {
        "style" => {
            if let Some(style) = row.value_text.as_deref().and_then(AtomStyle::from_token) {
                viewport.atom_styles.insert(index, style);
            }
        }
        "hidden" if row.value_integer.unwrap_or_default() != 0 => {
            viewport.atom_hidden.insert(index);
        }
        _ => {}
    }
}

fn apply_view_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) -> Result<()> {
    match row.property.as_str() {
        "background_color" => {
            if let Some(color) = row.json_value()?.as_ref().and_then(parse_color_json) {
                viewport.background_color = color;
            }
        }
        "show_cell" => set_bool_from_integer(row.value_integer, &mut viewport.show_cell),
        "light_preset" => {
            if let Some(token) = row.value_text.as_deref() {
                viewport.lighting.preset = parse_light_preset(token);
            }
        }
        "silhouettes" => {
            set_bool_from_integer(row.value_integer, &mut viewport.lighting.silhouettes)
        }
        "silhouette_width" => {
            set_f32_from_real(row.value_real, &mut viewport.lighting.silhouette_width)
        }
        "cartoon_helix" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.helix);
            }
        }
        "cartoon_sheet" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.sheet);
            }
        }
        "cartoon_coil" => {
            if let Some(value) = row.json_value()? {
                apply_cartoon_section(&value, &mut viewport.cartoon.coil);
            }
        }
        "cartoon_smoothing" => {
            if let Some(value) = row.value_integer {
                viewport.cartoon.smoothing = value.max(1) as usize;
            }
        }
        "cartoon_profile_segments" => {
            if let Some(value) = row.value_integer {
                viewport.cartoon.profile_segments = value.max(1) as usize;
            }
        }
        "surface_style" => {
            if let Some(token) = row.value_text.as_deref() {
                viewport.surface.style = parse_surface_style(token);
            }
        }
        "surface_transparency" => {
            set_f32_from_real(row.value_real, &mut viewport.surface.transparency)
        }
        _ => {}
    }
    Ok(())
}

fn apply_chain_override(viewport: &mut ViewportVisualState, row: &RenderOverrideRow) -> Result<()> {
    let chain = string_to_char(&row.target_id);
    match row.property.as_str() {
        "color" => {
            if let Some(color) = row.json_value()?.as_ref().and_then(parse_color_json) {
                viewport.chain_colors.insert(chain, color);
            }
        }
        "surface_visible" => {
            if row.value_integer.unwrap_or_default() != 0 {
                viewport.surface.chains.insert(chain);
            } else {
                viewport.surface.chains.remove(&chain);
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_atom_category_override(
    viewport: &mut ViewportVisualState,
    row: &RenderOverrideRow,
) -> Result<()> {
    // Project-level category style override (e.g. solvent → wireframe).
    if row.property == "style"
        && let (Some(category), Some(style)) = (
            AtomCategory::from_token(&row.target_id),
            row.value_text.as_deref().and_then(AtomStyle::from_token),
        )
    {
        viewport.category_styles.insert(category, style);
        return Ok(());
    }
    match (row.target_id.as_str(), row.property.as_str()) {
        ("ion", "show_within") => {
            viewport.ions.show_within = match row.value_type.as_str() {
                "real" => row.value_real.map(|value| value as f32),
                "json" => None,
                _ => viewport.ions.show_within,
            };
        }
        ("ion", "color") => {
            viewport.ions.color = row.json_value()?.as_ref().and_then(parse_color_json);
        }
        ("hetero", "auto_color") => {
            set_bool_from_integer(row.value_integer, &mut viewport.hetero_atom_colors);
        }
        _ => {}
    }
    Ok(())
}

fn apply_cartoon_section(
    value: &serde_json::Value,
    section: &mut crate::frontend::CartoonSectionStyle,
) {
    if let Some(width) = value.get("width").and_then(serde_json::Value::as_f64) {
        section.width = width as f32;
    }
    if let Some(thickness) = value.get("thickness").and_then(serde_json::Value::as_f64) {
        section.thickness = thickness as f32;
    }
}
