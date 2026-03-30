use anyhow::Context as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeClientConfig {
    pub base_url: String,
    pub model_id: String,
    pub connect_timeout: std::time::Duration,
    pub read_timeout: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsdMoeChatMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdMoeChatRequest {
    pub messages: Vec<SsdMoeChatMessage>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsdMoeStreamEvent {
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

pub fn default_local_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

pub fn validate_loopback_base_url(base_url: &str) -> Result<url::Url, String> {
    let normalized = base_url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(normalized).map_err(|error| format!("Invalid SSD-MOE base URL: {error}"))?;
    if parsed.scheme() != "http" {
        return Err("SSD-MOE base URL must use http:// on loopback.".to_string());
    }
    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    };
    if !is_loopback {
        return Err("SSD-MOE base URL must stay on localhost or a loopback IP.".to_string());
    }
    Ok(parsed)
}

pub fn local_bearer_token(base_url: &str) -> Result<String, String> {
    validate_loopback_base_url(base_url)?;
    Ok("local".to_string())
}

pub fn build_request_body(
    config: &SsdMoeClientConfig,
    request: &SsdMoeChatRequest,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model_id,
        "messages": request.messages,
        "stream": true,
    });
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    if let Some(reasoning_effort) = request.reasoning_effort.as_ref() {
        body["reasoning_effort"] = serde_json::json!(reasoning_effort);
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

pub fn parse_sse_payload(payload: &str) -> Result<Vec<SsdMoeStreamEvent>, String> {
    if payload == "[DONE]" {
        return Ok(vec![SsdMoeStreamEvent::Finished]);
    }
    let chunk: ResponseChunk =
        serde_json::from_str(payload).map_err(|error| format!("Malformed SSE payload: {error}"))?;
    let mut events = Vec::new();
    for choice in chunk.choices {
        if let Some(delta) = choice.delta {
            if let Some(content) = delta.content.filter(|fragment| !fragment.is_empty()) {
                events.push(SsdMoeStreamEvent::TextDelta(content));
            }
            if let Some(content) = delta
                .reasoning_content
                .filter(|fragment| !fragment.is_empty())
            {
                events.push(SsdMoeStreamEvent::ReasoningDelta(content));
            }
        }
        if choice.finish_reason.is_some() {
            events.push(SsdMoeStreamEvent::Finished);
        }
    }
    Ok(events)
}

pub fn chat_completions_url(base_url: &str) -> anyhow::Result<String> {
    let parsed = validate_loopback_base_url(base_url)
        .map_err(anyhow::Error::msg)
        .context("validate SSD-MOE base URL")?;
    Ok(format!(
        "{}/chat/completions",
        parsed.as_str().trim_end_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_validation_allows_local_hosts() {
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:8080/v1",
            "http://[::1]:8080/v1",
        ] {
            validate_loopback_base_url(url).expect("loopback URL should pass");
        }
    }

    #[test]
    fn loopback_validation_rejects_remote_hosts() {
        let error = validate_loopback_base_url("http://example.com/v1").expect_err("remote host");
        assert!(error.contains("loopback"));
    }

    #[test]
    fn parse_sse_payload_supports_flash_moe_text_and_reasoning() {
        let payload = r#"{"choices":[{"index":0,"delta":{"content":"hello","reasoning_content":"think"},"finish_reason":null}]}"#;
        assert_eq!(
            parse_sse_payload(payload).expect("parse"),
            vec![
                SsdMoeStreamEvent::TextDelta("hello".to_string()),
                SsdMoeStreamEvent::ReasoningDelta("think".to_string()),
            ]
        );
    }

    #[test]
    fn parse_sse_payload_supports_done_marker() {
        assert_eq!(
            parse_sse_payload("[DONE]").expect("done"),
            vec![SsdMoeStreamEvent::Finished]
        );
    }

    #[test]
    fn parse_sse_data_line_ignores_comments_and_blank_lines() {
        assert_eq!(parse_sse_data_line(""), None);
        assert_eq!(parse_sse_data_line(": keep-alive"), None);
        assert_eq!(parse_sse_data_line("data: hello"), Some("hello"));
    }
}
