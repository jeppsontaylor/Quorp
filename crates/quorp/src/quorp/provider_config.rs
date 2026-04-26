use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::quorp::executor::InteractiveProviderKind;

pub(crate) const NVIDIA_NIM_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub(crate) const NVIDIA_QWEN_MODEL: &str = quorp_core::DEFAULT_NVIDIA_MODEL;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub auth_mode: String,
    pub proxy_visible_remote_egress_expected: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    RemoteApi,
}

impl RoutingMode {
    pub fn label(self) -> &'static str {
        match self {
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

pub(crate) fn home_env_path() -> Option<PathBuf> {
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

pub(crate) fn project_env_path() -> Option<PathBuf> {
    if !project_env_enabled() {
        return None;
    }
    std::env::current_dir().ok().map(|cwd| cwd.join(".env"))
}

pub(crate) fn load_project_env() -> BTreeMap<String, String> {
    let Some(path) = project_env_path() else {
        return BTreeMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    parse_env_text(&raw)
}

pub(crate) fn load_home_env() -> BTreeMap<String, String> {
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

pub(crate) fn env_value(name: &str) -> Option<String> {
    normalized_env_value(std::env::var(name).ok())
        .or_else(|| load_home_env().remove(name))
        .or_else(|| load_project_env().remove(name))
        .and_then(|value| normalized_env_value(Some(value)))
}

fn qwen_model_env_value(name: &str) -> Option<String> {
    env_value(name).filter(|value| value.trim() == NVIDIA_QWEN_MODEL)
}

pub(crate) fn resolved_model_env() -> Option<String> {
    qwen_model_env_value("QUORP_MODEL")
}

#[allow(dead_code)]
pub(crate) fn resolved_preflight_model_env() -> Option<String> {
    qwen_model_env_value("QUORP_PREFLIGHT_MODEL").or_else(resolved_model_env)
}

pub(crate) fn resolved_provider_env() -> Option<InteractiveProviderKind> {
    env_value("QUORP_PROVIDER").and_then(|raw| crate::quorp::executor::parse_provider(&raw))
}

fn parse_routing_mode(raw: &str) -> Option<RoutingMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "remote_api" | "remote-api" | "remote" => Some(RoutingMode::RemoteApi),
        _ => None,
    }
}

pub(crate) fn resolved_routing_mode() -> RoutingMode {
    if let Some(mode) = env_value("QUORP_ROUTING_MODE").and_then(|raw| parse_routing_mode(&raw)) {
        return mode;
    }
    RoutingMode::RemoteApi
}

pub(crate) fn scenario_label_for_routing_mode(mode: RoutingMode) -> &'static str {
    match mode {
        RoutingMode::RemoteApi => "QuorpRemoteApi",
    }
}

pub(crate) fn resolved_scenario_label() -> String {
    env_value("QUORP_SCENARIO_LABEL")
        .unwrap_or_else(|| scenario_label_for_routing_mode(resolved_routing_mode()).to_string())
}

pub(crate) fn normalize_remote_base_url(base_url: &str, append_v1: bool) -> anyhow::Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("base URL cannot be empty");
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|error| anyhow::anyhow!("invalid base URL `{trimmed}`: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => anyhow::bail!("unsupported base URL scheme `{scheme}`"),
    }
    let normalized = if append_v1 && !parsed.path().ends_with("/v1") {
        format!("{}/v1", trimmed)
    } else {
        trimmed.to_string()
    };
    Ok(normalized)
}

pub(crate) fn is_loopback_base_url(base_url: &str) -> bool {
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

pub(crate) fn enforce_managed_remote_guardrail(base_url: &str) -> anyhow::Result<()> {
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

pub(crate) fn resolve_nvidia_runtime(
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
        #[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn restore_env(name: &str, value: Option<String>) {
        match value {
            Some(value) => unsafe {
                std::env::set_var(name, value);
            },
            None => unsafe {
                std::env::remove_var(name);
            },
        }
    }

    #[test]
    fn env_value_uses_home_env_when_process_env_is_absent() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_model = std::env::var("QUORP_MODEL").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            format!("QUORP_MODEL={NVIDIA_QWEN_MODEL}\n"),
        )
        .expect("write env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_MODEL");
        }

        assert_eq!(resolved_model_env().as_deref(), Some(NVIDIA_QWEN_MODEL));

        restore_env("QUORP_MODEL", original_model);
        restore_env("HOME", original_home);
    }

    #[test]
    fn env_value_prefers_home_env_before_project_env_when_enabled() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_project = tempfile::tempdir().expect("temp project");
        let original_home = std::env::var("HOME").ok();
        let original_model = std::env::var("QUORP_MODEL").ok();
        let original_cwd = std::env::current_dir().ok();
        let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            format!("QUORP_MODEL={NVIDIA_QWEN_MODEL}\n"),
        )
        .expect("write home env");
        std::fs::write(
            temp_project.path().join(".env"),
            "QUORP_MODEL=ignored-non-qwen\n",
        )
        .expect("write project env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("QUORP_MODEL");
            std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "1");
            std::env::set_current_dir(temp_project.path()).expect("change cwd");
        }

        assert_eq!(resolved_model_env().as_deref(), Some(NVIDIA_QWEN_MODEL));

        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("QUORP_MODEL", original_model);
        restore_env("HOME", original_home);
        if let Some(path) = original_cwd {
            std::env::set_current_dir(path).expect("restore cwd");
        }
    }

    #[test]
    fn process_env_wins_over_home_env() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_model = std::env::var("QUORP_MODEL").ok();
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("create .quorp");
        std::fs::write(
            temp_home.path().join(".quorp/.env"),
            "QUORP_MODEL=from-home-env\n",
        )
        .expect("write env");
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::set_var("QUORP_MODEL", NVIDIA_QWEN_MODEL);
        }

        assert_eq!(resolved_model_env().as_deref(), Some(NVIDIA_QWEN_MODEL));

        restore_env("QUORP_MODEL", original_model);
        restore_env("HOME", original_home);
    }

    #[test]
    fn resolved_model_env_accepts_only_qwen_model() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let original_model = std::env::var("QUORP_MODEL").ok();
        unsafe {
            std::env::set_var("QUORP_MODEL", "not-the-supported-model");
        }

        assert_eq!(resolved_model_env(), None);

        restore_env("QUORP_MODEL", original_model);
    }

    #[test]
    fn nvidia_runtime_uses_default_endpoint_and_nvidia_api_key_first() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let original_nvidia_api_key = std::env::var("NVIDIA_API_KEY").ok();
        let original_quorp_nvidia_api_key = std::env::var("QUORP_NVIDIA_API_KEY").ok();
        let original_quorp_api_key = std::env::var("QUORP_API_KEY").ok();
        let original_base_url = std::env::var("QUORP_NVIDIA_BASE_URL").ok();
        let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
        unsafe {
            std::env::set_var("NVIDIA_API_KEY", "nvidia-key");
            std::env::set_var("QUORP_NVIDIA_API_KEY", "quorp-nvidia-key");
            std::env::set_var("QUORP_API_KEY", "generic-key");
            std::env::remove_var("QUORP_NVIDIA_BASE_URL");
            std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
        }

        let config = resolve_nvidia_runtime(None).expect("nvidia runtime");

        assert_eq!(config.base_url, NVIDIA_NIM_BASE_URL);
        assert_eq!(config.auth_mode, "nvidia_api_key");
        assert_eq!(config.api_key, "nvidia-key");

        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("QUORP_NVIDIA_BASE_URL", original_base_url);
        restore_env("QUORP_API_KEY", original_quorp_api_key);
        restore_env("QUORP_NVIDIA_API_KEY", original_quorp_nvidia_api_key);
        restore_env("NVIDIA_API_KEY", original_nvidia_api_key);
    }

    #[test]
    fn nvidia_runtime_falls_back_to_generic_quorp_key() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let temp_home = tempfile::tempdir().expect("temp home");
        let original_home = std::env::var("HOME").ok();
        let original_nvidia_api_key = std::env::var("NVIDIA_API_KEY").ok();
        let original_quorp_nvidia_api_key = std::env::var("QUORP_NVIDIA_API_KEY").ok();
        let original_quorp_api_key = std::env::var("QUORP_API_KEY").ok();
        let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
            std::env::remove_var("NVIDIA_API_KEY");
            std::env::remove_var("QUORP_NVIDIA_API_KEY");
            std::env::set_var("QUORP_API_KEY", "generic-key");
            std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
        }

        let config =
            resolve_nvidia_runtime(Some("https://nvidia.example.test")).expect("nvidia runtime");

        assert_eq!(config.base_url, "https://nvidia.example.test/v1");
        assert_eq!(config.auth_mode, "quorp_api_key");
        assert_eq!(config.api_key, "generic-key");

        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("QUORP_API_KEY", original_quorp_api_key);
        restore_env("QUORP_NVIDIA_API_KEY", original_quorp_nvidia_api_key);
        restore_env("NVIDIA_API_KEY", original_nvidia_api_key);
        restore_env("HOME", original_home);
    }

    #[test]
    fn routing_mode_defaults_to_remote_api() {
        let _guard = TEST_ENV_LOCK.lock().expect("env lock");
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_base_url = std::env::var("QUORP_BASE_URL").ok();
        let original_mode = std::env::var("QUORP_ROUTING_MODE").ok();
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "remote");
            std::env::set_var("QUORP_BASE_URL", "https://models.example.test/v1");
            std::env::remove_var("QUORP_ROUTING_MODE");
        }
        assert_eq!(resolved_routing_mode(), RoutingMode::RemoteApi);
        restore_env("QUORP_ROUTING_MODE", original_mode);
        restore_env("QUORP_BASE_URL", original_base_url);
        restore_env("QUORP_PROVIDER", original_provider);
    }
}
