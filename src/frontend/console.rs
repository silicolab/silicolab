use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::frontend::state::AppState;

mod args;
mod editing;
mod grammar;
mod loading;
mod render;

pub(crate) use args::*;
pub(crate) use editing::*;
pub(crate) use grammar::*;
pub(crate) use loading::*;
pub(crate) use render::*;

#[cfg(test)]
mod tests;

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

impl ScriptContext {
    pub(crate) fn resolve_path(&self, path: impl AsRef<Path>) -> PathBuf {
        path.as_ref().to_path_buf()
    }
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
    // The bare `*.sls` shortcut: a single whitespace-free `.sls` token runs as a
    // script. Checked before tokenization so it can't be mistaken for a command.
    if looks_like_script_path(&expanded) {
        run_script_path_with_context(state, context, &expanded)?;
        return Ok(String::new());
    }

    // Tokenize (Windows-backslash-safe), then let the clap grammar dispatch.
    // `source`/`run` is now an ordinary subcommand, so a quoted script path is
    // de-quoted in exactly one place — the tokenizer.
    let words = shell_words(&expanded)?;
    if words.is_empty() {
        return Ok(String::new());
    }
    grammar::run(state, context, &words)
}

pub(crate) fn run_script_path_with_context(
    state: &mut AppState,
    context: &mut ScriptContext,
    path: &str,
) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        bail!("script path is empty");
    }
    let resolved_path = context.resolve_path(path);
    if !resolved_path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".sls")
    {
        bail!("SilicoLab scripts use the .sls extension");
    }
    let script = std::fs::read_to_string(&resolved_path)?;
    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let output = execute_console_line_with_context(state, trimmed, context)?;
        if !output.is_empty() {
            context.stdout_lines.push(output);
        }
    }
    Ok(())
}

fn looks_like_script_path(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    !input.contains(char::is_whitespace) && lower.ends_with(".sls")
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
        "Switching entries (render/md/qm act on the ACTIVE entry only):",
        "  activate <#id|name>         Make an already-open entry active (e.g. `activate #2`).",
        "                              `open`/`fetch`/`sketch` create a new active entry; use",
        "                              `activate` to switch back. `inspect` lists ids and the active one.",
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
        "    also: --dispersion d3|d4  --smd <solvent>|--solvent <name>  --x2c  --fod  --sph  --grid 0-4",
        "  qm recommend <task>         Print the recommended QM level of theory (general|barriers|nci|",
        "                              thermochemistry). Read-only, not gated — consult it before choosing.",
        "  qm periodic [--functional pade] [--basis SZV-GTH] [--kmesh 2x2x2] [--cutoff 280] (needs a cell)",
        "  dock --receptor <entry> --ligand <entry> [--center x,y,z] [--size x,y,z] [--exhaustiveness 8] [--modes 9] [--seed 0]",
        "  score --receptor <entry> --ligand <entry> [--center x,y,z] [--size x,y,z]   (single-point pose score)",
        "",
        "Tips: render commands target the active entry unless given `--global`. Call `inspect` \
         first when you are unsure what is loaded.",
    ]
    .join("\n")
}
