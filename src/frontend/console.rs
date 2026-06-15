use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use anyhow::{Context, Result, anyhow, bail};
use eframe::egui::Color32;

use crate::{
    domain::{Biopolymer, ChainRecord, ResidueRecord},
    domain::{Bond, Structure},
    frontend::{
        LightPreset, SurfaceStyle, ViewportVisualState,
        state::{AppState, AtomStyle},
        viewport::{ViewportPngExport, export_viewport_png},
    },
    io::pdb_fetch,
};

#[derive(Debug, Clone, Default)]
pub struct CommandConsoleState {
    pub input: String,
    pub history: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptContext {
    variables: BTreeMap<String, String>,
    stdout_lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptRunResult {
    pub stdout_lines: Vec<String>,
}

pub fn execute_console_line(state: &mut AppState, line: &str) -> Result<String> {
    let mut context = ScriptContext::default();
    execute_console_line_with_context(state, line, &mut context)
}

pub fn run_script_file_with_args(
    state: &mut AppState,
    script_path: &std::path::Path,
    variables: BTreeMap<String, String>,
) -> Result<ScriptRunResult> {
    let mut context = ScriptContext {
        variables,
        ..ScriptContext::default()
    };
    run_script_path_with_context(state, &mut context, &script_path.display().to_string())?;
    Ok(ScriptRunResult {
        stdout_lines: context.stdout_lines,
    })
}

fn execute_console_line_with_context(
    state: &mut AppState,
    line: &str,
    context: &mut ScriptContext,
) -> Result<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(String::new());
    }

    let expanded = expand_script_variables(trimmed, &context.variables)?;
    if let Some(path) = script_source_path(trimmed) {
        let expanded_path = expand_script_variables(path, &context.variables)?;
        run_script_path_with_context(state, context, &expanded_path)?;
        return Ok(String::new());
    }
    if looks_like_script_path(&expanded) {
        run_script_path_with_context(state, context, &expanded)?;
        return Ok(String::new());
    }

    let words = shell_words(&expanded)?;
    let Some(command) = words.first().map(String::as_str) else {
        return Ok(String::new());
    };

    match command {
        "open" => open_command(state, context, &words[1..]),
        "sketch" => sketch_command(state, &words[1..]),
        "fetch" => fetch_command(state, &words[1..]),
        "source" | "run" => source_command(state, context, &words[1..]),
        "view" => view_command(state, &words[1..]),
        "cartoon" => cartoon_command(state, &words[1..]),
        "color" => color_command(state, &words[1..]),
        "surface" => surface_command(state, &words[1..]),
        "show" => show_command(state, &words[1..]),
        "hydrogen" | "hydrogens" => hydrogen_command(state, &words[1..]),
        "delete" => delete_command(state, &words[1..]),
        "representation" => representation_command(state, &words[1..]),
        "save" => save_command(state, context, &words[1..]),
        "md" => crate::frontend::md_commands::md_command(state, &words[1..]),
        "disorder" | "pack" => {
            crate::frontend::disorder_commands::disorder_command(state, &words[1..])
        }
        "qm" => crate::frontend::qm_commands::qm_command(state, &words[1..]),
        "help" => Ok(help_text()),
        _ => bail!("unknown command `{command}`"),
    }
}

impl ScriptContext {
    fn resolve_path(&self, path: &str) -> PathBuf {
        PathBuf::from(path)
    }
}

fn open_command(state: &mut AppState, context: &ScriptContext, args: &[String]) -> Result<String> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("usage: open <structure-path>"))?;
    let path = context.resolve_path(path);
    open_structure_path(state, path.clone())?;
    Ok(format!("opened {}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFetchCommand {
    id: String,
    base_url: String,
    dir: Option<PathBuf>,
}

fn fetch_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let parsed = parse_fetch_command_args(args)?;
    let target_dir = parsed.dir.unwrap_or_else(|| state.structures_dir());
    let fetched = pdb_fetch::fetch_pdb(&parsed.id, &parsed.base_url, &target_dir)?;
    open_structure_path(state, fetched.path.clone())?;
    let verb = if fetched.downloaded {
        "fetched"
    } else {
        "loaded cached"
    };
    Ok(format!("{verb} {}", fetched.path.display()))
}

fn parse_fetch_command_args(args: &[String]) -> Result<ParsedFetchCommand> {
    let mut id: Option<String> = None;
    let mut base_url = pdb_fetch::RCSB_DEFAULT_BASE_URL.to_string();
    let mut dir = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                base_url = args.get(index + 1).cloned().ok_or_else(|| {
                    anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
                })?;
                index += 2;
            }
            "--dir" => {
                dir = Some(args.get(index + 1).map(PathBuf::from).ok_or_else(|| {
                    anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
                })?);
                index += 2;
            }
            flag if flag.starts_with("--") => bail!("unknown flag `{flag}` for fetch"),
            value => {
                if id.is_some() {
                    bail!("unexpected extra argument `{value}`; fetch takes a single PDB id");
                }
                id = Some(value.to_string());
                index += 1;
            }
        }
    }

    Ok(ParsedFetchCommand {
        id: id.ok_or_else(|| {
            anyhow!("usage: fetch <pdb-id> [--db <base-url>] [--dir <directory>]")
        })?,
        base_url,
        dir,
    })
}

/// Load a structure file at `path` into a new active entry, resetting the
/// viewport. Shared by the `open` and `fetch` commands.
fn open_structure_path(state: &mut AppState, path: PathBuf) -> Result<()> {
    let document = crate::frontend::structure_import::load_document(&path)?;
    state.save_viewport_for_active_entry();
    let entry_id =
        crate::frontend::structure_import::import_document(&mut state.entries, document, path)
            .ok_or_else(|| anyhow!("structure file did not contain any models"))?;
    state.entries.activate_entry(entry_id);
    state.history.set_active_entry(Some(entry_id));
    state.ui.entry_list.selected_entry_ids.clear();
    state.ui.entry_list.selected_entry_ids.insert(entry_id);
    state.ui.selection.clear();
    state.ui.camera = crate::frontend::ViewCamera::default();
    state.ui.viewport_cache.clear();
    state.load_viewport_for_active_entry();
    Ok(())
}

/// `sketch <SMILES>` — parse a SMILES string, generate a 3D structure, and add
/// it as a new active entry. The scriptable counterpart of the GUI sketcher's
/// Build action; available in both the console and headless `.sls` scripts.
fn sketch_command(state: &mut AppState, args: &[String]) -> Result<String> {
    let smiles = args
        .first()
        .ok_or_else(|| anyhow!("usage: sketch <SMILES>"))?;
    let structure = crate::workflows::sketch_to_structure::smiles_to_structure(smiles, None)
        .with_context(|| format!("could not sketch `{smiles}`"))?;
    let atom_count = structure.atoms.len();
    let save_path = crate::io::structure_io::default_structure_save_path(&structure, None);
    let entry_id = state.entries.add_entry(structure, None, save_path);
    state.show_entry(entry_id);
    Ok(format!(
        "sketched {smiles} as entry #{entry_id} ({atom_count} atoms)"
    ))
}

fn source_command(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: &[String],
) -> Result<String> {
    let path = args
        .first()
        .ok_or_else(|| anyhow!("usage: source <script.sls>"))?;
    run_script_path_with_context(state, context, path)?;
    Ok(String::new())
}

fn run_script_path_with_context(
    state: &mut AppState,
    context: &mut ScriptContext,
    path: &str,
) -> Result<()> {
    let path = normalize_script_path(path)?;
    let resolved_path = context.resolve_path(path);
    if !resolved_path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".sls")
    {
        bail!("SilicoLab scripts use the .sls extension");
    }
    let script = std::fs::read_to_string(&resolved_path)?;
    let mut count = 0;
    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let output = execute_console_line_with_context(state, trimmed, context)?;
        if !output.is_empty() {
            context.stdout_lines.push(output);
        }
        count += 1;
    }
    let _ = count;
    Ok(())
}

fn looks_like_script_path(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !input.contains(char::is_whitespace) && lower.ends_with(".sls")
}

fn script_source_path(input: &str) -> Option<&str> {
    for command in ["source", "run"] {
        if let Some(rest) = input.strip_prefix(command)
            && rest.chars().next().is_some_and(char::is_whitespace)
        {
            return Some(rest.trim());
        }
    }
    None
}

fn normalize_script_path(path: &str) -> Result<&str> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("script path is empty");
    }

    if trimmed.starts_with('"') || trimmed.ends_with('"') {
        let Some(inner) = trimmed
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            bail!("double-quoted script paths must start and end with \"");
        };
        if inner.contains('"') {
            bail!("script paths cannot contain quote characters");
        }
        return Ok(inner);
    }

    if trimmed.starts_with('\'') || trimmed.ends_with('\'') {
        let Some(inner) = trimmed
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
        else {
            bail!("single-quoted script paths must start and end with '");
        };
        if inner.contains('\'') {
            bail!("script paths cannot contain quote characters");
        }
        return Ok(inner);
    }

    if trimmed.contains(char::is_whitespace) {
        bail!("script paths with spaces must be wrapped in matching quotes");
    }
    Ok(trimmed)
}

fn view_command(state: &mut AppState, args: &[String]) -> Result<String> {
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

fn cartoon_command(state: &mut AppState, args: &[String]) -> Result<String> {
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

fn color_command(state: &mut AppState, args: &[String]) -> Result<String> {
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

fn surface_command(state: &mut AppState, args: &[String]) -> Result<String> {
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

fn show_command(state: &mut AppState, args: &[String]) -> Result<String> {
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

fn hydrogen_command(state: &mut AppState, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("add") | Some("fill") => add_hydrogens_command(state),
        _ => bail!("usage: hydrogen add"),
    }
}

fn add_hydrogens_command(state: &mut AppState) -> Result<String> {
    if !state.has_active_entry() {
        bail!("hydrogen add requires an open entry");
    }
    let before = state.capture_edit_snapshot();
    let old_atom_count = state.structure().atoms.len();
    let added = state.structure_mut().add_missing_hydrogens();
    attach_added_hydrogens_to_biopolymer(state.structure_mut(), old_atom_count);
    state.mark_structure_changed();
    state.set_source_path(None);
    state
        .ui
        .selection
        .retain_valid(state.structure().atoms.len());
    state.history.push_undo(before);
    Ok(format!("added {added} hydrogen(s)"))
}

fn delete_command(state: &mut AppState, args: &[String]) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("chain") => {
            let chains = args
                .get(1)
                .ok_or_else(|| anyhow!("usage: delete chain <A,B,...>"))?
                .split(',')
                .filter_map(|token| token.trim().chars().next())
                .collect::<BTreeSet<_>>();
            if chains.is_empty() {
                bail!("delete chain requires at least one chain id");
            }
            let before = state.capture_edit_snapshot();
            let removed = retain_chains(state.structure_mut(), &chains);
            state.mark_structure_changed();
            state.history.push_undo(before);
            Ok(format!("deleted {removed} atom(s) from chain selection"))
        }
        _ => bail!("usage: delete chain <A,B,...>"),
    }
}

fn representation_command(state: &mut AppState, args: &[String]) -> Result<String> {
    // Per-atom styles are entry-specific, so this always targets the active
    // structure; the `--global` flag (if present) is ignored.
    let (args, _global) = without_global_arg(args);
    let style = args
        .first()
        .and_then(|token| AtomStyle::from_token(token))
        .ok_or_else(|| {
            anyhow!("usage: representation cartoon|ball-stick|stick|wireframe|sphere|dots|hidden")
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

fn save_command(
    state: &mut AppState,
    context: &mut ScriptContext,
    args: &[String],
) -> Result<String> {
    match args.first().map(String::as_str) {
        Some("image") => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow!("usage: save image <path.png>"))?;
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
                .ok_or_else(|| anyhow!("usage: save view <path.sls>"))?;
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

fn retain_chains(structure: &mut Structure, deleted_chains: &BTreeSet<char>) -> usize {
    let Some(biopolymer) = structure.biopolymer.clone() else {
        return 0;
    };
    let delete_atom = biopolymer
        .residue_for_atom
        .iter()
        .map(|residue_index| {
            residue_index
                .and_then(|index| biopolymer.residues.get(index))
                .is_some_and(|residue| deleted_chains.contains(&residue.id.chain_id))
        })
        .collect::<Vec<_>>();
    let removed = delete_atom.iter().filter(|delete| **delete).count();
    if removed == 0 {
        return 0;
    }

    let mut remap = vec![None; structure.atoms.len()];
    let mut atoms = Vec::with_capacity(structure.atoms.len() - removed);
    for (index, atom) in structure.atoms.iter().enumerate() {
        if !delete_atom[index] {
            remap[index] = Some(atoms.len());
            atoms.push(atom.clone());
        }
    }
    let bonds = structure
        .bonds
        .iter()
        .filter_map(|bond| {
            Some(Bond {
                a: remap[bond.a]?,
                b: remap[bond.b]?,
                bond_type: bond.bond_type,
            })
        })
        .collect();

    structure.atoms = atoms;
    structure.bonds = bonds;
    structure.biopolymer = biopolymer_after_atom_retain(&biopolymer, &remap);
    removed
}

fn attach_added_hydrogens_to_biopolymer(structure: &mut Structure, old_atom_count: usize) {
    let Some(biopolymer) = &mut structure.biopolymer else {
        return;
    };
    if old_atom_count >= structure.atoms.len()
        || biopolymer.residue_for_atom.len() != old_atom_count
    {
        return;
    }

    biopolymer
        .residue_for_atom
        .resize(structure.atoms.len(), None);
    for atom_index in old_atom_count..structure.atoms.len() {
        let Some(parent_residue) = structure.bonds.iter().find_map(|bond| {
            if bond.a == atom_index && bond.b < old_atom_count {
                biopolymer.residue_for_atom[bond.b]
            } else if bond.b == atom_index && bond.a < old_atom_count {
                biopolymer.residue_for_atom[bond.a]
            } else {
                None
            }
        }) else {
            continue;
        };
        biopolymer.residue_for_atom[atom_index] = Some(parent_residue);
        if let Some(residue) = biopolymer.residues.get_mut(parent_residue) {
            residue.atom_indices.push(atom_index);
        }
    }
}

fn biopolymer_after_atom_retain(
    source: &Biopolymer,
    atom_remap: &[Option<usize>],
) -> Option<Biopolymer> {
    let mut residues = Vec::new();
    let mut residue_remap = vec![None; source.residues.len()];

    for (old_residue_index, residue) in source.residues.iter().enumerate() {
        let atom_indices = residue
            .atom_indices
            .iter()
            .filter_map(|&atom_index| atom_remap.get(atom_index).copied().flatten())
            .collect::<Vec<_>>();
        if atom_indices.is_empty() {
            continue;
        }
        let alpha_carbon = residue
            .alpha_carbon
            .and_then(|atom_index| atom_remap.get(atom_index).copied().flatten());
        residue_remap[old_residue_index] = Some(residues.len());
        residues.push(ResidueRecord {
            id: residue.id.clone(),
            residue_name: residue.residue_name.clone(),
            atom_indices,
            alpha_carbon,
            is_standard_amino_acid: residue.is_standard_amino_acid,
        });
    }

    let chains = source
        .chains
        .iter()
        .filter_map(|chain| {
            let residue_indices = chain
                .residue_indices
                .iter()
                .filter_map(|&index| residue_remap[index])
                .collect::<Vec<_>>();
            (!residue_indices.is_empty()).then_some(ChainRecord {
                id: chain.id,
                residue_indices,
            })
        })
        .collect::<Vec<_>>();

    if residues.is_empty() {
        return None;
    }

    let new_atom_count = atom_remap.iter().filter(|entry| entry.is_some()).count();
    let mut residue_for_atom = vec![None; new_atom_count];
    let mut atom_name_for_atom = vec![None; new_atom_count];
    for (old_atom_index, new_atom_index) in atom_remap.iter().enumerate() {
        let Some(new_atom_index) = new_atom_index else {
            continue;
        };
        residue_for_atom[*new_atom_index] = source
            .residue_for_atom
            .get(old_atom_index)
            .copied()
            .flatten()
            .and_then(|residue_index| residue_remap[residue_index]);
        atom_name_for_atom[*new_atom_index] = source
            .atom_name_for_atom
            .get(old_atom_index)
            .cloned()
            .flatten();
    }

    Some(Biopolymer {
        residues,
        chains,
        secondary_structures: source.secondary_structures.clone(),
        residue_for_atom,
        atom_name_for_atom,
    })
}

fn parse_chain_arg(arg: Option<&String>) -> Result<char> {
    arg.and_then(|value| value.chars().next())
        .ok_or_else(|| anyhow!("chain id is required"))
}

fn parse_color_arg(arg: Option<&String>) -> Result<Color32> {
    let value = arg.ok_or_else(|| anyhow!("color is required"))?;
    parse_color(value).ok_or_else(|| anyhow!("unknown color `{value}`"))
}

fn parse_color(value: &str) -> Option<Color32> {
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

fn parse_bool(arg: Option<&String>, name: &str) -> Result<bool> {
    match arg.map(String::as_str) {
        Some("true" | "on" | "yes" | "1") => Ok(true),
        Some("false" | "off" | "no" | "0") => Ok(false),
        _ => bail!("{name} must be on or off"),
    }
}

fn parse_f32(arg: Option<&String>, name: &str) -> Result<f32> {
    arg.ok_or_else(|| anyhow!("{name} is required"))?
        .parse::<f32>()
        .map_err(Into::into)
}

fn parse_usize(arg: Option<&String>, name: &str) -> Result<usize> {
    arg.ok_or_else(|| anyhow!("{name} is required"))?
        .parse::<usize>()
        .map_err(Into::into)
}

fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}

fn without_global_arg(args: &[String]) -> (Vec<String>, bool) {
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

fn update_viewport<F>(state: &mut AppState, global: bool, mut update: F)
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

fn shell_words(input: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for ch in input.chars() {
        match ch {
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if quote.is_some() {
        bail!("unterminated quote");
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

fn expand_script_variables(input: &str, variables: &BTreeMap<String, String>) -> Result<String> {
    let mut expanded = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '$' || chars.peek() != Some(&'{') {
            expanded.push(ch);
            continue;
        }

        chars.next();
        let mut expression = String::new();
        let mut closed = false;
        for next in chars.by_ref() {
            if next == '}' {
                closed = true;
                break;
            }
            expression.push(next);
        }
        if !closed {
            bail!("unterminated variable expression");
        }

        let (name, default_value) = if let Some((name, default_value)) = expression.split_once(":-")
        {
            (name.trim(), Some(default_value))
        } else {
            (expression.trim(), None)
        };
        if !is_valid_variable_name(name) {
            bail!("invalid script variable `{name}`");
        }

        if let Some(value) = variables.get(name) {
            expanded.push_str(value);
        } else if let Some(default_value) = default_value {
            expanded.push_str(default_value);
        } else {
            bail!("missing script variable `{name}`");
        }
    }

    Ok(expanded)
}

fn is_valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The full `.sls` command catalog with examples — the static, cacheable system
/// prompt for the in-app assistant. Kept here (next to the dispatch it
/// documents) so it stays in step as commands change; never include volatile
/// per-turn state here (that flows through the agent's `inspect` tool).
pub fn command_catalog() -> String {
    [
        "SilicoLab `.sls` command catalog. One command per `run_command` call.",
        "",
        "Loading structures:",
        "  open <path>                 Load a structure file (.pdb/.cif/.xyz/.mol2/.gro) as a new entry.",
        "  fetch <pdb-id>              Download a structure by PDB id (e.g. `fetch 4hhb`).",
        "    [--db <base-url>] [--dir <directory>]",
        "  sketch <SMILES>             Build a 3D structure from SMILES (e.g. `sketch CCO`).",
        "",
        "Viewport (per active entry; add `--global` to apply project-wide):",
        "  view background <color>     Named color or #rrggbb (e.g. `view background white`).",
        "  view cell on|off            Show/hide the unit cell.",
        "  view water on|off           Show/hide solvent.",
        "  view light soft|gentle|studio",
        "  view silhouette on|off [--width n]",
        "  representation <style>      cartoon | ball-stick | stick | wireframe | sphere | dots | hidden.",
        "  cartoon helix|sheet|coil --width n --thickness n",
        "  cartoon smoothing n; cartoon profile n",
        "  color chain <id> <color>; color ions <color>; color hetero",
        "  surface chain <id>; surface style fill|mesh; surface transparency <0-100>; surface clear",
        "  show ions [--within 3.5]",
        "",
        "Editing (gated — the user confirms before these run):",
        "  hydrogen add                Add missing hydrogens to the active structure.",
        "  delete chain <A,B,...>      Delete the listed chains.",
        "  save image <path.png>       Render the viewport to a PNG.",
        "  save view <path.sls>        Save the current view as a replayable script.",
        "",
        "Simulation (gated — minutes/GPU):",
        "  md build                    Box + capture topology for the active structure.",
        "  md simulate [--time 1ns] [--temperature 300] [--no-relax]",
        "  qm energy|optimize|freq [--method b3lyp] [--basis def2-svp] [--charge 0] [--spin 1]",
        "  qm periodic [--functional pade] [--basis SZV-GTH] [--kmesh 2x2x2] [--cutoff 280] (needs a cell)",
        "",
        "Tips: render commands target the active entry unless given `--global`. Call `inspect` \
         first when you are unsure what is loaded.",
    ]
    .join("\n")
}

fn help_text() -> String {
    [
        "commands:",
        "open <path>",
        "sketch <SMILES>   build a 3D structure from a SMILES string and add it as a new entry",
        "fetch <pdb-id> [--db <base-url>] [--dir <directory>]   download a structure by PDB id",
        "source <script.sls>",
        "save image <path.png>",
        "view background <color>; view cell on|off; view water on|off; view light soft|gentle|studio",
        "cartoon helix|sheet|coil --width n --thickness n; cartoon smoothing n; cartoon profile n",
        "surface chain <id>; surface style fill|mesh; surface transparency <0-100>",
        "color chain <id> <color>; color ions <color>; color hetero",
        "show ions --within 3.5",
        "hydrogen add",
        "delete chain <A,B,...>",
        "md build   wrap the active structure in a simulation box and capture its topology",
        "md simulate [--time 1ns] [--temperature 300] [--no-relax]   run EM/NVT/NPT/production + analysis",
        "disorder --of <entry> [--count n|--density g/cm3|--conc mol/L] --box X,Y,Z|--sphere R|--cylinder R,L   pack molecules (alias: pack)",
        "qm energy|optimize|freq [--method b3lyp] [--basis def2-svp] [--charge 0] [--spin 1] [--properties]",
        "qm periodic [--functional pade|lda] [--basis SZV-GTH] [--kmesh 2x2x2] [--cutoff 280] [--forces] [--stress]   periodic (crystal) DFT on the active cell",
        "add --global to render commands to apply them project-wide",
        "script variables: ${name} or ${name:-default}",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        execute_console_line, expand_script_variables, normalize_script_path,
        parse_fetch_command_args, script_source_path,
    };
    use crate::frontend::{
        LightPreset, SurfaceStyle, ViewportVisualState,
        state::{AppState, AtomStyle},
    };
    use eframe::egui::Color32;

    const CONSOLE_TEST_PDB: &str = "\
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  N   ALA A   2       2.900   0.000   0.000  1.00  0.00           N
END
";

    fn write_console_fixture(name: &str, contents: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir = std::env::temp_dir().join("silicolab_console_tests");
        fs::create_dir_all(&dir).expect("fixture dir");
        let path = dir.join(format!("{name}_{nonce}.pdb"));
        fs::write(&path, contents).expect("fixture file");
        path
    }

    fn open_fixture_command(path: &Path) -> String {
        format!("open {}", path.display())
    }

    #[test]
    fn source_and_run_take_the_rest_of_the_line_as_script_path() {
        let path = r#""C:\projects\silicolab\reference\sls.sls""#;
        assert_eq!(script_source_path(&format!("source {path}")), Some(path));
        assert_eq!(
            script_source_path("run C:\\tmp\\demo.sls"),
            Some("C:\\tmp\\demo.sls")
        );
    }

    #[test]
    fn script_paths_allow_one_wrapping_quote_pair() {
        assert_eq!(
            normalize_script_path(r#""C:\tmp\demo.sls""#).unwrap(),
            r"C:\tmp\demo.sls"
        );
        assert_eq!(
            normalize_script_path(r#"'C:\tmp\demo.sls'"#).unwrap(),
            r"C:\tmp\demo.sls"
        );
    }

    #[test]
    fn malformed_script_quotes_fail_before_filesystem_access() {
        assert!(normalize_script_path(r#""C:\tmp\demo.sls"#).is_err());
        assert!(normalize_script_path(r#"C:\tmp\demo.sls""#).is_err());
        assert!(normalize_script_path(r#"'C:\tmp\demo.sls"#).is_err());
    }

    #[test]
    fn expands_script_variables_with_optional_defaults() {
        let mut variables = BTreeMap::new();
        variables.insert("width".to_string(), "900".to_string());

        assert_eq!(
            expand_script_variables("view size ${width} ${height:-500}", &variables).unwrap(),
            "view size 900 500"
        );
    }

    #[test]
    fn missing_script_variables_fail_without_default() {
        let error = expand_script_variables("save image ${output}", &BTreeMap::new())
            .expect_err("missing output should fail");
        assert!(
            error
                .to_string()
                .contains("missing script variable `output`")
        );
    }

    #[test]
    fn fetch_command_args_support_db_and_dir_flags() {
        let parsed = parse_fetch_command_args(&[
            "4hhb".to_string(),
            "--db".to_string(),
            "https://example.org/pdb".to_string(),
            "--dir".to_string(),
            "tmp/structures".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed.id, "4hhb");
        assert_eq!(parsed.base_url, "https://example.org/pdb");
        assert_eq!(parsed.dir.unwrap(), PathBuf::from("tmp/structures"));
    }

    #[test]
    fn fetch_command_args_reject_unknown_flags() {
        let error = parse_fetch_command_args(&["4hhb".to_string(), "--oops".to_string()])
            .expect_err("unknown flags should fail");
        assert!(
            error
                .to_string()
                .contains("unknown flag `--oops` for fetch")
        );
    }

    #[test]
    fn render_commands_are_scoped_to_the_active_entry() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let fixture = write_console_fixture("render_scope", CONSOLE_TEST_PDB);

        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
        let first_entry_id = state.entries.active_entry_id().unwrap();
        execute_console_line(&mut state, "surface style mesh").unwrap();
        execute_console_line(&mut state, "surface chain A").unwrap();
        // `sphere` differs from the protein smart default (cartoon), so it is
        // observable that the style is scoped to this entry.
        execute_console_line(&mut state, "representation sphere").unwrap();

        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
        let second_entry_id = state.entries.active_entry_id().unwrap();
        assert_ne!(first_entry_id, second_entry_id);
        // The fresh entry resolves protein → cartoon from the category tiers,
        // not the first entry's per-atom sphere styling.
        assert_eq!(
            state.ui.viewport.resolved_atom_style(state.structure(), 0),
            AtomStyle::Cartoon
        );
        assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
        assert!(state.ui.viewport.surface.chains.is_empty());

        state.save_viewport_for_active_entry();
        state.entries.activate_entry(first_entry_id);
        state.load_viewport_for_active_entry();
        assert_eq!(
            state.ui.viewport.resolved_atom_style(state.structure(), 0),
            AtomStyle::Sphere
        );
        assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
        assert!(state.ui.viewport.surface.chains.contains(&'A'));
    }

    #[test]
    fn global_render_commands_update_project_defaults() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let fixture = write_console_fixture("render_global", CONSOLE_TEST_PDB);

        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
        execute_console_line(&mut state, "surface style mesh --global").unwrap();
        execute_console_line(&mut state, "representation sphere --global").unwrap();

        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
        // Global surface settings propagate to new entries. The `representation`
        // command is per-entry atom-level, so the fresh entry still resolves its
        // protein to cartoon via the category tiers.
        assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
        assert_eq!(
            state.ui.viewport.resolved_atom_style(state.structure(), 0),
            AtomStyle::Cartoon
        );
    }

    #[test]
    fn view_script_export_roundtrips() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        let fixture = write_console_fixture("view_export", CONSOLE_TEST_PDB);
        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();

        for line in [
            "view background #102030",
            "view cell off",
            "view light studio",
            "cartoon helix --width 3 --thickness 0.4",
            "color chain A #ff8800",
            "surface style mesh",
            "surface transparency 50",
            "surface chain A",
            "show ions --within 4",
        ] {
            execute_console_line(&mut state, line).unwrap();
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let script_path = std::env::temp_dir()
            .join("silicolab_console_tests")
            .join(format!("view_{nonce}.sls"));
        execute_console_line(&mut state, &format!("save view {}", script_path.display())).unwrap();

        // A fresh entry resets the viewport to defaults...
        execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
        assert_eq!(
            state.ui.viewport.background_color,
            ViewportVisualState::default().background_color
        );

        // ...and replaying the exported script reproduces every setting.
        execute_console_line(&mut state, &format!("run {}", script_path.display())).unwrap();
        let viewport = &state.ui.viewport;
        assert_eq!(
            viewport.background_color,
            Color32::from_rgb(0x10, 0x20, 0x30)
        );
        assert!(!viewport.show_cell);
        assert_eq!(viewport.lighting.preset, LightPreset::Studio);
        assert!((viewport.cartoon.helix.width - 3.0).abs() < 1e-4);
        assert!((viewport.cartoon.helix.thickness - 0.4).abs() < 1e-4);
        assert_eq!(
            viewport.chain_colors.get(&'A'),
            Some(&Color32::from_rgb(0xff, 0x88, 0x00))
        );
        assert_eq!(viewport.surface.style, SurfaceStyle::Mesh);
        assert!((viewport.surface.transparency - 0.5).abs() < 1e-4);
        assert!(viewport.surface.chains.contains(&'A'));
        assert_eq!(viewport.ions.show_within, Some(4.0));
    }
}
