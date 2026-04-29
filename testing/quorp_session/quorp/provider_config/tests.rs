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
fn resolved_model_env_accepts_local_model_prefix() {
    let _guard = TEST_ENV_LOCK.lock().expect("env lock");
    let original_model = std::env::var("QUORP_MODEL").ok();
    unsafe {
        std::env::set_var("QUORP_MODEL", "ssd_moe/qwen3-coder-30b-a3b");
    }

    assert_eq!(
        resolved_model_env().as_deref(),
        Some("ssd_moe/qwen3-coder-30b-a3b")
    );

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
    let temp_home = tempfile::tempdir().expect("temp home");
    let original_home = std::env::var("HOME").ok();
    let original_provider = std::env::var("QUORP_PROVIDER").ok();
    let original_base_url = std::env::var("QUORP_BASE_URL").ok();
    let original_mode = std::env::var("QUORP_ROUTING_MODE").ok();
    let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
        std::env::set_var("QUORP_PROVIDER", "remote");
        std::env::set_var("QUORP_BASE_URL", "https://models.example.test/v1");
        std::env::remove_var("QUORP_ROUTING_MODE");
        std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
    }
    assert_eq!(resolved_routing_mode(), RoutingMode::RemoteApi);
    restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
    restore_env("QUORP_ROUTING_MODE", original_mode);
    restore_env("QUORP_BASE_URL", original_base_url);
    restore_env("QUORP_PROVIDER", original_provider);
    restore_env("HOME", original_home);
}

#[test]
fn routing_mode_defaults_to_local_when_provider_is_local() {
    let _guard = TEST_ENV_LOCK.lock().expect("env lock");
    let temp_home = tempfile::tempdir().expect("temp home");
    let original_home = std::env::var("HOME").ok();
    let original_provider = std::env::var("QUORP_PROVIDER").ok();
    let original_mode = std::env::var("QUORP_ROUTING_MODE").ok();
    let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
    unsafe {
        std::env::set_var("HOME", temp_home.path());
        std::env::set_var("QUORP_PROVIDER", "local");
        std::env::remove_var("QUORP_ROUTING_MODE");
        std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
    }

    assert_eq!(resolved_routing_mode(), RoutingMode::Local);

    restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
    restore_env("QUORP_ROUTING_MODE", original_mode);
    restore_env("QUORP_PROVIDER", original_provider);
    restore_env("HOME", original_home);
}

#[test]
fn local_runtime_defaults_to_capture_probe_base_url_without_api_key() {
    let _guard = TEST_ENV_LOCK.lock().expect("env lock");
    let original_local_base_url = std::env::var("QUORP_LOCAL_BASE_URL").ok();
    let original_network_mode = std::env::var("WARPOS_NETWORK_MODE").ok();
    unsafe {
        std::env::remove_var("QUORP_LOCAL_BASE_URL");
        std::env::remove_var("WARPOS_NETWORK_MODE");
    }

    let config = resolve_local_runtime(None).expect("local runtime");

    assert_eq!(config.base_url, LOCAL_CAPTURE_PROBE_BASE_URL);
    assert_eq!(config.auth_mode, "local_bearer");
    assert!(config.proxy_visible_remote_egress_expected);

    restore_env("WARPOS_NETWORK_MODE", original_network_mode);
    restore_env("QUORP_LOCAL_BASE_URL", original_local_base_url);
}
