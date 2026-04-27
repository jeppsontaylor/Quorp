use crate::agent_protocol::ActionOutcome;
use crate::agent_protocol::AgentAction;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ToolResultEnvelope {
    pub status: String,
    pub summary: String,
    pub text: String,
    pub artifacts: Vec<String>,
    pub evidence: Vec<String>,
    pub risk: Vec<String>,
    pub next_suggested_tools: Vec<String>,
}

impl ToolResultEnvelope {
    pub fn from_outcome(action: &AgentAction, outcome: &ActionOutcome) -> Self {
        let status = match outcome {
            ActionOutcome::Success { .. } => "success",
            ActionOutcome::Failure { .. } => "failure",
        };
        let text = outcome.output_text().to_string();
        let risk = match outcome {
            ActionOutcome::Failure { .. } if !text.trim().is_empty() => vec![text.clone()],
            _ => Vec::new(),
        };
        Self {
            status: status.to_string(),
            summary: action.summary(),
            text,
            artifacts: Vec::new(),
            evidence: Vec::new(),
            risk,
            next_suggested_tools: Vec::new(),
        }
    }
}

pub(crate) fn action_edit_summary(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::WriteFile { content, .. } => {
            Some(format!("write {} lines", content.lines().count()))
        }
        AgentAction::ApplyPatch { patch, .. } => {
            Some(format!("patch {} hunks", patch.matches("@@").count()))
        }
        AgentAction::ReplaceBlock {
            search_block,
            replace_block,
            ..
        } => Some(format!(
            "replace {} lines -> {} lines",
            search_block.lines().count(),
            replace_block.lines().count()
        )),
        AgentAction::ReplaceRange {
            range, replacement, ..
        } => Some(format!(
            "replace_range {} with {} lines",
            range.label(),
            replacement.lines().count()
        )),
        AgentAction::ModifyToml { operations, .. } => {
            Some(format!("modify_toml {} operations", operations.len()))
        }
        AgentAction::ApplyPreview { preview_id } => Some(format!("apply_preview {preview_id}")),
        AgentAction::SetExecutable { .. } => Some("set executable bit".to_string()),
        AgentAction::ProcessStart { command, args, cwd } => Some(format!(
            "process_start {} {}{}",
            command,
            if args.is_empty() {
                String::new()
            } else {
                format!("args({}) ", args.join(" "))
            },
            cwd.as_deref()
                .map(|cwd| format!("cwd({cwd})"))
                .unwrap_or_default()
        )),
        AgentAction::ProcessRead {
            process_id,
            tail_lines,
        } => Some(format!("process_read {} tail {}", process_id, tail_lines)),
        AgentAction::ProcessWrite { process_id, stdin } => Some(format!(
            "process_write {} bytes({})",
            process_id,
            stdin.len()
        )),
        AgentAction::ProcessStop { process_id } => Some(format!("process_stop {}", process_id)),
        AgentAction::ProcessWaitForPort {
            process_id,
            host,
            port,
            timeout_ms,
        } => Some(format!(
            "process_wait_for_port {} {}:{} timeout {}",
            process_id, host, port, timeout_ms
        )),
        AgentAction::BrowserOpen {
            url,
            headless,
            width,
            height,
        } => Some(format!(
            "browser_open {} headless={} viewport={:?}x{:?}",
            url, headless, width, height
        )),
        AgentAction::BrowserScreenshot { browser_id } => {
            Some(format!("browser_screenshot {}", browser_id))
        }
        AgentAction::BrowserConsoleLogs { browser_id, limit } => Some(format!(
            "browser_console_logs {} limit {}",
            browser_id, limit
        )),
        AgentAction::BrowserNetworkErrors { browser_id, limit } => Some(format!(
            "browser_network_errors {} limit {}",
            browser_id, limit
        )),
        AgentAction::BrowserAccessibilitySnapshot { browser_id } => {
            Some(format!("browser_accessibility_snapshot {}", browser_id))
        }
        AgentAction::BrowserClose { browser_id } => Some(format!("browser_close {}", browser_id)),
        _ => None,
    }
}
