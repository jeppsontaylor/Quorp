use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveProviderKind {
    #[default]
    Nvidia,
    Local,
}

impl InteractiveProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia",
            Self::Local => "local",
        }
    }
}

pub(crate) fn parse_provider(raw: &str) -> Option<InteractiveProviderKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "nvidia" | "nim" | "nvidia-nim" | "nvidia-qwen3-coder" => {
            Some(InteractiveProviderKind::Nvidia)
        }
        "local" => Some(InteractiveProviderKind::Local),
        _ => None,
    }
}

pub fn interactive_provider_from_env() -> InteractiveProviderKind {
    crate::quorp::provider_config::resolved_provider_env().unwrap_or(InteractiveProviderKind::Nvidia)
}

pub fn interactive_provider_for_workspace(_workspace: &Path) -> InteractiveProviderKind {
    interactive_provider_from_env()
}
#[cfg(test)]
#[path = "../../../../testing/quorp/quorp/executor/tests.rs"]
mod tests;
