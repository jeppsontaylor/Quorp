use super::*;

#[test]
fn chat_completions_url_accepts_remote_urls() {
    assert_eq!(
        chat_completions_url("https://example.com/v1").expect("url"),
        "https://example.com/v1/chat/completions"
    );
}

#[test]
fn normalize_base_url_appends_v1_once() {
    assert_eq!(
        normalize_base_url("https://example.com", true).expect("normalize"),
        "https://example.com/v1"
    );
    assert_eq!(
        normalize_base_url("https://example.com/v1/", true).expect("normalize"),
        "https://example.com/v1"
    );
}

#[test]
fn parse_sse_payload_supports_text_and_reasoning() {
    let payload = r#"{"choices":[{"index":0,"delta":{"content":"hello","reasoning_content":"think"},"finish_reason":null}]}"#;
    assert_eq!(
        parse_sse_payload(payload).expect("parse"),
        vec![
            OpenAiCompatibleStreamEvent::TextDelta("hello".to_string()),
            OpenAiCompatibleStreamEvent::ReasoningDelta("think".to_string()),
        ]
    );
}

#[test]
fn parse_sse_chunk_preserves_metadata_usage_and_payload_hash() {
    let payload = r#"{"id":"chatcmpl-1","model":"qwen","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"completion_tokens_details":{"reasoning_tokens":2},"provider_request_id":"provider-1"}}"#;
    let chunk = parse_sse_chunk(payload).expect("parse");

    assert_eq!(chunk.provider_request_id.as_deref(), Some("chatcmpl-1"));
    assert_eq!(chunk.model_id.as_deref(), Some("qwen"));
    assert_eq!(chunk.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        chunk.events,
        vec![
            OpenAiCompatibleStreamEvent::TextDelta("hello".to_string()),
            OpenAiCompatibleStreamEvent::Finished,
        ]
    );
    let usage = chunk.usage.expect("usage");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.total_tokens, 14);
    assert_eq!(usage.reasoning_tokens, Some(2));
    assert_eq!(usage.provider_request_id.as_deref(), Some("chatcmpl-1"));
    assert_eq!(chunk.raw_payload_sha256.len(), 64);
}

#[test]
fn retry_backoff_prefers_retry_after_and_clamps() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "7".parse().expect("header"));
    assert_eq!(retry_backoff_seconds(&headers, 0), 7);
    headers.insert(reqwest::header::RETRY_AFTER, "999".parse().expect("header"));
    assert_eq!(retry_backoff_seconds(&headers, 0), 120);
    headers.clear();
    assert_eq!(retry_backoff_seconds(&headers, 1), 60);
}

#[test]
fn parse_sse_payload_supports_done_marker() {
    assert_eq!(
        parse_sse_payload("[DONE]").expect("done"),
        vec![OpenAiCompatibleStreamEvent::Finished]
    );
}

#[test]
fn parse_sse_chunk_reports_malformed_payloads() {
    let error = parse_sse_chunk("{not json").expect_err("malformed");
    assert!(error.contains("Malformed SSE payload"));
}

#[test]
fn parse_sse_data_line_ignores_comments_and_blank_lines() {
    assert_eq!(parse_sse_data_line(""), None);
    assert_eq!(parse_sse_data_line(": keep-alive"), None);
    assert_eq!(parse_sse_data_line("data: hello"), Some("hello"));
}

#[test]
fn build_request_body_merges_extra_body_fields() {
    let mut extra_body = serde_json::Map::new();
    extra_body.insert(
        "models".to_string(),
        serde_json::json!(["qwen/qwen3-coder:free", "qwen/qwen2.5-coder:free"]),
    );
    extra_body.insert(
        "provider".to_string(),
        serde_json::json!({ "sort": "throughput" }),
    );
    let body = build_request_body(
        &OpenAiCompatibleClientConfig {
            base_url: "https://example.test/api/v1".to_string(),
            model_id: "qwen/qwen3-coder:free".to_string(),
            connect_timeout: std::time::Duration::from_secs(2),
            read_timeout: std::time::Duration::from_secs(30),
            extra_headers: std::collections::BTreeMap::new(),
            extra_body,
        },
        &OpenAiCompatibleChatRequest {
            messages: vec![OpenAiCompatibleChatMessage {
                role: "user",
                content: "hello".to_string(),
            }],
            max_tokens: Some(64),
            reasoning_effort: None,
        },
        true,
    );
    assert_eq!(
        body["models"],
        serde_json::json!(["qwen/qwen3-coder:free", "qwen/qwen2.5-coder:free"])
    );
    assert_eq!(body["provider"]["sort"], serde_json::json!("throughput"));
    assert_eq!(
        body["stream_options"]["include_usage"],
        serde_json::json!(true)
    );
}

#[test]
fn build_http_client_constructs_successfully() {
    let client = build_http_client(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(2),
    )
    .expect("client");
    drop(client);
}
