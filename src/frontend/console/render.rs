use super::*;

use anyhow::{Context, Result, bail};
use eframe::egui::Color32;

use crate::frontend::{
    LightPreset, SurfaceStyle, ViewportVisualState,
    state::{AppState, AtomStyle},
    viewport::{ViewportPngExport, export_viewport_png},
};

pub(crate) fn view_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let (args, global) = without_global_arg(args);
    match args.first().map(String::as_str) {
        Some("background") => {
            let color = parse_color_arg(args.get(1))?;
            update_viewport(state, global, |viewport| {
                viewport.background_color = color;
            });
            Ok("set view background".to_string())
        }
        Some("size") => {
            let width = parse_f32(args.get(1), "width")?;
            let height = parse_f32(args.get(2), "height")?;
            state.ui.scripted_viewport_size = [
                width.round().clamp(1.0, u32::MAX as f32) as u32,
                height.round().clamp(1.0, u32::MAX as f32) as u32,
            ];
            Ok(format!("requested viewport size {width:.0}x{height:.0}"))
        }
        Some("cell") => {
            let value = parse_bool(args.get(1), "cell")?;
            update_viewport(state, global, |viewport| {
                viewport.show_cell = value;
            });
            Ok("updated unit cell visibility".to_string())
        }
        Some("water") => {
            let value = parse_bool(args.get(1), "water")?;
            let style = if value {
                crate::frontend::viewport::software_default_style(
                    crate::domain::AtomCategory::Solvent,
                )
            } else {
                AtomStyle::Hidden
            };
            update_viewport(state, global, |viewport| {
                viewport.set_category_style(crate::domain::AtomCategory::Solvent, style);
            });
            Ok("updated water visibility".to_string())
        }
        Some("light") => {
            let preset = match args.get(1).map(String::as_str) {
                Some("soft") => LightPreset::Soft,
                Some("gentle") => LightPreset::Gentle,
                Some("studio") => LightPreset::Studio,
                _ => bail!("usage: view light soft|gentle|studio"),
            };
            update_viewport(state, global, |viewport| {
                viewport.lighting.preset = preset;
            });
            Ok(format!("set light {}", preset.label().to_ascii_lowercase()))
        }
        Some("silhouette") | Some("silhouettes") => {
            let value = parse_bool(args.get(1), "silhouette")?;
            let width = option_value(&args, "--width")
                .map(str::parse::<f32>)
                .transpose()?
                .map(|width| width.clamp(0.0, 6.0));
            update_viewport(state, global, |viewport| {
                viewport.lighting.silhouettes = value;
                if let Some(width) = width {
                    viewport.lighting.silhouette_width = width;
                }
            });
            Ok("updated silhouettes".to_string())
        }
        _ => bail!(
            "usage: view background <color> | view cell <on|off> | view water <on|off> | view light <preset> | view silhouette <on|off> [--width n]"
        ),
    }
}

pub(crate) fn cartoon_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let (args, global) = without_global_arg(args);
    match args.first().map(String::as_str) {
        Some("helix") | Some("sheet") | Some("coil") => {
            let section = args[0].as_str();
            let width = option_value(&args, "--width")
                .map(str::parse::<f32>)
                .transpose()?;
            let thickness = option_value(&args, "--thickness")
                .map(str::parse::<f32>)
                .transpose()?;
            update_viewport(state, global, |viewport| {
                let target = match section {
                    "helix" => &mut viewport.cartoon.helix,
                    "sheet" => &mut viewport.cartoon.sheet,
                    _ => &mut viewport.cartoon.coil,
                };
                if let Some(width) = width {
                    target.width = width.clamp(0.05, 10.0);
                }
                if let Some(thickness) = thickness {
                    target.thickness = thickness.clamp(0.05, 10.0);
                }
            });
            Ok(format!("updated cartoon {section}"))
        }
        Some("smoothing") => {
            let smoothing = parse_usize(args.get(1), "smoothing")?.clamp(1, 32);
            update_viewport(state, global, |viewport| {
                viewport.cartoon.smoothing = smoothing;
            });
            Ok("updated cartoon smoothing".to_string())
        }
        Some("profile") => {
            let profile_segments = parse_usize(args.get(1), "profile segments")?.clamp(6, 48);
            update_viewport(state, global, |viewport| {
                viewport.cartoon.profile_segments = profile_segments;
            });
            Ok("updated cartoon profile".to_string())
        }
        _ => bail!(
            "usage: cartoon helix|sheet|coil --width n --thickness n | cartoon smoothing n | cartoon profile n"
        ),
    }
}

pub(crate) fn color_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let (args, global) = without_global_arg(args);
    match args.first().map(String::as_str) {
        Some("chain") => {
            let chain = parse_chain_arg(args.get(1))?;
            let color = parse_color_arg(args.get(2))?;
            update_viewport(state, global, |viewport| {
                viewport.chain_colors.insert(chain, color);
            });
            Ok(format!("colored chain {chain}"))
        }
        Some("ions") => {
            let color = parse_color_arg(args.get(1))?;
            update_viewport(state, global, |viewport| {
                viewport.ions.color = Some(color);
            });
            Ok("colored ions".to_string())
        }
        Some("hetero") => {
            update_viewport(state, global, |viewport| {
                viewport.hetero_atom_colors = true;
            });
            Ok("using hetero atom colors".to_string())
        }
        _ => bail!("usage: color chain <id> <color> | color ions <color> | color hetero"),
    }
}

pub(crate) fn surface_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let (args, global) = without_global_arg(args);
    match args.first().map(String::as_str) {
        Some("chain") => {
            let chain = parse_chain_arg(args.get(1))?;
            update_viewport(state, global, |viewport| {
                viewport.surface.chains.insert(chain);
            });
            Ok(format!("enabled surface for chain {chain}"))
        }
        Some("style") => {
            let style = match args.get(1).map(String::as_str) {
                Some("fill") => SurfaceStyle::Fill,
                Some("mesh") => SurfaceStyle::Mesh,
                _ => bail!("usage: surface style fill|mesh"),
            };
            update_viewport(state, global, |viewport| {
                viewport.surface.style = style;
            });
            Ok(format!(
                "surface style set to {}",
                style.label().to_ascii_lowercase()
            ))
        }
        Some("clear") => {
            update_viewport(state, global, |viewport| {
                viewport.surface.chains.clear();
            });
            Ok("cleared surfaces".to_string())
        }
        Some("transparency") => {
            let transparency = (parse_f32(args.get(1), "transparency")? / 100.0).clamp(0.0, 1.0);
            update_viewport(state, global, |viewport| {
                viewport.surface.transparency = transparency;
            });
            Ok("updated surface transparency".to_string())
        }
        _ => bail!(
            "usage: surface chain <id> | surface style fill|mesh | surface transparency <0-100> | surface clear"
        ),
    }
}

pub(crate) fn show_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let (args, global) = without_global_arg(args);
    match args.first().map(String::as_str) {
        Some("ions") => {
            let distance = option_value(&args, "--within")
                .map(str::parse::<f32>)
                .transpose()?
                .unwrap_or(3.5);
            update_viewport(state, global, |viewport| {
                viewport.ions.show_within = Some(distance.max(0.0));
            });
            Ok(format!("showing ions within {distance:.1} A"))
        }
        _ => bail!("usage: show ions [--within distance]"),
    }
}

pub(crate) fn representation_command(state: &mut AppState, args: &[String]) -> Result<String> {
    // Per-atom styles are entry-specific, so this always targets the active
    // structure; the `--global` flag (if present) is ignored.
    let (args, _global) = without_global_arg(args);
    let style = args
        .first()
        .and_then(|token| AtomStyle::from_token(token))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "usage: representation cartoon|ball-stick|stick|wireframe|sphere|dots|hidden"
            )
        })?;
    let items: Vec<(usize, crate::domain::AtomCategory)> = {
        let structure = state.structure();
        (0..structure.atoms.len())
            .map(|index| (index, structure.atom_category(index)))
            .collect()
    };
    state.ui.viewport.apply_atom_styles(items, style);
    Ok(format!("representation {}", style.label()))
}

pub(crate) fn save_command(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: &[String],
) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("image") => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: save image <path.png>"))?;
            let resolved_path = context.resolve_path(path);
            export_viewport_png(
                state.structure(),
                ViewportPngExport {
                    camera: state.ui.camera,
                    selection: &state.ui.selection,
                    visual_state: &state.ui.viewport,
                    width: state.ui.scripted_viewport_size[0],
                    height: state.ui.scripted_viewport_size[1],
                    output_path: &resolved_path,
                },
            )?;
            Ok(format!("saved image to {}", resolved_path.display()))
        }
        Some("view") => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: save view <path.sls>"))?;
            let resolved_path = context.resolve_path(path);
            let script = view_state_to_script(&state.ui.viewport);
            std::fs::write(&resolved_path, script)
                .with_context(|| format!("failed to write {}", resolved_path.display()))?;
            Ok(format!("saved view script to {}", resolved_path.display()))
        }
        _ => bail!("usage: save image <path.png> | save view <path.sls>"),
    }
}

/// Serialize the current viewport into a replayable `.sls` script of console
/// commands. Only settings that differ from the defaults are emitted, mirroring
/// how the project database stores sparse render overrides. The result can be
/// re-applied to any entry with `run <file>`, making a visualization setup
/// portable and human-readable (cf. Maestro's `*_cmd.txt` view scripts).
fn view_state_to_script(viewport: &ViewportVisualState) -> String {
    let default = ViewportVisualState::default();
    let mut lines = vec![
        "# SilicoLab view script — generated.".to_string(),
        "# Replay on the active entry with:  run <this-file>".to_string(),
    ];

    if viewport.background_color != default.background_color {
        lines.push(format!(
            "view background {}",
            color_to_hex(viewport.background_color)
        ));
    }
    if viewport.show_cell != default.show_cell {
        lines.push(format!(
            "view cell {}",
            if viewport.show_cell { "on" } else { "off" }
        ));
    }
    if viewport.lighting.preset != default.lighting.preset {
        lines.push(format!(
            "view light {}",
            viewport.lighting.preset.label().to_ascii_lowercase()
        ));
    }
    if viewport.lighting.silhouettes != default.lighting.silhouettes
        || viewport.lighting.silhouette_width != default.lighting.silhouette_width
    {
        lines.push(format!(
            "view silhouette {} --width {}",
            if viewport.lighting.silhouettes {
                "on"
            } else {
                "off"
            },
            trim_float(viewport.lighting.silhouette_width)
        ));
    }

    for (section, style, base) in [
        ("helix", viewport.cartoon.helix, default.cartoon.helix),
        ("sheet", viewport.cartoon.sheet, default.cartoon.sheet),
        ("coil", viewport.cartoon.coil, default.cartoon.coil),
    ] {
        if style.width != base.width || style.thickness != base.thickness {
            lines.push(format!(
                "cartoon {section} --width {} --thickness {}",
                trim_float(style.width),
                trim_float(style.thickness)
            ));
        }
    }
    if viewport.cartoon.smoothing != default.cartoon.smoothing {
        lines.push(format!("cartoon smoothing {}", viewport.cartoon.smoothing));
    }
    if viewport.cartoon.profile_segments != default.cartoon.profile_segments {
        lines.push(format!(
            "cartoon profile {}",
            viewport.cartoon.profile_segments
        ));
    }

    for (chain, color) in &viewport.chain_colors {
        lines.push(format!("color chain {chain} {}", color_to_hex(*color)));
    }
    if let Some(color) = viewport.ions.color {
        lines.push(format!("color ions {}", color_to_hex(color)));
    }
    if viewport.hetero_atom_colors {
        lines.push("color hetero".to_string());
    }

    if viewport.surface.style != default.surface.style {
        lines.push(format!(
            "surface style {}",
            viewport.surface.style.label().to_ascii_lowercase()
        ));
    }
    if viewport.surface.transparency != default.surface.transparency {
        lines.push(format!(
            "surface transparency {}",
            trim_float(viewport.surface.transparency * 100.0)
        ));
    }
    for chain in &viewport.surface.chains {
        lines.push(format!("surface chain {chain}"));
    }

    if let Some(distance) = viewport.ions.show_within {
        lines.push(format!("show ions --within {}", trim_float(distance)));
    }
    // Solvent hidden via the project category style maps to `view water off`.
    if viewport
        .category_styles
        .get(&crate::domain::AtomCategory::Solvent)
        == Some(&AtomStyle::Hidden)
    {
        lines.push("view water off".to_string());
    }

    lines.push(String::new());
    lines.join("\n")
}

fn color_to_hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// Format a float without a trailing `.0` so emitted commands stay tidy.
fn trim_float(value: f32) -> String {
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}
