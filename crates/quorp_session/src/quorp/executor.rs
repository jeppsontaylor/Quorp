use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveProviderKind {
    #[default]
    Nvidia,
}

impl InteractiveProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia",
        }
    }
}

pub(crate) fn parse_provider(raw: &str) -> Option<InteractiveProviderKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "nvidia" | "nim" | "nvidia-nim" | "nvidia-qwen3-coder" => {
            Some(InteractiveProviderKind::Nvidia)
        }
        _ => None,
    }
}

pub fn interactive_provider_from_env() -> InteractiveProviderKind {
    crate::quorp::provider_config::resolved_provider_env()
        .and_then(|provider| {
            if provider == InteractiveProviderKind::Nvidia {
                Some(provider)
            } else {
                None
            }
        })
        .unwrap_or(InteractiveProviderKind::Nvidia)
}

pub fn interactive_provider_for_workspace(_workspace: &Path) -> InteractiveProviderKind {
    interactive_provider_from_env()
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
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "nvidia");
        }
        assert_eq!(
            interactive_provider_from_env(),
            InteractiveProviderKind::Nvidia
        );
        unsafe {
            std::env::remove_var("QUORP_PROVIDER");
        }
    }
}
