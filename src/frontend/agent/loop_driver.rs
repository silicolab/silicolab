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
- `save_script` saves a reusable `.sls` workflow of commands to the project.

Working style:
- Call `inspect` before acting when you are unsure of the current state — never guess \
what is loaded.
- Take one concrete step at a time and keep your prose short; the user sees both your \
text and the commands you run.
- Destructive or expensive commands (delete, save, md, qm, running a script) are gated: \
when you call one, the user is asked to approve it first. Call them anyway when they are \
the right step; explain why briefly.
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
- A `qm energy` result is the electronic energy near 0 K, not an enthalpy. For reaction, \
formation, or combustion energies, run `qm freq` on each species to add zero-point and \
thermal corrections before comparing to an experimental ΔH, and state the method/basis \
caveats (def2-svp is a small basis) in the result.

The full command catalog follows.";

/// The static, cacheable system prompt: persona + the `.sls` command catalog.
/// Holds no volatile per-turn state (that flows through `inspect`), so it caches.
fn system_prompt() -> String {
    format!(
        "{PERSONA}\n\n{}",
        crate::frontend::console::command_catalog()
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
        "save_script" => call
            .input
            .get("filename")
            .and_then(|value| value.as_str())
            .map(|filename| format!("save_script {filename}"))
            .unwrap_or_else(|| "save_script".to_string()),
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
