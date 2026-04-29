use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::quorp::executor::InteractiveProviderKind;

pub const NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const NVIDIA_QWEN_MODEL: &str = quorp_core::DEFAULT_NVIDIA_MODEL;
pub const LOCAL_CAPTURE_PROBE_BASE_URL: &str = "https://warpos-capture-probe:8443/quorp/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub auth_mode: String,
    pub proxy_visible_remote_egress_expected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRuntimeConfig {
    pub base_url: String,
    pub auth_mode: String,
    pub proxy_visible_remote_egress_expected: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Local,
    RemoteApi,
}

impl RoutingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::RemoteApi => "remote_api",
        }
    }
}

fn parse_env_text(text: &str) -> BTreeMap<String, String> {
    let mut envs = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && !value.is_empty() {
                envs.insert(key.to_string(), value.to_string());
            }
        }
    }
    envs
}

pub fn home_env_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".quorp/.env"))
}

fn project_env_enabled() -> bool {
    #[cfg(test)]
    {
        std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
    }
    #[cfg(not(test))]
    {
        true
    }
}

pub fn project_env_path() -> Option<PathBuf> {
    if !project_env_enabled() {
        return None;
    }
    std::env::current_dir().ok().map(|cwd| cwd.join(".env"))
}

pub fn load_project_env() -> BTreeMap<String, String> {
    let Some(path) = project_env_path() else {
        return BTreeMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    parse_env_text(&raw)
}

pub fn load_home_env() -> BTreeMap<String, String> {
    let Some(path) = home_env_path() else {
        return BTreeMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    parse_env_text(&raw)
}

fn normalized_env_value(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn env_value(name: &str) -> Option<String> {
    normalized_env_value(std::env::var(name).ok())
        .or_else(|| load_home_env().remove(name))
        .or_else(|| load_project_env().remove(name))
        .and_then(|value| normalized_env_value(Some(value)))
}

fn allowed_model_env_value(name: &str) -> Option<String> {
    env_value(name)
        .filter(|value| value.trim() == NVIDIA_QWEN_MODEL || value.trim().starts_with("ssd_moe/"))
}

pub fn resolved_model_env() -> Option<String> {
    allowed_model_env_value("QUORP_MODEL")
}

#[allow(dead_code)]
pub fn resolved_preflight_model_env() -> Option<String> {
    allowed_model_env_value("QUORP_PREFLIGHT_MODEL").or_else(resolved_model_env)
}

pub fn resolved_provider_env() -> Option<InteractiveProviderKind> {
    env_value("QUORP_PROVIDER").and_then(|raw| crate::quorp::executor::parse_provider(&raw))
}

fn parse_routing_mode(raw: &str) -> Option<RoutingMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "local" => Some(RoutingMode::Local),
        "remote_api" | "remote-api" | "remote" => Some(RoutingMode::RemoteApi),
        _ => None,
    }
}

pub fn resolved_routing_mode() -> RoutingMode {
    if let Some(mode) = env_value("QUORP_ROUTING_MODE").and_then(|raw| parse_routing_mode(&raw)) {
        return mode;
    }
    if matches!(
        resolved_provider_env(),
        Some(InteractiveProviderKind::Local)
    ) {
        return RoutingMode::Local;
    }
    RoutingMode::RemoteApi
}

pub fn scenario_label_for_routing_mode(mode: RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Local => "QuorpLocal",
        RoutingMode::RemoteApi => "QuorpRemoteApi",
    }
}

pub fn resolved_scenario_label() -> String {
    env_value("QUORP_SCENARIO_LABEL")
        .unwrap_or_else(|| scenario_label_for_routing_mode(resolved_routing_mode()).to_string())
}

pub fn normalize_remote_base_url(base_url: &str, append_v1: bool) -> anyhow::Result<String> {
    quorp_provider::openai_compatible_client::normalize_base_url(base_url, append_v1)
}

pub fn is_loopback_base_url(base_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(base_url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    }
}

fn allow_loopback_in_managed_mode() -> bool {
    env_value("WARPOS_QUORP_ALLOW_LOOPBACK").is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub fn enforce_managed_remote_guardrail(base_url: &str) -> anyhow::Result<()> {
    if env_value("WARPOS_NETWORK_MODE").as_deref() != Some("inspect") {
        return Ok(());
    }
    if is_loopback_base_url(base_url) && !allow_loopback_in_managed_mode() {
        anyhow::bail!(
            "loopback OpenAI-compatible base URLs are disabled when WARPOS_NETWORK_MODE=inspect; \
use a remote https:// endpoint or set WARPOS_QUORP_ALLOW_LOOPBACK=1 for test-only runs"
        );
    }
    Ok(())
}

fn resolved_nvidia_api_key() -> Option<(String, String)> {
    if let Some(value) = env_value("NVIDIA_API_KEY") {
        return Some(("nvidia_api_key".to_string(), value));
    }
    if let Some(value) = env_value("QUORP_NVIDIA_API_KEY") {
        return Some(("quorp_nvidia_api_key".to_string(), value));
    }
    env_value("QUORP_API_KEY").map(|value| ("quorp_api_key".to_string(), value))
}

pub fn resolve_nvidia_runtime(
    base_url_override: Option<&str>,
) -> anyhow::Result<NvidiaRuntimeConfig> {
    let raw_base_url = base_url_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| env_value("QUORP_NVIDIA_BASE_URL"))
        .unwrap_or_else(|| NVIDIA_NIM_BASE_URL.to_string());
    let base_url = normalize_remote_base_url(&raw_base_url, true)?;
    enforce_managed_remote_guardrail(&base_url)?;
    let (auth_mode, api_key) = match resolved_nvidia_api_key() {
        Some(value) => value,
        None if is_loopback_base_url(&base_url) => (
            "test_loopback_api_key".to_string(),
            "test-api-key".to_string(),
        ),
        None => anyhow::bail!(
            "NVIDIA NIM provider requires NVIDIA_API_KEY, QUORP_NVIDIA_API_KEY, or QUORP_API_KEY"
        ),
    };
    Ok(NvidiaRuntimeConfig {
        proxy_visible_remote_egress_expected: !is_loopback_base_url(&base_url),
        base_url,
        api_key,
        auth_mode,
    })
}

pub fn resolve_local_runtime(
    base_url_override: Option<&str>,
) -> anyhow::Result<LocalRuntimeConfig> {
    let raw_base_url = base_url_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| env_value("QUORP_LOCAL_BASE_URL"))
        .unwrap_or_else(|| LOCAL_CAPTURE_PROBE_BASE_URL.to_string());
    let base_url = normalize_remote_base_url(&raw_base_url, false)?;
    enforce_managed_remote_guardrail(&base_url)?;
    Ok(LocalRuntimeConfig {
        proxy_visible_remote_egress_expected: !is_loopback_base_url(&base_url),
        base_url,
        auth_mode: "local_bearer".to_string(),
    })
}
#[cfg(test)]
#[path = "../../../../testing/quorp_session/quorp/provider_config/tests.rs"]
mod tests;
