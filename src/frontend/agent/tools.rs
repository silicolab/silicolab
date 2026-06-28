//! The agent's tool surface and its dispatch over `AppState`.
//!
//! Two high-leverage tools: `run_command` (executes one `.sls` line through the
//! same [`execute_console_line`](crate::frontend::console::execute_console_line)
//! a human types, so it exposes the entire command surface and auto-syncs as
//! commands are added) and `inspect` (read-only perception, so the agent does
//! not act blind). Confirmation gating classifies destructive/expensive commands
//! that need a one-click approval before they run.

use std::collections::HashSet;
use std::fmt::Write;

use serde_json::{Value, json};

use crate::backend::config::ApprovalMode;
use crate::engines::registry::{EngineId, EngineRegistry};
use crate::frontend::console::{RiskLevel, command_risk};
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
                command line a user types in the console. Commands are risk-classified \
                (read-only, structure edit, file write, compute, destructive); whether one \
                needs the user's approval depends on their approval mode, and destructive \
                commands (`delete`, running a script) always ask. Call the right command \
                regardless and explain briefly."
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
                sequence of steps you ran. Writes a file, so it may need the user's approval \
                depending on their approval mode."
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

/// The approval [`RiskLevel`] of a tool call. `save_script` writes a file
/// (FileWrite); `run_command` defers to the grammar's [`command_risk`] so risk is
/// declared once, next to each command; perception tools are read-only.
pub fn risk_of_call(call: &ToolCall) -> RiskLevel {
    match call.name.as_str() {
        "save_script" => RiskLevel::FileWrite,
        "run_command" => {
            let command = call
                .input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            command_risk(command)
        }
        _ => RiskLevel::ReadOnly,
    }
}

/// The allow-set key for a call: a `run_command`'s top-level verb, else the tool
/// name. "Always allow this command" remembers this key for the conversation.
pub fn call_allow_key(call: &ToolCall) -> String {
    match call.name.as_str() {
        "run_command" => call
            .input
            .get("command")
            .and_then(Value::as_str)
            .and_then(|command| command.split_whitespace().next())
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
    }
}

/// Whether a tool call must be confirmed, given the active [`ApprovalMode`] and
/// the conversation's allow-set. `Destructive` always confirms — the
/// non-bypassable floor, checked before mode and allow-set.
pub fn needs_confirmation(
    call: &ToolCall,
    mode: ApprovalMode,
    allowed_verbs: &HashSet<String>,
    allowed_risks: &HashSet<RiskLevel>,
) -> bool {
    let risk = risk_of_call(call);
    if risk == RiskLevel::Destructive {
        return true;
    }
    if allowed_risks.contains(&risk) || allowed_verbs.contains(&call_allow_key(call)) {
        return false;
    }
    match mode {
        ApprovalMode::Manual => risk != RiskLevel::ReadOnly,
        ApprovalMode::AutoSafe => matches!(risk, RiskLevel::FileWrite | RiskLevel::Expensive),
        // Auto and Plan return false here only because Destructive is handled
        // above and Plan never reaches execution (the batch driver short-circuits).
        ApprovalMode::Auto | ApprovalMode::Plan => false,
    }
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

    fn save_script_call() -> ToolCall {
        ToolCall {
            id: "s".to_string(),
            name: "save_script".to_string(),
            input: json!({ "filename": "wf", "commands": ["open x.pdb"] }),
        }
    }

    /// Gate with the default mode (AutoSafe) and an empty allow-set.
    fn gated(command: &str) -> bool {
        needs_confirmation(
            &call(command),
            ApprovalMode::AutoSafe,
            &HashSet::new(),
            &HashSet::new(),
        )
    }

    #[test]
    fn risk_classification_matches_the_grammar() {
        assert_eq!(
            risk_of_call(&call("view background white")),
            RiskLevel::ReadOnly
        );
        assert_eq!(
            risk_of_call(&call("color chain A red")),
            RiskLevel::ReadOnly
        );
        assert_eq!(risk_of_call(&call("open 1abc.pdb")), RiskLevel::ReadOnly);
        assert_eq!(risk_of_call(&call("hydrogen add")), RiskLevel::ReadOnly);
        assert_eq!(
            risk_of_call(&call("qm recommend thermochemistry")),
            RiskLevel::ReadOnly
        );
        assert_eq!(
            risk_of_call(&call("phosphorylate --protein active --at A:1")),
            RiskLevel::Mutating
        );
        assert_eq!(
            risk_of_call(&call("save image out.png")),
            RiskLevel::FileWrite
        );
        assert_eq!(risk_of_call(&save_script_call()), RiskLevel::FileWrite);
        assert_eq!(risk_of_call(&call("md build")), RiskLevel::Expensive);
        assert_eq!(
            risk_of_call(&call("qm energy --method r2scan-3c")),
            RiskLevel::Expensive
        );
        assert_eq!(
            risk_of_call(&call("dock --receptor active --ligand 2")),
            RiskLevel::Expensive
        );
        assert_eq!(
            risk_of_call(&call("score --receptor active")),
            RiskLevel::Expensive
        );
        assert_eq!(risk_of_call(&call("run setup.sls")), RiskLevel::Destructive);
        // A bare `*.sls` token runs as a script via the console's pre-clap
        // shortcut, so it must classify Destructive like an explicit `run`.
        assert_eq!(risk_of_call(&call("setup.sls")), RiskLevel::Destructive);
        assert_eq!(
            risk_of_call(&call("delete chain A")),
            RiskLevel::Destructive
        );
    }

    #[test]
    fn structure_editing_commands_are_never_read_only() {
        // The silent-bypass bug: PTM / structure-editing verbs must not auto-run
        // as if read-only. Each must be Mutating; `delete` Destructive.
        for command in [
            "phosphorylate --protein active --at A:84",
            "acetylate --protein active --at A:120",
            "methylate --protein active --at A:9",
            "lipidate --protein active --at A:3",
            "ubiquitinate --protein active --at A:48",
            "glycosylate --protein active",
            "glycan GlcNAc",
        ] {
            assert_eq!(
                risk_of_call(&call(command)),
                RiskLevel::Mutating,
                "{command}"
            );
            assert!(
                gated_in(command, ApprovalMode::Manual),
                "{command} must gate in Manual"
            );
        }
        assert_eq!(
            risk_of_call(&call("save image out.png")),
            RiskLevel::FileWrite
        );
        assert!(gated_in("save image out.png", ApprovalMode::Manual));
        assert_eq!(
            risk_of_call(&call("delete chain A")),
            RiskLevel::Destructive
        );
    }

    fn gated_in(command: &str, mode: ApprovalMode) -> bool {
        needs_confirmation(&call(command), mode, &HashSet::new(), &HashSet::new())
    }

    #[test]
    fn autosafe_runs_edits_but_gates_writes_compute_and_destructive() {
        assert!(!gated("view background white"));
        assert!(!gated("phosphorylate --protein active --at A:1"));
        assert!(gated("save image out.png"));
        assert!(gated("qm energy"));
        assert!(gated("delete chain A"));
        assert!(needs_confirmation(
            &save_script_call(),
            ApprovalMode::AutoSafe,
            &HashSet::new(),
            &HashSet::new(),
        ));
    }

    #[test]
    fn manual_gates_everything_but_read_only() {
        assert!(!gated_in("view background white", ApprovalMode::Manual));
        assert!(gated_in("save image out.png", ApprovalMode::Manual));
        assert!(gated_in("qm energy", ApprovalMode::Manual));
        assert!(gated_in("delete chain A", ApprovalMode::Manual));
    }

    #[test]
    fn auto_runs_everything_except_destructive() {
        assert!(!gated_in("qm energy", ApprovalMode::Auto));
        assert!(!gated_in("save image out.png", ApprovalMode::Auto));
        assert!(gated_in("delete chain A", ApprovalMode::Auto));
    }

    #[test]
    fn destructive_always_confirms_and_allow_set_cannot_bypass_it() {
        let mut verbs = HashSet::new();
        verbs.insert("delete".to_string());
        let mut risks = HashSet::new();
        risks.insert(RiskLevel::Destructive);
        for mode in ApprovalMode::all() {
            if mode == ApprovalMode::Plan {
                continue; // Plan never executes; the gate is not consulted
            }
            for command in ["delete chain A", "run setup.sls", "setup.sls"] {
                assert!(
                    needs_confirmation(&call(command), mode, &verbs, &risks),
                    "{command} must confirm in {mode:?} even with an allow-set"
                );
            }
        }
    }

    #[test]
    fn allow_set_suppresses_gating_for_verb_and_risk() {
        // Allowing the `qm` verb auto-runs qm in AutoSafe.
        let mut verbs = HashSet::new();
        verbs.insert("qm".to_string());
        assert!(!needs_confirmation(
            &call("qm energy"),
            ApprovalMode::AutoSafe,
            &verbs,
            &HashSet::new()
        ));
        // Allowing the whole Expensive level auto-runs dock too.
        let mut risks = HashSet::new();
        risks.insert(RiskLevel::Expensive);
        assert!(!needs_confirmation(
            &call("dock --receptor active"),
            ApprovalMode::AutoSafe,
            &HashSet::new(),
            &risks,
        ));
    }

    #[test]
    fn unparseable_command_is_read_only_so_it_runs_and_self_reports_the_error() {
        // An invalid command reaches no effect (it errors before dispatch), so
        // gating it would only nag the user; let it run and fail instead.
        assert_eq!(risk_of_call(&call("inspect")), RiskLevel::ReadOnly);
        assert!(!gated("frobnicate the molecule"));
    }

    #[test]
    fn perception_tools_are_never_gated() {
        let inspect_call = ToolCall {
            id: "t".to_string(),
            name: "inspect".to_string(),
            input: json!({}),
        };
        let recommend = ToolCall {
            id: "t".to_string(),
            name: "recommend_method".to_string(),
            input: json!({ "task": "thermochemistry" }),
        };
        for c in [&inspect_call, &recommend] {
            assert_eq!(risk_of_call(c), RiskLevel::ReadOnly);
            assert!(!needs_confirmation(
                c,
                ApprovalMode::Manual,
                &HashSet::new(),
                &HashSet::new()
            ));
        }
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
