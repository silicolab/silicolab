//! Turn orchestration for the agent loop. Driven from the UI thread: the
//! dispatcher's action handlers start/cancel/approve, and
//! [`poll_agent_turn`] (called from `poll_jobs`) drains each finished network
//! turn, runs its tool calls inline, and spawns the next turn — ping-ponging
//! between the worker thread (network) and the UI thread (state-mutating tools)
//! using the existing `JobManager` + channel-poll pattern. No new concurrency
//! primitive.

use std::time::Duration;

use crate::backend::config::save_config;
use crate::frontend::agent::session::TranscriptEntry;
use crate::frontend::state::AppState;
use crate::io::llm::types::{AssistantTurn, ChatMessage, ContentBlock, ReasoningBlob, Role};

mod heavy;
mod settings;
mod tool_batch;
mod turn;

pub use heavy::*;
pub use settings::*;
pub use tool_batch::*;
pub use turn::*;

#[cfg(test)]
mod tests;

/// Hard cap on model turns per user message; on truncation we stop and notice.
const MAX_ITERATIONS: usize = 25;
/// Compact the replayed history once it exceeds this many messages. Generous —
/// even at 1M context a runaway agent session could otherwise grow unbounded.
const MAX_HISTORY_MESSAGES: usize = 240;
/// Target message count to trim down to when compacting.
const TARGET_HISTORY_MESSAGES: usize = 140;
/// Per-turn output budget. Tool-call turns are short; 16k is ample.
const MAX_OUTPUT_TOKENS: u32 = 16_000;
/// How often to re-poll the in-flight turn.
const AGENT_POLL: Duration = Duration::from_millis(120);
/// Display clamp for a transcript line.
const DISPLAY_CLAMP: usize = 600;

const PERSONA: &str = "\
You are SilicoLab's built-in assistant. SilicoLab is a desktop app for molecular \
and materials modeling. You drive it by emitting tool calls — you do not answer in \
the abstract, you act.

Tools:
- `run_command` runs one SilicoLab `.sls` console command (the same line a user types \
in the console). One command per call.
- `inspect` returns a read-only view of the current workspace.
- `list_jobs` returns local, assistant, and remote jobs from the unified job control plane.
- `cancel_job` requests cancellation for a job id returned by `list_jobs`.
- `save_skill` saves a reusable, discoverable skill (a named set of `.sls` command steps) so a \
workflow can be found again via `recommend_method` and replayed with placeholders filled in.

Working style:
- Call `inspect` before acting when you are unsure of the current state — never guess \
what is loaded.
- For task control, use `list_jobs` and `cancel_job`. Do not guess or invent \
cancel/stop/kill/abort console commands.
- Take one concrete step at a time and keep your prose short; the user sees both your \
text and the commands you run.
- Commands are risk-classified (read-only, structure edit, file write, compute, destructive). \
Whether one needs the user's approval depends on their approval mode; destructive commands \
(delete, running a script) always ask. When approval is pending the user sees a card and \
decides. Call the right command regardless and explain briefly. In Plan mode your console \
commands are recorded as proposals, not run — but you can still `inspect` and read state to \
ground the plan.
- Heavy computations (qm energy/optimize/freq, md run/simulate, dock) run in the BACKGROUND. \
The tool returns immediately with a job id and you get control back right away — do NOT wait \
for the result inline. Tell the user it started, then help with something else or stop. When \
the job finishes you receive a `[Background job] … finished` message carrying the result; \
continue the task from there (e.g. run a frequency calculation after an optimization). Only \
one heavy job runs at a time: if you start a second you are told to wait, so let the running \
one finish first.
- If a command fails, read the error in the tool result and recover or ask the user.
- When the task is done, stop and say so in one line.

Working with multiple entries:
- Every command acts on the ACTIVE entry only. `open`/`fetch`/`sketch` each create a NEW \
active entry; to act on a different open entry, switch to it first with \
`activate <#id|name>` (ids and the active marker come from `inspect`). Do NOT re-sketch or \
re-open a structure you already built just to make it active — that leaves duplicates.
- When a task spans several molecules (e.g. a reaction), build them all, then `activate` \
each in turn to run its calculation.

Domain notes:
- Diatomics need explicit SMILES: H₂ is `[H][H]`, O₂ is `O=O` (triplet ground state — set \
`--spin 3`), N₂ is `N#N`. A bare `O` is water, `C` is methane.
- Choosing a calculation method? Consult the method-selection guide below and the \
`recommend_method` tool; for the QM level of theory run `qm recommend <task>`. Don't pick a \
functional/basis blind, and don't recommend a method this app can't run.

The full command catalog and a method-selection guide follow.";

/// The static, cacheable system prompt: persona + the `.sls` command catalog +
/// the always-on method-selection table. Holds no volatile per-turn state (that
/// flows through `inspect`), so it caches; the KB table is built from compiled-in
/// data and is byte-stable across turns.
fn system_prompt(skills: &[crate::skills::Skill]) -> String {
    format!(
        "{PERSONA}\n\n{}\n\n{}",
        crate::frontend::console::command_catalog(),
        crate::skills::skills_manifest(skills)
    )
}

fn notice(state: &mut AppState, message: &str) {
    state
        .ui
        .agent
        .transcript
        .push(TranscriptEntry::Notice(message.to_string()));
}

fn persist(state: &mut AppState) {
    if let Err(error) = save_config(&state.config) {
        state.set_message(format!("Could not save assistant settings: {error}"));
    }
}

/// A one-line description of a tool call for the transcript.
fn describe_call(call: &crate::io::llm::types::ToolCall) -> String {
    match call.name.as_str() {
        "run_command" => call
            .input
            .get("command")
            .and_then(|value| value.as_str())
            .map(|command| command.to_string())
            .unwrap_or_else(|| "run_command".to_string()),
        "inspect" => "inspect".to_string(),
        "list_jobs" => "list_jobs".to_string(),
        "cancel_job" => call
            .input
            .get("id")
            .and_then(|value| value.as_str())
            .map(|id| format!("cancel_job {id}"))
            .unwrap_or_else(|| "cancel_job".to_string()),
        "recommend_method" => call
            .input
            .get("task")
            .and_then(|value| value.as_str())
            .map(|task| format!("recommend_method {task}"))
            .unwrap_or_else(|| "recommend_method".to_string()),
        "save_skill" => call
            .input
            .get("name")
            .and_then(|value| value.as_str())
            .map(|name| format!("save_skill {name}"))
            .unwrap_or_else(|| "save_skill".to_string()),
        other => other.to_string(),
    }
}

fn clamp_display(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= DISPLAY_CLAMP {
        return text.to_string();
    }
    let kept: String = text.chars().take(DISPLAY_CLAMP).collect();
    format!("{kept}…")
}

/// Provider-agnostic assistant encoding used when a provider can't be built
/// (no key, tests). Mirrors the adapters: reasoning first, then text, then
/// tool-use blocks.
fn fallback_encode(turn: &AssistantTurn) -> ChatMessage {
    let mut content: Vec<ContentBlock> = Vec::new();
    if !matches!(turn.reasoning, ReasoningBlob::None) {
        content.push(ContentBlock::OpaqueReasoning(turn.reasoning.clone()));
    }
    if !turn.text.is_empty() {
        content.push(ContentBlock::Text(turn.text.clone()));
    }
    for call in &turn.tool_calls {
        content.push(ContentBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
    ChatMessage {
        role: Role::Assistant,
        content,
    }
}

/// Trim the oldest whole exchanges when the history grows past
/// [`MAX_HISTORY_MESSAGES`], cutting at a genuine user-turn boundary so the kept
/// history stays valid (starts at a user turn; tool_use/tool_result pairs intact).
/// Returns whether anything was trimmed.
fn compact_history(history: &mut Vec<ChatMessage>) -> bool {
    if history.len() <= MAX_HISTORY_MESSAGES {
        return false;
    }
    // Conversation boundaries are genuine user turns (tool results are Role::Tool).
    let user_turns: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::User)
        .map(|(index, _)| index)
        .collect();
    // The earliest boundary whose tail fits the target; else the most recent
    // boundary. Never the very first (index 0) — that would trim nothing.
    let boundary = user_turns
        .iter()
        .copied()
        .find(|&start| history.len() - start <= TARGET_HISTORY_MESSAGES)
        .or_else(|| user_turns.last().copied())
        .unwrap_or(0);
    if boundary == 0 {
        return false;
    }
    history.drain(0..boundary);
    true
}

/// Drop opaque reasoning blocks from every message — done on a provider/model
/// switch, since a different backend ignores foreign reasoning (and is still
/// billed for it) or rejects its shape.
fn strip_reasoning(history: &mut [ChatMessage]) {
    for message in history.iter_mut() {
        message
            .content
            .retain(|block| !matches!(block, ContentBlock::OpaqueReasoning(_)));
    }
}
