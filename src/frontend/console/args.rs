//! Value parsers and the shared viewport mutator for the console grammar.
//!
//! These functions are the bridge between clap and the domain types: each
//! `parse_*` is used as a clap `value_parser`, so the grammar in
//! [`super::grammar`] stays declarative and the accepted token sets (named
//! colors, `on`/`off`, atom-style aliases, …) live in one place. clap value
//! parsers must return `Result<T, String>` (or any `Display` error), so these
//! mirror the old hand-rolled helpers' accepted inputs while letting clap render
//! the failures.

use eframe::egui::Color32;

use crate::frontend::{
    LightPreset, SurfaceStyle, ViewportVisualState,
    state::{AppState, AtomStyle},
};

/// Parse a color token: a named color or `#rrggbb`. Used both as a clap
/// `value_parser` (via [`parse_color_value`]) and by the view-script exporter.
pub(crate) fn parse_color(value: &str) -> Option<Color32> {
    let lower = value.to_ascii_lowercase().replace('-', " ");
    match lower.as_str() {
        "white" => Some(Color32::WHITE),
        "black" => Some(Color32::BLACK),
        "cornflower blue" | "cornflowerblue" => Some(Color32::from_rgb(100, 149, 237)),
        "light salmon" | "lightsalmon" => Some(Color32::from_rgb(255, 160, 122)),
        "yellow" => Some(Color32::from_rgb(255, 226, 79)),
        "red" => Some(Color32::from_rgb(220, 70, 70)),
        "green" => Some(Color32::from_rgb(76, 166, 96)),
        "blue" => Some(Color32::from_rgb(80, 130, 230)),
        _ if value.starts_with('#') && value.len() == 7 => {
            let r = u8::from_str_radix(&value[1..3], 16).ok()?;
            let g = u8::from_str_radix(&value[3..5], 16).ok()?;
            let b = u8::from_str_radix(&value[5..7], 16).ok()?;
            Some(Color32::from_rgb(r, g, b))
        }
        _ => None,
    }
}

/// clap value parser for a color argument (named or `#rrggbb`).
pub(crate) fn parse_color_value(value: &str) -> Result<Color32, String> {
    parse_color(value).ok_or_else(|| format!("unknown color `{value}`"))
}

/// clap value parser for an `on`/`off` toggle. Accepts the same spellings the
/// old hand-written `parse_bool` did, so scripts keep working verbatim.
pub(crate) fn parse_onoff(value: &str) -> Result<bool, String> {
    match value {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err("expected on or off".to_string()),
    }
}

/// clap value parser for a chain id: the first character of the token, matching
/// the old `parse_chain_arg` (a multi-character token uses its first char).
pub(crate) fn parse_chain(value: &str) -> Result<char, String> {
    value
        .chars()
        .next()
        .ok_or_else(|| "chain id is required".to_string())
}

/// clap value parser for a per-atom representation style (with its aliases).
pub(crate) fn parse_atom_style(value: &str) -> Result<AtomStyle, String> {
    AtomStyle::from_token(value)
        .ok_or_else(|| "expected cartoon|ball-stick|stick|wireframe|sphere|dots|hidden".to_string())
}

/// clap value parser for a lighting preset.
pub(crate) fn parse_light_preset(value: &str) -> Result<LightPreset, String> {
    match value {
        "soft" => Ok(LightPreset::Soft),
        "gentle" => Ok(LightPreset::Gentle),
        "studio" => Ok(LightPreset::Studio),
        _ => Err("expected soft|gentle|studio".to_string()),
    }
}

/// clap value parser for a surface fill style.
pub(crate) fn parse_surface_style(value: &str) -> Result<SurfaceStyle, String> {
    match value {
        "fill" => Ok(SurfaceStyle::Fill),
        "mesh" => Ok(SurfaceStyle::Mesh),
        _ => Err("expected fill|mesh".to_string()),
    }
}

/// Apply a viewport mutation to the active entry, and — when `global` — to the
/// project default and every open entry's viewport too. Shared by every render
/// command so the `--global` semantics live in one place.
pub(crate) fn update_viewport<F>(state: &mut AppState, global: bool, mut update: F)
where
    F: FnMut(&mut ViewportVisualState),
{
    if global {
        update(&mut state.ui.project_viewport);
        for entry in &state.entries.records {
            let viewport = state
                .ui
                .entry_viewports
                .entry(entry.id)
                .or_insert_with(|| state.ui.project_viewport.clone());
            update(viewport);
        }
    }
    update(&mut state.ui.viewport);
}
