//! OpenAI-compatible provider request shapes.

pub mod openai_compatible_client;

use anyhow::Context as _;
use quorp_core::ProviderProfile;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleProvider {
    profile: ProviderProfile,
}

impl OpenAiCompatibleProvider {
    pub fn new(profile: ProviderProfile) -> Self {
        Self { profile }
    }

    pub fn nvidia_qwen() -> Self {
        Self::new(ProviderProfile::nvidia_qwen())
    }

    pub fn profile(&self) -> &ProviderProfile {
        &self.profile
    }

    pub fn chat_completions_url(&self) -> anyhow::Result<Url> {
        let base = self.profile.base_url.trim_end_matches('/');
        Url::parse(&format!("{base}/chat/completions"))
            .with_context(|| format!("invalid provider base URL `{base}`"))
    }

    pub fn chat_request(&self, messages: Vec<ChatMessage>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: self.profile.model.clone(),
            messages,
            stream: true,
            temperature: None,
        }
    }

    pub fn build_http_request(
        &self,
        client: &reqwest::Client,
        api_key: &str,
        request: &ChatCompletionRequest,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        Ok(client
            .post(self.chat_completions_url()?)
            .bearer_auth(api_key)
            .json(request))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use quorp_core::{DEFAULT_NVIDIA_BASE_URL, DEFAULT_NVIDIA_MODEL};

    #[test]
    fn nvidia_profile_uses_openai_compatible_chat_endpoint() {
        let provider = OpenAiCompatibleProvider::nvidia_qwen();

        assert_eq!(provider.profile().base_url, DEFAULT_NVIDIA_BASE_URL);
        assert_eq!(provider.profile().model, DEFAULT_NVIDIA_MODEL);
        assert_eq!(
            provider.chat_completions_url().expect("url").as_str(),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
    }

    #[test]
    fn request_uses_profile_model_and_streaming() {
        let provider = OpenAiCompatibleProvider::nvidia_qwen();
        let request = provider.chat_request(vec![ChatMessage {
            role: ChatRole::User,
            content: "repair the issue".to_string(),
        }]);

        assert_eq!(request.model, DEFAULT_NVIDIA_MODEL);
        assert!(request.stream);
        assert_eq!(request.messages[0].role, ChatRole::User);
    }
}
