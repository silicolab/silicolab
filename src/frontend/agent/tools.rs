//! The agent's tool surface and its dispatch over `AppState`.
//!
//! Two high-leverage tools: `run_command` (executes one `.sls` line through the
//! same [`execute_console_line`](crate::frontend::console::execute_console_line)
//! a human types, so it exposes the entire command surface and auto-syncs as
//! commands are added) and `inspect` (read-only perception, so the agent does
//! not act blind). Confirmation gating classifies destructive/expensive commands
//! that need a one-click approval before they run.

use std::fmt::Write;

use serde_json::{Value, json};

use crate::engines::registry::{EngineId, EngineRegistry};
use crate::frontend::state::AppState;
use crate::io::llm::types::{ToolCall, ToolDef};

/// How much of a tool result is replayed into history. Large md/qm/inspect
/// outputs otherwise poison the cached transcript and inflate cost.
const MAX_RESULT_CHARS: usize = 4000;

/// The tools advertised to the model. JSON Schema input, neutral [`ToolDef`].
pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "run_command".to_string(),
            description: "Run one SilicoLab `.sls` console command (e.g. `open 1abc.pdb`, \
                `fetch 4hhb`, `sketch CCO`, `view background white`, `color chain A red`, \
                `representation cartoon`, `hydrogen add`, `md build`, `qm energy`, \
                `dock --receptor active --ligand 2`). One command per call; this is the same \
                command line a user types in the console. Destructive or expensive commands \
                (delete, save, md, qm, dock, score, running a script) require the user to \
                confirm before they run."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "A single .sls command line."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "inspect".to_string(),
            description: "Read-only look at the current workspace: the active entry, its \
                atom/bond/chain counts, composition and bond geometry, MD/QM provenance and \
                trajectory state, the list of open entries, available compute engines, and the \
                latest status. Call this before acting so you know the current state. Takes no \
                required arguments."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional free-text focus (currently advisory)."
                    }
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "recommend_method".to_string(),
            description: "Look up SilicoLab's method-selection guidance for a task (e.g. \
                `thermochemistry of a reaction`, `optimize a molecule`, `dock a ligand`, \
                `periodic DFT`, `anion energy`, `non-covalent interaction`). Returns which \
                engine/command to use, the runnable `.sls` steps, and the caveats. Read-only \
                (no confirmation). For the curated QM level of theory it points you to \
                `qm recommend <task>`. Consult it before choosing a method by hand."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "What you want to compute, in a few words."
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "save_script".to_string(),
            description: "Save a reusable SilicoLab `.sls` script of console commands to the \
                project so a workflow can be replayed with `run <file>`. Use this to capture a \
                sequence of steps you ran. Writing a file requires the user to confirm."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "Script file name (a `.sls` suffix is added if missing). \
                            No directories — a bare name."
                    },
                    "commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One `.sls` command per array element."
                    }
                },
                "required": ["filename", "commands"],
                "additionalProperties": false
            }),
        },
    ]
}

/// The textual result of a tool call plus whether it was an error.
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
}

/// Whether a tool call must be confirmed by the user before running.
/// `save_script` writes a file (always gated); `run_command` is gated by verb.
pub fn needs_confirmation(call: &ToolCall) -> bool {
    match call.name.as_str() {
        "save_script" => true,
        "run_command" => {
            let command = call
                .input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            command_needs_confirmation(command)
        }
        _ => false,
    }
}

/// Classify a `.sls` command line as needing confirmation. Read-only / cheaply
/// reversible commands (view/color/surface/representation/inspect) auto-run;
/// delete, save/overwrite, md/qm runs, and script execution are gated.
pub fn command_needs_confirmation(command: &str) -> bool {
    let mut tokens = command.split_whitespace();
    let verb = tokens.next().unwrap_or_default();
    // `qm recommend` only prints hartree's level-of-theory table — it runs no
    // calculation, so it is read-only introspection and should not be gated like
    // an actual `qm energy|optimize|freq` job. Let the agent consult it freely.
    if verb == "qm" && tokens.next() == Some("recommend") {
        return false;
    }
    matches!(
        verb,
        "delete" | "save" | "md" | "qm" | "dock" | "score" | "run" | "source"
    )
}

/// Execute one tool call against `AppState`, returning its textual result.
pub fn execute_tool(state: &mut AppState, call: &ToolCall) -> ToolOutcome {
    match call.name.as_str() {
        "run_command" => match call.input.get("command").and_then(Value::as_str) {
            Some(command) => run_command_tool(state, command),
            None => ToolOutcome {
                content: "run_command requires a `command` string.".to_string(),
                is_error: true,
            },
        },
        "inspect" => {
            let query = call.input.get("query").and_then(Value::as_str);
            ToolOutcome {
                content: inspect(state, query),
                is_error: false,
            }
        }
        "recommend_method" => {
            let task = call
                .input
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default();
            ToolOutcome {
                content: crate::engines::methods::recommend(task),
                is_error: false,
            }
        }
        "save_script" => save_script_tool(state, &call.input),
        other => ToolOutcome {
            content: format!("Unknown tool `{other}`."),
            is_error: true,
        },
    }
}

/// Write a `.sls` script of console commands into the project's `scripts/`
/// directory (or a temp scripts dir in a scratch workspace). The filename is
/// restricted to a bare name to keep writes inside that directory.
fn save_script_tool(state: &mut AppState, input: &Value) -> ToolOutcome {
    let Some(filename) = input.get("filename").and_then(Value::as_str) else {
        return ToolOutcome {
            content: "save_script requires a `filename`.".to_string(),
            is_error: true,
        };
    };
    let commands: Vec<String> = match input.get("commands").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        None => {
            return ToolOutcome {
                content: "save_script requires a `commands` array.".to_string(),
                is_error: true,
            };
        }
    };

    let name = std::path::Path::new(filename.trim());
    // Reject any path components — only a bare file name is allowed.
    if filename.contains(['/', '\\']) || name.file_name() != Some(name.as_os_str()) {
        return ToolOutcome {
            content: format!("`{filename}` must be a bare file name (no directories)."),
            is_error: true,
        };
    }
    let mut file_name = name.to_string_lossy().to_string();
    if !file_name.to_ascii_lowercase().ends_with(".sls") {
        file_name.push_str(".sls");
    }

    let dir = agent_scripts_dir(state);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return ToolOutcome {
            content: format!("could not create {}: {error}", dir.display()),
            is_error: true,
        };
    }
    let path = dir.join(&file_name);
    let mut body = String::from("# SilicoLab script — generated by the assistant.\n");
    for command in &commands {
        body.push_str(command.trim());
        body.push('\n');
    }
    match std::fs::write(&path, body) {
        Ok(()) => {
            let summary = format!(
                "saved {} ({} command(s)); replay with `run {}`",
                path.display(),
                commands.len(),
                path.display()
            );
            state.output_log.push(summary.clone());
            ToolOutcome {
                content: summary,
                is_error: false,
            }
        }
        Err(error) => ToolOutcome {
            content: format!("could not write {}: {error}", path.display()),
            is_error: true,
        },
    }
}

/// Where agent-authored scripts are written: a `scripts/` dir in the open
/// project, or a temp fallback in a scratch workspace.
fn agent_scripts_dir(state: &AppState) -> std::path::PathBuf {
    state
        .workspace
        .project()
        .map(|project| project.root.join("scripts"))
        .unwrap_or_else(|| std::env::temp_dir().join("silicolab").join("scripts"))
}

/// Run one `.sls` command, echoing it and its result into the shared
/// `output_log` so agent-executed commands share the console's audit trail.
fn run_command_tool(state: &mut AppState, command: &str) -> ToolOutcome {
    state.output_log.push(format!("agent> {command}"));
    match crate::frontend::console::execute_console_line(state, command) {
        Ok(message) => {
            let summary = if message.trim().is_empty() {
                format!("ok: {command}")
            } else {
                message
            };
            state.output_log.push(summary.clone());
            ToolOutcome {
                content: clamp_result(&summary),
                is_error: false,
            }
        }
        Err(error) => {
            let message = format!("command failed: {error}");
            state.output_log.push(message.clone());
            ToolOutcome {
                content: clamp_result(&message),
                is_error: true,
            }
        }
    }
}

/// Truncate a tool result for replay into history (keeps the cached transcript
/// from bloating on large outputs).
pub fn clamp_result(text: &str) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!("{kept}\n… (truncated)")
}

/// Read-only perception over `AppState`: active entry, composition, open
/// entries, engine availability, and the latest status. Never mutates.
pub fn inspect(state: &AppState, _query: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "workspace: {}", state.workspace_label());

    let entries = &state.entries.records;
    let _ = writeln!(out, "open entries: {}", entries.len());

    match state.entries.active_entry() {
        Some(active) => {
            let _ = writeln!(out, "active entry: #{} {}", active.id, active.name);
            let _ = writeln!(
                out,
                "structure: {}",
                crate::frontend::status_text(&active.structure, &state.ui.selection)
            );
            let formula = element_histogram(&active.structure);
            if !formula.is_empty() {
                let _ = writeln!(out, "composition: {formula}");
            }
            if !active.structure.bonds.is_empty() {
                let _ = writeln!(
                    out,
                    "geometry: {}",
                    crate::frontend::bond_geometry_summary(&active.structure)
                );
            }
            if let Some(bio) = &active.structure.biopolymer {
                let _ = writeln!(
                    out,
                    "biopolymer: {} chains, {} residues",
                    bio.chains.len(),
                    bio.residues.len()
                );
            }
            // Provenance: mark MD-run / QM-run outputs and trajectory availability.
            if active.origin.trajectory().is_some() {
                let _ = writeln!(out, "provenance: MD-run output (trajectory available)");
            } else if active.origin.qm_output().is_some() {
                let _ = writeln!(out, "provenance: QM-run output (report saved)");
            }
        }
        None => {
            out.push_str("active entry: none (workspace is empty)\n");
        }
    }

    if entries.len() > 1 {
        // Each entry is listed with its id so the agent can `activate <#id>` a
        // non-active one; the active entry is marked so it is not re-activated.
        let active_id = state.entries.active_entry_id();
        let listed: Vec<String> = entries
            .iter()
            .take(12)
            .map(|entry| {
                let marker = if Some(entry.id) == active_id {
                    " (active)"
                } else {
                    ""
                };
                format!("#{} {}{}", entry.id, entry.name, marker)
            })
            .collect();
        let _ = writeln!(out, "entries: {}", listed.join(", "));
    }

    if let Some(playback) = &state.ui.trajectory {
        let _ = writeln!(
            out,
            "trajectory: playing frame {}/{}",
            playback.current_frame + 1,
            playback.frame_count()
        );
    }

    let registry = EngineRegistry::probe(&state.config.engine_overrides);
    let _ = writeln!(
        out,
        "engines: GROMACS {}",
        if registry.available(EngineId::GROMACS) {
            "available"
        } else {
            "not configured"
        }
    );

    let _ = writeln!(out, "status: {}", state.message);
    out
}

/// A compact element histogram, most-common first (e.g. `C 6, H 12, O 6`).
fn element_histogram(structure: &crate::domain::Structure) -> String {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for atom in &structure.atoms {
        *counts.entry(atom.element.as_str()).or_default() += 1;
    }
    let mut pairs: Vec<(&str, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    pairs
        .into_iter()
        .take(12)
        .map(|(element, count)| format!("{element} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::llm::types::ToolCall;

    fn call(command: &str) -> ToolCall {
        ToolCall {
            id: "t".to_string(),
            name: "run_command".to_string(),
            input: json!({ "command": command }),
        }
    }

    #[test]
    fn read_only_commands_auto_run() {
        assert!(!needs_confirmation(&call("view background white")));
        assert!(!needs_confirmation(&call("color chain A red")));
        assert!(!needs_confirmation(&call("representation cartoon")));
        assert!(!needs_confirmation(&call("open 1abc.pdb")));
    }

    #[test]
    fn destructive_and_expensive_commands_are_gated() {
        assert!(needs_confirmation(&call("delete chain A")));
        assert!(needs_confirmation(&call("save image out.png")));
        assert!(needs_confirmation(&call("md build")));
        assert!(needs_confirmation(&call("qm energy")));
        assert!(needs_confirmation(&call(
            "dock --receptor active --ligand 2"
        )));
        assert!(needs_confirmation(&call(
            "score --receptor active --ligand 2"
        )));
        assert!(needs_confirmation(&call("run setup.sls")));
    }

    #[test]
    fn inspect_is_never_gated() {
        let inspect_call = ToolCall {
            id: "t".to_string(),
            name: "inspect".to_string(),
            input: json!({}),
        };
        assert!(!needs_confirmation(&inspect_call));
    }

    #[test]
    fn recommend_method_tool_is_never_gated() {
        let recommend = ToolCall {
            id: "t".to_string(),
            name: "recommend_method".to_string(),
            input: json!({ "task": "thermochemistry" }),
        };
        assert!(!needs_confirmation(&recommend));
    }

    #[test]
    fn qm_recommend_is_read_only_and_not_gated() {
        // `qm recommend` only prints the level-of-theory table — never gated,
        // even though a real `qm` calculation is.
        assert!(!needs_confirmation(&call("qm recommend thermochemistry")));
        assert!(needs_confirmation(&call("qm energy --method r2scan-3c")));
    }

    #[test]
    fn save_script_is_gated() {
        let call = ToolCall {
            id: "s".to_string(),
            name: "save_script".to_string(),
            input: json!({ "filename": "wf", "commands": ["open x.pdb"] }),
        };
        assert!(needs_confirmation(&call));
    }

    #[test]
    fn save_script_writes_and_rejects_paths() {
        let mut state = AppState::scratch(Default::default(), Vec::new());
        // A bare name writes a .sls file.
        let ok = save_script_tool(
            &mut state,
            &json!({ "filename": "demo", "commands": ["fetch 4hhb", "color hetero"] }),
        );
        assert!(!ok.is_error, "expected success: {}", ok.content);
        assert!(ok.content.contains("demo.sls"));
        // A path with directories is rejected.
        let bad = save_script_tool(
            &mut state,
            &json!({ "filename": "../evil", "commands": [] }),
        );
        assert!(bad.is_error);
    }

    #[test]
    fn clamp_truncates_large_output() {
        let big = "x".repeat(MAX_RESULT_CHARS + 50);
        let clamped = clamp_result(&big);
        assert!(clamped.ends_with("… (truncated)"));
        assert!(clamped.chars().count() < big.chars().count());
    }
}
