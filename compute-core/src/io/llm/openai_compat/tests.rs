use super::*;

fn caps(effort: bool) -> ProviderCaps {
    ProviderCaps {
        supports_effort: effort,
        supports_thinking: effort,
        supports_prompt_cache: false,
        supports_streaming: false,
        supports_pdf_input: false,
    }
}

#[test]
fn non_json_200_points_at_the_base_url() {
    // The duckcoding/"New API" symptom: a base URL missing /v1 hits the web
    // UI, which answers 200 with an HTML page.
    let html = non_json_response_message("<!doctype html><html><head><title>New API</title>");
    assert!(html.contains("HTML"), "should name HTML: {html}");
    assert!(html.contains("/v1"), "should hint the API root: {html}");

    // An empty 200 body gets its own clear message.
    let empty = non_json_response_message("   \n");
    assert!(empty.contains("empty"), "should say empty: {empty}");
    assert!(
        empty.contains("Base URL"),
        "should point at Base URL: {empty}"
    );

    // Any other non-JSON 200 still points at the Base URL and shows a snippet
    // instead of a raw parser offset.
    let other = non_json_response_message("upstream timeout");
    assert!(
        other.contains("Base URL"),
        "should point at Base URL: {other}"
    );
    assert!(
        other.contains("upstream timeout"),
        "should echo the body: {other}"
    );
    assert!(
        !other.contains("line 1 column 1"),
        "should not leak the parser offset: {other}"
    );
}

#[test]
fn endpoint_joins_base_url() {
    let provider = OpenAiCompatProvider::new(
        "k".into(),
        "https://api.deepseek.com".into(),
        "deepseek-chat".into(),
        caps(false),
        true,
        "deepseek",
    );
    assert_eq!(
        provider.endpoint(),
        "https://api.deepseek.com/chat/completions"
    );
}

#[test]
fn tools_use_function_envelope_and_effort_is_gated() {
    let provider = OpenAiCompatProvider::new(
        "k".into(),
        "https://api.openai.com/v1".into(),
        "o4".into(),
        caps(true),
        false,
        "openai",
    );
    let cfg = LlmConfig {
        working_dir: None,
        model: "o4".into(),
        effort: Effort::Medium,
        max_output_tokens: 1000,
        stream: false,
        system: "sys".into(),
    };
    let tool = ToolDef {
        name: "run_command".into(),
        description: "run".into(),
        input_schema: json!({ "type": "object" }),
    };
    let body = provider
        .build_request_body(
            &cfg,
            std::slice::from_ref(&tool),
            &[],
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "run_command");
    assert_eq!(body["reasoning_effort"], "medium");
    // System prompt is the first message.
    assert_eq!(body["messages"][0]["role"], "system");
}

#[test]
fn non_reasoning_model_omits_effort() {
    let provider = OpenAiCompatProvider::new(
        "k".into(),
        "https://x".into(),
        "m".into(),
        caps(false),
        false,
        "local",
    );
    let cfg = LlmConfig {
        working_dir: None,
        model: "m".into(),
        effort: Effort::High,
        max_output_tokens: 100,
        stream: false,
        system: "s".into(),
    };
    let body = provider
        .build_request_body(&cfg, &[], &[], &AtomicBool::new(false))
        .unwrap();
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn tool_result_message_expands_to_tool_role_messages() {
    let provider = OpenAiCompatProvider::new(
        "k".into(),
        "https://x".into(),
        "m".into(),
        caps(false),
        false,
        "local",
    );
    let history = vec![ChatMessage {
        role: Role::Tool,
        content: vec![
            ContentBlock::ToolResult {
                tool_use_id: "a".into(),
                content: "first".into(),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "b".into(),
                content: "second".into(),
                is_error: true,
            },
        ],
    }];
    let cfg = LlmConfig {
        working_dir: None,
        model: "m".into(),
        effort: Effort::Low,
        max_output_tokens: 100,
        stream: false,
        system: "s".into(),
    };
    let body = provider
        .build_request_body(&cfg, &[], &history, &AtomicBool::new(false))
        .unwrap();
    let messages = body["messages"].as_array().unwrap();
    // system + two tool messages.
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "a");
    assert_eq!(messages[2]["tool_call_id"], "b");
}

#[test]
fn parses_tool_call_with_string_arguments() {
    let json = json!({
        "choices": [{
            "message": {
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "run_command",
                                  "arguments": "{\"command\":\"open x.pdb\"}" }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 3,
                   "prompt_tokens_details": { "cached_tokens": 5 } }
    });
    let turn = parse_response(&json).unwrap();
    assert_eq!(turn.stop, StopReason::ToolUse);
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].input["command"], "open x.pdb");
    assert_eq!(turn.usage.input, 12);
    assert_eq!(turn.usage.cache_read, 5);
}

#[test]
fn deepseek_reasoning_replays_when_enabled() {
    let turn = AssistantTurn {
        text: "ok".into(),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "run_command".into(),
            input: json!({ "command": "open x" }),
        }],
        reasoning: ReasoningBlob::OpenAiCompat {
            reasoning_content: Some("thinking…".into()),
        },
        stop: StopReason::ToolUse,
        usage: Usage::default(),
    };
    let message = encode_assistant(&turn);
    let rendered = assistant_to_json(&message, true);
    assert_eq!(rendered["reasoning_content"], "thinking…");
    assert_eq!(rendered["tool_calls"][0]["function"]["name"], "run_command");
    // The arguments must be a JSON string on the wire.
    assert!(rendered["tool_calls"][0]["function"]["arguments"].is_string());

    // With replay disabled, reasoning_content is omitted.
    let without = assistant_to_json(&message, false);
    assert!(without.get("reasoning_content").is_none());
}
#[test]
fn interrupted_exchange_keeps_reasoning_and_pairs_results_before_continuation() {
    let history = vec![
        ChatMessage::user_text("original goal"),
        ChatMessage {
            role: Role::Assistant,
            content: vec![
                ContentBlock::OpaqueReasoning(ReasoningBlob::OpenAiCompat {
                    reasoning_content: Some("opaque".into()),
                }),
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
    let mut messages = Vec::new();
    for message in &history {
        append_messages(message, true, &Resolved::default(), &mut messages);
    }
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["content"], "original goal");
    assert_eq!(messages[1]["reasoning_content"], "opaque");
    for (index, id) in ["started", "pending"].iter().enumerate() {
        assert_eq!(messages[1]["tool_calls"][index]["id"], *id);
        assert_eq!(messages[index + 2]["role"], "tool");
        assert_eq!(messages[index + 2]["tool_call_id"], *id);
    }
    assert_eq!(messages[2]["content"], "Started job #42");
    assert_eq!(messages[3]["content"], "Not executed: turn cancelled.");
    assert_eq!(messages[4]["content"], "continue");
}

#[cfg(feature = "pdf")]
mod pdf_documents {
    use super::*;
    use crate::io::pdf::tests::{TempPdf, pdf_bytes};

    fn provider(pdf_input: bool) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(
            "k".into(),
            "https://x".into(),
            "m".into(),
            ProviderCaps {
                supports_pdf_input: pdf_input,
                ..caps(false)
            },
            false,
            "openai",
        )
    }

    fn user_content(pdf_input: bool, file: &TempPdf) -> Value {
        let cfg = LlmConfig {
            working_dir: None,
            model: "m".into(),
            effort: Effort::High,
            max_output_tokens: 100,
            stream: false,
            system: "s".into(),
        };
        let history = [ChatMessage {
            role: Role::User,
            content: vec![
                ContentBlock::Document(documents::describe(&file.0).unwrap()),
                ContentBlock::Text("which basis?".into()),
            ],
        }];
        let body = provider(pdf_input)
            .build_request_body(&cfg, &[], &history, &AtomicBool::new(false))
            .unwrap();
        body["messages"][1]["content"].clone()
    }

    #[test]
    fn native_cap_sends_a_file_part_before_the_text() {
        let file = TempPdf::new("openai", &pdf_bytes(&["def2-TZVP"]));
        let content = user_content(true, &file);
        assert_eq!(content[0]["type"], "file");
        let data = content[0]["file"]["file_data"].as_str().unwrap();
        assert!(data.starts_with("data:application/pdf;base64,"));
        assert!(
            content[0]["file"]["filename"]
                .as_str()
                .unwrap()
                .ends_with(".pdf")
        );
        assert_eq!(content[1]["text"], "which basis?");
    }

    #[test]
    fn without_the_cap_content_stays_a_string_of_extracted_text() {
        let file = TempPdf::new("fallback", &pdf_bytes(&["def2-TZVP"]));
        let content = user_content(false, &file);
        let text = content.as_str().expect("content stays a plain string");
        assert!(text.contains("def2-TZVP"));
        assert!(text.ends_with("which basis?"));
    }
}
