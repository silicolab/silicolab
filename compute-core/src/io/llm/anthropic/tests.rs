use super::*;

#[test]
fn haiku_disables_effort_and_thinking() {
    let caps = caps_for_model("claude-haiku-4-5");
    assert!(!caps.supports_effort);
    assert!(!caps.supports_thinking);
}

#[test]
fn opus_and_sonnet_46_support_effort() {
    assert!(caps_for_model("claude-opus-4-8").supports_effort);
    assert!(caps_for_model("claude-sonnet-4-6").supports_effort);
}

#[test]
fn sonnet_45_disables_effort() {
    assert!(!caps_for_model("claude-sonnet-4-5").supports_effort);
}

#[test]
fn haiku_request_omits_thinking_and_effort() {
    let provider = AnthropicProvider::new("k".into(), "claude-haiku-4-5".into());
    let cfg = LlmConfig {
        working_dir: None,
        model: "claude-haiku-4-5".into(),
        effort: Effort::High,
        max_output_tokens: 1000,
        stream: false,
        system: "sys".into(),
    };
    let body = provider
        .build_request_body(
            &cfg,
            &[],
            &[ChatMessage::user_text("hi")],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(body.get("thinking").is_none());
    assert!(body.get("output_config").is_none());
    // Sampling params are never sent.
    assert!(body.get("temperature").is_none());
}

#[test]
fn opus_request_sends_adaptive_thinking_and_effort() {
    let provider = AnthropicProvider::new("k".into(), "claude-opus-4-8".into());
    let cfg = LlmConfig {
        working_dir: None,
        model: "claude-opus-4-8".into(),
        effort: Effort::XHigh,
        max_output_tokens: 1000,
        stream: false,
        system: "sys".into(),
    };
    let body = provider
        .build_request_body(
            &cfg,
            &[],
            &[ChatMessage::user_text("hi")],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "xhigh");
    // Cache breakpoints: static system + rolling last message.
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    let last = body["messages"].as_array().unwrap().last().unwrap();
    let last_block = last["content"].as_array().unwrap().last().unwrap();
    assert_eq!(last_block["cache_control"]["type"], "ephemeral");
}

#[test]
fn parses_tool_use_and_thinking() {
    let json = json!({
        "content": [
            { "type": "thinking", "thinking": "hmm", "signature": "sig" },
            { "type": "text", "text": "Opening it." },
            { "type": "tool_use", "id": "t1", "name": "run_command",
              "input": { "command": "open x.pdb" } }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 10, "output_tokens": 5,
                   "cache_read_input_tokens": 3, "cache_creation_input_tokens": 2 }
    });
    let turn = parse_response(&json).unwrap();
    assert_eq!(turn.text, "Opening it.");
    assert_eq!(turn.stop, StopReason::ToolUse);
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "run_command");
    assert_eq!(turn.usage.input, 10);
    assert_eq!(turn.usage.cache_read, 3);
    match turn.reasoning {
        ReasoningBlob::Anthropic(ref blocks) => assert_eq!(blocks.len(), 1),
        _ => panic!("expected anthropic reasoning"),
    }
}

#[test]
fn thinking_replays_before_text() {
    // A tool-using turn with reasoning re-encodes thinking first, so the wire
    // render keeps Anthropic's required ordering.
    let turn = AssistantTurn {
        text: "Doing it".into(),
        tool_calls: vec![ToolCall {
            id: "t1".into(),
            name: "run_command".into(),
            input: json!({ "command": "open x" }),
        }],
        reasoning: ReasoningBlob::Anthropic(vec![json!({ "type": "thinking",
                "thinking": "plan", "signature": "s" })]),
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    };
    let message = encode_assistant(&turn);
    let rendered = message_to_json(&message, &Resolved::default());
    let blocks = rendered["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[2]["type"], "tool_use");
}

#[test]
fn parse_sse_reconstructs_turn_and_streams_text() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":2}}}\n",
        "\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n",
        "\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Open\"}}\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ing.\"}}\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"run_command\"}}\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"open x\\\"}\"}}\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n",
        "data: {\"type\":\"message_stop\"}\n",
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let mut deltas: Vec<String> = Vec::new();
    let mut sink = |event: StreamEvent| {
        if let StreamEvent::TextDelta(text) = event {
            deltas.push(text);
        }
    };
    let turn = parse_sse(std::io::Cursor::new(sse.as_bytes()), &cancel, &mut sink).unwrap();
    assert_eq!(turn.text, "Opening.");
    assert_eq!(deltas, vec!["Open".to_string(), "ing.".to_string()]);
    assert_eq!(turn.stop, StopReason::ToolUse);
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "run_command");
    assert_eq!(turn.tool_calls[0].input["command"], "open x");
    assert_eq!(turn.usage.input, 10);
    assert_eq!(turn.usage.cache_read, 2);
    assert_eq!(turn.usage.output, 7);
}

#[test]
fn streaming_request_sets_stream_flag() {
    let provider = AnthropicProvider::new("k".into(), "claude-sonnet-4-6".into());
    let cfg = LlmConfig {
        working_dir: None,
        model: "claude-sonnet-4-6".into(),
        effort: Effort::High,
        max_output_tokens: 1000,
        stream: true,
        system: "sys".into(),
    };
    let body = provider
        .build_request_body(
            &cfg,
            &[],
            &[ChatMessage::user_text("hi")],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(body["stream"], true);
    assert!(caps_for_model("claude-sonnet-4-6").supports_streaming);
}

#[test]
fn classifies_statuses() {
    assert!(matches!(
        classify_status(429, "{}", Some(Duration::from_secs(2))),
        LlmError::RateLimited {
            retry_after: Some(_)
        }
    ));
    assert!(matches!(
        classify_status(529, "{}", None),
        LlmError::Overloaded
    ));
    assert!(matches!(
        classify_status(503, "{}", None),
        LlmError::Server(503)
    ));
    assert!(matches!(classify_status(401, "{}", None), LlmError::Auth));
    let bad = classify_status(400, r#"{"error":{"message":"bad model"}}"#, None);
    match bad {
        LlmError::BadRequest(message) => assert_eq!(message, "bad model"),
        _ => panic!("expected BadRequest"),
    }
}

#[test]
fn empty_assistant_turn_renders_nonempty_content() {
    // An empty end_turn (no text, tool calls, or reasoning) must not
    // serialize to an empty `content` array — Anthropic 400s on that.
    let empty = AssistantTurn {
        text: String::new(),
        tool_calls: Vec::new(),
        reasoning: ReasoningBlob::Anthropic(Vec::new()),
        stop: StopReason::EndTurn,
        usage: Usage::default(),
    };
    let rendered = message_to_json(&encode_assistant(&empty), &Resolved::default());
    let blocks = rendered["content"].as_array().unwrap();
    assert!(!blocks.is_empty(), "content array must never be empty");
    assert_eq!(rendered["role"], "assistant");
}

#[test]
fn interrupted_exchange_keeps_reasoning_and_pairs_results_before_continuation() {
    let thinking = json!({"type": "thinking", "thinking": "opaque", "signature": "sig"});
    let history = vec![
        ChatMessage::user_text("original goal"),
        ChatMessage {
            role: Role::Assistant,
            content: vec![
                ContentBlock::OpaqueReasoning(ReasoningBlob::Anthropic(vec![thinking.clone()])),
                ContentBlock::ToolUse {
                    id: "started".into(),
                    name: "run_command".into(),
                    input: json!({}),
                },
                ContentBlock::ToolUse {
                    id: "pending".into(),
                    name: "run_command".into(),
                    input: json!({}),
                },
            ],
        },
        ChatMessage {
            role: Role::Tool,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "started".into(),
                    content: "Started job #42".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "pending".into(),
                    content: "Not executed: turn cancelled.".into(),
                    is_error: true,
                },
            ],
        },
        ChatMessage::user_text("continue"),
    ];
    let provider = AnthropicProvider::new("unused".into(), "claude-sonnet-4-6".into());
    let cfg = LlmConfig {
        model: "claude-sonnet-4-6".into(),
        effort: Effort::High,
        max_output_tokens: 1000,
        stream: false,
        system: "system".into(),
        working_dir: None,
    };
    let body = provider
        .build_request_body(&cfg, &[], &history, &AtomicBool::new(false))
        .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["content"][0]["text"], "original goal");
    assert_eq!(messages[1]["content"][0], thinking);
    for (index, id) in ["started", "pending"].iter().enumerate() {
        assert_eq!(messages[1]["content"][index + 1]["id"], *id);
        assert_eq!(messages[2]["content"][index]["tool_use_id"], *id);
    }
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["is_error"], false);
    assert_eq!(messages[2]["content"][1]["is_error"], true);
    assert_eq!(messages[3]["content"][0]["text"], "continue");
}

#[cfg(feature = "pdf")]
mod pdf_documents {
    use super::*;
    use crate::io::pdf::tests::{TempPdf, pdf_bytes};

    fn cfg() -> LlmConfig {
        LlmConfig {
            working_dir: None,
            model: "claude-opus-4-8".into(),
            effort: Effort::High,
            max_output_tokens: 1000,
            stream: false,
            system: "sys".into(),
        }
    }

    fn attached(file: &TempPdf) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: vec![
                ContentBlock::Document(documents::describe(&file.0).unwrap()),
                ContentBlock::Text("which functional?".into()),
            ],
        }
    }

    fn body_for(provider: &AnthropicProvider, history: &[ChatMessage]) -> Value {
        provider
            .build_request_body(&cfg(), &[], history, &AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn document_block_precedes_the_text_and_carries_the_file() {
        let file = TempPdf::new("anthropic", &pdf_bytes(&["B3LYP"]));
        let provider = AnthropicProvider::new("k".into(), "claude-opus-4-8".into());
        let body = body_for(&provider, &[attached(&file)]);
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "document");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "application/pdf");
        assert!(!blocks[0]["source"]["data"].as_str().unwrap().is_empty());
        assert_eq!(blocks[1]["text"], "which functional?");
    }

    #[test]
    fn newest_document_gets_its_own_cache_breakpoint() {
        let file = TempPdf::new("cache", &pdf_bytes(&["B3LYP"]));
        let provider = AnthropicProvider::new("k".into(), "claude-opus-4-8".into());
        let history = [
            attached(&file),
            ChatMessage::user_text("and the basis set?"),
        ];
        let body = body_for(&provider, &history);
        let document = &body["messages"][0]["content"][0];
        assert_eq!(document["cache_control"]["type"], "ephemeral");
        let breakpoints = body.to_string().matches("cache_control").count();
        assert_eq!(breakpoints, 3);
    }

    #[test]
    fn no_document_breakpoint_without_prompt_cache() {
        let file = TempPdf::new("nocache", &pdf_bytes(&["B3LYP"]));
        let mut provider = AnthropicProvider::new("k".into(), "claude-opus-4-8".into());
        provider.caps.supports_prompt_cache = false;
        let body = body_for(&provider, &[attached(&file)]);
        assert!(!body.to_string().contains("cache_control"));
    }

    #[test]
    fn missing_file_replays_as_a_text_placeholder() {
        let file = TempPdf::new("missing", &pdf_bytes(&["B3LYP"]));
        let history = [attached(&file)];
        std::fs::remove_file(&file.0).unwrap();
        let provider = AnthropicProvider::new("k".into(), "claude-opus-4-8".into());
        let body = body_for(&provider, &history);
        let first = &body["messages"][0]["content"][0];
        assert_eq!(first["type"], "text");
        assert!(
            first["text"]
                .as_str()
                .unwrap()
                .contains("no longer available")
        );
    }
}
