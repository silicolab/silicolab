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
    let body = provider.build_request_body(&cfg, &[], &[ChatMessage::user_text("hi")]);
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
    let body = provider.build_request_body(&cfg, &[], &[ChatMessage::user_text("hi")]);
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
    let rendered = message_to_json(&message);
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
    let body = provider.build_request_body(&cfg, &[], &[ChatMessage::user_text("hi")]);
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
    let rendered = message_to_json(&encode_assistant(&empty));
    let blocks = rendered["content"].as_array().unwrap();
    assert!(!blocks.is_empty(), "content array must never be empty");
    assert_eq!(rendered["role"], "assistant");
}
