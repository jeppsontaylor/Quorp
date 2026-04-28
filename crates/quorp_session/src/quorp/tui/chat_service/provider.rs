use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use super::{CONNECT_TIMEOUT, READ_TIMEOUT};
use crate::quorp::agent_runner::RoutingDecision;
use crate::quorp::executor::InteractiveProviderKind;
use crate::quorp::provider_config;
use crate::quorp::tui::model_registry;

use super::request::StreamRequest;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedClientConfig {
    pub provider: InteractiveProviderKind,
    pub client: quorp_provider::openai_compatible_client::OpenAiCompatibleClientConfig,
    pub bearer_token: Option<String>,
    pub routing: RoutingDecision,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedModelTarget {
    provider: InteractiveProviderKind,
    pub(crate) provider_model_id: String,
}

pub(crate) fn resolved_provider(model_id: &str) -> InteractiveProviderKind {
    model_registry::chat_model_provider(
        model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    )
}

pub(crate) fn resolve_model_target(model_id: &str) -> ResolvedModelTarget {
    ResolvedModelTarget {
        provider: resolved_provider(model_id),
        provider_model_id: model_registry::chat_model_raw_id(model_id).to_string(),
    }
}

pub(crate) fn env_u64(name: &str) -> Option<u64> {
    provider_config::env_value(name)?.trim().parse::<u64>().ok()
}

pub(crate) fn nvidia_rate_limit_retries() -> u64 {
    env_u64("QUORP_NVIDIA_RATE_LIMIT_RETRIES").unwrap_or(2)
}

pub(crate) fn nvidia_rate_limit_backoff_seconds(
    headers: &reqwest::header::HeaderMap,
    attempt_index: u64,
) -> u64 {
    quorp_provider::openai_compatible_client::retry_backoff_seconds(headers, attempt_index)
}

pub(crate) fn is_nvidia_qwen_coder_model(model_id: &str) -> bool {
    model_id
        .to_ascii_lowercase()
        .starts_with("qwen/qwen3-coder-480b-a35b-instruct")
}

pub(crate) fn request_uses_nvidia_qwen_coder(request: &StreamRequest) -> bool {
    let model_target = resolve_model_target(&request.model_id);
    model_target.provider == InteractiveProviderKind::Nvidia
        && is_nvidia_qwen_coder_model(&model_target.provider_model_id)
}

pub(crate) fn nvidia_qwen_benchmark_profile(request: &StreamRequest) -> bool {
    request_uses_nvidia_qwen_coder(request)
        && request
            .safety_mode_label
            .as_deref()
            .is_some_and(|label| label == "nvidia_qwen_benchmark")
}

pub(crate) fn nvidia_controller_benchmark_profile(request: &StreamRequest) -> bool {
    nvidia_qwen_benchmark_profile(request)
}

pub(crate) fn nvidia_request_body_overrides(
    provider_model_id: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let body = serde_json::Map::new();
    let _ = provider_model_id;
    body
}

pub(crate) fn remote_request_headers(
    request: &StreamRequest,
    _provider: InteractiveProviderKind,
    routing_mode: &str,
) -> BTreeMap<String, String> {
    let action_contract_mode = if request.native_tool_calls {
        "native_tool_calls_v1"
    } else {
        "json_action_contract_v1"
    };
    let mut headers = BTreeMap::from([
        (
            "User-Agent".to_string(),
            format!("quorp/{}", env!("CARGO_PKG_VERSION")),
        ),
        (
            "X-Quorp-Run-Id".to_string(),
            correlation_run_id(&request.project_root),
        ),
        (
            "X-Quorp-Session-Id".to_string(),
            request.session_id.to_string(),
        ),
        (
            "X-Quorp-Request-Id".to_string(),
            request.request_id.to_string(),
        ),
        ("X-Quorp-Routing-Mode".to_string(), routing_mode.to_string()),
        (
            "X-Quorp-Action-Contract-Mode".to_string(),
            action_contract_mode.to_string(),
        ),
        (
            "X-Quorp-Repo-Capsule-Injected".to_string(),
            request.include_repo_capsule.to_string(),
        ),
        (
            "X-Quorp-Reasoning-Enabled".to_string(),
            (!request.disable_reasoning).to_string(),
        ),
        (
            "X-Quorp-Executor-Model".to_string(),
            request.model_id.clone(),
        ),
        ("X-WarpOS-Agent".to_string(), "quorp".to_string()),
    ]);
    if let Some(scope) = request.capture_scope.as_deref() {
        headers.insert("X-WarpOS-Scope".to_string(), scope.to_string());
    }
    if let Some(call_class) = request.capture_call_class.as_deref() {
        headers.insert("X-WarpOS-Call-Class".to_string(), call_class.to_string());
    }
    headers
}

pub(crate) fn correlation_run_id(project_root: &std::path::Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_root.to_string_lossy().hash(&mut hasher);
    format!("quorp-run-{:016x}", hasher.finish())
}

pub(crate) fn default_routing_decision(
    provider: InteractiveProviderKind,
    requested_model: String,
    effective_model: String,
    provider_base_url: Option<String>,
    auth_mode: Option<String>,
    comparable: bool,
    proxy_visible_remote_egress_expected: bool,
) -> RoutingDecision {
    RoutingDecision {
        routing_mode: provider_config::resolved_routing_mode().label().to_string(),
        requested_provider: provider.label().to_string(),
        requested_model,
        candidate_models: vec![effective_model.clone()],
        effective_provider: provider.label().to_string(),
        effective_model,
        used_fallback: false,
        fallback_reason: None,
        comparable,
        provider_base_url,
        auth_mode,
        proxy_visible_remote_egress_expected,
    }
}

pub(crate) fn wrap_raw_provider_response(
    provider_response: serde_json::Value,
    routing: &RoutingDecision,
) -> serde_json::Value {
    serde_json::json!({
        "provider_response": provider_response,
        "routing": routing,
    })
}

pub(crate) fn chat_completions_url_for_provider(
    provider: InteractiveProviderKind,
    base_url: &str,
) -> anyhow::Result<String> {
    match provider {
        InteractiveProviderKind::Nvidia => Ok(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        )),
    }
}

pub(crate) fn provider_connection_name(provider: InteractiveProviderKind) -> &'static str {
    match provider {
        InteractiveProviderKind::Nvidia => "NVIDIA NIM",
    }
}

pub(crate) fn resolve_client_config(
    request: &StreamRequest,
) -> anyhow::Result<ResolvedClientConfig> {
    let model_target = resolve_model_target(&request.model_id);
    resolve_nvidia_client_config(request, &model_target.provider_model_id)
}

pub(crate) fn resolve_nvidia_client_config(
    request: &StreamRequest,
    provider_model_id: &str,
) -> anyhow::Result<ResolvedClientConfig> {
    let runtime = provider_config::resolve_nvidia_runtime(request.base_url_override.as_deref())?;
    let routing = default_routing_decision(
        InteractiveProviderKind::Nvidia,
        request.model_id.clone(),
        provider_model_id.to_string(),
        Some(runtime.base_url.clone()),
        Some(runtime.auth_mode.clone()),
        true,
        runtime.proxy_visible_remote_egress_expected,
    );
    Ok(ResolvedClientConfig {
        provider: InteractiveProviderKind::Nvidia,
        client: quorp_provider::openai_compatible_client::OpenAiCompatibleClientConfig {
            base_url: runtime.base_url.clone(),
            model_id: provider_model_id.to_string(),
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            extra_headers: remote_request_headers(
                request,
                InteractiveProviderKind::Nvidia,
                provider_config::resolved_routing_mode().label(),
            ),
            extra_body: nvidia_request_body_overrides(provider_model_id),
        },
        bearer_token: Some(runtime.api_key),
        routing,
    })
}

pub(crate) async fn finalize_client_config_for_request(
    _request: &StreamRequest,
    client_config: ResolvedClientConfig,
) -> anyhow::Result<ResolvedClientConfig> {
    Ok(client_config)
}
