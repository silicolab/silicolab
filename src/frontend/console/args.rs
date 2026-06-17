use anyhow::{Result, anyhow, bail};
use eframe::egui::Color32;

use crate::frontend::{ViewportVisualState, state::AppState};

pub(crate) fn parse_chain_arg(arg: Option<&String>) -> Result<char> {
    arg.and_then(|value| value.chars().next())
        .ok_or_else(|| anyhow!("chain id is required"))
}

pub(crate) fn parse_color_arg(arg: Option<&String>) -> Result<Color32> {
    let value = arg.ok_or_else(|| anyhow!("color is required"))?;
    parse_color(value).ok_or_else(|| anyhow!("unknown color `{value}`"))
}

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

pub(crate) fn parse_bool(arg: Option<&String>, name: &str) -> Result<bool> {
    match arg.map(String::as_str) {
        Some("true" | "on" | "yes" | "1") => Ok(true),
        Some("false" | "off" | "no" | "0") => Ok(false),
        _ => bail!("{name} must be on or off"),
    }
}

pub(crate) fn parse_f32(arg: Option<&String>, name: &str) -> Result<f32> {
    arg.ok_or_else(|| anyhow!("{name} is required"))?
        .parse::<f32>()
        .map_err(Into::into)
}

pub(crate) fn parse_usize(arg: Option<&String>, name: &str) -> Result<usize> {
    arg.ok_or_else(|| anyhow!("{name} is required"))?
        .parse::<usize>()
        .map_err(Into::into)
}

pub(crate) fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

pub(crate) fn without_global_arg(args: &[String]) -> (Vec<String>, bool) {
    let mut global = false;
    let filtered = args
        .iter()
        .filter_map(|arg| {
            if arg == "--global" {
                global = true;
                None
            } else {
                Some(arg.clone())
            }
        })
        .collect();
    (filtered, global)
}

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
