//! Adapters for locally installed, authenticated agent CLIs.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};

use super::{
    provider::{LlmProvider, ProviderCaps},
    types::{
        AssistantTurn, ChatMessage, ContentBlock, LlmConfig, LlmError, ReasoningBlob, Role,
        StopReason, StreamEvent, ToolCall, ToolDef, Usage,
    },
};
use crate::engines::process::{self, ProcessConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAgentKind {
    Codex,
    Claude,
}

#[derive(Debug, Clone)]
pub struct ExternalAgent {
    kind: ExternalAgentKind,
    executable: Option<PathBuf>,
    access: ExternalAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAccess {
    Controlled,
    Unrestricted,
}

impl ExternalAgent {
    pub fn new(
        kind: ExternalAgentKind,
        executable: Option<String>,
        access: ExternalAccess,
    ) -> Self {
        Self {
            kind,
            executable: executable.map(PathBuf::from),
            access,
        }
    }

    fn executable(&self) -> Result<PathBuf, LlmError> {
        if let Some(path) = &self.executable {
            return Ok(prefer_windows_executable(path));
        }
        let names: &[&str] = match self.kind {
            ExternalAgentKind::Codex => &["codex"],
            ExternalAgentKind::Claude => &["claude"],
        };
        names
            .iter()
            .find_map(|name| process::find_on_path(name).map(|path| prefer_windows_executable(&path)))
            .ok_or_else(|| {
                LlmError::Network(format!(
                    "{} CLI was not found. Install it separately and sign in with its own login flow.",
                    self.label()
                ))
            })
    }

    fn label(&self) -> &'static str {
        match self.kind {
            ExternalAgentKind::Codex => "Codex",
            ExternalAgentKind::Claude => "Claude",
        }
    }

    fn args(&self, cfg: &LlmConfig, schema_path: &str) -> Vec<String> {
        let mut args = match self.kind {
            ExternalAgentKind::Codex => vec![
                "exec".into(),
                "--json".into(),
                "--ephemeral".into(),
                "--output-schema".into(),
                schema_path.into(),
                "-".into(),
            ],
            ExternalAgentKind::Claude => vec![
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                // stream-json output is rejected in print mode without --verbose.
                "--verbose".into(),
                "--include-partial-messages".into(),
                "--no-session-persistence".into(),
                "--json-schema".into(),
                schema_path.into(),
            ],
        };
        if !cfg.model.trim().is_empty() {
            args.extend(["--model".into(), cfg.model.clone()]);
        }
        match (self.kind, self.access) {
            (ExternalAgentKind::Codex, ExternalAccess::Controlled) => {
                args.extend(["--sandbox".into(), "read-only".into()])
            }
            (ExternalAgentKind::Codex, ExternalAccess::Unrestricted) => {
                args.push("--dangerously-bypass-approvals-and-sandbox".into())
            }
            (ExternalAgentKind::Claude, ExternalAccess::Controlled) => {
                args.extend(["--permission-mode".into(), "plan".into()])
            }
            (ExternalAgentKind::Claude, ExternalAccess::Unrestricted) => {
                args.push("--dangerously-skip-permissions".into())
            }
        }
        args
    }
}

impl LlmProvider for ExternalAgent {
    fn complete(
        &self,
        cfg: &LlmConfig,
        tools: &[ToolDef],
        history: &[ChatMessage],
        cancel: &Arc<AtomicBool>,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<AssistantTurn, LlmError> {
        let executable = self.executable()?;
        let schema = external_schema(tools).to_string();
        // Both CLIs take the output schema as a file path, not inline JSON.
        let schema_file = std::env::temp_dir().join(format!(
            "silicolab-agent-schema-{}-{}.json",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&schema_file, &schema).map_err(|e| {
            LlmError::Network(format!(
                "could not prepare {} output schema: {e}",
                self.label()
            ))
        })?;
        let schema_arg = schema_file.to_string_lossy().into_owned();
        let work = cfg
            .working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                LlmError::Network("could not determine external agent working directory".into())
            })?;
        let stdin = render_prompt(cfg, history, tools).into_bytes();
        // Share the turn's cancel flag with the child so a user cancel kills it
        // promptly instead of waiting out the timeout.
        let result = process::spawn_with_cancel(
            ProcessConfig::new(executable, work)
                .args(self.args(cfg, &schema_arg))
                .stdin_bytes(stdin)
                .timeout(Duration::from_secs(30 * 60)),
            Arc::clone(cancel),
        )
        .and_then(process::ProcessHandle::join);
        let _ = std::fs::remove_file(&schema_file);
        let result = result.map_err(|e| LlmError::Network(e.to_string()))?;
        if cancel.load(Ordering::Relaxed) || result.cancelled {
            return Err(LlmError::Cancelled);
        }
        if result.timed_out {
            return Err(LlmError::Network("external agent timed out".into()));
        }
        if result.exit_code != 0 {
            return Err(classify_cli_error(
                &result.stderr,
                result.exit_code,
                self.label(),
            ));
        }
        parse_jsonl(&result.stdout, on_event)
    }

    fn encode_assistant_for_replay(&self, turn: &AssistantTurn) -> ChatMessage {
        let mut content = Vec::new();
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
    fn id(&self) -> &str {
        match self.kind {
            ExternalAgentKind::Codex => "codex-cli",
            ExternalAgentKind::Claude => "claude-cli",
        }
    }
    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            supports_effort: false,
            supports_thinking: false,
            supports_prompt_cache: false,
            supports_streaming: true,
        }
    }
}

fn external_schema(_tools: &[ToolDef]) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "text": {"type": "string"},
            "tool_calls": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "id": {"type": "string"},
                        "name": {"type": "string"},
                        "input": {"type": "string"}
                    },
                    "required": ["id", "name", "input"]
                }
            }
        },
        "required": ["text", "tool_calls"]
    })
}

fn render_prompt(cfg: &LlmConfig, history: &[ChatMessage], tools: &[ToolDef]) -> String {
    let messages: Vec<Value> = history.iter().map(|message| json!({"role": format_role(message.role), "content": message.content.iter().map(render_block).collect::<Vec<_>>() })).collect();
    json!({"system": cfg.system, "messages": messages, "tools": tools.iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.input_schema})).collect::<Vec<_>>()}).to_string()
}

fn format_role(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}
fn render_block(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(text) => json!({"type":"text","text":text}),
        ContentBlock::ToolUse { id, name, input } => {
            json!({"type":"tool_use","id":id,"name":name,"input":input})
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content,"is_error":is_error})
        }
        ContentBlock::OpaqueReasoning(_) => json!({"type":"reasoning"}),
    }
}

fn parse_jsonl(
    stdout: &str,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<AssistantTurn, LlmError> {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut usage = Usage::default();
    let mut final_value = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|e| LlmError::BadRequest(format!("invalid external agent JSONL: {e}")))?;
        if let Some(message) = value
            .get("message")
            .and_then(Value::as_str)
            .filter(|_| value.get("type").and_then(Value::as_str) == Some("error"))
            .or_else(|| {
                value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .filter(|_| value.get("type").and_then(Value::as_str) == Some("turn.failed"))
            })
        {
            return Err(LlmError::BadRequest(message.to_string()));
        }
        collect_usage(&value, &mut usage);
        for delta in text_delta(&value) {
            text.push_str(&delta);
            on_event(StreamEvent::TextDelta(delta));
        }
        if let Some(value) = structured_value(&value) {
            final_value = Some(value);
        }
        collect_tool(&value, &mut calls);
    }
    if let Some(value) = final_value {
        apply_structured(&value, &mut text, &mut calls);
    }
    if text.is_empty() && calls.is_empty() {
        return Err(LlmError::BadRequest(
            "external agent exited without a final result".into(),
        ));
    }
    let stop = if calls.is_empty() {
        StopReason::EndTurn
    } else {
        StopReason::ToolUse
    };
    Ok(AssistantTurn {
        text,
        tool_calls: calls,
        reasoning: ReasoningBlob::None,
        stop,
        usage,
    })
}

fn text_delta(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(text) = value
        .get("delta")
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
    {
        out.push(text.into());
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| matches!(t, "content_block_delta" | "text_delta"))
        && let Some(text) = value.get("text").and_then(Value::as_str)
    {
        out.push(text.into());
    } else if value.get("type").and_then(Value::as_str) == Some("item.completed")
        && let Some(item) = value.get("item")
        && item.get("type").and_then(Value::as_str) == Some("agent_message")
        && let Some(text) = item.get("text").and_then(Value::as_str)
        && serde_json::from_str::<Value>(text).is_err()
    {
        out.push(text.into());
    }
    out
}
fn structured_value(value: &Value) -> Option<Value> {
    ["output", "result", "response", "structured_output"]
        .iter()
        .find_map(|key| value.get(*key).cloned().filter(Value::is_object))
        .or_else(|| {
            value
                .get("item")
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("agent_message"))
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str(text).ok())
        })
}
fn apply_structured(value: &Value, text: &mut String, calls: &mut Vec<ToolCall>) {
    if let Some(s) = value.get("text").and_then(Value::as_str)
        && text.is_empty()
    {
        text.push_str(s);
    }
    collect_tool(value, calls);
}
fn collect_tool(value: &Value, calls: &mut Vec<ToolCall>) {
    if let Some(items) = value.get("tool_calls").and_then(Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("external-{index}"));
                calls.push(ToolCall {
                    id,
                    name: name.into(),
                    input: item
                        .get("input")
                        .and_then(|input| {
                            input
                                .as_str()
                                .and_then(|input| serde_json::from_str(input).ok())
                                .or_else(|| Some(input.clone()))
                        })
                        .unwrap_or_else(|| json!({})),
                });
            }
        }
    }
}
fn collect_usage(value: &Value, usage: &mut Usage) {
    let u = value
        .get("usage")
        .or_else(|| value.get("result").and_then(|v| v.get("usage")));
    if let Some(u) = u {
        usage.input = usage.input.max(number(u, &["input_tokens", "input"]));
        usage.output = usage.output.max(number(u, &["output_tokens", "output"]));
        usage.cache_read = usage.cache_read.max(number(
            u,
            &[
                "cache_read_input_tokens",
                "cache_read",
                "cached_input_tokens",
            ],
        ));
        usage.cache_write = usage.cache_write.max(number(
            u,
            &[
                "cache_creation_input_tokens",
                "cache_write",
                "cache_creation",
            ],
        ));
    }
}
fn number(value: &Value, keys: &[&str]) -> u32 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0) as u32
}
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}
fn classify_cli_error(stderr: &str, code: i32, label: &str) -> LlmError {
    let detail = stderr.trim();
    let lower = detail.to_ascii_lowercase();
    if lower.contains("login") || lower.contains("auth") || lower.contains("unauthorized") {
        LlmError::Auth
    } else {
        LlmError::BadRequest(if detail.is_empty() {
            format!("{label} CLI exited with status {code}")
        } else {
            detail.to_string()
        })
    }
}

fn prefer_windows_executable(path: &std::path::Path) -> PathBuf {
    if !cfg!(windows) || path.extension().is_some() {
        return path.to_path_buf();
    }
    for extension in ["exe", "cmd", "bat", "com"] {
        let candidate = path.with_extension(extension);
        if candidate.is_file() {
            return candidate;
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_structured_output_and_usage() {
        let mut events = Vec::new();
        let turn = parse_jsonl(
            r#"{"type":"text_delta","text":"hi"}
{"usage":{"input_tokens":4,"output_tokens":2}}
{"result":{"text":"ignored","tool_calls":[{"id":"1","name":"inspect","input":{}}]}}"#,
            &mut |event| events.push(event),
        )
        .unwrap();
        assert_eq!(turn.text, "hi");
        assert_eq!(turn.tool_calls[0].name, "inspect");
        assert_eq!(turn.usage.input, 4);
        assert_eq!(events.len(), 1);
    }
    #[test]
    fn captures_cache_tokens_from_usage() {
        let turn = parse_jsonl(
            r#"{"type":"result","usage":{"input_tokens":10,"output_tokens":3,"cache_read_input_tokens":900,"cache_creation_input_tokens":40}}
{"result":{"text":"done","tool_calls":[]}}"#,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.usage.input, 10);
        assert_eq!(turn.usage.cache_read, 900);
        assert_eq!(turn.usage.cache_write, 40);
        // Fresh input is a fraction of what actually got billed — the rest is cache.
        assert_eq!(turn.usage.input_total(), 950);
    }
    #[test]
    fn unknown_events_are_ignored() {
        let turn = parse_jsonl(
            r#"{"type":"future_event"}
{"result":{"text":"done","tool_calls":[]}}"#,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.text, "done");
    }

    #[test]
    fn parses_codex_completed_agent_message() {
        let turn = parse_jsonl(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.text, "hello");
    }
    #[test]
    fn corrupt_json_is_an_error() {
        assert!(matches!(
            parse_jsonl("{", &mut |_| {}),
            Err(LlmError::BadRequest(_))
        ));
    }
}
