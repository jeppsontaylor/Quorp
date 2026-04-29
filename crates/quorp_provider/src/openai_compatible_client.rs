use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::time::Duration;

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
                &["reasoning_output_tokens"],
                &["output_tokens_details", "reasoning_tokens"],
                &["completion_tokens_details", "reasoning_tokens"],
            ]),
            cache_read_input_tokens: usage_u64(&[
                &["cache_read_input_tokens"],
                &["cached_input_tokens"],
                &["input_tokens_details", "cached_tokens"],
                &["prompt_tokens_details", "cached_tokens"],
            ]),
            cache_write_input_tokens: usage_u64(&[
                &["cache_write_input_tokens"],
                &["cache_write_tokens"],
                &["input_tokens_details", "cache_write_tokens"],
                &["prompt_tokens_details", "cache_write_tokens"],
            ]),
            provider_request_id: provider_request_id.map(str::to_string).or_else(|| {
                payload
                    .get("provider_request_id")
                    .or_else(|| payload.get("id"))
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
    stream: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model_id,
        "messages": request.messages,
        "stream": stream,
    });
    if stream {
        body["stream_options"] = serde_json::json!({
            "include_usage": true
        });
    }
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

pub fn build_http_client(
    connect_timeout: Duration,
    read_timeout: Duration,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd();

    if let Some(certificate) = load_root_certificate_from_env()? {
        builder = builder.add_root_certificate(certificate);
    }

    builder
        .build()
        .map_err(|error| anyhow::Error::msg(format!("Failed to build HTTP client: {error}")))
}

fn load_root_certificate_from_env() -> anyhow::Result<Option<reqwest::Certificate>> {
    for variable_name in ["SSL_CERT_FILE", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE"] {
        let Some(path) = env::var_os(variable_name) else {
            continue;
        };
        let path = std::path::PathBuf::from(path);
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read certificate bundle {}", path.display()))?;
        if let Ok(certificate) = reqwest::Certificate::from_pem(&bytes) {
            return Ok(Some(certificate));
        }
        if let Ok(certificate) = reqwest::Certificate::from_der(&bytes) {
            return Ok(Some(certificate));
        }
        anyhow::bail!(
            "failed to parse certificate bundle {} from {}",
            path.display(),
            variable_name
        );
    }
    Ok(None)
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
#[path = "../../../testing/quorp_provider/openai_compatible_client/tests.rs"]
mod tests;
