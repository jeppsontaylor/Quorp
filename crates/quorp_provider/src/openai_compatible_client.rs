use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCompatibleUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub provider_request_id: Option<String>,
}

impl OpenAiCompatibleUsage {
    pub fn from_payload(payload: &serde_json::Value, provider_request_id: Option<&str>) -> Self {
        let usage_u64 = |paths: &[&[&str]]| {
            paths.iter().find_map(|path| {
                let mut current = payload;
                for key in *path {
                    current = current.get(*key)?;
                }
                current
                    .as_u64()
                    .or_else(|| current.as_i64().map(|value| value.max(0) as u64))
                    .or_else(|| current.as_str().and_then(|value| value.parse::<u64>().ok()))
            })
        };
        let input_tokens = usage_u64(&[&["prompt_tokens"], &["input_tokens"]]).unwrap_or_default();
        let output_tokens =
            usage_u64(&[&["completion_tokens"], &["output_tokens"]]).unwrap_or_default();
        let total_tokens = usage_u64(&[&["total_tokens"]])
            .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            reasoning_tokens: usage_u64(&[
                &["reasoning_tokens"],
                &["output_tokens_details", "reasoning_tokens"],
                &["completion_tokens_details", "reasoning_tokens"],
            ]),
            cache_read_input_tokens: usage_u64(&[
                &["cache_read_input_tokens"],
                &["input_tokens_details", "cached_tokens"],
                &["prompt_tokens_details", "cached_tokens"],
            ]),
            cache_write_input_tokens: usage_u64(&[
                &["cache_write_input_tokens"],
                &["input_tokens_details", "cache_write_tokens"],
                &["prompt_tokens_details", "cache_write_tokens"],
            ]),
            provider_request_id: provider_request_id.map(str::to_string).or_else(|| {
                payload
                    .get("provider_request_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleStreamChunk {
    pub events: Vec<OpenAiCompatibleStreamEvent>,
    pub provider_request_id: Option<String>,
    pub model_id: Option<String>,
    pub finish_reason: Option<String>,
    pub usage: Option<OpenAiCompatibleUsage>,
    pub raw_payload_sha256: String,
}

#[derive(Debug, Deserialize)]
struct ResponseChunk {
    id: Option<String>,
    model: Option<String>,
    choices: Vec<ResponseChoice>,
    usage: Option<serde_json::Value>,
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
    parse_sse_chunk(payload).map(|chunk| chunk.events)
}

pub fn parse_sse_chunk(payload: &str) -> Result<OpenAiCompatibleStreamChunk, String> {
    let raw_payload_sha256 = sha256_hex(payload.as_bytes());
    if payload == "[DONE]" {
        return Ok(OpenAiCompatibleStreamChunk {
            events: vec![OpenAiCompatibleStreamEvent::Finished],
            provider_request_id: None,
            model_id: None,
            finish_reason: None,
            usage: None,
            raw_payload_sha256,
        });
    }
    let chunk: ResponseChunk =
        serde_json::from_str(payload).map_err(|error| format!("Malformed SSE payload: {error}"))?;
    let mut events = Vec::new();
    let mut finish_reason = None;
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
        if let Some(reason) = choice.finish_reason {
            finish_reason = Some(reason);
            events.push(OpenAiCompatibleStreamEvent::Finished);
        }
    }
    let usage = chunk
        .usage
        .as_ref()
        .map(|usage| OpenAiCompatibleUsage::from_payload(usage, chunk.id.as_deref()));
    Ok(OpenAiCompatibleStreamChunk {
        events,
        provider_request_id: chunk.id,
        model_id: chunk.model,
        finish_reason,
        usage,
        raw_payload_sha256,
    })
}

#[allow(dead_code)]
pub fn chat_completions_url(base_url: &str) -> anyhow::Result<String> {
    let parsed = url::Url::parse(base_url.trim().trim_end_matches('/'))
        .map_err(anyhow::Error::msg)
        .context("parse OpenAI-compatible base URL")?;
    Ok(format!(
        "{}/chat/completions",
        parsed.as_str().trim_end_matches('/')
    ))
}

pub fn normalize_base_url(base_url: &str, append_v1: bool) -> anyhow::Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("base URL cannot be empty");
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parse OpenAI-compatible base URL `{trimmed}`"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => anyhow::bail!("unsupported OpenAI-compatible base URL scheme `{scheme}`"),
    }
    if append_v1 && !parsed.path().ends_with("/v1") {
        Ok(format!("{trimmed}/v1"))
    } else {
        Ok(trimmed.to_string())
    }
}

pub fn parse_retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

pub fn retry_backoff_seconds(headers: &reqwest::header::HeaderMap, attempt_index: u64) -> u64 {
    parse_retry_after_seconds(headers)
        .unwrap_or_else(|| 30_u64.saturating_mul(attempt_index.saturating_add(1)))
        .clamp(1, 120)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
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
