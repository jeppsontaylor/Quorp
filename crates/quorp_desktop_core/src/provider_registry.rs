//! Single-provider registry: NVIDIA NIM Qwen3-Coder.
//!
//! Quorp ships with one provider — the `quorp_provider::nvidia_qwen()`
//! factory. The desktop never exposes a provider picker; Settings only
//! lets the user paste their NIM API key (stored in macOS Keychain via
//! [`crate::secret_keychain`]) and run a health check against the
//! configured endpoint.

use std::sync::Arc;
use std::time::Duration;

use quorp_desktop_ipc::{DEFAULT_MODEL_ID, DEFAULT_PROVIDER_NAME, ProviderHealth, ProviderSummary};
use quorp_provider::{ChatCompletionRequest, ChatMessage, ChatRole, OpenAiCompatibleProvider};

use crate::secret_keychain::SecretStore;

/// Account name under which the NIM API key is stored in the keychain.
/// Stable across releases; changing it breaks existing installs.
pub const NIM_KEYCHAIN_ACCOUNT: &str = "nvidia-nim";

/// Errors returned by the provider registry.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key set for {0}")]
    MissingKey(String),
    #[error("keychain access failed: {0}")]
    Keychain(#[from] crate::secret_keychain::KeychainError),
    #[error("provider configuration is invalid: {0}")]
    Invalid(String),
    #[error("HTTP error during health check: {0}")]
    Http(String),
    #[error("provider returned status {status}: {body}")]
    Endpoint { status: u16, body: String },
}

/// Single-provider registry. Holds a reference to the secret store
/// (so it can read/write the NIM API key) and stays the canonical
/// place where provider construction happens. The frontend never sees
/// the API key — it only sees [`ProviderSummary`] which carries
/// `has_key: bool`.
#[derive(Debug)]
pub struct ProviderRegistry {
    secret_store: Arc<dyn SecretStore>,
    provider: OpenAiCompatibleProvider,
    http_client: reqwest::Client,
}

impl ProviderRegistry {
    pub fn new(secret_store: Arc<dyn SecretStore>) -> Self {
        let provider = OpenAiCompatibleProvider::nvidia_qwen();
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            secret_store,
            provider,
            http_client,
        }
    }

    pub fn provider(&self) -> &OpenAiCompatibleProvider {
        &self.provider
    }

    /// Snapshot used to populate Settings → Provider. Always one row.
    pub fn summary(&self) -> ProviderSummary {
        let endpoint = self.provider.endpoint();
        let (display_name, base_url, model) = match endpoint {
            Ok(ep) => (
                "NVIDIA NIM (Qwen3-Coder)".to_string(),
                ep.base_url,
                ep.model_id,
            ),
            Err(_) => (
                "NVIDIA NIM (Qwen3-Coder)".to_string(),
                String::new(),
                DEFAULT_MODEL_ID.to_string(),
            ),
        };
        ProviderSummary {
            name: DEFAULT_PROVIDER_NAME.to_string(),
            display_name,
            base_url,
            default_model: model,
            has_key: self.secret_store.has(NIM_KEYCHAIN_ACCOUNT),
        }
    }

    /// Store the user's NIM API key in the keychain. The string is
    /// taken by value so the caller's copy can be dropped/zeroed
    /// immediately after this returns.
    pub fn set_api_key(&self, secret: &str) -> Result<(), ProviderError> {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            return Err(ProviderError::Invalid(
                "API key must not be empty".to_string(),
            ));
        }
        self.secret_store
            .set(NIM_KEYCHAIN_ACCOUNT, trimmed)
            .map_err(ProviderError::from)
    }

    pub fn clear_api_key(&self) -> Result<(), ProviderError> {
        self.secret_store
            .clear(NIM_KEYCHAIN_ACCOUNT)
            .map_err(ProviderError::from)
    }

    pub fn has_api_key(&self) -> bool {
        self.secret_store.has(NIM_KEYCHAIN_ACCOUNT)
    }

    /// Send a minimal chat completion request to verify the saved key
    /// works against the configured endpoint. Consumes a tiny amount
    /// of the user's token budget (a one-token completion). On failure
    /// the error message is redacted of any auth header content.
    pub async fn validate(&self) -> Result<ProviderHealth, ProviderError> {
        let api_key = match self.secret_store.get(NIM_KEYCHAIN_ACCOUNT)? {
            Some(key) => key,
            None => {
                return Err(ProviderError::MissingKey(DEFAULT_PROVIDER_NAME.to_string()));
            }
        };

        let request = ChatCompletionRequest {
            model: self
                .provider
                .endpoint()
                .map(|ep| ep.model_id)
                .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string()),
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: "ping".to_string(),
            }],
            stream: false,
            temperature: Some(0.0),
        };

        let started = std::time::Instant::now();
        let builder = self
            .provider
            .build_http_request(&self.http_client, &api_key, &request)
            .map_err(|err| ProviderError::Invalid(format!("{err:?}")))?;
        let response = match builder.send().await {
            Ok(resp) => resp,
            Err(err) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let redacted = redact_secret(&format!("{err}"), &api_key);
                return Ok(ProviderHealth {
                    ok: false,
                    latency_ms,
                    model_id_echo: None,
                    error: Some(redacted),
                });
            }
        };
        let latency_ms = started.elapsed().as_millis() as u64;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Ok(ProviderHealth {
                ok: false,
                latency_ms,
                model_id_echo: None,
                error: Some(format!(
                    "{status}: {}",
                    redact_secret(&truncate(&body, 240), &api_key)
                )),
            });
        }
        let body: serde_json::Value = match response.json().await {
            Ok(body) => body,
            Err(err) => {
                return Ok(ProviderHealth {
                    ok: false,
                    latency_ms,
                    model_id_echo: None,
                    error: Some(format!("malformed JSON: {err}")),
                });
            }
        };
        let model_id_echo = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok(ProviderHealth {
            ok: true,
            latency_ms,
            model_id_echo,
            error: None,
        })
    }
}

fn redact_secret(s: &str, secret: &str) -> String {
    if secret.is_empty() {
        return s.to_string();
    }
    s.replace(secret, "<redacted>")
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
