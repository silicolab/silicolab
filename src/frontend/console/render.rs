use super::*;

use anyhow::{Context, Result};
use eframe::egui::Color32;

use crate::frontend::{
    ViewportVisualState,
    state::{AppState, AtomStyle},
    viewport::PendingViewportPngExport,
};

pub(crate) fn view_command(state: &mut AppState, args: ViewArgs) -> Result<String> {
    let global = args.global.global;
    match args.kind {
        ViewKind::Background { color } => {
            update_viewport(state, global, |viewport| {
                viewport.background_color = color;
            });
            Ok("set view background".to_string())
        }
        ViewKind::Size { width, height } => {
            state.ui.scripted_viewport_size = [
                width.round().clamp(1.0, u32::MAX as f32) as u32,
                height.round().clamp(1.0, u32::MAX as f32) as u32,
            ];
            Ok(format!("requested viewport size {width:.0}x{height:.0}"))
        }
        ViewKind::Cell { on } => {
            update_viewport(state, global, |viewport| {
                viewport.show_cell = on;
            });
            Ok("updated unit cell visibility".to_string())
        }
        ViewKind::Water { on } => {
            let style = if on {
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
        ViewKind::Light { preset } => {
            update_viewport(state, global, |viewport| {
                viewport.lighting.preset = preset;
            });
            Ok(format!("set light {}", preset.label().to_ascii_lowercase()))
        }
        ViewKind::Silhouette { on, width } => {
            let width = width.map(|width| width.clamp(0.0, 6.0));
            update_viewport(state, global, |viewport| {
                viewport.lighting.silhouettes = on;
                if let Some(width) = width {
                    viewport.lighting.silhouette_width = width;
                }
            });
            Ok("updated silhouettes".to_string())
        }
    }
}

pub(crate) fn cartoon_command(state: &mut AppState, args: CartoonArgs) -> Result<String> {
    let global = args.global.global;
    match args.kind {
        CartoonKind::Helix(section) => apply_cartoon_section(state, global, "helix", section),
        CartoonKind::Sheet(section) => apply_cartoon_section(state, global, "sheet", section),
        CartoonKind::Coil(section) => apply_cartoon_section(state, global, "coil", section),
        CartoonKind::Smoothing { value } => {
            let smoothing = value.clamp(1, 32);
            update_viewport(state, global, |viewport| {
                viewport.cartoon.smoothing = smoothing;
            });
            Ok("updated cartoon smoothing".to_string())
        }
        CartoonKind::Profile { value } => {
            let profile_segments = value.clamp(6, 48);
            update_viewport(state, global, |viewport| {
                viewport.cartoon.profile_segments = profile_segments;
            });
            Ok("updated cartoon profile".to_string())
        }
    }
}

fn apply_cartoon_section(
    state: &mut AppState,
    global: bool,
    section: &str,
    section_args: CartoonSection,
) -> Result<String> {
    let CartoonSection { width, thickness } = section_args;
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

pub(crate) fn color_command(state: &mut AppState, args: ColorArgs) -> Result<String> {
    let global = args.global.global;
    match args.kind {
        ColorKind::Chain { id, color } => {
            update_viewport(state, global, |viewport| {
                viewport.chain_colors.insert(id, color);
            });
            Ok(format!("colored chain {id}"))
        }
        ColorKind::Ions { color } => {
            update_viewport(state, global, |viewport| {
                viewport.ions.color = Some(color);
            });
            Ok("colored ions".to_string())
        }
        ColorKind::Hetero => {
            update_viewport(state, global, |viewport| {
                viewport.hetero_atom_colors = true;
            });
            Ok("using hetero atom colors".to_string())
        }
    }
}

pub(crate) fn surface_command(state: &mut AppState, args: SurfaceArgs) -> Result<String> {
    let global = args.global.global;
    match args.kind {
        SurfaceKind::Chain { id } => {
            update_viewport(state, global, |viewport| {
                viewport.surface.chains.insert(id);
            });
            Ok(format!("enabled surface for chain {id}"))
        }
        SurfaceKind::Style { value } => {
            update_viewport(state, global, |viewport| {
                viewport.surface.style = value;
            });
            Ok(format!(
                "surface style set to {}",
                value.label().to_ascii_lowercase()
            ))
        }
        SurfaceKind::Clear => {
            update_viewport(state, global, |viewport| {
                viewport.surface.chains.clear();
            });
            Ok("cleared surfaces".to_string())
        }
        SurfaceKind::Transparency { value } => {
            let transparency = (value / 100.0).clamp(0.0, 1.0);
            update_viewport(state, global, |viewport| {
                viewport.surface.transparency = transparency;
            });
            Ok("updated surface transparency".to_string())
        }
    }
}

pub(crate) fn show_command(state: &mut AppState, args: ShowArgs) -> Result<String> {
    let global = args.global.global;
    match args.kind {
        ShowKind::Ions { within } => {
            let distance = within.unwrap_or(3.5);
            update_viewport(state, global, |viewport| {
                viewport.ions.show_within = Some(distance.max(0.0));
            });
            Ok(format!("showing ions within {distance:.1} A"))
        }
    }
}

pub(crate) fn representation_command(
    state: &mut AppState,
    args: RepresentationArgs,
) -> Result<String> {
    // Per-atom styles are entry-specific, so this always targets the active
    // structure; the `--global` flag (if present) is ignored.
    let style = args.style;
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
    target: SaveTarget,
) -> Result<String> {
    match target {
        SaveTarget::Image { path } => {
            if !context.gpu_image_export {
                anyhow::bail!(
                    "image export is unavailable in CLI mode; launch the GUI to use GPU export"
                );
            }
            let resolved_path = context.resolve_path(&path);
            let request = PendingViewportPngExport {
                structure: state.structure().clone(),
                camera: state.ui.camera,
                selection: state.ui.selection.clone(),
                visual_state: state.ui.viewport.clone(),
                width: state.ui.scripted_viewport_size[0],
                height: state.ui.scripted_viewport_size[1],
                output_path: resolved_path.clone(),
            };
            state.ui.pending_viewport_exports.push_back(request);
            Ok(format!(
                "queued image export to {}",
                resolved_path.display()
            ))
        }
        SaveTarget::View { path } => {
            let resolved_path = context.resolve_path(&path);
            let script = view_state_to_script(&state.ui.viewport);
            std::fs::write(&resolved_path, script)
                .with_context(|| format!("failed to write {}", resolved_path.display()))?;
            Ok(format!("saved view script to {}", resolved_path.display()))
        }
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
