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
use crate::engines::registry::{EngineRegistry, EngineStatus, external_engine_specs};
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
                commands (`delete`, running a script) always ask. For task control, use \
                `list_jobs` and `cancel_job`; do not guess cancel/stop/kill/abort console \
                commands. Call the right command regardless and explain briefly."
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
                latest status, including a short running-jobs summary. Call this before acting so \
                you know the current state. For detailed task control, use `list_jobs` and \
                `cancel_job`; do not guess cancel/stop/kill/abort commands. Takes no required \
                arguments."
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
            name: "list_jobs".to_string(),
            description: "Read-only task-control view. Lists local, assistant, and remote jobs \
                from the unified job control plane with job id, kind, label, status, stage, \
                backend, and cancel capability. Use this before cancelling a job."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "cancel_job".to_string(),
            description: "Request cancellation for a job id returned by `list_jobs`, using the \
                unified job control plane. Do not invent stop/kill/abort console commands."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Job id from list_jobs, e.g. local:qm, agent:7, or remote:<uuid>."
                    }
                },
                "required": ["id"],
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
            name: "save_skill".to_string(),
            description: "Save a reusable SilicoLab skill so a workflow can be discovered and \
                replayed later. A skill is a named set of `.sls` command steps with a short \
                description and trigger keywords; use `{placeholder}` in a step for a value the \
                user supplies at run time and declare each in `params`. Saved skills surface \
                through `recommend_method` on the next turn. Writes a file, so it may need the \
                user's approval depending on their approval mode."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Kebab-case skill id (letters, digits, dashes), e.g. `dock-a-ligand`."
                    },
                    "description": {
                        "type": "string",
                        "description": "One line: what the skill does and when to use it (<= 200 chars)."
                    },
                    "triggers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Keywords that should surface this skill."
                    },
                    "commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One `.sls` command per element; `{placeholder}` for run-time values."
                    },
                    "params": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "description": "Placeholder name, without braces, referenced as `{name}` in a command."
                                },
                                "required": {
                                    "type": "boolean",
                                    "description": "Whether the caller must supply this placeholder at run time."
                                }
                            },
                            "required": ["name"],
                            "additionalProperties": false
                        },
                        "description": "One entry per `{placeholder}` used in `commands`; every placeholder must be declared here."
                    },
                    "caveats": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional trade-offs or gotchas."
                    }
                },
                "required": ["name", "description", "triggers", "commands"],
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

/// The approval [`RiskLevel`] of a tool call. `save_skill` writes a file
/// (FileWrite); `run_command` defers to the grammar's [`command_risk`] so risk is
/// declared once, next to each command; perception tools are read-only.
pub fn risk_of_call(call: &ToolCall) -> RiskLevel {
    match call.name.as_str() {
        "save_skill" => RiskLevel::FileWrite,
        "cancel_job" => RiskLevel::Destructive,
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
        "list_jobs" => ToolOutcome {
            content: list_jobs_tool(state),
            is_error: false,
        },
        "cancel_job" => match call.input.get("id").and_then(Value::as_str) {
            Some(id) => cancel_job_tool(state, id),
            None => ToolOutcome {
                content: "cancel_job requires an `id` string from list_jobs.".to_string(),
                is_error: true,
            },
        },
        "recommend_method" => {
            let task = call
                .input
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let project_root = state
                .workspace
                .project()
                .map(|project| project.root.clone());
            state.ui.agent.ensure_skills_loaded(project_root);
            ToolOutcome {
                content: crate::skills::recommend(&state.ui.agent.skills, task),
                is_error: false,
            }
        }
        "save_skill" => save_skill_tool(state, &call.input),
        other => ToolOutcome {
            content: format!("Unknown tool `{other}`."),
            is_error: true,
        },
    }
}

fn list_jobs_tool(state: &AppState) -> String {
    let jobs = crate::frontend::jobs::list_controlled_jobs(state);
    if jobs.is_empty() {
        return "[]".to_string();
    }
    let rows = jobs
        .iter()
        .map(|job| {
            json!({
                "id": job.id.token(),
                "kind": job.kind.label(),
                "label": job.label.clone(),
                "status": crate::frontend::jobs::job_status_display(job),
                "stage": job.stage.clone(),
                "backend": job.backend.label(),
                "cancel_capability": job.cancel.label(),
                "can_cancel": job.cancel.can_cancel(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
}

fn cancel_job_tool(state: &mut AppState, id: &str) -> ToolOutcome {
    match crate::frontend::jobs::cancel_job_by_token(state, id) {
        Ok(message) => {
            state.output_log.push(format!("agent cancel_job> {id}"));
            state.output_log.push(message.clone());
            ToolOutcome {
                content: message,
                is_error: false,
            }
        }
        Err(error) => {
            let message = format!("cancel_job failed: {error}");
            state.output_log.push(message.clone());
            ToolOutcome {
                content: message,
                is_error: true,
            }
        }
    }
}

/// Write a skill as `<dir>/<name>/SKILL.md` — the project's `skills/` dir when a
/// project is open, else the user dir (`~/.silicolab/skills/`). Validates before
/// writing and reloads the session's skills so the new one is discoverable.
fn save_skill_tool(state: &mut AppState, input: &Value) -> ToolOutcome {
    let Some(name) = input.get("name").and_then(Value::as_str) else {
        return ToolOutcome {
            content: "save_skill requires a `name`.".to_string(),
            is_error: true,
        };
    };
    let Some(description) = input.get("description").and_then(Value::as_str) else {
        return ToolOutcome {
            content: "save_skill requires a `description`.".to_string(),
            is_error: true,
        };
    };
    let triggers = string_array(input, "triggers");
    let commands = string_array(input, "commands");
    let caveats = string_array(input, "caveats");
    let params = param_array(input, "params");
    if commands.is_empty() {
        return ToolOutcome {
            content: "save_skill requires at least one command.".to_string(),
            is_error: true,
        };
    }

    let md = render_skill_md(name, description, &triggers, &commands, &caveats, &params);

    // Validate by round-tripping through the parser + validator before writing.
    // The write path below is derived from `skill.name` (the parsed/validated
    // value), never the raw `name` input, so a name that only *looks* safe
    // after YAML parsing (e.g. truncated by an unquoted `#` comment) can never
    // steer `dir` outside the skills tree: it either round-trips to the exact
    // raw string and gets checked by `validate_skill`, or it's rejected here.
    let skill = match crate::skills::parse_skill_md(&md, crate::skills::SkillSource::User) {
        Ok(skill) => match crate::skills::validate_skill(&skill) {
            Ok(()) => skill,
            Err(reason) => {
                return ToolOutcome {
                    content: format!("invalid skill: {reason}"),
                    is_error: true,
                };
            }
        },
        Err(reason) => {
            return ToolOutcome {
                content: format!("invalid skill: {reason}"),
                is_error: true,
            };
        }
    };

    let project_root = state
        .workspace
        .project()
        .map(|project| project.root.clone());
    let base = project_root
        .clone()
        .map(|root| root.join("skills"))
        .unwrap_or_else(|| crate::hosts::config_dir().join("skills"));
    let dir = base.join(&skill.name);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return ToolOutcome {
            content: format!("could not create {}: {error}", dir.display()),
            is_error: true,
        };
    }
    let path = dir.join("SKILL.md");
    match std::fs::write(&path, md) {
        Ok(()) => {
            state.ui.agent.reload_skills(project_root);
            let summary = format!(
                "saved skill `{}` to {}; discoverable via recommend_method",
                skill.name,
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

/// Collect a JSON string-array field into a `Vec<String>` (missing → empty).
fn string_array(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A declared `{placeholder}`, as read from the tool's `params` input. Mirrors
/// [`crate::skills::SkillParam`], the shape `render_skill_md` emits and
/// `parse_skill_md` reads back.
struct ToolParam {
    name: String,
    required: bool,
}

/// Collect the JSON `params` array into `Vec<ToolParam>` (missing/malformed
/// entries → skipped; missing key → empty).
fn param_array(input: &Value, key: &str) -> Vec<ToolParam> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let required = item
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(ToolParam { name, required })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Render a `SKILL.md` document from the tool's fields. Every scalar,
/// including `name`, is routed through [`yaml_scalar`] so the value
/// `parse_skill_md` reads back matches the raw input exactly (no YAML
/// comment/special-character truncation can desync the two).
fn render_skill_md(
    name: &str,
    description: &str,
    triggers: &[String],
    commands: &[String],
    caveats: &[String],
    params: &[ToolParam],
) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", yaml_scalar(name)));
    out.push_str(&format!("description: {}\n", yaml_scalar(description)));
    out.push_str("triggers:\n");
    for t in triggers {
        out.push_str(&format!("  - {}\n", yaml_scalar(t)));
    }
    // Saved skills are lookup-only by default: discoverable via recommend_method
    // and triggers, but kept out of the always-on system-prompt manifest so a
    // growing library can't inflate every turn's prompt.
    out.push_str("in_table: false\n");
    if !params.is_empty() {
        out.push_str("params:\n");
        for p in params {
            out.push_str(&format!(
                "  - name: {}\n    required: {}\n",
                yaml_scalar(&p.name),
                p.required
            ));
        }
    }
    out.push_str("command:\n");
    for c in commands {
        out.push_str(&format!("  - {}\n", yaml_scalar(c)));
    }
    if !caveats.is_empty() {
        out.push_str("caveats:\n");
        for c in caveats {
            out.push_str(&format!("  - {}\n", yaml_scalar(c)));
        }
    }
    out.push_str("---\n");
    out.push_str(&format!("# {name}\n\nGenerated by the assistant.\n"));
    out
}

/// Quote a YAML scalar so special characters (`:`, `{`, `#`, quotes, embedded
/// newlines/carriage returns) are safe and round-trip exactly.
fn yaml_scalar(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    format!("\"{escaped}\"")
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
            } else if active.origin.is_qm_run() {
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
    let engines = external_engine_specs()
        .iter()
        .map(|spec| {
            let status = match registry.status(spec.id) {
                Some(EngineStatus::Verified { version, .. }) => {
                    format!("{version} (verified)")
                }
                Some(EngineStatus::Unverified { launch }) => {
                    format!("configured at {}, not verified", launch.display_command())
                }
                _ => "not configured".to_string(),
            };
            format!("{} {status}", spec.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let _ = writeln!(out, "engines: {engines}");

    let running_jobs = crate::frontend::jobs::list_controlled_jobs(state)
        .into_iter()
        .filter(|job| job.status.is_running())
        .collect::<Vec<_>>();
    if running_jobs.is_empty() {
        let _ = writeln!(out, "running jobs: none");
    } else {
        let summary = running_jobs
            .iter()
            .take(5)
            .map(|job| {
                format!(
                    "{} {} ({})",
                    job.id.token(),
                    job.label,
                    job.stage.as_deref().unwrap_or(job.status.label())
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let suffix = if running_jobs.len() > 5 {
            format!("; +{} more", running_jobs.len() - 5)
        } else {
            String::new()
        };
        let _ = writeln!(out, "running jobs: {summary}{suffix}");
    }

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
mod tests;
