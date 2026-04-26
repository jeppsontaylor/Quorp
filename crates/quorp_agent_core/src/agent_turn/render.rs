//! Pretty-printing for `AgentTurnResponse`. The renderer assembles a
//! multi-line string suitable for transcript scrollback.
//!
//! Extracted from `agent_turn.rs` so the parser logic file stays under
//! the 2000-LOC hard cap.

use crate::agent_protocol::ActionApprovalPolicy;
use crate::agent_context::AgentConfig;
use crate::agent_turn::AgentTurnResponse;
use crate::effective_approval_policy;

pub fn render_agent_turn_text(turn: &AgentTurnResponse, config: &AgentConfig) -> String {
    let mut lines = Vec::new();
    let assistant_message = turn.assistant_message.trim();
    if !assistant_message.is_empty() {
        lines.push(assistant_message.to_string());
    }

    if !turn.parse_warnings.is_empty() {
        lines.push("Parsing notes:".to_string());
        lines.extend(
            turn.parse_warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }

    if let Some(mode) = turn.requested_mode_change {
        lines.push(format!("Mode request: switch to {}", mode.label()));
    }

    if !turn.task_updates.is_empty() {
        lines.push("Task updates:".to_string());
        lines.extend(
            turn.task_updates
                .iter()
                .map(|item| format!("- [{}] {}", item.status.label(), item.title)),
        );
    }

    if let Some(plan) = turn.verifier_plan.as_ref() {
        let summary = plan.summary();
        if !summary.is_empty() {
            lines.push(format!("Verifier plan: {summary}"));
        }
    }

    if !turn.actions.is_empty() {
        lines.push("Action receipts:".to_string());
        for action in &turn.actions {
            let approval = match effective_approval_policy(action, config) {
                ActionApprovalPolicy::AutoApproveReadOnly => "auto",
                ActionApprovalPolicy::RequireExplicitConfirmation => "confirm",
            };
            lines.push(format!("- {} [{approval}]", action.summary()));
        }
    }

    if !turn.memory_updates.is_empty() {
        lines.push("Memory updates:".to_string());
        lines.extend(
            turn.memory_updates
                .iter()
                .map(|item| match item.path.as_deref() {
                    Some(path) => {
                        format!("- {} ({}): {}", item.kind.trim(), path, item.content.trim())
                    }
                    None => format!("- {}: {}", item.kind.trim(), item.content.trim()),
                }),
        );
    }

    lines.join("\n\n")
}
