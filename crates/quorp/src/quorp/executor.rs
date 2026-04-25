use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum QuorpExecutor {
    #[default]
    Native,
    Codex,
}

impl QuorpExecutor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveProviderKind {
    #[default]
    Local,
    Ollama,
    OpenAiCompatible,
    Nvidia,
    Codex,
}

impl InteractiveProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ollama => "ollama",
            Self::OpenAiCompatible => "openai-compatible",
            Self::Nvidia => "nvidia",
            Self::Codex => "codex",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Ollama => "Ollama",
            Self::OpenAiCompatible => "OpenAI-Compatible",
            Self::Nvidia => "NVIDIA",
            Self::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum CodexSessionMode {
    Fresh,
    ResumeLast,
    ResumeId,
    ResumeLastForCwd,
}

impl CodexSessionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::ResumeLast => "resume-last",
            Self::ResumeId => "resume-id",
            Self::ResumeLastForCwd => "resume-last-for-cwd",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodexSessionStrategy {
    pub mode: CodexSessionMode,
    pub session_id: Option<String>,
}

impl CodexSessionStrategy {
    pub fn fresh() -> Self {
        Self {
            mode: CodexSessionMode::Fresh,
            session_id: None,
        }
    }
}

fn parse_executor(raw: &str) -> Option<QuorpExecutor> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "native" => Some(QuorpExecutor::Native),
        "codex" => Some(QuorpExecutor::Codex),
        _ => None,
    }
}

pub(crate) fn parse_provider(raw: &str) -> Option<InteractiveProviderKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "local" | "native" => Some(InteractiveProviderKind::Local),
        "nvidia" | "nim" | "nvidia-nim" => Some(InteractiveProviderKind::Nvidia),
        "codex" => Some(InteractiveProviderKind::Codex),
        _ => None,
    }
}

fn parse_session_mode(raw: &str) -> Option<CodexSessionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fresh" => Some(CodexSessionMode::Fresh),
        "resume-last" => Some(CodexSessionMode::ResumeLast),
        "resume-id" => Some(CodexSessionMode::ResumeId),
        "resume-last-for-cwd" => Some(CodexSessionMode::ResumeLastForCwd),
        _ => None,
    }
}

pub fn executor_from_env() -> QuorpExecutor {
    std::env::var("QUORP_EXECUTOR")
        .ok()
        .as_deref()
        .and_then(parse_executor)
        .unwrap_or_default()
}

pub fn interactive_provider_from_env() -> InteractiveProviderKind {
    if let Some(provider) = crate::quorp::provider_config::resolved_provider_env()
        && matches!(
            provider,
            InteractiveProviderKind::Local
                | InteractiveProviderKind::Nvidia
                | InteractiveProviderKind::Codex
        )
    {
        return provider;
    }

    match executor_from_env() {
        QuorpExecutor::Native => InteractiveProviderKind::Local,
        QuorpExecutor::Codex => InteractiveProviderKind::Codex,
    }
}

pub fn codex_session_strategy_from_env(default_mode: CodexSessionMode) -> CodexSessionStrategy {
    let mode = std::env::var("QUORP_CODEX_SESSION_MODE")
        .ok()
        .as_deref()
        .and_then(parse_session_mode)
        .unwrap_or(default_mode);
    let session_id = std::env::var("QUORP_CODEX_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    CodexSessionStrategy { mode, session_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn restore_env(name: &str, value: Option<String>) {
        unsafe {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }

    fn isolate_provider_env() -> (Option<String>, Option<String>) {
        let original_home = std::env::var("HOME").ok();
        let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
        unsafe {
            std::env::remove_var("HOME");
            std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
        }
        (original_home, original_project_env)
    }

    #[test]
    fn executor_env_defaults_to_native() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::remove_var("QUORP_EXECUTOR");
            std::env::remove_var("QUORP_PROVIDER");
        }
        assert_eq!(executor_from_env(), QuorpExecutor::Native);
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }

    #[test]
    fn interactive_provider_prefers_quorp_provider_env() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::set_var("QUORP_EXECUTOR", "codex");
            std::env::set_var("QUORP_PROVIDER", "local");
        }
        assert_eq!(
            interactive_provider_from_env(),
            InteractiveProviderKind::Local
        );
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
            std::env::remove_var("QUORP_EXECUTOR");
        }
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }

    #[test]
    fn parse_provider_supports_nvidia_aliases() {
        assert_eq!(
            parse_provider("nvidia"),
            Some(InteractiveProviderKind::Nvidia)
        );
        assert_eq!(parse_provider("nim"), Some(InteractiveProviderKind::Nvidia));
        assert_eq!(
            parse_provider("nvidia-nim"),
            Some(InteractiveProviderKind::Nvidia)
        );
    }

    #[test]
    fn interactive_provider_accepts_nvidia_env() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::remove_var("QUORP_EXECUTOR");
            std::env::set_var("QUORP_PROVIDER", "nvidia");
        }
        assert_eq!(
            interactive_provider_from_env(),
            InteractiveProviderKind::Nvidia
        );
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
        }
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }

    #[test]
    fn interactive_provider_ignores_legacy_remote_provider_value() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "remote");
            std::env::remove_var("QUORP_EXECUTOR");
        }
        assert_eq!(
            interactive_provider_from_env(),
            InteractiveProviderKind::Local
        );
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
        }
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }

    #[test]
    fn interactive_provider_falls_back_to_executor_env() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
            std::env::set_var("QUORP_EXECUTOR", "codex");
        }
        assert_eq!(
            interactive_provider_from_env(),
            InteractiveProviderKind::Codex
        );
        unsafe {
            std::env::remove_var("QUORP_EXECUTOR");
        }
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }

    #[test]
    fn session_strategy_uses_env_overrides() {
        let _guard = env_lock();
        let (original_home, original_project_env) = isolate_provider_env();
        unsafe {
            std::env::set_var("QUORP_CODEX_SESSION_MODE", "resume-id");
            std::env::set_var("QUORP_CODEX_SESSION_ID", "abc-123");
        }
        let strategy = codex_session_strategy_from_env(CodexSessionMode::Fresh);
        assert_eq!(strategy.mode, CodexSessionMode::ResumeId);
        assert_eq!(strategy.session_id.as_deref(), Some("abc-123"));
        unsafe {
            std::env::remove_var("QUORP_CODEX_SESSION_MODE");
            std::env::remove_var("QUORP_CODEX_SESSION_ID");
        }
        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
    }
}
