//! src/llm の型（MessageRequest / MessageResponse）と ScriptedLlm のテスト
//!
//! 形は Claude API（anthropic-version 2026-10-21）の raw HTTP 例に合わせる。

use serde_json::{Value, json};
use subagent::llm::{
    DEFAULT_MODEL, Effort, LlmClient, LlmError, Message, MessageRequest, MessageResponse,
    OutputConfig, Role, ScriptedLlm, StopReason, ToolDef, UnconfiguredLlm,
};

fn as_json(req: &MessageRequest) -> Value {
    serde_json::to_value(req).expect("request serializes")
}

// 1. 既定の request JSON
#[test]
fn default_request_uses_opus_adaptive_thinking_and_top_level_cache_control() {
    let req = MessageRequest::new(16000, vec![Message::user_text("hello")]);
    let v = as_json(&req);

    assert_eq!(DEFAULT_MODEL, "claude-opus-5-5");
    assert_eq!(v["model"], "claude-opus-5-5");
    assert_eq!(v["max_tokens"], 16000);
    assert_eq!(v["thinking"], json!({"type": "adaptive"}));
    assert_eq!(v["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(
        v["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "hello"}]}])
    );

    let obj = v.as_object().unwrap();
    for forbidden in [
        "temperature",
        "top_p",
        "top_k",
        "tool_choice",
        "stream",
        "system",
        "tools",
        "output_config",
    ] {
        assert!(
            !obj.contains_key(forbidden),
            "default request must not send `{forbidden}`: {v}"
        );
    }
}

#[test]
fn request_with_system_tools_and_output_config_serializes_documented_shapes() {
    let mut req = MessageRequest::new(1024, vec![Message::user_text("q")]);
    req.system = Some("you are a reviewer".into());
    req.output_config = Some(OutputConfig {
        effort: Some(Effort::Low),
        format: None,
    });
    req.tools = vec![
        ToolDef::custom(
            "get_weather",
            "Get current weather for a location",
            json!({
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"]
            }),
        ),
        ToolDef::text_editor(),
        ToolDef::web_search(),
        ToolDef::web_fetch(),
    ];
    let v = as_json(&req);

    assert_eq!(v["system"], "you are a reviewer");
    assert_eq!(v["output_config"], json!({"effort": "low"}));
    assert_eq!(
        v["tools"],
        json!([
            {
                "name": "get_weather",
                "description": "Get current weather for a location",
                "input_schema": {
                    "type": "object",
                    "properties": {"location": {"type": "string"}},
                    "required": ["location"]
                }
            },
            {"type": "text_editor_20250728", "name": "str_replace_based_edit_tool"},
            {"type": "web_search_20260209", "name": "web_search"},
            {"type": "web_fetch_20260209", "name": "web_fetch"}
        ])
    );
    let obj = v.as_object().unwrap();
    for forbidden in ["temperature", "top_p", "top_k", "tool_choice"] {
        assert!(!obj.contains_key(forbidden), "{forbidden} must not be sent");
    }
}

#[test]
fn cache_control_can_be_turned_off() {
    let mut req = MessageRequest::new(10, vec![Message::user_text("x")]);
    req.cache_control = None;
    assert!(
        !as_json(&req)
            .as_object()
            .unwrap()
            .contains_key("cache_control")
    );
}

// 2. 応答の assistant content をそのまま送り返す
fn rich_response() -> Value {
    json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "let me think", "signature": "EqQBCkYIARgCIkB/sig=="},
            {"type": "redacted_thinking", "data": "opaque-bytes"},
            {"type": "server_tool_use", "id": "srvtoolu_01", "name": "web_search",
             "input": {"query": "rust serde"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_01",
             "content": [{"type": "web_search_result", "url": "https://serde.rs/",
                          "title": "Serde", "encrypted_content": "Ep4CCioIBhgC", "page_age": null}]},
            {"type": "text", "text": "Serde is a framework.",
             "citations": [{"type": "web_search_result_location", "url": "https://serde.rs/",
                            "title": "Serde", "encrypted_index": "Eo8BCioI", "cited_text": "Serde is"}]},
            {"type": "tool_use", "id": "toolu_01", "name": "get_weather",
             "input": {"location": "Paris"}},
            {"type": "some_future_block", "z_last": 1, "a_first": {"nested": [1, 2, 3]}}
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {"input_tokens": 10, "output_tokens": 20,
                  "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}
    })
}

#[test]
fn assistant_content_round_trips_verbatim_including_unknown_blocks() {
    let raw = rich_response();
    let resp: MessageResponse = serde_json::from_value(raw.clone()).expect("response parses");
    assert_eq!(resp.stop_reason, StopReason::ToolUse);
    assert_eq!(resp.usage.total(), 37);

    let req = MessageRequest::new(
        100,
        vec![Message::user_text("q"), resp.to_assistant_message()],
    );
    let sent = as_json(&req);
    assert_eq!(sent["messages"][1]["role"], "assistant");
    assert_eq!(sent["messages"][1]["content"], raw["content"]);
    // key order も保つ（preserve_order）: 文字列で比べる
    assert_eq!(
        serde_json::to_string(&sent["messages"][1]["content"]).unwrap(),
        serde_json::to_string(&raw["content"]).unwrap()
    );
}

#[test]
fn response_helpers_read_text_and_client_tool_uses_only() {
    let resp: MessageResponse = serde_json::from_value(rich_response()).unwrap();
    assert_eq!(resp.text(), "Serde is a framework.");
    let uses = resp.tool_uses();
    assert_eq!(uses.len(), 1, "server_tool_use is not a client tool call");
    assert_eq!(uses[0].id, "toolu_01");
    assert_eq!(uses[0].name, "get_weather");
    assert_eq!(uses[0].input, json!({"location": "Paris"}));
}

#[test]
fn tool_result_message_matches_documented_shape() {
    let m = Message::tool_results(vec![
        Message::tool_result_block("toolu_a", "72F", false),
        Message::tool_result_block("toolu_b", "boom", true),
    ]);
    assert_eq!(m.role, Role::User);
    assert_eq!(
        serde_json::to_value(&m).unwrap(),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "72F"},
            {"type": "tool_result", "tool_use_id": "toolu_b", "content": "boom", "is_error": true}
        ]})
    );
}

// 3. 未知の stop_reason は Other
#[test]
fn stop_reasons_parse_and_unknown_becomes_other() {
    let cases = [
        ("end_turn", StopReason::EndTurn),
        ("tool_use", StopReason::ToolUse),
        ("max_tokens", StopReason::MaxTokens),
        ("stop_sequence", StopReason::StopSequence),
        ("pause_turn", StopReason::PauseTurn),
        ("refusal", StopReason::Refusal),
        (
            "model_context_window_exceeded",
            StopReason::Other("model_context_window_exceeded".into()),
        ),
    ];
    for (s, want) in cases {
        let mut v = rich_response();
        v["stop_reason"] = json!(s);
        let resp: MessageResponse = serde_json::from_value(v).unwrap();
        assert_eq!(resp.stop_reason, want, "{s}");
        assert_eq!(serde_json::to_value(&resp.stop_reason).unwrap(), json!(s));
    }
    let mut v = rich_response();
    v["stop_reason"] = Value::Null;
    let resp: MessageResponse = serde_json::from_value(v).unwrap();
    assert_eq!(resp.stop_reason, StopReason::Other(String::new()));
}

#[test]
fn missing_usage_fields_default_to_zero() {
    let resp: MessageResponse = serde_json::from_value(json!({
        "id": "m", "role": "assistant", "model": "claude-opus-5-5",
        "content": [], "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 2}
    }))
    .unwrap();
    assert_eq!(resp.usage.total(), 3);
}

// client / fake
#[tokio::test]
async fn unconfigured_llm_refuses_without_network() {
    let llm = UnconfiguredLlm;
    assert!(!llm.is_configured());
    let req = MessageRequest::new(10, vec![Message::user_text("x")]);
    let err = llm.create_message(&req).await.unwrap_err();
    assert!(matches!(err, LlmError::NotConfigured), "{err:?}");
    let e: subagent::Error = err.into();
    assert_eq!(e.name(), "LlmError");
}

#[tokio::test]
async fn scripted_llm_replays_in_order_and_records_requests() {
    let llm = ScriptedLlm::new(vec![
        Ok(ScriptedLlm::text_response("one")),
        Err(LlmError::Other("scripted failure".into())),
    ]);
    assert!(llm.is_configured());
    let req = MessageRequest::new(10, vec![Message::user_text("x")]);
    let first = llm.create_message(&req).await.unwrap();
    assert_eq!(first.text(), "one");
    assert_eq!(first.stop_reason, StopReason::EndTurn);
    assert!(llm.create_message(&req).await.is_err());
    let exhausted = llm.create_message(&req).await.unwrap_err();
    assert!(format!("{exhausted}").contains("exhausted"), "{exhausted}");
    let recorded = llm.requests();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0]["model"], "claude-opus-5-5");
}
