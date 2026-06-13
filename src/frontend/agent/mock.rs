//! A deterministic [`LlmProvider`] that replays a canned script of turns, so the
//! loop and adapters can be exercised without a network. Test-only.

#![cfg(test)]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::io::llm::provider::{LlmProvider, ProviderCaps};
use crate::io::llm::types::{
    AssistantTurn, ChatMessage, ContentBlock, LlmConfig, LlmError, ReasoningBlob, Role,
    StreamEvent, ToolDef,
};

/// Yields the next scripted [`AssistantTurn`] on each `complete` call.
pub struct MockProvider {
    script: Mutex<std::collections::VecDeque<AssistantTurn>>,
}

impl MockProvider {
    pub fn new(turns: Vec<AssistantTurn>) -> Self {
        Self {
            script: Mutex::new(turns.into_iter().collect()),
        }
    }
}

impl LlmProvider for MockProvider {
    fn complete(
        &self,
        _cfg: &LlmConfig,
        _tools: &[ToolDef],
        _history: &[ChatMessage],
        _cancel: &Arc<AtomicBool>,
        _on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<AssistantTurn, LlmError> {
        self.script
            .lock()
            .expect("mock script lock")
            .pop_front()
            .ok_or_else(|| LlmError::BadRequest("mock script exhausted".to_string()))
    }

    fn encode_assistant_for_replay(&self, turn: &AssistantTurn) -> ChatMessage {
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

    fn id(&self) -> &str {
        "mock"
    }

    fn caps(&self) -> ProviderCaps {
        ProviderCaps {
            supports_effort: true,
            supports_thinking: true,
            supports_prompt_cache: true,
            supports_streaming: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::llm::retry::complete_with_retry;
    use crate::io::llm::types::{Effort, StopReason, Usage};

    fn cfg() -> LlmConfig {
        LlmConfig {
            model: "mock".to_string(),
            effort: Effort::High,
            max_output_tokens: 1000,
            stream: false,
            system: "sys".to_string(),
        }
    }

    #[test]
    fn replays_scripted_turns_in_order() {
        let provider = MockProvider::new(vec![
            AssistantTurn {
                text: "first".to_string(),
                tool_calls: Vec::new(),
                reasoning: ReasoningBlob::None,
                stop: StopReason::EndTurn,
                usage: Usage::default(),
            },
            AssistantTurn {
                text: "second".to_string(),
                tool_calls: Vec::new(),
                reasoning: ReasoningBlob::None,
                stop: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut sink = |_event: StreamEvent| {};
        let first = complete_with_retry(&provider, &cfg(), &[], &[], &cancel, &mut sink).unwrap();
        assert_eq!(first.text, "first");
        let second = complete_with_retry(&provider, &cfg(), &[], &[], &cancel, &mut sink).unwrap();
        assert_eq!(second.text, "second");
    }
}
