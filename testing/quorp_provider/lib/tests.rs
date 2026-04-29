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

#[test]
fn endpoint_normalizes_profile_for_session_receipts() {
    let provider = OpenAiCompatibleProvider::new(ProviderProfile {
        name: "local-openai-compatible".to_string(),
        base_url: "http://127.0.0.1:8080/v1/".to_string(),
        model: "mock-model".to_string(),
        api_key_env: "MOCK_API_KEY".to_string(),
    });

    let endpoint = provider.endpoint().expect("endpoint");

    assert_eq!(endpoint.provider_name, "local-openai-compatible");
    assert_eq!(endpoint.base_url, "http://127.0.0.1:8080/v1");
    assert_eq!(
        endpoint.chat_completions_url,
        "http://127.0.0.1:8080/v1/chat/completions"
    );
    assert_eq!(endpoint.model_id, "mock-model");
    assert!(endpoint.stream);
    assert!(endpoint.include_usage);
}
