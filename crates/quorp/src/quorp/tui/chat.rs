#![allow(unused)]
//! Chat pane: transcript, composer, and model row.
//!
//! The TUI-only build keeps chat state, transcript rendering, and tool output in this module.
//! Native chat and command services drive prompt submission and tool execution for the pane.
//!
//! Pane focus uses **Tab** / **Shift+Tab** for cycling panes (same as the rest of the TUI). Model
//! selection uses **`[`** and **`]`** while the Chat pane is focused (see the model row hint).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use quorp_agent_core::PromptCompactionPolicy;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;

use crate::quorp::tui::agent_context::{
    AgentConfig, AutonomyProfile, effective_approval_policy, load_agent_config,
};
use crate::quorp::tui::agent_protocol::{
    ActionOutcome, AgentAction, AgentMode, PreviewEditPayload, ValidationPlan,
};
use crate::quorp::tui::agent_turn::{parse_agent_turn_response, render_agent_turn_text};
use crate::quorp::tui::assistant_transcript::{
    self, AssistantSegment, SegmentRenderOptions, TranscriptSurface,
};
use crate::quorp::tui::chat_service::{ChatServiceMessage, ChatServiceRequest, ChatServiceRole};
use crate::quorp::tui::mention_links::{expand_mentions_for_api_message, mention_link_for_path};
use crate::quorp::tui::path_index::{PathEntry, PathIndex, PathIndexProgress};
use crate::quorp::tui::slash_commands::{
    FullAutoLaunchSpec, LaunchDefaults, SlashCommand, latest_artifact_summary,
    latest_resume_target, parse_slash_command, prepare_launch_spec,
};

const MAX_MESSAGE_CHARS: usize = 512 * 1024;
const MAX_MESSAGES: usize = 500;

#[derive(Debug, Clone)]
pub enum ChatUiEvent {
    AssistantDelta(usize, String),
    StreamFinished(usize),
    Error(usize, String),
    CommandOutput(usize, String),
    CommandFinished(usize, ActionOutcome),
}

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum PersistedChatMessage {
    User(String),
    Assistant(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PersistedPendingCommand {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub read_start_line: Option<usize>,
    #[serde(default)]
    pub read_end_line: Option<usize>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(default)]
    pub search_block: Option<String>,
    #[serde(default)]
    pub replace_block: Option<String>,
    pub mcp_server_name: Option<String>,
    #[serde(default)]
    pub mcp_tool_name: Option<String>,
    #[serde(default)]
    pub mcp_arguments: Option<serde_json::Value>,
    #[serde(default)]
    pub validation_plan: Option<ValidationPlan>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PersistedChatThreadSnapshot {
    pub title: String,
    pub messages: Vec<PersistedChatMessage>,
    pub transcript_scroll: usize,
    pub stick_to_bottom: bool,
    pub input: String,
    pub last_error: Option<String>,
    pub pending_command: Option<PersistedPendingCommand>,
    #[serde(default)]
    pub pending_commands: Vec<PersistedPendingCommand>,
    pub running_command: bool,
    pub running_command_name: Option<String>,
    pub command_output_lines: Vec<String>,
    pub model_id: String,
    #[serde(default)]
    pub mode: AgentMode,
    #[serde(default)]
    pub prompt_compaction_policy: Option<PromptCompactionPolicy>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingCommand {
    action: AgentAction,
}

impl PendingCommand {
    fn new(action: AgentAction) -> Self {
        Self { action }
    }

    fn action(&self) -> &AgentAction {
        &self.action
    }

    fn summary(&self) -> String {
        match self.action() {
            AgentAction::ApplyPatch { path, patch } => {
                summarize_apply_patch_confirmation(path, patch)
            }
            _ => self.action.summary(),
        }
    }

    fn running_name(&self) -> String {
        self.summary()
    }

    fn followup_command_label(&self) -> String {
        self.action.followup_command_label()
    }

    fn to_persisted(&self) -> PersistedPendingCommand {
        match &self.action {
            AgentAction::RunCommand {
                command,
                timeout_ms,
            } => PersistedPendingCommand {
                kind: Some("run".to_string()),
                command: command.clone(),
                timeout_ms: *timeout_ms,
                query: None,
                limit: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::ReadFile { path, range } => PersistedPendingCommand {
                kind: Some("read_file".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: range
                    .and_then(|value| value.normalized())
                    .map(|value| value.start_line),
                read_end_line: range
                    .and_then(|value| value.normalized())
                    .map(|value| value.end_line),
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::ListDirectory { path } => PersistedPendingCommand {
                kind: Some("list_directory".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::SearchText { query, limit } => PersistedPendingCommand {
                kind: Some("search_text".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(query.clone()),
                limit: Some(*limit),
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::SearchSymbols { query, limit } => PersistedPendingCommand {
                kind: Some("search_symbols".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(query.clone()),
                limit: Some(*limit),
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::FindFiles { query, limit } => PersistedPendingCommand {
                kind: Some("find_files".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(query.clone()),
                limit: Some(*limit),
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::StructuralSearch {
                pattern,
                language,
                path,
                limit,
            } => PersistedPendingCommand {
                kind: Some("structural_search".to_string()),
                command: pattern.clone(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: language.clone(),
                limit: Some(*limit),
                path: path.clone(),
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::StructuralEditPreview {
                pattern,
                rewrite,
                language,
                path,
            } => PersistedPendingCommand {
                kind: Some("structural_edit_preview".to_string()),
                command: pattern.clone(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: language.clone(),
                limit: None,
                path: path.clone(),
                read_start_line: None,
                read_end_line: None,
                content: Some(rewrite.clone()),
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::CargoDiagnostics {
                command,
                include_clippy,
            } => PersistedPendingCommand {
                kind: Some("cargo_diagnostics".to_string()),
                command: command.clone().unwrap_or_default(),
                timeout_ms: Duration::from_secs(120).as_millis() as u64,
                query: Some(include_clippy.to_string()),
                limit: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::GetRepoCapsule { query, limit } => PersistedPendingCommand {
                kind: Some("get_repo_capsule".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: query.clone(),
                limit: Some(*limit),
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::ExplainValidationFailure { command, output } => PersistedPendingCommand {
                kind: Some("explain_validation_failure".to_string()),
                command: command.clone(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: Some(output.clone()),
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::SuggestImplementationTargets {
                command,
                output,
                failing_path,
                failing_line,
            } => PersistedPendingCommand {
                kind: Some("suggest_implementation_targets".to_string()),
                command: command.clone(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: *failing_line,
                path: failing_path.clone(),
                read_start_line: None,
                read_end_line: None,
                content: Some(output.clone()),
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::SuggestEditAnchors {
                path,
                range,
                search_hint,
            } => PersistedPendingCommand {
                kind: Some("suggest_edit_anchors".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: search_hint.clone(),
                limit: None,
                path: Some(path.clone()),
                read_start_line: range
                    .and_then(|value| value.normalized())
                    .map(|value| value.start_line),
                read_end_line: range
                    .and_then(|value| value.normalized())
                    .map(|value| value.end_line),
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::PreviewEdit { path, edit } => match edit {
                PreviewEditPayload::ApplyPatch { patch } => PersistedPendingCommand {
                    kind: Some("preview_edit_apply_patch".to_string()),
                    command: String::new(),
                    timeout_ms: Duration::from_secs(30).as_millis() as u64,
                    query: None,
                    limit: None,
                    path: Some(path.clone()),
                    read_start_line: None,
                    read_end_line: None,
                    content: None,
                    patch: Some(patch.clone()),
                    search_block: None,
                    replace_block: None,
                    mcp_server_name: None,
                    mcp_tool_name: None,
                    mcp_arguments: None,
                    validation_plan: None,
                },
                PreviewEditPayload::ReplaceBlock {
                    search_block,
                    replace_block,
                    range,
                } => PersistedPendingCommand {
                    kind: Some("preview_edit_replace_block".to_string()),
                    command: String::new(),
                    timeout_ms: Duration::from_secs(30).as_millis() as u64,
                    query: None,
                    limit: None,
                    path: Some(path.clone()),
                    read_start_line: range
                        .and_then(|value| value.normalized())
                        .map(|value| value.start_line),
                    read_end_line: range
                        .and_then(|value| value.normalized())
                        .map(|value| value.end_line),
                    content: None,
                    patch: None,
                    search_block: Some(search_block.clone()),
                    replace_block: Some(replace_block.clone()),
                    mcp_server_name: None,
                    mcp_tool_name: None,
                    mcp_arguments: None,
                    validation_plan: None,
                },
                PreviewEditPayload::ReplaceRange {
                    range,
                    expected_hash,
                    replacement,
                } => PersistedPendingCommand {
                    kind: Some("preview_edit_replace_range".to_string()),
                    command: String::new(),
                    timeout_ms: Duration::from_secs(30).as_millis() as u64,
                    query: Some(expected_hash.clone()),
                    limit: None,
                    path: Some(path.clone()),
                    read_start_line: range.normalized().map(|value| value.start_line),
                    read_end_line: range.normalized().map(|value| value.end_line),
                    content: Some(replacement.clone()),
                    patch: None,
                    search_block: None,
                    replace_block: None,
                    mcp_server_name: None,
                    mcp_tool_name: None,
                    mcp_arguments: None,
                    validation_plan: None,
                },
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                } => PersistedPendingCommand {
                    kind: Some("preview_edit_modify_toml".to_string()),
                    command: String::new(),
                    timeout_ms: Duration::from_secs(30).as_millis() as u64,
                    query: Some(expected_hash.clone()),
                    limit: None,
                    path: Some(path.clone()),
                    read_start_line: None,
                    read_end_line: None,
                    content: None,
                    patch: None,
                    search_block: None,
                    replace_block: None,
                    mcp_server_name: None,
                    mcp_tool_name: None,
                    mcp_arguments: Some(serde_json::to_value(operations).unwrap_or_else(
                        |error| serde_json::json!({ "serialization_error": error.to_string() }),
                    )),
                    validation_plan: None,
                },
            },
            AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                replacement,
            } => PersistedPendingCommand {
                kind: Some("replace_range".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(expected_hash.clone()),
                limit: None,
                path: Some(path.clone()),
                read_start_line: range.normalized().map(|value| value.start_line),
                read_end_line: range.normalized().map(|value| value.end_line),
                content: Some(replacement.clone()),
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } => PersistedPendingCommand {
                kind: Some("modify_toml".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(expected_hash.clone()),
                limit: None,
                path: Some(path.clone()),
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: Some(serde_json::to_value(operations).unwrap_or_else(
                    |error| serde_json::json!({ "serialization_error": error.to_string() }),
                )),
                validation_plan: None,
            },
            AgentAction::ApplyPreview { preview_id } => PersistedPendingCommand {
                kind: Some("apply_preview".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: Some(preview_id.clone()),
                limit: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::WriteFile { path, content } => PersistedPendingCommand {
                kind: Some("write_file".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: None,
                read_end_line: None,
                content: Some(content.clone()),
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::ApplyPatch { path, patch } => PersistedPendingCommand {
                kind: Some("apply_patch".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: Some(patch.clone()),
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::RunValidation { plan } => PersistedPendingCommand {
                kind: Some("run_validation".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: Some(plan.clone()),
            },
            AgentAction::ReplaceBlock {
                path,
                search_block,
                replace_block,
                range,
            } => PersistedPendingCommand {
                kind: Some("replace_block".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: range
                    .and_then(|range| range.normalized())
                    .map(|range| range.start_line),
                read_end_line: range
                    .and_then(|range| range.normalized())
                    .map(|range| range.end_line),
                content: None,
                patch: None,
                search_block: Some(search_block.clone()),
                replace_block: Some(replace_block.clone()),
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::SetExecutable { path } => PersistedPendingCommand {
                kind: Some("set_executable".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(30).as_millis() as u64,
                query: None,
                limit: None,
                path: Some(path.clone()),
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: None,
                mcp_tool_name: None,
                mcp_arguments: None,
                validation_plan: None,
            },
            AgentAction::McpCallTool {
                server_name,
                tool_name,
                arguments,
            } => PersistedPendingCommand {
                kind: Some("mcp_call_tool".to_string()),
                command: String::new(),
                timeout_ms: Duration::from_secs(60).as_millis() as u64,
                limit: None,
                query: None,
                path: None,
                read_start_line: None,
                read_end_line: None,
                content: None,
                patch: None,
                search_block: None,
                replace_block: None,
                mcp_server_name: Some(server_name.clone()),
                mcp_tool_name: Some(tool_name.clone()),
                mcp_arguments: Some(arguments.clone()),
                validation_plan: None,
            },
        }
    }

    fn from_persisted(pending: PersistedPendingCommand) -> Option<Self> {
        match pending.kind.as_deref() {
            Some("run") => Some(Self::new(AgentAction::RunCommand {
                command: pending.command,
                timeout_ms: pending.timeout_ms,
            })),
            Some("read_file") => Some(Self::new(AgentAction::ReadFile {
                path: pending.path?,
                range: match (pending.read_start_line, pending.read_end_line) {
                    (Some(start_line), Some(end_line)) => quorp_agent_core::ReadFileRange {
                        start_line,
                        end_line,
                    }
                    .normalized(),
                    _ => None,
                },
            })),
            Some("list_directory") => Some(Self::new(AgentAction::ListDirectory {
                path: pending.path?,
            })),
            Some("search_text") => Some(Self::new(AgentAction::SearchText {
                query: pending.query.unwrap_or_default(),
                limit: pending.limit.unwrap_or(6),
            })),
            Some("search_symbols") => Some(Self::new(AgentAction::SearchSymbols {
                query: pending.query.unwrap_or_default(),
                limit: pending.limit.unwrap_or(6),
            })),
            Some("find_files") => Some(Self::new(AgentAction::FindFiles {
                query: pending.query.unwrap_or_default(),
                limit: pending.limit.unwrap_or(20),
            })),
            Some("structural_search") => Some(Self::new(AgentAction::StructuralSearch {
                pattern: pending.command,
                language: pending.query,
                path: pending.path,
                limit: pending.limit.unwrap_or(20),
            })),
            Some("structural_edit_preview") => {
                Some(Self::new(AgentAction::StructuralEditPreview {
                    pattern: pending.command,
                    rewrite: pending.content.unwrap_or_default(),
                    language: pending.query,
                    path: pending.path,
                }))
            }
            Some("cargo_diagnostics") => Some(Self::new(AgentAction::CargoDiagnostics {
                command: (!pending.command.trim().is_empty()).then_some(pending.command),
                include_clippy: pending
                    .query
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("true")),
            })),
            Some("get_repo_capsule") => Some(Self::new(AgentAction::GetRepoCapsule {
                query: pending.query,
                limit: pending.limit.unwrap_or(8),
            })),
            Some("explain_validation_failure") => {
                Some(Self::new(AgentAction::ExplainValidationFailure {
                    command: pending.command,
                    output: pending.content.unwrap_or_default(),
                }))
            }
            Some("suggest_implementation_targets") => {
                Some(Self::new(AgentAction::SuggestImplementationTargets {
                    command: pending.command,
                    output: pending.content.unwrap_or_default(),
                    failing_path: pending.path,
                    failing_line: pending.limit,
                }))
            }
            Some("suggest_edit_anchors") => Some(Self::new(AgentAction::SuggestEditAnchors {
                path: pending.path?,
                range: match (pending.read_start_line, pending.read_end_line) {
                    (Some(start_line), Some(end_line)) => quorp_agent_core::ReadFileRange {
                        start_line,
                        end_line,
                    }
                    .normalized(),
                    _ => None,
                },
                search_hint: pending.query,
            })),
            Some("preview_edit_apply_patch") => Some(Self::new(AgentAction::PreviewEdit {
                path: pending.path?,
                edit: PreviewEditPayload::ApplyPatch {
                    patch: pending.patch.unwrap_or_default(),
                },
            })),
            Some("preview_edit_replace_block") => Some(Self::new(AgentAction::PreviewEdit {
                path: pending.path?,
                edit: PreviewEditPayload::ReplaceBlock {
                    search_block: pending.search_block.unwrap_or_default(),
                    replace_block: pending.replace_block.unwrap_or_default(),
                    range: match (pending.read_start_line, pending.read_end_line) {
                        (Some(start_line), Some(end_line)) => quorp_agent_core::ReadFileRange {
                            start_line,
                            end_line,
                        }
                        .normalized(),
                        _ => None,
                    },
                },
            })),
            Some("preview_edit_replace_range") => Some(Self::new(AgentAction::PreviewEdit {
                path: pending.path?,
                edit: PreviewEditPayload::ReplaceRange {
                    range: quorp_agent_core::ReadFileRange {
                        start_line: pending.read_start_line?,
                        end_line: pending.read_end_line?,
                    }
                    .normalized()?,
                    expected_hash: pending.query?,
                    replacement: pending.content.unwrap_or_default(),
                },
            })),
            Some("preview_edit_modify_toml") => Some(Self::new(AgentAction::PreviewEdit {
                path: pending.path?,
                edit: PreviewEditPayload::ModifyToml {
                    expected_hash: pending.query?,
                    operations: serde_json::from_value(pending.mcp_arguments?).ok()?,
                },
            })),
            Some("replace_range") => Some(Self::new(AgentAction::ReplaceRange {
                path: pending.path?,
                range: quorp_agent_core::ReadFileRange {
                    start_line: pending.read_start_line?,
                    end_line: pending.read_end_line?,
                }
                .normalized()?,
                expected_hash: pending.query?,
                replacement: pending.content.unwrap_or_default(),
            })),
            Some("modify_toml") => Some(Self::new(AgentAction::ModifyToml {
                path: pending.path?,
                expected_hash: pending.query?,
                operations: serde_json::from_value(pending.mcp_arguments?).ok()?,
            })),
            Some("apply_preview") => Some(Self::new(AgentAction::ApplyPreview {
                preview_id: pending.query?,
            })),
            Some("write_file") => Some(Self::new(AgentAction::WriteFile {
                path: pending.path?,
                content: pending.content.unwrap_or_default(),
            })),
            Some("apply_patch") => Some(Self::new(AgentAction::ApplyPatch {
                path: pending.path?,
                patch: pending.patch.unwrap_or_default(),
            })),
            Some("run_validation") => Some(Self::new(AgentAction::RunValidation {
                plan: pending.validation_plan.unwrap_or_default(),
            })),
            Some("replace_block") => Some(Self::new(AgentAction::ReplaceBlock {
                path: pending.path.unwrap_or_default(),
                search_block: pending.search_block.unwrap_or_default(),
                replace_block: pending.replace_block.unwrap_or_default(),
                range: match (pending.read_start_line, pending.read_end_line) {
                    (Some(start_line), Some(end_line)) => quorp_agent_core::ReadFileRange {
                        start_line,
                        end_line,
                    }
                    .normalized(),
                    _ => None,
                },
            })),
            Some("set_executable") => Some(Self::new(AgentAction::SetExecutable {
                path: pending.path.unwrap_or_default(),
            })),
            Some("mcp_call_tool") => Some(Self::new(AgentAction::McpCallTool {
                server_name: pending.mcp_server_name.unwrap_or_default(),
                tool_name: pending.mcp_tool_name.unwrap_or_default(),
                arguments: pending
                    .mcp_arguments
                    .unwrap_or_else(|| serde_json::json!({})),
            })),
            None => {
                if !pending.command.is_empty() {
                    Some(Self::new(AgentAction::RunCommand {
                        command: pending.command,
                        timeout_ms: pending.timeout_ms,
                    }))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn summarize_apply_patch_confirmation(path: &str, patch: &str) -> String {
    let touched_paths = collect_patch_touched_paths(patch);
    if touched_paths.is_empty() {
        return format!("apply_patch {path}");
    }
    if touched_paths.len() == 1 {
        return format!("apply_patch {}", touched_paths[0]);
    }

    let preview = touched_paths.iter().take(3).cloned().collect::<Vec<_>>();
    let remaining = touched_paths.len().saturating_sub(preview.len());
    let mut summary = format!(
        "apply_patch {} files: {}",
        touched_paths.len(),
        preview.join(", ")
    );
    if remaining > 0 {
        summary.push_str(&format!(", +{remaining} more"));
    }
    summary
}

fn collect_patch_touched_paths(patch: &str) -> Vec<String> {
    let normalized = patch.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().starts_with("*** Begin Patch") {
        return collect_model_patch_touched_paths(&normalized);
    }
    collect_unified_patch_touched_paths(&normalized)
}

fn collect_unified_patch_touched_paths(patch: &str) -> Vec<String> {
    let mut touched_paths = Vec::new();
    let mut old_path: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;

    for line in patch.lines() {
        if line.starts_with("diff --git ") || line.starts_with("diff -") {
            old_path = None;
            rename_from = None;
            rename_to = None;
            continue;
        }
        if let Some(path) = line.strip_prefix("rename from ") {
            rename_from = Some(path.trim().to_string());
            continue;
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            rename_to = Some(path.trim().to_string());
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = Some(strip_patch_prefix(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            let new_path = strip_patch_prefix(path);
            if let (Some(rename_source), Some(rename_target)) =
                (rename_from.as_deref(), rename_to.as_deref())
            {
                push_unique_path(&mut touched_paths, rename_source);
                push_unique_path(&mut touched_paths, rename_target);
            } else if old_path.as_deref() == Some("/dev/null") {
                push_unique_path(&mut touched_paths, &new_path);
            } else if new_path == "/dev/null" {
                if let Some(previous) = old_path.as_deref() {
                    push_unique_path(&mut touched_paths, previous);
                }
            } else {
                push_unique_path(&mut touched_paths, &new_path);
            }
        }
    }

    touched_paths
}

fn collect_model_patch_touched_paths(patch: &str) -> Vec<String> {
    let mut touched_paths = Vec::new();
    let mut current_path: Option<String> = None;

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Begin File: ") {
            let path = path.trim();
            current_path = Some(path.to_string());
            push_unique_path(&mut touched_paths, path);
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            push_unique_path(&mut touched_paths, path.trim());
            current_path = None;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Move To: ") {
            push_unique_path(&mut touched_paths, path.trim());
            continue;
        }
        if line == "*** End File" {
            current_path = None;
            continue;
        }
        if current_path.is_none() && line == "*** End Patch" {
            break;
        }
    }

    touched_paths
}

fn strip_patch_prefix(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/dev/null" {
        return trimmed.to_string();
    }
    trimmed
        .strip_prefix("a/")
        .or_else(|| trimmed.strip_prefix("b/"))
        .unwrap_or(trimmed)
        .to_string()
}

fn push_unique_path(paths: &mut Vec<String>, path: &str) {
    if path.is_empty() {
        return;
    }
    if paths.iter().any(|existing| existing == path) {
        return;
    }
    paths.push(path.to_string());
}

impl ChatMessage {
    fn push_assistant(&mut self, delta: &str) {
        let ChatMessage::Assistant(s) = self else {
            return;
        };
        let remaining = MAX_MESSAGE_CHARS.saturating_sub(s.len());
        if delta.len() <= remaining {
            s.push_str(delta);
            return;
        }
        if remaining > 0 {
            let take = delta.floor_char_boundary(remaining);
            s.push_str(&delta[..take]);
        }
        s.push_str("\n… [truncated]");
    }
}

impl From<&ChatMessage> for PersistedChatMessage {
    fn from(value: &ChatMessage) -> Self {
        match value {
            ChatMessage::User(text) => Self::User(text.clone()),
            ChatMessage::Assistant(text) => Self::Assistant(text.clone()),
        }
    }
}

impl From<PersistedChatMessage> for ChatMessage {
    fn from(value: PersistedChatMessage) -> Self {
        match value {
            PersistedChatMessage::User(text) => Self::User(text),
            PersistedChatMessage::Assistant(text) => Self::Assistant(text),
        }
    }
}

#[derive(Debug)]
struct MentionPopup {
    at_byte: usize,
    selected: usize,
    /// Index of the first visible row in `matches` (for lists taller than the viewport).
    scroll_top: usize,
    matches: Vec<PathEntry>,
    last_query: String,
}

#[derive(Debug, Clone)]
struct CachedRenderedAssistantLines {
    theme_key: u64,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone)]
struct CachedAssistantMessage {
    segments: Vec<AssistantSegment>,
    chat_render: Option<CachedRenderedAssistantLines>,
    shell_render: Option<CachedRenderedAssistantLines>,
}

#[derive(Debug, Clone)]
enum CachedTranscriptMessage {
    User(String),
    Assistant(CachedAssistantMessage),
}

#[derive(Debug, Clone)]
struct TranscriptCache {
    dirty: bool,
    messages: Vec<CachedTranscriptMessage>,
}

impl Default for TranscriptCache {
    fn default() -> Self {
        Self {
            dirty: true,
            messages: Vec::new(),
        }
    }
}

pub struct ChatSession {
    pub title: String,
    pub mode: AgentMode,
    pub prompt_compaction_policy: Option<PromptCompactionPolicy>,
    pub messages: Vec<ChatMessage>,
    pub transcript_scroll: usize,
    pub stick_to_bottom: bool,
    pub input: String,
    pub cursor_char: usize,
    pub last_error: Option<String>,
    pub pending_commands: Vec<PendingCommand>,
    pub running_command: bool,
    pub running_command_name: Option<String>,
    pub command_output_lines: Vec<String>,
    mention_popup: Option<MentionPopup>,
    pub streaming: bool,
    transcript_cache: TranscriptCache,
}

impl ChatSession {
    pub(crate) fn new(title: String) -> Self {
        Self {
            title,
            mode: AgentMode::Act,
            prompt_compaction_policy: None,
            messages: Vec::new(),
            transcript_scroll: 0,
            stick_to_bottom: true,
            input: String::new(),
            cursor_char: 0,
            last_error: None,
            pending_commands: Vec::new(),
            running_command: false,
            running_command_name: None,
            command_output_lines: Vec::new(),
            mention_popup: None,
            streaming: false,
            transcript_cache: TranscriptCache::default(),
        }
    }
}

pub struct ChatPane {
    ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    sessions: Vec<ChatSession>,
    active_session: usize,
    models: Vec<String>,
    model_index: usize,
    viewport_transcript_lines: usize,
    last_text_width: usize,
    chat_service_tx: futures::channel::mpsc::UnboundedSender<ChatServiceRequest>,
    command_bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<
            crate::quorp::tui::command_bridge::CommandBridgeRequest,
        >,
    >,
    agent_config: AgentConfig,
    default_prompt_compaction_policy: Option<PromptCompactionPolicy>,
    project_root: PathBuf,
    path_index: Arc<PathIndex>,
    shell_feed_dirty: bool,
    shell_feed_submitted: bool,
    base_url_override: Option<String>,
}

fn char_index_for_byte(input: &str, byte: usize) -> usize {
    let byte = byte.min(input.len());
    input[..byte].chars().count()
}

fn char_byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(input.len())
}

const MENTION_POPUP_MAX_VISIBLE: usize = 8;

fn clamp_mention_scroll(
    scroll_top: &mut usize,
    selected: usize,
    visible_rows: usize,
    total: usize,
) {
    if total == 0 {
        *scroll_top = 0;
        return;
    }
    let v = visible_rows.min(total).max(1);
    if selected < *scroll_top {
        *scroll_top = selected;
    }
    if selected >= *scroll_top + v {
        *scroll_top = selected + 1 - v;
    }
    let max_top = total.saturating_sub(v);
    *scroll_top = (*scroll_top).min(max_top);
}

fn active_mention_token(input: &str, cursor_byte: usize) -> Option<(usize, String)> {
    let before = input.get(..cursor_byte)?;
    let at_byte = before.rfind('@')?;
    if at_byte > 0 {
        let prev = input[..at_byte].chars().next_back()?;
        if prev.is_alphanumeric() || prev == '_' {
            return None;
        }
    }
    let after_at_start = at_byte + '@'.len_utf8();
    let after_at = input.get(after_at_start..cursor_byte)?;
    if after_at.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((at_byte, after_at.to_string()))
}

fn parse_command_timeout(timeout_ms: Option<&str>) -> Duration {
    timeout_ms
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(30))
}

fn parse_compaction_command(
    input: &str,
) -> Option<std::result::Result<Option<PromptCompactionPolicy>, String>> {
    let remainder = input.strip_prefix("/compaction")?;
    let policy = remainder.trim();
    if policy.is_empty() {
        return Some(Err(
                "Usage: /compaction default|last6-ledger768|last8-ledger1024|benchmark-state-packet|off".to_string(),
        ));
    }
    Some(match policy {
        "default" => Ok(None),
        "last6-ledger768" => Ok(Some(PromptCompactionPolicy::Last6Ledger768)),
        "last8-ledger1024" => Ok(Some(PromptCompactionPolicy::Last8Ledger1024)),
        "benchmark-state-packet" => Ok(Some(PromptCompactionPolicy::BenchmarkStatePacket)),
        "off" => Ok(Some(PromptCompactionPolicy::Off)),
        other => Err(format!(
            "Unknown compaction policy `{other}`. Use /compaction default|last6-ledger768|last8-ledger1024|benchmark-state-packet|off."
        )),
    })
}

fn agent_action_from_assistant_segment(segment: &AssistantSegment) -> Option<AgentAction> {
    match segment {
        AssistantSegment::RunCommand {
            command,
            timeout_ms,
        } => Some(AgentAction::RunCommand {
            command: command.clone(),
            timeout_ms: *timeout_ms,
        }),
        AssistantSegment::ReadFile { path, range } => Some(AgentAction::ReadFile {
            path: path.clone(),
            range: *range,
        }),
        AssistantSegment::ListDirectory { path } => {
            Some(AgentAction::ListDirectory { path: path.clone() })
        }
        AssistantSegment::WriteFile { path, content } => Some(AgentAction::WriteFile {
            path: path.clone(),
            content: content.clone(),
        }),
        AssistantSegment::ApplyPatch { path, patch } => Some(AgentAction::ApplyPatch {
            path: path.clone(),
            patch: patch.clone(),
        }),
        AssistantSegment::ReplaceBlock {
            path,
            search_block,
            replace_block,
        } => Some(AgentAction::ReplaceBlock {
            path: path.clone(),
            search_block: search_block.clone(),
            replace_block: replace_block.clone(),
            range: None,
        }),
        AssistantSegment::McpCallTool {
            server_name,
            tool_name,
            arguments,
        } => Some(AgentAction::McpCallTool {
            server_name: server_name.clone(),
            tool_name: tool_name.clone(),
            arguments: arguments.clone(),
        }),
        _ => None,
    }
}

fn env_requested_chat_model_id(
    default_provider: crate::quorp::executor::InteractiveProviderKind,
) -> Option<String> {
    let raw = crate::quorp::provider_config::resolved_model_env()
        .or_else(|| crate::quorp::provider_config::env_value("QUORP_TUI_MODEL"))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let already_scoped = [
        "ollama/",
        "codex/",
        "openai-compatible/",
        "openai/",
        "nvidia/",
        "local/",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix));
    if already_scoped
        || crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(trimmed).is_some()
        || matches!(
            default_provider,
            crate::quorp::executor::InteractiveProviderKind::Local
        )
    {
        return Some(trimmed.to_string());
    }
    Some(format!("{}/{}", default_provider.label(), trimmed))
}

fn env_requested_prompt_compaction_policy() -> Option<PromptCompactionPolicy> {
    let raw = std::env::var("QUORP_PROMPT_COMPACTION_POLICY").ok()?;
    PromptCompactionPolicy::parse(raw.trim())
}

fn select_or_insert_model(models: &mut Vec<String>, candidate: &str) -> usize {
    if let Some(index) = models.iter().position(|model| model == candidate) {
        return index;
    }
    models.insert(0, candidate.to_string());
    0
}

fn is_validation_command(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    [
        "cargo test",
        "cargo clippy",
        "cargo fmt",
        "./evaluate.sh",
        "pytest",
        "go test",
        "npm test",
        "pnpm test",
        "bun test",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
}

impl ChatPane {
    pub fn new(
        ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        project_root: PathBuf,
        path_index: Arc<PathIndex>,
        unified_language_model: Option<(
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
            Vec<String>,
            usize,
        )>,
        command_bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<
                crate::quorp::tui::command_bridge::CommandBridgeRequest,
            >,
        >,
    ) -> Self {
        let agent_config = load_agent_config(project_root.as_path());
        let chat_service_tx =
            crate::quorp::tui::chat_service::spawn_chat_service_loop(ui_tx.clone());
        let default_provider = crate::quorp::executor::interactive_provider_from_env();
        let default_prompt_compaction_policy = env_requested_prompt_compaction_policy();
        let (models, model_index) = match unified_language_model {
            Some((_tx, m, idx)) => {
                let mut models = m;
                let mut model_index = if models.is_empty() {
                    0
                } else {
                    idx.min(models.len() - 1)
                };
                if let Some(requested_model_id) = env_requested_chat_model_id(default_provider) {
                    model_index = select_or_insert_model(&mut models, &requested_model_id);
                } else if let Some(default_model_id) =
                    agent_config.defaults.default_model_id.as_ref()
                {
                    model_index = select_or_insert_model(&mut models, default_model_id);
                } else if let Some(saved_model_id) =
                    crate::quorp::tui::model_registry::get_saved_chat_model_id()
                {
                    model_index = select_or_insert_model(&mut models, &saved_model_id);
                }
                (models, model_index)
            }
            None => {
                let mut models =
                    crate::quorp::tui::model_registry::interactive_chat_catalog(default_provider);
                let mut model_index = 0usize;
                if let Some(requested_model_id) = env_requested_chat_model_id(default_provider) {
                    model_index = select_or_insert_model(&mut models, &requested_model_id);
                } else if let Some(default_model_id) =
                    agent_config.defaults.default_model_id.as_ref()
                {
                    model_index = select_or_insert_model(&mut models, default_model_id);
                } else if let Some(saved_model_id) =
                    crate::quorp::tui::model_registry::get_saved_chat_model_id()
                {
                    model_index = select_or_insert_model(&mut models, &saved_model_id);
                } else if let Some(default_model_id) =
                    crate::quorp::tui::model_registry::default_interactive_model_id(
                        default_provider,
                    )
                {
                    model_index = select_or_insert_model(&mut models, &default_model_id);
                }
                (models, model_index)
            }
        };

        let mut first_session = ChatSession::new("Chat 1".to_string());
        first_session.mode = agent_config.defaults.mode;
        first_session.prompt_compaction_policy = default_prompt_compaction_policy;

        Self {
            ui_tx: ui_tx.clone(),
            sessions: vec![first_session],
            active_session: 0,
            models,
            model_index,
            viewport_transcript_lines: 1,
            last_text_width: 60,
            chat_service_tx,
            command_bridge_tx: command_bridge_tx.or_else(|| {
                let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
                let _command_thread = crate::quorp::tui::native_backend::spawn_command_service_loop(
                    ui_tx.clone(),
                    command_rx,
                );
                Some(command_tx)
            }),
            agent_config,
            default_prompt_compaction_policy,
            project_root,
            path_index,
            shell_feed_dirty: true,
            shell_feed_submitted: false,
            base_url_override: crate::quorp::provider_config::env_value("QUORP_CHAT_BASE_URL"),
        }
    }

    pub fn request_persist_default_model_to_agent_settings(&self, registry_line: &str) {
        if let Err(error) = crate::quorp::tui::model_registry::save_chat_model_id(registry_line) {
            log::error!("tui: failed to persist default chat model {registry_line:?}: {error}");
        }
    }

    fn active_session_mut(&mut self) -> &mut ChatSession {
        &mut self.sessions[self.active_session]
    }

    fn active_session_ref(&self) -> &ChatSession {
        &self.sessions[self.active_session]
    }

    fn pending_command_for_session(&self, session_index: usize) -> Option<&PendingCommand> {
        self.sessions
            .get(session_index)
            .and_then(|session| session.pending_commands.first())
    }

    fn pending_command_for_active_session(&self) -> Option<&PendingCommand> {
        self.pending_command_for_session(self.active_session)
    }

    fn clear_pending_commands_for_session(&mut self, session_index: usize) {
        if let Some(session) = self.sessions.get_mut(session_index) {
            session.pending_commands.clear();
        }
    }

    fn pending_queue_len_for_session(&self, session_index: usize) -> usize {
        self.sessions
            .get(session_index)
            .map(|session| session.pending_commands.len())
            .unwrap_or(0)
    }

    fn mark_session_transcript_dirty(&mut self, session_index: usize) {
        if let Some(session) = self.sessions.get_mut(session_index) {
            session.transcript_cache.dirty = true;
        }
        self.shell_feed_dirty = true;
    }

    fn mark_active_session_transcript_dirty(&mut self) {
        self.mark_session_transcript_dirty(self.active_session);
    }

    fn rebuild_session_transcript_cache_if_needed(&mut self, session_index: usize) {
        let Some(session) = self.sessions.get_mut(session_index) else {
            return;
        };
        if !session.transcript_cache.dirty {
            return;
        }

        session.transcript_cache.messages = session
            .messages
            .iter()
            .enumerate()
            .map(|(message_index, message)| match message {
                ChatMessage::User(text) => CachedTranscriptMessage::User(text.clone()),
                ChatMessage::Assistant(text) => {
                    CachedTranscriptMessage::Assistant(CachedAssistantMessage {
                        segments: assistant_transcript::parse_assistant_segments(
                            text,
                            session_index,
                            message_index,
                            TranscriptSurface::Chat,
                        ),
                        chat_render: None,
                        shell_render: None,
                    })
                }
            })
            .collect();
        session.transcript_cache.dirty = false;
    }

    fn cached_assistant_lines(
        &mut self,
        session_index: usize,
        message_index: usize,
        theme: &crate::quorp::tui::theme::Theme,
        options: SegmentRenderOptions,
    ) -> Vec<Line<'static>> {
        self.rebuild_session_transcript_cache_if_needed(session_index);
        let Some(session) = self.sessions.get_mut(session_index) else {
            return Vec::new();
        };
        let Some(CachedTranscriptMessage::Assistant(message)) =
            session.transcript_cache.messages.get_mut(message_index)
        else {
            return Vec::new();
        };

        let render_slot = match options.surface {
            TranscriptSurface::Chat => &mut message.chat_render,
            TranscriptSurface::Shell => &mut message.shell_render,
        };
        let theme_key = render_theme_key(theme);
        if let Some(cached) = render_slot.as_ref()
            && cached.theme_key == theme_key
        {
            return cached.lines.clone();
        }

        let lines =
            assistant_transcript::render_assistant_segments(&message.segments, theme, options);
        *render_slot = Some(CachedRenderedAssistantLines {
            theme_key,
            lines: lines.clone(),
        });
        lines
    }

    fn abort_streaming(&mut self) {
        let session_id = self.active_session;
        self.active_session_mut().streaming = false;
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id })
            .is_err()
        {
            log::warn!("tui: failed to cancel chat stream for session {session_id}");
        }
    }

    fn cancel_stream_for_session(&mut self, session_id: usize) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.streaming = false;
        }
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id })
            .is_err()
        {
            log::warn!("tui: failed to cancel chat stream for session {session_id}");
        }
    }

    fn build_service_messages_for_session(&self, session_index: usize) -> Vec<ChatServiceMessage> {
        let Some(session) = self.sessions.get(session_index) else {
            return Vec::new();
        };
        session
            .messages
            .iter()
            .filter_map(|message| match message {
                ChatMessage::User(content) => Some(ChatServiceMessage {
                    role: ChatServiceRole::User,
                    content: expand_mentions_for_api_message(content, self.project_root.as_path()),
                }),
                ChatMessage::Assistant(content) if !content.trim().is_empty() => {
                    Some(ChatServiceMessage {
                        role: ChatServiceRole::Assistant,
                        content: content.clone(),
                    })
                }
                ChatMessage::Assistant(_) => None,
            })
            .collect()
    }

    fn base_url_override_for_service(&self) -> Option<String> {
        self.base_url_override.clone()
    }

    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    pub fn export_active_thread_snapshot(&self) -> PersistedChatThreadSnapshot {
        let session = self.active_session_ref();
        PersistedChatThreadSnapshot {
            title: session.title.clone(),
            messages: session
                .messages
                .iter()
                .map(PersistedChatMessage::from)
                .collect(),
            transcript_scroll: session.transcript_scroll,
            stick_to_bottom: session.stick_to_bottom,
            input: session.input.clone(),
            last_error: session.last_error.clone(),
            pending_command: session
                .pending_commands
                .first()
                .map(|pending| pending.to_persisted()),
            pending_commands: session
                .pending_commands
                .iter()
                .map(PendingCommand::to_persisted)
                .collect(),
            running_command: session.running_command,
            running_command_name: session.running_command_name.clone(),
            command_output_lines: session.command_output_lines.clone(),
            model_id: self.current_model_id().to_string(),
            mode: session.mode,
            prompt_compaction_policy: session.prompt_compaction_policy,
        }
    }

    pub fn import_thread_snapshot(&mut self, snapshot: PersistedChatThreadSnapshot) {
        let PersistedChatThreadSnapshot {
            title,
            messages,
            transcript_scroll,
            stick_to_bottom,
            input,
            last_error,
            pending_command,
            pending_commands,
            running_command,
            running_command_name,
            command_output_lines,
            model_id,
            mode,
            prompt_compaction_policy,
        } = snapshot;
        let model_index = if let Some(requested_model_id) =
            env_requested_chat_model_id(crate::quorp::executor::interactive_provider_from_env())
        {
            self.models
                .iter()
                .position(|candidate| candidate == &requested_model_id)
                .unwrap_or(self.model_index.min(self.models.len().saturating_sub(1)))
        } else {
            self.models
                .iter()
                .position(|candidate| candidate == &model_id)
                .unwrap_or(self.model_index.min(self.models.len().saturating_sub(1)))
        };
        self.model_index = model_index;
        let pending_commands = if pending_commands.is_empty() {
            pending_command
                .into_iter()
                .filter_map(PendingCommand::from_persisted)
                .collect()
        } else {
            pending_commands
                .into_iter()
                .filter_map(PendingCommand::from_persisted)
                .collect()
        };
        self.sessions = vec![ChatSession {
            title,
            messages: messages.into_iter().map(ChatMessage::from).collect(),
            transcript_scroll,
            stick_to_bottom,
            cursor_char: input.chars().count(),
            input,
            last_error,
            pending_commands,
            running_command,
            running_command_name,
            command_output_lines,
            mode,
            prompt_compaction_policy,
            mention_popup: None,
            streaming: false,
            transcript_cache: TranscriptCache::default(),
        }];
        self.active_session = 0;
    }

    pub fn ensure_project_root(&mut self, root: &std::path::Path) {
        if self.project_root.as_path() == root {
            return;
        }
        self.project_root = root.to_path_buf();
        self.agent_config = load_agent_config(self.project_root.as_path());
        self.path_index.set_root(self.project_root.clone());
    }

    /// Match production + [`TuiTestHarness::new_with_backend_state`]: backend snapshots drive the index
    /// (no background `ignore` walk). Mention tests that need a disk scan use `new_with_root` instead.
    #[cfg(test)]
    pub fn use_project_backed_path_index_for_backend_flow_tests(&mut self, root: PathBuf) {
        let watch = std::sync::Arc::new(std::sync::RwLock::new(root.clone()));
        self.path_index = std::sync::Arc::new(
            crate::quorp::tui::path_index::PathIndex::new_project_backed(root, watch),
        );
    }

    pub fn blocking_wait_path_index_ready(&self, timeout: Duration) -> bool {
        self.path_index
            .blocking_wait_for_ready(self.project_root.as_path(), timeout)
    }

    pub fn path_index_progress(&self) -> PathIndexProgress {
        self.path_index.snapshot_progress()
    }

    pub fn apply_path_index_snapshot(
        &mut self,
        root: std::path::PathBuf,
        entries: Arc<Vec<PathEntry>>,
        files_seen: u64,
    ) {
        self.path_index
            .apply_bridge_snapshot(root, entries, files_seen);
    }

    pub fn is_streaming(&self) -> bool {
        self.active_session_ref().streaming
    }

    pub fn active_session_index(&self) -> usize {
        self.active_session
    }

    pub fn shell_session_pills(&self, max_sessions: usize) -> Vec<(String, bool, bool)> {
        self.sessions
            .iter()
            .take(max_sessions)
            .enumerate()
            .map(|(index, session)| {
                (
                    session.title.clone(),
                    index == self.active_session,
                    session.streaming,
                )
            })
            .collect()
    }

    pub fn shell_session_label(&self) -> String {
        let session = self.active_session_ref();
        format!("Assistant {}", session.title)
    }

    pub fn shell_session_identity(&self) -> String {
        format!(
            "{} · {} · Compaction: {}",
            self.shell_session_label(),
            self.active_session_ref().mode.label(),
            self.active_prompt_compaction_policy_label(),
        )
    }

    pub fn shell_composer_text(&self) -> String {
        let session = self.active_session_ref();
        if session.streaming {
            "Streaming response...".to_string()
        } else if session.input.is_empty() {
            format!("{} mode: ask the assistant...", session.mode.label())
        } else {
            session.input.clone()
        }
    }

    fn set_active_mode(&mut self, mode: AgentMode) {
        self.active_session_mut().mode = mode;
        self.shell_feed_dirty = true;
    }

    pub fn active_mode_for_test(&self) -> AgentMode {
        self.active_session_ref().mode
    }

    pub fn shell_transcript_blocks(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> Vec<crate::quorp::tui::shell::AssistantBlock> {
        let session_index = self.active_session;
        self.rebuild_session_transcript_cache_if_needed(session_index);
        let (
            cached_messages,
            last_error,
            running_command,
            running_command_name,
            command_output_lines,
        ) = {
            let session = self.active_session_ref();
            (
                session.transcript_cache.messages.clone(),
                session.last_error.clone(),
                session.running_command,
                session.running_command_name.clone(),
                session.command_output_lines.clone(),
            )
        };
        let mut blocks = Vec::new();
        for message in cached_messages {
            match message {
                CachedTranscriptMessage::User(text) => {
                    blocks.push(crate::quorp::tui::shell::AssistantBlock {
                        role: "User:",
                        text,
                        tone: crate::quorp::tui::shell::AssistantTone::Muted,
                        rich_lines: None,
                    });
                }
                CachedTranscriptMessage::Assistant(message) => {
                    for segment in message.segments {
                        let (role, tone) = match &segment {
                            AssistantSegment::Text(_) | AssistantSegment::Code { .. } => (
                                "Assistant:",
                                crate::quorp::tui::shell::AssistantTone::Normal,
                            ),
                            AssistantSegment::Think(_) => {
                                ("Reasoning:", crate::quorp::tui::shell::AssistantTone::Muted)
                            }
                            AssistantSegment::RunCommand { command, .. } => {
                                if is_validation_command(command) {
                                    (
                                        "Validation:",
                                        crate::quorp::tui::shell::AssistantTone::Success,
                                    )
                                } else {
                                    ("Command:", crate::quorp::tui::shell::AssistantTone::Success)
                                }
                            }
                            AssistantSegment::ReadFile { .. }
                            | AssistantSegment::ListDirectory { .. }
                            | AssistantSegment::McpCallTool { .. } => {
                                ("Tool:", crate::quorp::tui::shell::AssistantTone::Muted)
                            }
                            AssistantSegment::WriteFile { .. }
                            | AssistantSegment::ApplyPatch { .. }
                            | AssistantSegment::ReplaceBlock { .. } => {
                                ("Files:", crate::quorp::tui::shell::AssistantTone::Success)
                            }
                        };
                        let rich_lines = assistant_transcript::render_assistant_segments(
                            &[segment],
                            theme,
                            SegmentRenderOptions::shell(),
                        );
                        let text = rich_lines
                            .iter()
                            .map(|line| {
                                line.spans
                                    .iter()
                                    .map(|span| span.content.as_ref())
                                    .collect::<String>()
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        blocks.push(crate::quorp::tui::shell::AssistantBlock {
                            role,
                            text,
                            tone,
                            rich_lines: Some(rich_lines),
                        });
                    }
                }
            }
        }

        if let Some(error) = last_error {
            blocks.push(crate::quorp::tui::shell::AssistantBlock {
                role: "Error:",
                text: error,
                tone: crate::quorp::tui::shell::AssistantTone::Error,
                rich_lines: None,
            });
        }

        if running_command {
            blocks.push(crate::quorp::tui::shell::AssistantBlock {
                role: "Command:",
                text: running_command_name.unwrap_or_else(|| "Command running".to_string()),
                tone: crate::quorp::tui::shell::AssistantTone::Success,
                rich_lines: None,
            });
            blocks.extend(command_output_lines.iter().map(|line| {
                crate::quorp::tui::shell::AssistantBlock {
                    role: "Output:",
                    text: line.clone(),
                    tone: crate::quorp::tui::shell::AssistantTone::Muted,
                    rich_lines: None,
                }
            }));
        }

        blocks
    }

    pub fn shell_mention_popup_lines(&self, max_rows: usize) -> Option<Vec<String>> {
        let popup = self.active_session_ref().mention_popup.as_ref()?;
        Some(
            popup
                .matches
                .iter()
                .skip(popup.scroll_top)
                .take(max_rows)
                .enumerate()
                .map(|(offset, entry)| {
                    let marker = if popup.selected == popup.scroll_top + offset {
                        "> "
                    } else {
                        "  "
                    };
                    format!("{marker}{}", entry.relative_display)
                })
                .collect(),
        )
    }

    pub fn shell_quick_open_matches(
        &self,
        query: &str,
        limit: usize,
    ) -> Vec<(String, std::path::PathBuf)> {
        self.path_index
            .match_query(query, limit)
            .into_iter()
            .map(|entry| (entry.relative_display, entry.abs_path))
            .collect()
    }

    pub fn shell_directory_matches(
        &self,
        query: &str,
        limit: usize,
    ) -> Vec<(String, std::path::PathBuf)> {
        self.path_index
            .match_query(query, limit.saturating_mul(2))
            .into_iter()
            .filter_map(|entry| {
                entry
                    .is_directory
                    .then_some((entry.relative_display, entry.abs_path))
            })
            .take(limit)
            .collect()
    }

    pub fn transcript_metrics(&mut self) -> (usize, usize, usize) {
        let session_index = self.active_session;
        self.rebuild_session_transcript_cache_if_needed(session_index);
        let Some(session) = self.sessions.get(session_index) else {
            return (0, 0, 0);
        };
        let mut segment_count = 0usize;
        let mut code_block_count = 0usize;
        for message in &session.transcript_cache.messages {
            if let CachedTranscriptMessage::Assistant(assistant) = message {
                segment_count += assistant.segments.len();
                code_block_count += assistant
                    .segments
                    .iter()
                    .filter(|segment| matches!(segment, AssistantSegment::Code { .. }))
                    .count();
            }
        }
        (
            session.transcript_cache.messages.len(),
            segment_count,
            code_block_count,
        )
    }

    pub fn insert_context_link(
        &mut self,
        label: &str,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let link = mention_link_for_path(path, label)?;
        let session = self.active_session_mut();
        let byte = char_byte_index(&session.input, session.cursor_char);
        if byte > session.input.len() {
            return Err("chat input cursor moved out of bounds".to_string());
        }
        let needs_space = !session.input.is_empty()
            && session.input[..byte]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_whitespace());
        let insertion = if needs_space {
            format!(" {link}")
        } else {
            link
        };
        session.input.insert_str(byte, &insertion);
        session.cursor_char += insertion.chars().count();
        self.sync_mention_popup();
        Ok(())
    }

    pub fn apply_chat_event(
        &mut self,
        event: ChatUiEvent,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        match event {
            ChatUiEvent::AssistantDelta(idx, delta) => {
                if let Some(last) = self
                    .sessions
                    .get_mut(idx)
                    .and_then(|s| s.messages.last_mut())
                {
                    last.push_assistant(delta.as_str());
                }
                self.mark_session_transcript_dirty(idx);
                let stick = self.sessions.get(idx).is_some_and(|s| s.stick_to_bottom);
                if stick && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::StreamFinished(idx) => {
                if let Some(s) = self.sessions.get_mut(idx) {
                    s.streaming = false;
                }
                let structured_turn_applied = self.apply_structured_turn_for_session(idx);
                self.mark_session_transcript_dirty(idx);
                if !structured_turn_applied {
                    self.try_extract_pending_command_for_session(idx);
                }
                let stick = self.sessions.get(idx).is_some_and(|s| s.stick_to_bottom);
                if stick && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::Error(idx, err) => {
                if let Some(s) = self.sessions.get_mut(idx) {
                    s.last_error = Some(err.clone());
                    if let Some(ChatMessage::Assistant(content)) = s.messages.last_mut() {
                        if content.is_empty() {
                            *content = format!("Error: {err}");
                        } else {
                            content.push_str(&format!("\n[Error: {err}]"));
                        }
                    }
                    s.streaming = false;
                }
                self.mark_session_transcript_dirty(idx);
            }
            ChatUiEvent::CommandOutput(idx, line) => {
                let Some(s) = self.sessions.get_mut(idx) else {
                    return;
                };
                s.command_output_lines.push(line);
                if s.stick_to_bottom && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::CommandFinished(idx, outcome) => {
                let followup_output = outcome.output_text().to_string();
                let pending_name = outcome.action().summary();
                let action_succeeded = matches!(outcome, ActionOutcome::Success { .. });
                let mut should_submit_followup = false;
                {
                    let Some(s) = self.sessions.get_mut(idx) else {
                        return;
                    };
                    s.running_command = false;
                    s.running_command_name = None;
                    let context = format!(
                        "[Tool Output]\n{}\n{}\n[End Output]",
                        pending_name, followup_output
                    );
                    s.messages.push(ChatMessage::User(context));
                    s.command_output_lines.clear();

                    if !action_succeeded && !s.pending_commands.is_empty() {
                        let skipped_count = s.pending_commands.len();
                        s.pending_commands.clear();
                        s.messages.push(ChatMessage::User(format!(
                            "[Batch execution aborted]\nThe action `{pending_name}` failed, so the remaining {skipped_count} queued action(s) were skipped. Review the error and try again."
                        )));
                    }

                    if s.pending_commands.is_empty() {
                        s.messages.push(ChatMessage::Assistant(String::new()));
                        should_submit_followup = true;
                    }
                }
                self.mark_session_transcript_dirty(idx);

                if action_succeeded {
                    self.advance_action_queue(idx);
                }

                if should_submit_followup && idx == self.active_session {
                    self.submit_input_for_followup(theme, pending_name, followup_output);
                }
            }
        }
        self.shell_feed_dirty = true;
    }

    fn apply_structured_turn_for_session(&mut self, session_index: usize) -> bool {
        let Some(raw_text) = self
            .sessions
            .get(session_index)
            .and_then(|session| session.messages.last())
            .and_then(|message| match message {
                ChatMessage::Assistant(text) => Some(text.clone()),
                ChatMessage::User(_) => None,
            })
        else {
            return false;
        };

        let turn = match parse_agent_turn_response(&raw_text) {
            Ok(Some(turn)) => turn,
            Ok(None) => return false,
            Err(error) => {
                if let Some(session) = self.sessions.get_mut(session_index) {
                    session.last_error = Some(error);
                }
                return false;
            }
        };

        if let Some(ChatMessage::Assistant(text)) = self
            .sessions
            .get_mut(session_index)
            .and_then(|session| session.messages.last_mut())
        {
            *text = render_agent_turn_text(&turn, &self.agent_config);
        }

        self.sessions[session_index].pending_commands.clear();
        self.queue_actions_for_session(session_index, turn.actions);

        true
    }

    fn queue_actions_for_session(&mut self, session_index: usize, actions: Vec<AgentAction>) {
        if let Some(session) = self.sessions.get_mut(session_index) {
            session.pending_commands = actions.into_iter().map(PendingCommand::new).collect();
        }
        self.advance_action_queue(session_index);
    }

    fn append_batch_abort_message(
        &mut self,
        session_index: usize,
        failed_action_summary: &str,
        skipped_count: usize,
    ) {
        let skipped_label = if skipped_count == 1 {
            "1 queued action was"
        } else {
            "queued actions were"
        };
        if let Some(session) = self.sessions.get_mut(session_index) {
            session.messages.push(ChatMessage::User(format!(
                "[Batch execution aborted]\nThe action `{failed_action_summary}` failed, so the remaining {skipped_count} {skipped_label} skipped. Review the error and try again."
            )));
        }
    }

    fn advance_action_queue(&mut self, session_index: usize) {
        let Some(pending) = self.pending_command_for_session(session_index).cloned() else {
            return;
        };
        let action = pending.action().clone();

        if self
            .sessions
            .get(session_index)
            .is_some_and(|session| session.running_command)
        {
            return;
        }

        if !self.sessions[session_index].mode.allows_action(&action) {
            let mode_label = self.sessions[session_index].mode.label().to_string();
            let skipped_count = self
                .pending_queue_len_for_session(session_index)
                .saturating_sub(1);
            self.clear_pending_commands_for_session(session_index);
            if let Some(session) = self.sessions.get_mut(session_index) {
                session.messages.push(ChatMessage::User(format!(
                    "[Action blocked by {} mode: {}]",
                    mode_label,
                    action.summary()
                )));
            }
            if skipped_count > 0 {
                self.append_batch_abort_message(session_index, &action.summary(), skipped_count);
            }
            self.mark_session_transcript_dirty(session_index);
            return;
        }

        let approval_policy = effective_approval_policy(&action, &self.agent_config);
        if approval_policy
            == crate::quorp::tui::agent_protocol::ActionApprovalPolicy::RequireExplicitConfirmation
        {
            return;
        }

        if let Some(session) = self.sessions.get_mut(session_index)
            && !session.pending_commands.is_empty()
        {
            session.pending_commands.remove(0);
        }
        if !self.execute_agent_action_for_session(session_index, action) {
            self.clear_pending_commands_for_session(session_index);
            self.mark_session_transcript_dirty(session_index);
        }
    }

    fn try_extract_pending_command_for_session(&mut self, session_index: usize) {
        self.rebuild_session_transcript_cache_if_needed(session_index);
        let Some(session) = self.sessions.get(session_index) else {
            return;
        };
        let Some(CachedTranscriptMessage::Assistant(message)) =
            session.transcript_cache.messages.last()
        else {
            self.clear_pending_commands_for_session(session_index);
            return;
        };
        let actions = message
            .segments
            .iter()
            .filter_map(agent_action_from_assistant_segment)
            .collect::<Vec<_>>();
        if actions.is_empty() {
            self.clear_pending_commands_for_session(session_index);
            return;
        }
        self.queue_actions_for_session(session_index, actions);
    }

    fn execute_pending_command(&mut self) {
        let idx = self.active_session;
        let Some(pending) = self.pending_command_for_active_session().cloned() else {
            return;
        };
        let action = pending.action().clone();
        if let Some(session) = self.sessions.get_mut(idx)
            && !session.pending_commands.is_empty()
        {
            session.pending_commands.remove(0);
        }
        if !self.execute_agent_action_for_session(idx, action) {
            self.clear_pending_commands_for_session(idx);
            self.mark_session_transcript_dirty(idx);
        }
    }

    fn execute_agent_action_for_session(&mut self, session_id: usize, action: AgentAction) -> bool {
        {
            let session = &mut self.sessions[session_id];
            session.running_command = true;
            session.running_command_name = Some(action.summary());
            session.command_output_lines.clear();
        }
        if let Some(ref bridge_tx) = self.command_bridge_tx {
            let request = crate::quorp::tui::command_bridge::CommandBridgeRequest::ExecuteAction {
                session_id,
                action,
                cwd: self.project_root.clone(),
                project_root: self.project_root.clone(),
                responder: None,
                enable_rollback_on_validation_failure: true,
            };
            if bridge_tx.unbounded_send(request).is_err() {
                {
                    let session = &mut self.sessions[session_id];
                    session.running_command = false;
                    session.running_command_name = None;
                    session.messages.push(ChatMessage::User(
                        "Command bridge disconnected; could not run command.".to_string(),
                    ));
                    session.messages.push(ChatMessage::Assistant(String::new()));
                }
                self.mark_session_transcript_dirty(session_id);
                return false;
            }
            return true;
        }
        let session = &mut self.sessions[session_id];
        session.running_command = false;
        session.messages.push(ChatMessage::User(
            "Command bridge disconnected; could not run command.".to_string(),
        ));
        session.messages.push(ChatMessage::Assistant(String::new()));
        self.mark_session_transcript_dirty(session_id);
        false
    }

    fn cancel_pending_command(&mut self) {
        let cancelled_count = {
            let s = self.active_session_mut();
            if !s.pending_commands.is_empty() {
                let count = s.pending_commands.len();
                s.pending_commands.clear();
                s.messages
                    .push(ChatMessage::User("[Command cancelled by user]".to_string()));
                count
            } else {
                0
            }
        };
        if cancelled_count > 0 {
            if cancelled_count > 1 {
                let skipped = cancelled_count - 1;
                if let Some(session) = self.sessions.get_mut(self.active_session) {
                    session.messages.push(ChatMessage::User(format!(
                        "[Batch cancelled]\nThe current action was cancelled, so the remaining {skipped} queued action(s) were also cleared."
                    )));
                }
            }
            self.mark_active_session_transcript_dirty();
        }
    }

    fn submit_input_for_followup(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
        command: String,
        command_output: String,
    ) {
        if self.active_session_ref().messages.is_empty() || self.models.is_empty() {
            return;
        }
        let session_id = self.active_session;
        let model_id = self.resolve_effective_followup_model_id();
        let agent_mode = self.active_session_ref().mode;
        let messages = self.build_service_messages_for_session(session_id);
        self.active_session_mut().streaming = true;
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::SummarizeCommandOutput {
                session_id,
                model_id,
                agent_mode,
                command,
                command_output,
                messages,
                project_root: self.project_root.clone(),
                base_url_override: self.base_url_override_for_service(),
                prompt_compaction_policy: self.active_session_ref().prompt_compaction_policy,
            })
            .is_err()
        {
            {
                let session = self.active_session_mut();
                session.streaming = false;
                session.last_error = Some("Chat service disconnected.".to_string());
                if let Some(ChatMessage::Assistant(text)) = session.messages.last_mut() {
                    *text = "Chat service disconnected.".to_string();
                }
            }
            self.mark_active_session_transcript_dirty();
            self.scroll_transcript_to_bottom(theme);
        }
    }

    pub fn current_model_id(&self) -> &str {
        self.models
            .get(self.model_index)
            .map(|s| s.as_str())
            .unwrap_or("qwen3.5-35b-a3b")
    }

    pub fn current_provider_kind(&self) -> crate::quorp::executor::InteractiveProviderKind {
        crate::quorp::tui::model_registry::chat_model_provider(
            self.current_model_id(),
            crate::quorp::executor::interactive_provider_from_env(),
        )
    }

    pub fn current_provider_label(&self) -> &'static str {
        self.current_provider_kind().title()
    }

    fn select_or_insert_effective_model(&mut self, model_id: &str) -> String {
        let index = select_or_insert_model(&mut self.models, model_id);
        self.model_index = index;
        self.models[index].clone()
    }

    fn resolve_effective_chat_model_id(&mut self, latest_input: &str) -> String {
        let routed = crate::quorp::tui::model_registry::managed_chat_model_id(
            self.current_model_id(),
            latest_input,
        );
        self.select_or_insert_effective_model(&routed)
    }

    fn resolve_effective_followup_model_id(&mut self) -> String {
        let routed = crate::quorp::tui::model_registry::managed_command_summary_model_id(
            self.current_model_id(),
        );
        self.select_or_insert_effective_model(&routed)
    }

    fn resolve_effective_autonomous_model_id(&mut self, goal: &str) -> String {
        let routed = crate::quorp::tui::model_registry::managed_autonomous_model_id(
            self.current_model_id(),
            goal,
        );
        self.select_or_insert_effective_model(&routed)
    }

    pub fn current_model_display_label(&self) -> String {
        crate::quorp::tui::model_registry::chat_model_display_label(
            self.current_model_id(),
            crate::quorp::executor::interactive_provider_from_env(),
        )
    }

    fn active_prompt_compaction_policy_label(&self) -> &'static str {
        match self.active_session_ref().prompt_compaction_policy {
            None | Some(PromptCompactionPolicy::CurrentDefault) => "default",
            Some(PromptCompactionPolicy::Last6Ledger768) => "last6-ledger768",
            Some(PromptCompactionPolicy::Last8Ledger1024) => "last8-ledger1024",
            Some(PromptCompactionPolicy::BenchmarkRepairMinimal) => "benchmark-repair-minimal",
            Some(PromptCompactionPolicy::BenchmarkStatePacket) => "benchmark-state-packet",
            Some(PromptCompactionPolicy::Off) => "off",
        }
    }

    pub fn model_list(&self) -> &[String] {
        &self.models
    }

    #[cfg(test)]
    pub fn set_models_for_test(&mut self, models: Vec<String>, model_index: usize) {
        self.models = models;
        self.model_index = if self.models.is_empty() {
            0
        } else {
            model_index.min(self.models.len() - 1)
        };
    }

    pub fn model_index(&self) -> usize {
        self.model_index
    }

    pub fn set_model_index(&mut self, index: usize) {
        if self.models.is_empty() {
            return;
        }
        self.model_index = index.min(self.models.len() - 1);
    }

    fn scroll_transcript_to_bottom(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, self.last_text_width);
        let v = self.viewport_transcript_lines.max(1);
        self.active_session_mut().transcript_scroll = wrapped.len().saturating_sub(v);
    }

    fn wrapped_line_count(&mut self, theme: &crate::quorp::tui::theme::Theme) -> usize {
        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, self.last_text_width);
        wrapped.len().max(1)
    }

    fn clamp_transcript_scroll(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let total = self.wrapped_line_count(theme);
        let v = self.viewport_transcript_lines.max(1);
        let max_scroll = total.saturating_sub(v);
        let s = self.active_session_mut();
        if s.transcript_scroll > max_scroll {
            s.transcript_scroll = max_scroll;
        }
    }

    fn cycle_model(&mut self, delta: isize) {
        if self.models.is_empty() {
            return;
        }
        let len = self.models.len() as isize;
        self.model_index = (self.model_index as isize + delta).rem_euclid(len) as usize;
    }

    pub fn handle_key_event(
        &mut self,
        key: &KeyEvent,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Tab {
            return false;
        }

        if self.pending_command_for_active_session().is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.execute_pending_command();
                    return true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.cancel_pending_command();
                    return true;
                }
                _ => return true,
            }
        }

        if self.active_session_ref().running_command {
            return true;
        }

        match key.code {
            KeyCode::Esc => {
                if self.active_session_ref().mention_popup.is_some() {
                    self.active_session_mut().mention_popup = None;
                    return true;
                }
                false
            }
            KeyCode::Char('a') if key.modifiers == KeyModifiers::ALT => {
                self.set_active_mode(AgentMode::Ask);
                true
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::ALT => {
                self.set_active_mode(AgentMode::Plan);
                true
            }
            KeyCode::Char('x') if key.modifiers == KeyModifiers::ALT => {
                self.set_active_mode(AgentMode::Act);
                true
            }
            KeyCode::Tab => {
                if self
                    .active_session_ref()
                    .mention_popup
                    .as_ref()
                    .is_some_and(|p| !p.matches.is_empty())
                {
                    self.accept_mention();
                    return true;
                }
                false
            }
            KeyCode::Char('[') if key.modifiers.is_empty() => {
                self.cycle_model(-1);
                true
            }
            KeyCode::Char(']') if key.modifiers.is_empty() => {
                self.cycle_model(1);
                true
            }
            KeyCode::Enter => {
                if self
                    .active_session_ref()
                    .mention_popup
                    .as_ref()
                    .is_some_and(|p| !p.matches.is_empty())
                {
                    self.accept_mention();
                    return true;
                }
                if self.active_session_ref().mention_popup.is_some() {
                    self.active_session_mut().mention_popup = None;
                    return true;
                }
                self.submit_input(theme);
                true
            }
            KeyCode::Left => {
                let s = self.active_session_mut();
                if s.cursor_char > 0 {
                    s.cursor_char -= 1;
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Right => {
                let max_c = {
                    let s = self.active_session_ref();
                    s.input.chars().count()
                };
                let s = self.active_session_mut();
                if s.cursor_char < max_c {
                    s.cursor_char += 1;
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Home => {
                self.active_session_mut().cursor_char = 0;
                self.sync_mention_popup();
                true
            }
            KeyCode::End => {
                let count = self.active_session_ref().input.chars().count();
                self.active_session_mut().cursor_char = count;
                self.sync_mention_popup();
                true
            }
            KeyCode::Backspace => {
                let s = self.active_session_mut();
                if s.cursor_char > 0 {
                    let end = char_byte_index(&s.input, s.cursor_char);
                    s.cursor_char -= 1;
                    let start = char_byte_index(&s.input, s.cursor_char);
                    s.input.replace_range(start..end, "");
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Delete => {
                let (byte, len) = {
                    let s = self.active_session_ref();
                    let byte = char_byte_index(&s.input, s.cursor_char);
                    (byte, s.input.len())
                };
                let s = self.active_session_mut();
                if byte < len {
                    let end = s.input.ceil_char_boundary(byte + 1);
                    s.input.replace_range(byte..end, "");
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Up => {
                let handled = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            p.selected = p.selected.saturating_sub(1);
                            clamp_mention_scroll(
                                &mut p.scroll_top,
                                p.selected,
                                MENTION_POPUP_MAX_VISIBLE,
                                p.matches.len(),
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if handled {
                    return true;
                }
                let s = self.active_session_mut();
                s.stick_to_bottom = false;
                s.transcript_scroll = s.transcript_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                let handled = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let max_sel = p.matches.len().saturating_sub(1);
                            p.selected = (p.selected + 1).min(max_sel);
                            clamp_mention_scroll(
                                &mut p.scroll_top,
                                p.selected,
                                MENTION_POPUP_MAX_VISIBLE,
                                p.matches.len(),
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if handled {
                    return true;
                }
                self.clamp_transcript_scroll(theme);
                let max = self
                    .wrapped_line_count(theme)
                    .saturating_sub(self.viewport_transcript_lines.max(1));
                let s = self.active_session_mut();
                s.transcript_scroll = (s.transcript_scroll + 1).min(max);
                if s.transcript_scroll >= max {
                    s.stick_to_bottom = true;
                }
                true
            }
            KeyCode::PageUp => {
                let popup_page = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let v = MENTION_POPUP_MAX_VISIBLE.min(p.matches.len()).max(1);
                            let max_top = p.matches.len().saturating_sub(v);
                            p.scroll_top = p.scroll_top.saturating_sub(v).min(max_top);
                            if p.selected < p.scroll_top {
                                p.selected = p.scroll_top;
                            }
                            let last_visible =
                                (p.scroll_top + v).min(p.matches.len()).saturating_sub(1);
                            if p.selected > last_visible {
                                p.selected = last_visible;
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if popup_page {
                    return true;
                }
                let step = self.viewport_transcript_lines.saturating_sub(1).max(1);
                let s = self.active_session_mut();
                s.stick_to_bottom = false;
                s.transcript_scroll = s.transcript_scroll.saturating_sub(step);
                true
            }
            KeyCode::PageDown => {
                let popup_page = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let v = MENTION_POPUP_MAX_VISIBLE.min(p.matches.len()).max(1);
                            let max_top = p.matches.len().saturating_sub(v);
                            p.scroll_top = (p.scroll_top + v).min(max_top);
                            if p.selected < p.scroll_top {
                                p.selected = p.scroll_top;
                            }
                            let last_visible =
                                (p.scroll_top + v).min(p.matches.len()).saturating_sub(1);
                            if p.selected > last_visible {
                                p.selected = last_visible;
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if popup_page {
                    return true;
                }
                let step = self.viewport_transcript_lines.saturating_sub(1).max(1);
                self.clamp_transcript_scroll(theme);
                let max = self
                    .wrapped_line_count(theme)
                    .saturating_sub(self.viewport_transcript_lines.max(1));
                let s = self.active_session_mut();
                s.transcript_scroll = (s.transcript_scroll + step).min(max);
                if s.transcript_scroll >= max {
                    s.stick_to_bottom = true;
                }
                true
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.insert_char(c);
                true
            }
            _ => false,
        }
    }

    fn byte_at_char_index(&self) -> usize {
        let s = self.active_session_ref();
        char_byte_index(&s.input, s.cursor_char)
    }

    fn sync_mention_popup(&mut self) {
        let cursor_byte = self.byte_at_char_index();
        let token = {
            let s = self.active_session_ref();
            active_mention_token(&s.input, cursor_byte)
        };
        let Some((at_byte, query)) = token else {
            self.active_session_mut().mention_popup = None;
            return;
        };
        let matches = self.path_index.match_query(&query, 80);
        let s = self.active_session_mut();
        let prev = s.mention_popup.take();
        let (selected, mut scroll_top) = match prev {
            Some(p) if p.at_byte == at_byte && p.last_query == query => {
                let sel = p.selected.min(matches.len().saturating_sub(1));
                (sel, p.scroll_top)
            }
            _ => (0, 0),
        };
        clamp_mention_scroll(
            &mut scroll_top,
            selected,
            MENTION_POPUP_MAX_VISIBLE,
            matches.len(),
        );
        s.mention_popup = Some(MentionPopup {
            at_byte,
            selected,
            scroll_top,
            matches,
            last_query: query,
        });
    }

    fn accept_mention(&mut self) {
        let Some(pop) = self.active_session_mut().mention_popup.take() else {
            return;
        };
        if pop.matches.is_empty() {
            self.active_session_mut().mention_popup = Some(pop);
            return;
        }
        let sel = pop.selected.min(pop.matches.len().saturating_sub(1));
        let entry = &pop.matches[sel];
        let link = match mention_link_for_path(&entry.abs_path, &entry.relative_display) {
            Ok(l) => format!("{l} "),
            Err(_) => {
                self.active_session_mut().mention_popup = Some(pop);
                return;
            }
        };
        let end_byte = self.byte_at_char_index();
        let s = self.active_session_mut();
        if end_byte < pop.at_byte || pop.at_byte > s.input.len() {
            self.sync_mention_popup();
            return;
        }
        s.input.replace_range(pop.at_byte..end_byte, &link);
        s.cursor_char = char_index_for_byte(&s.input, pop.at_byte + link.len());
        self.sync_mention_popup();
    }

    fn insert_char(&mut self, c: char) {
        let byte = self.byte_at_char_index();
        let mut buf = [0u8; 4];
        let slice = c.encode_utf8(&mut buf);
        let s = self.active_session_mut();
        s.input.insert_str(byte, slice);
        s.cursor_char += 1;
        self.sync_mention_popup();
    }

    fn title_from_user_message(text: &str) -> String {
        let t = text.trim();
        let line = t.lines().next().unwrap_or("");
        let mut s: String = line.chars().take(32).collect();
        s = s.trim().to_string();
        if s.is_empty() { "Chat".to_string() } else { s }
    }

    pub fn chat_tab_specs(
        &self,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> Vec<crate::quorp::tui::chrome_v2::LeafTabSpec> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(i, session)| crate::quorp::tui::chrome_v2::LeafTabSpec {
                label: session.title.clone(),
                active: i == self.active_session,
                icon: Some(theme.glyphs.activity_agent),
                show_close: self.sessions.len() > 1,
            })
            .collect()
    }

    pub fn draw_tab_strip(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        strip: ratatui::layout::Rect,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> (Vec<crate::quorp::tui::chrome_v2::LeafTabLayoutCell>, usize) {
        let specs = self.chat_tab_specs(theme);
        let (cells, overflow) = crate::quorp::tui::chrome_v2::layout_leaf_tabs(strip, &specs);
        crate::quorp::tui::chrome_v2::render_leaf_tabs_laid_out(
            buf,
            strip,
            &cells,
            &specs,
            &theme.palette,
            theme.glyphs.close_icon,
            theme.palette.chat_accent,
        );
        if overflow > 0 {
            crate::quorp::tui::chrome_v2::render_tab_overflow_hint(
                buf,
                strip,
                overflow,
                &theme.palette,
            );
        }
        (cells, overflow)
    }

    pub fn activate_chat_session(
        &mut self,
        index: usize,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> bool {
        if index >= self.sessions.len() {
            return false;
        }
        if index == self.active_session {
            return true;
        }
        self.active_session = index;
        self.clamp_transcript_scroll(theme);
        true
    }

    pub fn close_chat_session_at(
        &mut self,
        index: usize,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> bool {
        if index >= self.sessions.len() || self.sessions.len() <= 1 {
            return false;
        }
        if self.sessions.get(index).is_some_and(|s| s.streaming) {
            self.cancel_stream_for_session(index);
        }
        let was_active = self.active_session == index;
        self.sessions.remove(index);
        let new_len = self.sessions.len();
        if self.active_session > index {
            self.active_session -= 1;
        } else if was_active {
            self.active_session = self.active_session.min(new_len.saturating_sub(1));
        }
        self.active_session = self.active_session.min(new_len.saturating_sub(1));
        self.clamp_transcript_scroll(theme);
        true
    }

    pub fn close_all_chat_sessions(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let streaming_sessions: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| session.streaming.then_some(index))
            .collect();
        for session_id in streaming_sessions {
            self.cancel_stream_for_session(session_id);
        }
        let mut session = ChatSession::new("Chat 1".to_string());
        session.mode = self.agent_config.defaults.mode;
        session.prompt_compaction_policy = self.default_prompt_compaction_policy;
        self.sessions = vec![session];
        self.active_session = 0;
        self.clamp_transcript_scroll(theme);
    }

    pub fn cycle_chat_session(&mut self, delta: isize, theme: &crate::quorp::tui::theme::Theme) {
        if self.sessions.len() <= 1 {
            return;
        }
        if self.active_session_ref().streaming {
            self.abort_streaming();
        }
        let len = self.sessions.len() as isize;
        self.active_session = (self.active_session as isize + delta).rem_euclid(len) as usize;
        self.clamp_transcript_scroll(theme);
    }

    pub fn new_chat_session(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        if self.active_session_ref().streaming {
            self.abort_streaming();
        }
        let n = self.sessions.len() + 1;
        let mut session = ChatSession::new(format!("Chat {n}"));
        session.mode = self.active_session_ref().mode;
        session.prompt_compaction_policy = self.default_prompt_compaction_policy;
        self.sessions.push(session);
        self.active_session = self.sessions.len() - 1;
        self.clamp_transcript_scroll(theme);
    }

    fn push_local_response(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
        user_text: String,
        assistant_text: String,
    ) {
        {
            let session = self.active_session_mut();
            let was_empty = session.messages.is_empty();
            session.input.clear();
            session.cursor_char = 0;
            session.last_error = None;
            session.stick_to_bottom = true;
            if was_empty {
                session.title = Self::title_from_user_message(&user_text);
            }
            session.messages.push(ChatMessage::User(user_text));
            session
                .messages
                .push(ChatMessage::Assistant(assistant_text));
            if session.messages.len() > MAX_MESSAGES {
                let excess = session.messages.len() - MAX_MESSAGES;
                let drop_count = if !excess.is_multiple_of(2) {
                    excess + 1
                } else {
                    excess
                };
                session.messages.drain(0..drop_count);
            }
        }
        self.mark_active_session_transcript_dirty();
        self.shell_feed_submitted = true;
        self.shell_feed_dirty = true;
        self.scroll_transcript_to_bottom(theme);
    }

    fn begin_submitted_exchange(&mut self, user_text: String, assistant_placeholder: String) {
        let session = self.active_session_mut();
        let was_empty = session.messages.is_empty();
        session.input.clear();
        session.cursor_char = 0;
        session.last_error = None;
        session.stick_to_bottom = true;
        if was_empty {
            session.title = Self::title_from_user_message(&user_text);
        }
        session.messages.push(ChatMessage::User(user_text));
        session
            .messages
            .push(ChatMessage::Assistant(assistant_placeholder));
        if session.messages.len() > MAX_MESSAGES {
            let excess = session.messages.len() - MAX_MESSAGES;
            let drop_count = if !excess.is_multiple_of(2) {
                excess + 1
            } else {
                excess
            };
            session.messages.drain(0..drop_count);
        }
        self.mark_active_session_transcript_dirty();
        self.shell_feed_submitted = true;
        self.shell_feed_dirty = true;
    }

    fn handle_slash_command(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
        user_text: String,
        command: SlashCommand,
    ) -> bool {
        match command {
            SlashCommand::OpenRunArtifacts => {
                let response = latest_artifact_summary(None)
                    .unwrap_or_else(|error| format!("No run artifacts are available yet: {error}"));
                self.push_local_response(theme, user_text, response);
                true
            }
            SlashCommand::ResumeLast { result_dir } => {
                let resume_result = latest_resume_target(result_dir.as_deref()).and_then(|path| {
                    FullAutoLaunchSpec::load_from(&path)
                        .with_context(|| format!("failed to resume from {}", path.display()))
                });
                match resume_result {
                    Ok(spec) => {
                        let placeholder = format!(
                            "[Full Auto Resumed] {}\nworkspace: {}\nsandbox: {:?}\nartifacts: {}",
                            spec.goal,
                            spec.workspace_root.display(),
                            spec.sandbox_mode,
                            spec.result_dir.display()
                        );
                        self.begin_submitted_exchange(user_text, placeholder);
                        let session_id = self.active_session;
                        let messages = self.build_service_messages_for_session(session_id);
                        let model_id = self.resolve_effective_autonomous_model_id(&spec.goal);
                        let _ = self.ui_tx.send(crate::quorp::tui::TuiEvent::StartAgentTask(
                            spec.to_agent_task_request(
                                messages,
                                model_id,
                                self.active_session_ref().mode,
                                self.base_url_override_for_service(),
                            ),
                        ));
                        self.scroll_transcript_to_bottom(theme);
                    }
                    Err(error) => {
                        self.push_local_response(
                            theme,
                            user_text,
                            format!("Unable to resume the last full-auto run: {error}"),
                        );
                    }
                }
                true
            }
            command => {
                let defaults = LaunchDefaults {
                    autonomy_profile: AutonomyProfile::AutonomousSandboxed,
                    max_seconds: None,
                    max_total_tokens: None,
                };
                match prepare_launch_spec(
                    &command,
                    &self.project_root,
                    self.current_model_id(),
                    defaults,
                ) {
                    Ok(Some(spec)) => {
                        if let Err(error) = spec.write_to_disk() {
                            self.push_local_response(
                                theme,
                                user_text,
                                format!("Unable to initialize the run directory: {error}"),
                            );
                            return true;
                        }
                        let placeholder = format!(
                            "[Full Auto Launched] {}\nworkspace: {}\nsandbox: {:?}\nartifacts: {}",
                            spec.goal,
                            spec.workspace_root.display(),
                            spec.sandbox_mode,
                            spec.result_dir.display()
                        );
                        self.begin_submitted_exchange(user_text, placeholder);
                        let session_id = self.active_session;
                        let messages = self.build_service_messages_for_session(session_id);
                        let model_id = self.resolve_effective_autonomous_model_id(&spec.goal);
                        let _ = self.ui_tx.send(crate::quorp::tui::TuiEvent::StartAgentTask(
                            spec.to_agent_task_request(
                                messages,
                                model_id,
                                self.active_session_ref().mode,
                                self.base_url_override_for_service(),
                            ),
                        ));
                        self.scroll_transcript_to_bottom(theme);
                    }
                    Ok(None) => {
                        self.push_local_response(
                            theme,
                            user_text,
                            "That slash command does not launch a run.".to_string(),
                        );
                    }
                    Err(error) => {
                        self.push_local_response(
                            theme,
                            user_text,
                            format!("Unable to prepare the run: {error}"),
                        );
                    }
                }
                true
            }
        }
    }

    fn submit_input(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let trimmed = {
            let s = self.active_session_ref();
            s.input.trim()
        };
        if trimmed.is_empty() {
            return;
        }
        let trimmed = trimmed.to_string();

        self.abort_streaming();

        if let Some(compaction_result) = parse_compaction_command(&trimmed) {
            let response = match compaction_result {
                Ok(prompt_compaction_policy) => {
                    self.active_session_mut().prompt_compaction_policy = prompt_compaction_policy;
                    format!(
                        "Compaction policy set to {} for this session.",
                        self.active_prompt_compaction_policy_label()
                    )
                }
                Err(error) => error,
            };
            {
                let session = self.active_session_mut();
                session.input.clear();
                session.cursor_char = 0;
                session.last_error = None;
                session.stick_to_bottom = true;
                session.messages.push(ChatMessage::User(trimmed));
                session.messages.push(ChatMessage::Assistant(response));
                if session.messages.len() > MAX_MESSAGES {
                    let excess = session.messages.len() - MAX_MESSAGES;
                    let drop_count = if !excess.is_multiple_of(2) {
                        excess + 1
                    } else {
                        excess
                    };
                    session.messages.drain(0..drop_count);
                }
            }
            self.mark_active_session_transcript_dirty();
            self.shell_feed_submitted = true;
            self.scroll_transcript_to_bottom(theme);
            return;
        }

        if trimmed.starts_with('/') {
            match parse_slash_command(&trimmed) {
                Ok(Some(command)) => {
                    self.handle_slash_command(theme, trimmed, command);
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    self.push_local_response(theme, trimmed, error);
                    return;
                }
            }
        }

        let request_input = trimmed.clone();
        self.begin_submitted_exchange(trimmed, String::new());

        if self.models.is_empty() {
            if let Some(ChatMessage::Assistant(text)) =
                self.active_session_mut().messages.last_mut()
            {
                *text = "No configured chat models are available.".to_string();
            }
            self.scroll_transcript_to_bottom(theme);
            return;
        }

        self.active_session_mut().streaming = true;

        let session_id = self.active_session;
        let model_id = self.resolve_effective_chat_model_id(&request_input);
        let agent_mode = self.active_session_ref().mode;
        let messages = self.build_service_messages_for_session(session_id);
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::SubmitPrompt {
                session_id,
                model_id,
                agent_mode,
                latest_input: request_input,
                messages,
                project_root: self.project_root.clone(),
                base_url_override: self.base_url_override_for_service(),
                prompt_compaction_policy: self.active_session_ref().prompt_compaction_policy,
            })
            .is_err()
        {
            let session = self.active_session_mut();
            session.streaming = false;
            session.last_error = Some("Chat service disconnected.".to_string());
            if let Some(ChatMessage::Assistant(text)) = session.messages.last_mut() {
                *text = "Chat service disconnected.".to_string();
            }
        }
        self.scroll_transcript_to_bottom(theme);
    }

    pub fn take_shell_feed_dirty(&mut self) -> bool {
        let dirty = self.shell_feed_dirty;
        self.shell_feed_dirty = false;
        dirty
    }

    pub fn take_shell_feed_submitted(&mut self) -> bool {
        let submitted = self.shell_feed_submitted;
        self.shell_feed_submitted = false;
        submitted
    }

    pub fn mention_popup_open(&self) -> bool {
        self.active_session_ref().mention_popup.is_some()
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        rects: &crate::quorp::tui::workbench::LeafRects,
        is_focused: bool,
        assistant_status: Option<&str>,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        if let Some(banner_rect) = rects.banner {
            let text = assistant_status
                .filter(|status| !status.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "{} {} · Compaction: {}",
                        self.current_provider_label(),
                        self.current_model_display_label(),
                        self.active_prompt_compaction_policy_label(),
                    )
                });
            crate::quorp::tui::chrome_v2::render_agent_banner(
                buf,
                banner_rect,
                "A",
                &text,
                &theme.palette,
            );
        }

        let body_rect = rects.body;
        self.viewport_transcript_lines = body_rect.height as usize;

        let show_scrollbar_hint = body_rect.width > 2;
        let text_width = if show_scrollbar_hint {
            body_rect.width.saturating_sub(1)
        } else {
            body_rect.width
        };
        self.last_text_width = text_width as usize;

        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, text_width as usize);
        let total_lines = wrapped.len().max(1);
        let v = self.viewport_transcript_lines.max(1);

        self.clamp_transcript_scroll(theme);
        let max_scroll = total_lines.saturating_sub(v);
        let scroll = self.active_session_ref().transcript_scroll.min(max_scroll);

        let visible: Vec<Line> = wrapped.into_iter().skip(scroll).take(v).collect();

        let transcript_block = ratatui::widgets::Paragraph::new(visible);
        let show_scrollbar = total_lines > v && body_rect.width > 1;

        let text_area = if show_scrollbar {
            ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Horizontal)
                .constraints([
                    ratatui::layout::Constraint::Min(1),
                    ratatui::layout::Constraint::Length(1),
                ])
                .split(body_rect)[0]
        } else {
            body_rect
        };

        ratatui::widgets::Widget::render(transcript_block, text_area, buf);

        if show_scrollbar {
            let mut state = ratatui::widgets::ScrollbarState::new(total_lines).position(scroll);
            ratatui::widgets::StatefulWidget::render(
                ratatui::widgets::Scrollbar::new(
                    ratatui::widgets::ScrollbarOrientation::VerticalRight,
                ),
                body_rect,
                buf,
                &mut state,
            );
        }

        if let Some(composer_rect) = rects.composer {
            if let Some(ref mp) = self.active_session_ref().mention_popup
                && is_focused
                && !mp.matches.is_empty()
            {
                let space_above = composer_rect.y.saturating_sub(body_rect.y);
                if space_above > 0 && composer_rect.width > 2 {
                    let max_rows = MENTION_POPUP_MAX_VISIBLE.min(mp.matches.len());
                    let popup_rows = (max_rows as u16).min(space_above).max(1);
                    let popup_y = composer_rect.y.saturating_sub(popup_rows);
                    let line_budget = (composer_rect.width.saturating_sub(2)) as usize;
                    let slice = mp
                        .matches
                        .iter()
                        .skip(mp.scroll_top)
                        .take(popup_rows as usize)
                        .map(|e| {
                            let raw = if e.is_directory {
                                format!("{}/", e.relative_display)
                            } else {
                                e.relative_display.clone()
                            };
                            crate::quorp::tui::text_width::truncate_middle_fit(
                                &raw,
                                line_budget.max(1),
                            )
                        })
                        .collect::<Vec<_>>();
                    let vm = crate::quorp::tui::chrome_v2::MentionPopupVm {
                        lines: slice,
                        selected: mp.selected.saturating_sub(mp.scroll_top),
                    };
                    let popup_rect = ratatui::layout::Rect::new(
                        composer_rect.x,
                        popup_y,
                        composer_rect.width,
                        popup_rows,
                    );
                    crate::quorp::tui::chrome_v2::render_mention_popup(
                        buf,
                        popup_rect,
                        &vm,
                        &theme.palette,
                    );
                }
            }

            let composer = crate::quorp::tui::chrome_v2::ComposerVm {
                placeholder: " Ask the assistant. Use @file or /command".to_string(),
                input: self.active_session_ref().input.clone(),
                mode_chips: vec![self.active_session_ref().mode.label().to_string()],
                focused: is_focused,
            };
            crate::quorp::tui::chrome_v2::render_composer(
                buf,
                composer_rect,
                &composer,
                &theme.palette,
            );
        }
    }

    fn build_transcript_lines(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> Vec<Line<'static>> {
        let session_index = self.active_session;
        self.rebuild_session_transcript_cache_if_needed(session_index);
        let (
            session_messages_empty,
            last_error,
            cached_messages,
            streaming,
            pending_command,
            pending_command_count,
            running_command,
            command_output_lines,
        ) = {
            let session = self.active_session_ref();
            (
                session.messages.is_empty(),
                session.last_error.clone(),
                session.transcript_cache.messages.clone(),
                session.streaming,
                session.pending_commands.first().cloned(),
                session.pending_commands.len(),
                session.running_command,
                session.command_output_lines.clone(),
            )
        };
        let mut lines = Vec::new();
        if let Some(ref err) = last_error
            && session_messages_empty
        {
            lines.push(Line::from(Span::styled(
                format!("Error: {err}"),
                Style::default()
                    .fg(theme.palette.success_green)
                    .fg(Color::Red),
            )));
            return lines;
        }

        for (message_index, message) in cached_messages.into_iter().enumerate() {
            match message {
                CachedTranscriptMessage::User(u) => {
                    if !lines.is_empty() {
                        lines.push(Line::from(""));
                    }
                    for line in u.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(theme.palette.text)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                    }
                    if u.is_empty() {
                        lines.push(Line::from(Span::styled("  ", Style::default())));
                    }
                }
                CachedTranscriptMessage::Assistant(_) => {
                    let current_text = self
                        .active_session_ref()
                        .messages
                        .get(message_index)
                        .and_then(|message| match message {
                            ChatMessage::Assistant(text) => Some(text.clone()),
                            ChatMessage::User(_) => None,
                        })
                        .unwrap_or_default();
                    if current_text.starts_with("Error:") || current_text.contains("\n[Error:") {
                        lines.push(Line::from(Span::styled(
                            current_text.clone(),
                            Style::default().fg(Color::Red),
                        )));
                        continue;
                    }
                    let rendered = self.cached_assistant_lines(
                        session_index,
                        message_index,
                        theme,
                        SegmentRenderOptions::chat(),
                    );
                    if !rendered.is_empty() {
                        lines.push(Line::from(""));
                        lines.extend(rendered);
                    }
                    if current_text.is_empty() && streaming {
                        lines.push(Line::from(Span::styled(
                            "  …",
                            Style::default().fg(theme.palette.text_muted),
                        )));
                    }
                }
            }
        }

        if let Some(cmd) = pending_command.as_ref() {
            let queued_suffix = if pending_command_count > 1 {
                format!(" (+{} queued)", pending_command_count - 1)
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(theme.palette.success_green)),
                Span::styled(cmd.summary(), Style::default().fg(theme.palette.text)),
                Span::styled(queued_suffix, Style::default().fg(theme.palette.text_muted)),
                Span::styled(" ? [y/n]", Style::default().fg(theme.palette.text_muted)),
            ]));
        }

        if running_command {
            lines.push(Line::from(Span::styled(
                "⏳ Running command…",
                Style::default().fg(theme.palette.text_muted),
            )));
            for output_line in &command_output_lines {
                lines.push(Line::from(vec![
                    Span::styled("  │ ", Style::default().fg(theme.palette.subtle_border)),
                    Span::styled(
                        output_line.clone(),
                        Style::default().fg(theme.palette.text_faint),
                    ),
                ]));
            }
        }

        lines
    }

    #[cfg(test)]
    pub fn input_for_test(&self) -> &str {
        self.active_session_ref().input.as_str()
    }

    pub fn input_text(&self) -> &str {
        self.active_session_ref().input.as_str()
    }

    pub fn composer_is_empty(&self) -> bool {
        self.active_session_ref().input.trim().is_empty()
    }

    pub fn set_input_text(&mut self, text: &str) {
        let session = self.active_session_mut();
        session.input = text.to_string();
        session.cursor_char = text.chars().count();
        session.mention_popup = None;
    }

    pub fn execute_input(&mut self, theme: &crate::quorp::tui::theme::Theme, text: &str) {
        self.set_input_text(text);
        self.submit_input(theme);
    }

    #[cfg(test)]
    pub fn set_input_for_test(&mut self, s: &str) {
        self.set_input_text(s);
    }

    #[cfg(test)]
    pub fn set_base_url_for_test(&mut self, base_url: String) {
        self.base_url_override = Some(base_url);
    }

    #[cfg(test)]
    pub fn mention_popup_open_for_test(&self) -> bool {
        self.active_session_ref().mention_popup.is_some()
    }

    #[cfg(test)]
    pub fn mention_match_count_for_test(&self) -> usize {
        self.active_session_ref()
            .mention_popup
            .as_ref()
            .map(|p| p.matches.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn mention_selected_label_for_test(&self) -> Option<String> {
        let p = self.active_session_ref().mention_popup.as_ref()?;
        let e = p.matches.get(p.selected)?;
        Some(e.relative_display.clone())
    }

    #[cfg(test)]
    pub fn mention_scroll_top_for_test(&self) -> Option<usize> {
        Some(self.active_session_ref().mention_popup.as_ref()?.scroll_top)
    }

    #[cfg(test)]
    pub fn transcript_scroll_for_test(&self) -> usize {
        self.active_session_ref().transcript_scroll
    }

    #[cfg(test)]
    pub fn last_error_for_test(&self) -> Option<&str> {
        self.active_session_ref().last_error.as_deref()
    }

    #[cfg(test)]
    pub fn assistant_messages_for_test(&self) -> Vec<String> {
        self.active_session_ref()
            .messages
            .iter()
            .filter_map(|message| match message {
                ChatMessage::Assistant(text) => Some(text.clone()),
                ChatMessage::User(_) => None,
            })
            .collect()
    }

    /// Lines accumulated from [`ChatUiEvent::CommandOutput`] before [`ChatUiEvent::CommandFinished`].
    #[cfg(test)]
    pub fn command_output_lines_for_test(&self) -> Vec<String> {
        self.active_session_ref().command_output_lines.clone()
    }

    /// Output lines are drawn in the transcript only while a command is marked running (see
    /// [`ChatPane::execute_pending_command`]).
    #[cfg(test)]
    pub fn set_running_command_for_test(&mut self, running: bool) {
        let session = self.active_session_mut();
        session.running_command = running;
        if !running {
            session.running_command_name = None;
        }
    }

    #[cfg(test)]
    pub fn model_index_for_test(&self) -> usize {
        self.model_index
    }

    pub fn set_model_index_for_test(&mut self, index: usize) {
        if !self.models.is_empty() {
            self.model_index = index % self.models.len();
        }
    }

    #[cfg(test)]
    pub fn seed_messages_for_test(&mut self, messages: Vec<ChatMessage>) {
        self.active_session_mut().messages = messages;
        self.mark_active_session_transcript_dirty();
    }

    #[cfg(test)]
    pub fn last_assistant_text_for_test(&self) -> Option<&str> {
        self.active_session_ref()
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                ChatMessage::Assistant(s) => Some(s.as_str()),
                ChatMessage::User(_) => None,
            })
    }

    #[cfg(test)]
    pub fn prompt_compaction_policy_for_test(&self) -> Option<PromptCompactionPolicy> {
        self.active_session_ref().prompt_compaction_policy
    }

    #[cfg(test)]
    pub fn set_streaming_for_test(&mut self, streaming: bool) {
        self.active_session_mut().streaming = streaming;
    }
}

fn render_theme_key(theme: &crate::quorp::tui::theme::Theme) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!(
        "{:?}-{:?}-{:?}-{:?}-{:?}",
        theme.palette.text,
        theme.palette.text_faint,
        theme.palette.success_green,
        theme.palette.subtle_border,
        theme.palette.code_block_bg
    )
    .hash(&mut hasher);
    hasher.finish()
}

fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return lines;
    }
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= max_width {
            result.push(line);
            continue;
        }
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        let mut current_width = 0usize;
        for span in line.spans {
            let content = span.content.to_string();
            let style = span.style;
            let mut remaining = content.as_str();
            while !remaining.is_empty() {
                let mut chunk_end = 0usize;
                let mut chunk_width = 0usize;
                for ch in remaining.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if current_width + chunk_width + cw > max_width {
                        break;
                    }
                    chunk_width += cw;
                    chunk_end += ch.len_utf8();
                }
                if chunk_end == 0 {
                    result.push(Line::from(std::mem::take(&mut current_spans)));
                    let cw = remaining
                        .chars()
                        .next()
                        .map(|c| c.width().unwrap_or(0))
                        .unwrap_or(1);
                    let len = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    current_spans.push(Span::styled(remaining[..len].to_string(), style));
                    current_width = cw;
                    remaining = &remaining[len..];
                } else {
                    current_spans.push(Span::styled(remaining[..chunk_end].to_string(), style));
                    current_width += chunk_width;
                    remaining = &remaining[chunk_end..];
                    if current_width >= max_width && !remaining.is_empty() {
                        result.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                }
            }
        }
        result.push(Line::from(current_spans));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorp::tui::agent_protocol::AgentAction;
    use crate::quorp::tui::agent_turn::{AgentTurnResponse, MemoryUpdate, TaskItem, TaskStatus};
    use crate::quorp::tui::command_bridge::CommandBridgeRequest;
    use crate::quorp::tui::theme::Theme;
    use futures::StreamExt;
    use std::sync::{Arc, RwLock};
    use tempfile::tempdir;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    fn restore_env(name: &str, value: Option<String>) {
        unsafe {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }

    fn make_chat_pane_with_bridge() -> (
        TempDir,
        ChatPane,
        futures::channel::mpsc::UnboundedReceiver<CommandBridgeRequest>,
    ) {
        let temp_dir = tempdir().expect("tempdir for chat pane");
        let path_index = Arc::new(PathIndex::new(temp_dir.path().to_path_buf()));
        let (ui_tx, _ui_rx) = std::sync::mpsc::sync_channel(32);
        let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
        let pane = ChatPane::new(
            ui_tx,
            temp_dir.path().to_path_buf(),
            path_index,
            None,
            Some(command_tx),
        );
        (temp_dir, pane, command_rx)
    }

    fn make_chat_pane_for_snapshot() -> (TempDir, ChatPane) {
        let temp_dir = tempdir().expect("tempdir for snapshot test");
        let path_index = Arc::new(PathIndex::new(temp_dir.path().to_path_buf()));
        let (ui_tx, _ui_rx) = std::sync::mpsc::sync_channel(32);
        let pane = ChatPane::new(ui_tx, temp_dir.path().to_path_buf(), path_index, None, None);
        (temp_dir, pane)
    }

    #[test]
    fn chat_pane_reads_quorp_chat_base_url_env() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original = std::env::var("QUORP_CHAT_BASE_URL").ok();
        unsafe {
            std::env::set_var("QUORP_CHAT_BASE_URL", "http://127.0.0.1:4321/v1");
        }

        let (_temp_dir, pane) = make_chat_pane_for_snapshot();
        assert_eq!(
            pane.base_url_override_for_service(),
            Some("http://127.0.0.1:4321/v1".to_string())
        );

        if let Some(value) = original {
            unsafe {
                std::env::set_var("QUORP_CHAT_BASE_URL", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_CHAT_BASE_URL");
            }
        }
    }

    #[test]
    fn chat_pane_ignores_remote_provider_env_model_selection() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        let original_model = std::env::var("QUORP_MODEL").ok();
        let original_home = std::env::var("HOME").ok();
        let original_project_env = std::env::var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS").ok();
        unsafe {
            std::env::set_var("QUORP_PROVIDER", "ollama");
            std::env::set_var("QUORP_MODEL", "qwen2.5-coder:32b");
            std::env::remove_var("HOME");
            std::env::set_var("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", "0");
        }

        let (_temp_dir, pane) = make_chat_pane_for_snapshot();
        assert_eq!(
            pane.current_provider_kind(),
            crate::quorp::executor::InteractiveProviderKind::Local
        );
        assert_eq!(pane.current_model_id(), "qwen2.5-coder:32b");

        restore_env("QUORP_ENABLE_PROJECT_ENV_FOR_TESTS", original_project_env);
        restore_env("HOME", original_home);
        restore_env("QUORP_PROVIDER", original_provider);
        restore_env("QUORP_MODEL", original_model);
    }

    #[test]
    fn chat_pane_reads_prompt_compaction_policy_from_env() {
        let _env_lock = crate::quorp::tui::ssd_moe_tui::test_env_lock();
        let original = std::env::var("QUORP_PROMPT_COMPACTION_POLICY").ok();
        unsafe {
            std::env::set_var("QUORP_PROMPT_COMPACTION_POLICY", "last6-ledger768");
        }

        let (_temp_dir, pane) = make_chat_pane_for_snapshot();
        assert_eq!(
            pane.prompt_compaction_policy_for_test(),
            Some(PromptCompactionPolicy::Last6Ledger768)
        );

        if let Some(value) = original {
            unsafe {
                std::env::set_var("QUORP_PROMPT_COMPACTION_POLICY", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROMPT_COMPACTION_POLICY");
            }
        }
    }

    fn extract_request_type(request: CommandBridgeRequest) -> &'static str {
        match request {
            CommandBridgeRequest::ExecuteAction { action, .. } => action.tool_name(),
        }
    }

    fn flatten_lines(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn try_extract_pending_command_for_all_supported_tool_tags() {
        let cases = [
            (
                "<run_command timeout_ms=\"5000\">echo hi</run_command>",
                "run",
                "echo hi",
                None,
                true,
            ),
            (
                "<read_file path=\"src/main.rs\"></read_file>",
                "read_file",
                "src/main.rs",
                None,
                false,
            ),
            (
                "<list_directory path=\"src\"></list_directory>",
                "list_directory",
                "src",
                None,
                false,
            ),
            (
                "<write_file path=\"notes.md\">hello\\nworld</write_file>",
                "write_file",
                "notes.md",
                Some("hello\\nworld"),
                true,
            ),
            (
                "<apply_patch path=\"notes.md\">replacement</apply_patch>",
                "apply_patch",
                "notes.md",
                Some("replacement"),
                true,
            ),
            (
                r#"<mcp_call_tool server_name="docs" tool_name="search">{"query":"validation"}</mcp_call_tool>"#,
                "mcp_call_tool",
                "docs/search",
                Some(r#"{"query":"validation"}"#),
                true,
            ),
        ];

        for (tag, expected_kind, expected_path, expected_content, requires_confirmation) in cases {
            let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
            pane.seed_messages_for_test(vec![
                ChatMessage::User("u".to_string()),
                ChatMessage::Assistant(tag.to_string()),
            ]);
            pane.try_extract_pending_command_for_session(0);
            if requires_confirmation {
                let pending = pane
                    .active_session_ref()
                    .pending_commands
                    .first()
                    .expect("pending");
                match (expected_kind, pending) {
                    (
                        "run",
                        PendingCommand {
                            action:
                                AgentAction::RunCommand {
                                    command,
                                    timeout_ms,
                                },
                        },
                    ) => {
                        assert_eq!(command, expected_path);
                        assert_eq!(*timeout_ms, 5000);
                    }
                    (
                        "write_file",
                        PendingCommand {
                            action: AgentAction::WriteFile { path, content },
                        },
                    ) => {
                        assert_eq!(path, expected_path);
                        assert_eq!(content, expected_content.unwrap_or_default());
                    }
                    (
                        "apply_patch",
                        PendingCommand {
                            action: AgentAction::ApplyPatch { path, patch },
                        },
                    ) => {
                        assert_eq!(path, expected_path);
                        assert_eq!(patch, expected_content.unwrap_or_default());
                    }
                    (
                        "mcp_call_tool",
                        PendingCommand {
                            action:
                                AgentAction::McpCallTool {
                                    server_name,
                                    tool_name,
                                    arguments,
                                },
                        },
                    ) => {
                        assert_eq!(format!("{server_name}/{tool_name}"), expected_path);
                        assert_eq!(arguments.to_string(), expected_content.unwrap_or_default());
                    }
                    _ => panic!("unexpected pending variant"),
                }
            } else {
                let request = futures::executor::block_on(async { command_rx.next().await })
                    .expect("request");
                assert_eq!(extract_request_type(request), expected_kind);
                assert!(pane.active_session_ref().pending_commands.is_empty());
            }
        }
    }

    #[test]
    fn execute_pending_command_dispatches_expected_bridge_request() {
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(
                "<write_file path=\"src/main.rs\">fn main() {}</write_file>".to_string(),
            ),
        ]);
        pane.try_extract_pending_command_for_session(0);

        pane.execute_pending_command();
        assert!(pane.active_session_ref().running_command);

        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        assert_eq!(extract_request_type(request), "write_file");
        assert!(pane.active_session_ref().running_command);
        assert_eq!(
            pane.active_session_ref().running_command_name.as_deref(),
            Some("write_file src/main.rs")
        );
    }

    #[test]
    fn command_confirmation_keys_confirm_and_cancel() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant("<write_file path=\"README.md\">hi</write_file>".to_string()),
        ]);
        pane.try_extract_pending_command_for_session(0);
        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &theme
        ));
        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        assert_eq!(extract_request_type(request), "write_file");
        let (_temp_dir, mut pane, _command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant("<write_file path=\"README.md\">hi</write_file>".to_string()),
        ]);
        pane.try_extract_pending_command_for_session(0);
        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &theme
        ));
        assert!(pane.active_session_ref().pending_commands.is_empty());
        let has_cancel_msg = pane
            .active_session_ref()
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::User(text) if text == "[Command cancelled by user]"));
        assert!(has_cancel_msg);
    }

    #[test]
    fn legacy_pending_command_snapshot_compatibility_round_trip() {
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        let legacy = PersistedPendingCommand {
            kind: None,
            command: "echo hello".to_string(),
            timeout_ms: 12_000,
            query: None,
            limit: None,
            path: None,
            read_start_line: None,
            read_end_line: None,
            content: None,
            patch: None,
            search_block: None,
            replace_block: None,
            mcp_server_name: None,
            mcp_tool_name: None,
            mcp_arguments: None,
            validation_plan: None,
        };
        let snapshot = PersistedChatThreadSnapshot {
            title: "Chat 1".to_string(),
            messages: vec![PersistedChatMessage::User("hi".to_string())],
            transcript_scroll: 0,
            stick_to_bottom: true,
            input: String::new(),
            last_error: None,
            pending_command: Some(legacy),
            pending_commands: Vec::new(),
            running_command: false,
            running_command_name: None,
            command_output_lines: Vec::new(),
            model_id: "qwen3.5-35b-a3b".to_string(),
            mode: AgentMode::Act,
            prompt_compaction_policy: None,
        };
        pane.import_thread_snapshot(snapshot);
        match pane
            .active_session_ref()
            .pending_commands
            .first()
            .cloned()
            .expect("legacy pending")
        {
            PendingCommand {
                action:
                    AgentAction::RunCommand {
                        command,
                        timeout_ms,
                    },
            } => {
                assert_eq!(command, "echo hello");
                assert_eq!(timeout_ms, 12_000);
            }
            _ => panic!("legacy pending did not round-trip as run command"),
        };

        let modern = PendingCommand::new(AgentAction::ReadFile {
            path: "src/main.rs".to_string(),
            range: None,
        });
        let persisted = modern.to_persisted();
        let restored = PendingCommand::from_persisted(persisted).expect("restored");
        assert!(matches!(
            restored,
            PendingCommand {
                action: AgentAction::ReadFile { path, range }
            } if path == "src/main.rs" && range.is_none()
        ));

        let search = PendingCommand::new(AgentAction::SearchText {
            query: "AgentTurnResponse".to_string(),
            limit: 4,
        });
        let persisted = search.to_persisted();
        let restored = PendingCommand::from_persisted(persisted).expect("restored search");
        assert!(matches!(
            restored,
            PendingCommand {
                action: AgentAction::SearchText { query, limit }
            } if query == "AgentTurnResponse" && limit == 4
        ));
    }

    #[test]
    fn read_only_actions_auto_execute_after_stream_finish() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant("<read_file path=\"README.md\"></read_file>".to_string()),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);

        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        assert_eq!(extract_request_type(request), "read_file");
        assert!(pane.active_session_ref().pending_commands.is_empty());
        assert!(pane.active_session_ref().running_command);
    }

    #[test]
    fn alt_shortcuts_switch_agent_mode() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        assert_eq!(pane.active_mode_for_test(), AgentMode::Act);

        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT),
            &theme
        ));
        assert_eq!(pane.active_mode_for_test(), AgentMode::Plan);

        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT),
            &theme
        ));
        assert_eq!(pane.active_mode_for_test(), AgentMode::Ask);

        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
            &theme
        ));
        assert_eq!(pane.active_mode_for_test(), AgentMode::Act);
    }

    #[test]
    fn structured_turn_json_replaces_raw_payload_and_queues_action() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, _command_rx) = make_chat_pane_with_bridge();
        let raw_turn = serde_json::to_string(&AgentTurnResponse {
            assistant_message: "I found the file to update.".to_string(),
            actions: vec![AgentAction::WriteFile {
                path: "README.md".to_string(),
                content: "updated".to_string(),
            }],
            task_updates: vec![TaskItem {
                title: "Inspect README".to_string(),
                status: TaskStatus::Completed,
            }],
            memory_updates: vec![MemoryUpdate {
                kind: "note".to_string(),
                content: "README drives first-run instructions".to_string(),
                path: None,
            }],
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        })
        .expect("serialize");
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(raw_turn),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);

        assert!(
            pane.last_assistant_text_for_test()
                .is_some_and(|text| text.contains("Action receipts"))
        );
        assert!(matches!(
            pane.active_session_ref().pending_commands.first(),
            Some(PendingCommand {
                action: AgentAction::WriteFile { path, .. }
            }) if path == "README.md"
        ));
    }

    #[test]
    fn ask_mode_blocks_write_action_from_structured_turn() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, _command_rx) = make_chat_pane_with_bridge();
        pane.set_active_mode(AgentMode::Ask);
        let raw_turn = serde_json::to_string(&AgentTurnResponse {
            assistant_message: "Attempting a write in ask mode.".to_string(),
            actions: vec![AgentAction::WriteFile {
                path: "README.md".to_string(),
                content: "updated".to_string(),
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        })
        .expect("serialize");
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(raw_turn),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);

        assert!(pane.active_session_ref().pending_commands.is_empty());
        assert!(pane
            .active_session_ref()
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::User(text) if text.contains("Action blocked by Ask mode"))));
    }

    #[test]
    fn structured_turn_json_preserves_multiple_queued_actions() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        let raw_turn = serde_json::to_string(&AgentTurnResponse {
            assistant_message: "Reading, then updating.".to_string(),
            actions: vec![
                AgentAction::ReadFile {
                    path: "README.md".to_string(),
                    range: None,
                },
                AgentAction::WriteFile {
                    path: "README.md".to_string(),
                    content: "updated".to_string(),
                },
            ],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        })
        .expect("serialize");
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(raw_turn),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);

        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        match request {
            CommandBridgeRequest::ExecuteAction { action, .. } => {
                assert!(
                    matches!(action, AgentAction::ReadFile { ref path, range } if path == "README.md" && range.is_none())
                );
            }
        }
        assert_eq!(pane.active_session_ref().pending_commands.len(), 1);
        assert!(matches!(
            pane.active_session_ref().pending_commands.first(),
            Some(PendingCommand {
                action: AgentAction::WriteFile { path, .. }
            }) if path == "README.md"
        ));
    }

    #[test]
    fn auto_approved_reads_advance_until_confirmation_is_needed() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(
                serde_json::to_string(&AgentTurnResponse {
                    assistant_message: "Inspecting before editing.".to_string(),
                    actions: vec![
                        AgentAction::ReadFile {
                            path: "README.md".to_string(),
                            range: None,
                        },
                        AgentAction::SearchText {
                            query: "validation".to_string(),
                            limit: 4,
                        },
                        AgentAction::WriteFile {
                            path: "README.md".to_string(),
                            content: "updated".to_string(),
                        },
                    ],
                    task_updates: Vec::new(),
                    memory_updates: Vec::new(),
                    requested_mode_change: None,
                    verifier_plan: None,
                    parse_warnings: Vec::new(),
                })
                .expect("serialize"),
            ),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);
        let first =
            futures::executor::block_on(async { command_rx.next().await }).expect("read request");
        assert_eq!(extract_request_type(first), "read_file");

        pane.apply_chat_event(
            ChatUiEvent::CommandFinished(
                0,
                ActionOutcome::Success {
                    action: AgentAction::ReadFile {
                        path: "README.md".to_string(),
                        range: None,
                    },
                    output: "README".to_string(),
                },
            ),
            &theme,
        );
        let second =
            futures::executor::block_on(async { command_rx.next().await }).expect("search request");
        assert_eq!(extract_request_type(second), "search_text");

        pane.apply_chat_event(
            ChatUiEvent::CommandFinished(
                0,
                ActionOutcome::Success {
                    action: AgentAction::SearchText {
                        query: "validation".to_string(),
                        limit: 4,
                    },
                    output: "matched".to_string(),
                },
            ),
            &theme,
        );

        assert_eq!(pane.active_session_ref().pending_commands.len(), 1);
        assert!(matches!(
            pane.active_session_ref().pending_commands.first(),
            Some(PendingCommand {
                action: AgentAction::WriteFile { path, .. }
            }) if path == "README.md"
        ));
    }

    #[test]
    fn structured_turn_json_round_trips_mcp_confirmation() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        let raw_turn = serde_json::to_string(&AgentTurnResponse {
            assistant_message: "Checking docs MCP.".to_string(),
            actions: vec![AgentAction::McpCallTool {
                server_name: "docs".to_string(),
                tool_name: "search".to_string(),
                arguments: serde_json::json!({"query":"validation"}),
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        })
        .expect("serialize");
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(raw_turn),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);
        assert!(matches!(
            pane.active_session_ref().pending_commands.first(),
            Some(PendingCommand {
                action:
                    AgentAction::McpCallTool {
                        server_name,
                        tool_name,
                        ..
                    }
            }) if server_name == "docs" && tool_name == "search"
        ));

        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &theme
        ));
        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        assert_eq!(extract_request_type(request), "mcp_call_tool");
    }

    #[test]
    fn failed_command_aborts_remaining_queued_actions() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, mut command_rx) = make_chat_pane_with_bridge();
        let failed_action = AgentAction::ReadFile {
            path: "README.md".to_string(),
            range: None,
        };
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(
                serde_json::to_string(&AgentTurnResponse {
                    assistant_message: "Trying a batch.".to_string(),
                    actions: vec![
                        failed_action.clone(),
                        AgentAction::WriteFile {
                            path: "README.md".to_string(),
                            content: "updated".to_string(),
                        },
                    ],
                    task_updates: Vec::new(),
                    memory_updates: Vec::new(),
                    requested_mode_change: None,
                    verifier_plan: None,
                    parse_warnings: Vec::new(),
                })
                .expect("serialize"),
            ),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);
        let request =
            futures::executor::block_on(async { command_rx.next().await }).expect("request");
        assert_eq!(extract_request_type(request), "read_file");

        pane.apply_chat_event(
            ChatUiEvent::CommandFinished(
                0,
                ActionOutcome::Failure {
                    action: failed_action,
                    error: "read_file: missing file".to_string(),
                },
            ),
            &theme,
        );

        assert!(pane.active_session_ref().pending_commands.is_empty());
        assert!(pane
            .active_session_ref()
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::User(text) if text.contains("[Batch execution aborted]"))));
    }

    #[test]
    fn cancelling_current_confirmation_clears_entire_batch() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane, _command_rx) = make_chat_pane_with_bridge();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant(
                serde_json::to_string(&AgentTurnResponse {
                    assistant_message: "Need two writes.".to_string(),
                    actions: vec![
                        AgentAction::WriteFile {
                            path: "README.md".to_string(),
                            content: "first".to_string(),
                        },
                        AgentAction::WriteFile {
                            path: "src/main.rs".to_string(),
                            content: "second".to_string(),
                        },
                    ],
                    task_updates: Vec::new(),
                    memory_updates: Vec::new(),
                    requested_mode_change: None,
                    verifier_plan: None,
                    parse_warnings: Vec::new(),
                })
                .expect("serialize"),
            ),
        ]);

        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &theme);
        assert_eq!(pane.active_session_ref().pending_commands.len(), 2);

        assert!(pane.handle_key_event(
            &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &theme
        ));

        assert!(pane.active_session_ref().pending_commands.is_empty());
        assert!(pane
            .active_session_ref()
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::User(text) if text.contains("[Batch cancelled]"))));
    }

    #[test]
    fn thread_snapshot_round_trip_preserves_pending_command_queue() {
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        pane.seed_messages_for_test(vec![ChatMessage::User("u".to_string())]);
        pane.active_session_mut().pending_commands = vec![
            PendingCommand::new(AgentAction::WriteFile {
                path: "README.md".to_string(),
                content: "first".to_string(),
            }),
            PendingCommand::new(AgentAction::RunValidation {
                plan: ValidationPlan::default(),
            }),
        ];

        let snapshot = pane.export_active_thread_snapshot();
        let mut restored = make_chat_pane_for_snapshot().1;
        restored.import_thread_snapshot(snapshot);

        assert_eq!(restored.active_session_ref().pending_commands.len(), 2);
        assert!(matches!(
            restored.active_session_ref().pending_commands.first(),
            Some(PendingCommand {
                action: AgentAction::WriteFile { path, .. }
            }) if path == "README.md"
        ));
        assert!(matches!(
            restored.active_session_ref().pending_commands.get(1),
            Some(PendingCommand {
                action: AgentAction::RunValidation { .. }
            })
        ));
    }

    #[test]
    fn thread_snapshot_round_trip_preserves_prompt_compaction_policy() {
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        pane.active_session_mut().prompt_compaction_policy =
            Some(PromptCompactionPolicy::Last6Ledger768);

        let snapshot = pane.export_active_thread_snapshot();
        let mut restored = make_chat_pane_for_snapshot().1;
        restored.import_thread_snapshot(snapshot);

        assert_eq!(
            restored.prompt_compaction_policy_for_test(),
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
    }

    #[test]
    fn compaction_command_updates_active_session_locally() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        pane.set_input_for_test("/compaction last6-ledger768");

        pane.submit_input(&theme);

        assert_eq!(
            pane.prompt_compaction_policy_for_test(),
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert!(
            pane.last_assistant_text_for_test()
                .is_some_and(|text| text.contains("Compaction policy set to last6-ledger768"))
        );

        pane.set_input_for_test("/compaction default");
        pane.submit_input(&theme);

        assert_eq!(pane.prompt_compaction_policy_for_test(), None);
        assert!(
            pane.last_assistant_text_for_test()
                .is_some_and(|text| text.contains("Compaction policy set to default"))
        );
    }

    #[test]
    fn multi_file_apply_patch_confirmation_lists_touched_files() {
        let theme = Theme::core_tui();
        let (_temp_dir, mut pane) = make_chat_pane_for_snapshot();
        pane.active_session_mut().pending_commands =
            vec![PendingCommand::new(AgentAction::ApplyPatch {
                path: "placeholder.txt".to_string(),
                patch: concat!(
                    "--- a/src/main.rs\n",
                    "+++ b/src/main.rs\n",
                    "@@ -1 +1 @@\n",
                    "-old\n",
                    "+new\n",
                    "--- a/README.md\n",
                    "+++ b/README.md\n",
                    "@@ -1 +1 @@\n",
                    "-before\n",
                    "+after\n",
                )
                .to_string(),
            })];

        let rendered = flatten_lines(&pane.build_transcript_lines(&theme));
        assert!(rendered.contains("apply_patch 2 files: src/main.rs, README.md"));
        assert!(!rendered.contains("placeholder.txt"));
    }
}
