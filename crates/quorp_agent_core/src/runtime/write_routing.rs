use crate::agent_protocol::AgentAction;
use quorp_patch_vm::{WriteLease, WriteLeaseOperation};

use super::AgentTaskState;

pub(crate) fn large_source_write_violation(
    state: &AgentTaskState,
    action: &AgentAction,
) -> Option<String> {
    let AgentAction::WriteFile { path, content } = action else {
        return None;
    };
    let line_count = content.lines().count();
    if line_count <= 200 {
        return None;
    }
    if let Some(lease) = source_write_lease(state, path)
        && lease.permits_operation(WriteLeaseOperation::WriteFile)
    {
        return None;
    }
    if is_generated_path(path) {
        return None;
    }
    Some(format!(
        "source WriteFile for `{path}` is {line_count} lines; lower it to a semantic edit or attach an explicit lease"
    ))
}

fn source_write_lease(state: &AgentTaskState, path: &str) -> Option<WriteLease> {
    let lease_path = state
        .agent_repair_memory
        .implementation_target_lease
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if lease_path != path {
        return None;
    }
    Some(WriteLease {
        path: std::path::PathBuf::from(lease_path),
        range: None,
        expected_hash: None,
        allowed_operations: vec![WriteLeaseOperation::WriteFile],
        reason: "runtime write lease anchored to the implementation target".to_string(),
        expiry_turn: None,
    })
}

fn is_generated_path(path: &str) -> bool {
    std::path::Path::new(path).components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(part) if part == "target" || part == ".quorp-runs"
        )
    })
}
