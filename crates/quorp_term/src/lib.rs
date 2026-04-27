//! Terminal-native command parsing and compact transcript text.

use quorp_core::{PermissionMode, RunMode, SandboxMode};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlashCommand {
    Plan,
    Act,
    Auto,
    Manual,
    FullAuto,
    FullPermissions,
    Permissions(Option<String>),
    Sandbox(Option<SandboxMode>),
    Clear,
    Model(Option<String>),
    Provider(Option<String>),
    Memory,
    Rules,
    Session(Option<String>),
    Status,
    Init,
    Edit(Option<String>),
    Undo,
    Redo,
    Files,
    Hooks,
    Mcp,
    Diff,
    Apply,
    Revert,
    Test,
    Verify,
    Save,
    Load(Option<String>),
    Think,
    Compact,
    Doctor,
    Tasks,
    Checkpoint,
    Rollback,
    Theme,
    Help,
    Unknown(String),
}

pub fn parse_slash_command(input: &str) -> Option<SlashCommand> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    let (name, argument) = rest
        .split_once(char::is_whitespace)
        .map(|(name, argument)| (name, Some(argument.trim().to_string())))
        .unwrap_or((rest, None));
    Some(match name {
        "plan" => SlashCommand::Plan,
        "act" => SlashCommand::Act,
        "auto" => SlashCommand::Auto,
        "manual" => SlashCommand::Manual,
        "full-auto" => SlashCommand::FullAuto,
        "full-permissions" => SlashCommand::FullPermissions,
        "clear" => SlashCommand::Clear,
        "model" => SlashCommand::Model(argument),
        "provider" => SlashCommand::Provider(argument),
        "permissions" => SlashCommand::Permissions(argument),
        "perms" => SlashCommand::Permissions(argument),
        "sandbox" => SlashCommand::Sandbox(argument.and_then(|value| parse_sandbox_mode(&value))),
        "memory" | "mem" => SlashCommand::Memory,
        "rules" => SlashCommand::Rules,
        "session" => SlashCommand::Session(argument),
        "status" => SlashCommand::Status,
        "init" => SlashCommand::Init,
        "edit" => SlashCommand::Edit(argument),
        "undo" => SlashCommand::Undo,
        "redo" => SlashCommand::Redo,
        "files" | "f" => SlashCommand::Files,
        "hooks" => SlashCommand::Hooks,
        "mcp" => SlashCommand::Mcp,
        "diff" => SlashCommand::Diff,
        "apply" => SlashCommand::Apply,
        "revert" => SlashCommand::Revert,
        "test" => SlashCommand::Test,
        "verify" => SlashCommand::Verify,
        "save" => SlashCommand::Save,
        "load" => SlashCommand::Load(argument),
        "think" => SlashCommand::Think,
        "compact" => SlashCommand::Compact,
        "doctor" => SlashCommand::Doctor,
        "tasks" => SlashCommand::Tasks,
        "checkpoint" => SlashCommand::Checkpoint,
        "rollback" => SlashCommand::Rollback,
        "theme" => SlashCommand::Theme,
        "help" => SlashCommand::Help,
        "h" | "?" => SlashCommand::Help,
        other => SlashCommand::Unknown(other.to_string()),
    })
}

pub fn apply_mode_command(
    command: &SlashCommand,
    run_mode: &mut RunMode,
    permission_mode: &mut PermissionMode,
    sandbox: &mut SandboxMode,
) {
    match command {
        SlashCommand::Plan => *run_mode = RunMode::Plan,
        SlashCommand::Act => *run_mode = RunMode::Act,
        SlashCommand::Auto => *permission_mode = PermissionMode::FullAuto,
        SlashCommand::Manual => *permission_mode = PermissionMode::Ask,
        SlashCommand::FullAuto => *permission_mode = PermissionMode::FullAuto,
        SlashCommand::FullPermissions => *permission_mode = PermissionMode::FullPermissions,
        SlashCommand::Permissions(Some(value)) => {
            if let Some(mode) = parse_permission_mode(value) {
                *permission_mode = mode;
            }
        }
        SlashCommand::Sandbox(Some(mode)) => *sandbox = *mode,
        _ => {}
    }
}

pub fn startup_card(workspace: &str, model: &str, sandbox: SandboxMode) -> String {
    let width = workspace.width().max(model.width()).max(24);
    format!(
        "quorp\nworkspace  {workspace}\nmodel      {model}\nsandbox    {sandbox:?}\n{}",
        "-".repeat(width.min(72))
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationStatus {
    Running,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranscriptCard {
    Plan {
        title: String,
        steps: Vec<String>,
    },
    AssistantDelta {
        text: String,
    },
    ToolCall {
        name: String,
        detail: String,
    },
    ShellResult {
        command: String,
        exit_code: i32,
    },
    DiffPreview {
        files_changed: usize,
        summary: String,
    },
    Validation {
        label: String,
        status: ValidationStatus,
        frame: usize,
    },
    ApprovalWarning {
        title: String,
        detail: String,
    },
    ProofReceipt {
        path: String,
        summary: String,
    },
}

pub fn render_card(card: &TranscriptCard) -> String {
    match card {
        TranscriptCard::Plan { title, steps } => {
            let mut output = format!("\x1b[36mplan\x1b[0m {title}");
            for (index, step) in steps.iter().enumerate() {
                output.push_str(&format!("\n  {}. {step}", index + 1));
            }
            output
        }
        TranscriptCard::AssistantDelta { text } => text.to_string(),
        TranscriptCard::ToolCall { name, detail } => {
            format!("\x1b[35mtool\x1b[0m {name}  {detail}")
        }
        TranscriptCard::ShellResult { command, exit_code } => {
            let color = if *exit_code == 0 {
                "\x1b[32m"
            } else {
                "\x1b[31m"
            };
            format!("{color}shell\x1b[0m `{command}` exited {exit_code}")
        }
        TranscriptCard::DiffPreview {
            files_changed,
            summary,
        } => {
            format!("\x1b[34mdiff\x1b[0m {files_changed} file(s)  {summary}")
        }
        TranscriptCard::Validation {
            label,
            status,
            frame,
        } => render_validation(label, *status, *frame),
        TranscriptCard::ApprovalWarning { title, detail } => {
            format!("\x1b[33mapproval\x1b[0m {title}\n  {detail}")
        }
        TranscriptCard::ProofReceipt { path, summary } => {
            format!("\x1b[32mreceipt\x1b[0m {path}\n  {summary}")
        }
    }
}

pub fn render_validation(label: &str, status: ValidationStatus, frame: usize) -> String {
    match status {
        ValidationStatus::Running => {
            const FRAMES: &[&str] = &["-", "\\", "|", "/"];
            let glyph = FRAMES[frame % FRAMES.len()];
            format!("\x1b[36m{glyph} validating\x1b[0m {label}")
        }
        ValidationStatus::Passed => format!("\x1b[32m+ validated\x1b[0m {label}"),
        ValidationStatus::Failed => format!("\x1b[31mx failed\x1b[0m {label}"),
    }
}

fn parse_sandbox_mode(value: &str) -> Option<SandboxMode> {
    match value.trim() {
        "host" => Some(SandboxMode::Host),
        "tmp-copy" | "tmp_copy" => Some(SandboxMode::TmpCopy),
        _ => None,
    }
}

fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value.trim() {
        "ask" | "manual" => Some(PermissionMode::Ask),
        "auto" | "auto-safe" | "full-auto" | "full_auto" => Some(PermissionMode::FullAuto),
        "full-permissions" | "full_permissions" | "yolo" => Some(PermissionMode::FullPermissions),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quorp_slash_commands() {
        assert_eq!(parse_slash_command("/plan"), Some(SlashCommand::Plan));
        assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
        assert_eq!(
            parse_slash_command("/sandbox tmp-copy"),
            Some(SlashCommand::Sandbox(Some(SandboxMode::TmpCopy)))
        );
    }

    #[test]
    fn mode_commands_mutate_agent_state() {
        let mut run_mode = RunMode::Act;
        let mut permission_mode = PermissionMode::Ask;
        let mut sandbox = SandboxMode::Host;

        apply_mode_command(
            &SlashCommand::FullPermissions,
            &mut run_mode,
            &mut permission_mode,
            &mut sandbox,
        );
        apply_mode_command(
            &SlashCommand::Sandbox(Some(SandboxMode::TmpCopy)),
            &mut run_mode,
            &mut permission_mode,
            &mut sandbox,
        );

        assert_eq!(permission_mode, PermissionMode::FullPermissions);
        assert_eq!(sandbox, SandboxMode::TmpCopy);
    }

    #[test]
    fn renders_validation_with_colorful_running_frames() {
        assert_eq!(
            render_validation("cargo test", ValidationStatus::Running, 2),
            "\x1b[36m| validating\x1b[0m cargo test"
        );
        assert_eq!(
            render_validation("cargo test", ValidationStatus::Passed, 0),
            "\x1b[32m+ validated\x1b[0m cargo test"
        );
    }
}
