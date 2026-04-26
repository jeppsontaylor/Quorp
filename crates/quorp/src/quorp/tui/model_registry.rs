//! Qwen model resolution for QUORP CLI sessions.

use crate::quorp::executor::InteractiveProviderKind;

pub fn chat_model_provider(
    _model_id: &str,
    default_provider: InteractiveProviderKind,
) -> InteractiveProviderKind {
    default_provider
}

pub fn chat_model_raw_id(model_id: &str) -> &str {
    model_id
        .strip_prefix("nvidia/")
        .filter(|raw| !raw.trim().is_empty())
        .unwrap_or(model_id)
}
