use super::*;

pub(crate) fn assistant_action_mismatch(
    assistant_message: &str,
    actions: &[AgentAction],
) -> Option<String> {
    let lower = assistant_message.to_ascii_lowercase();
    let claims_write = contains_any(
        &lower,
        &[
            "patched",
            "updated",
            "edited",
            "changed",
            "fixed",
            "wrote",
            "applied",
            "implemented",
            "committed",
        ],
    );
    let claims_validation = contains_any(
        &lower,
        &[
            "ran tests",
            "validated",
            "verified",
            "checked",
            "test passed",
            "tests passed",
        ],
    );
    let has_write_action = actions.iter().any(AgentAction::is_write_like);
    let has_validation_action = actions.iter().any(is_validation_action);

    if claims_write && !has_write_action {
        return Some(
            "assistant message claims a write or patch, but the tool actions were read-only"
                .to_string(),
        );
    }
    if claims_validation && !has_validation_action {
        return Some(
            "assistant message claims validation work, but no validation tool action was emitted"
                .to_string(),
        );
    }
    None
}

pub(crate) fn recovery_refresh_message(reason: &str) -> String {
    format!(
        "[Recovery]\n{reason}. Refresh the recovery packet, restate the intended tools, and emit a structured turn that matches the tool plan."
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn is_validation_action(action: &AgentAction) -> bool {
    match action {
        AgentAction::RunValidation { .. } => true,
        AgentAction::RunCommand { command, .. } => {
            let lower = command.to_ascii_lowercase();
            lower.contains("cargo test")
                || lower.contains("cargo check")
                || lower.contains("cargo clippy")
                || lower.contains("cargo fmt")
        }
        _ => false,
    }
}
