use std::collections::HashMap;

use super::{AssistantConversation, TranscriptEntry};
use crate::io::llm::types::{ChatMessage, ContentBlock, Role};

impl AssistantConversation {
    pub fn recover_interrupted(&mut self, reason: &str) {
        // Live queues belong to the final batch; providers can reuse IDs across turns.
        let last_batch = self.history.iter().rposition(|message| {
            message.role == Role::Assistant
                && message
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
        });
        let mut collected = result_map(std::mem::take(&mut self.collected_results));
        let mut history = std::mem::take(&mut self.history)
            .into_iter()
            .enumerate()
            .peekable();
        while let Some((index, message)) = history.next() {
            let calls: Vec<_> = if message.role == Role::Assistant {
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            self.history.push(message);
            if calls.is_empty() {
                continue;
            }
            let mut blocks = Vec::new();
            while history.peek().is_some_and(|(_, m)| m.role == Role::Tool) {
                if let Some((_, results)) = history.next() {
                    blocks.extend(results.content);
                }
            }
            let mut results = result_map(blocks);
            let mut content = Vec::new();
            for (id, name) in calls {
                let result = results.remove(&id).or_else(|| {
                    (Some(index) == last_batch)
                        .then(|| collected.remove(&id))
                        .flatten()
                });
                content.push(result.unwrap_or_else(|| {
                    let pending = Some(index) == last_batch
                        && self.pending_calls.iter().any(|call| call.id == id);
                    let explanation = if pending {
                        format!("Not executed: {reason}.")
                    } else {
                        format!("Result unknown: {reason}. Verify the current state before continuing; do not automatically replay this call.")
                    };
                    self.transcript.push(TranscriptEntry::Notice(format!(
                        "{name} ({id}): {explanation}"
                    )));
                    ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: explanation,
                        is_error: true,
                    }
                }));
            }
            self.history.push(ChatMessage {
                role: Role::Tool,
                content,
            });
        }
        // Display entries have no call IDs, so they cannot establish execution status.
        for entry in &mut self.transcript {
            if let TranscriptEntry::Tool {
                result, is_error, ..
            } = entry
                && result.is_none()
            {
                *result = Some(format!(
                    "Interrupted: {reason}. Consult the recorded tool results; verify any unknown execution status before continuing."
                ));
                *is_error = true;
            }
        }
        self.pending_calls.clear();
        self.approved_ids.clear();
        self.approval_inputs = None;
    }
}

fn result_map(blocks: Vec<ContentBlock>) -> HashMap<String, ContentBlock> {
    let mut results = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolResult { tool_use_id, .. } = &block {
            results.entry(tool_use_id.clone()).or_insert(block);
        }
    }
    results
}
