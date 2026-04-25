use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleClientConfig {
    pub base_url: String,
    pub model_id: String,
    pub connect_timeout: std::time::Duration,
    pub read_timeout: std::time::Duration,
    pub extra_headers: BTreeMap<String, String>,
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenAiCompatibleChatMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleChatRequest {
    pub messages: Vec<OpenAiCompatibleChatMessage>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAiCompatibleStreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    Finished,
}

#[derive(Debug, Deserialize)]
struct ResponseChunk {
    choices: Vec<ResponseChoice>,
}

#[derive(Debug, Deserialize)]
struct ResponseChoice {
    delta: Option<ResponseDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
}

pub fn build_request_body(
    config: &OpenAiCompatibleClientConfig,
    request: &OpenAiCompatibleChatRequest,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model_id,
        "messages": request.messages,
        "stream": true,
        "stream_options": {
            "include_usage": true
        },
    });
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    if let Some(reasoning_effort) = request.reasoning_effort.as_ref() {
        body["reasoning_effort"] = serde_json::json!(reasoning_effort);
    }
    for (key, value) in &config.extra_body {
        body[key] = value.clone();
    }
    body
}

pub fn parse_sse_data_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() || trimmed.starts_with(':') {
        return None;
    }
    let payload = trimmed.strip_prefix("data:")?.trim_start();
    if payload.is_empty() {
        return None;
    }
    Some(payload)
}

pub fn parse_sse_payload(payload: &str) -> Result<Vec<OpenAiCompatibleStreamEvent>, String> {
    if payload == "[DONE]" {
        return Ok(vec![OpenAiCompatibleStreamEvent::Finished]);
    }
    let chunk: ResponseChunk =
        serde_json::from_str(payload).map_err(|error| format!("Malformed SSE payload: {error}"))?;
    let mut events = Vec::new();
    for choice in chunk.choices {
        if let Some(delta) = choice.delta {
            if let Some(content) = delta.content.filter(|fragment| !fragment.is_empty()) {
                events.push(OpenAiCompatibleStreamEvent::TextDelta(content));
            }
            if let Some(content) = delta
                .reasoning_content
                .filter(|fragment| !fragment.is_empty())
            {
                events.push(OpenAiCompatibleStreamEvent::ReasoningDelta(content));
            }
        }
        if choice.finish_reason.is_some() {
            events.push(OpenAiCompatibleStreamEvent::Finished);
        }
    }
    Ok(events)
}

pub fn chat_completions_url(base_url: &str) -> anyhow::Result<String> {
    let parsed = url::Url::parse(base_url.trim().trim_end_matches('/'))
        .map_err(anyhow::Error::msg)
        .context("parse OpenAI-compatible base URL")?;
    Ok(format!(
        "{}/chat/completions",
        parsed.as_str().trim_end_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completions_url_accepts_remote_urls() {
        assert_eq!(
            chat_completions_url("https://example.com/v1").expect("url"),
            "https://example.com/v1/chat/completions"
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
    fn parse_sse_payload_supports_done_marker() {
        assert_eq!(
            parse_sse_payload("[DONE]").expect("done"),
            vec![OpenAiCompatibleStreamEvent::Finished]
        );
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
}
