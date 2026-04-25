use serde::{Deserialize, Serialize};

pub fn stable_content_hash(text: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn default_run_command_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActionApprovalPolicy {
    AutoApproveReadOnly,
    RequireExplicitConfirmation,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Ask,
    Plan,
    #[default]
    Act,
}

impl AgentMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Act => "Act",
        }
    }

    pub fn allows_action(self, action: &AgentAction) -> bool {
        match self {
            Self::Ask => matches!(
                action,
                AgentAction::ReadFile { .. }
                    | AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ExplainValidationFailure { .. }
                    | AgentAction::SuggestImplementationTargets { .. }
                    | AgentAction::SuggestEditAnchors { .. }
                    | AgentAction::PreviewEdit { .. }
            ),
            Self::Plan => matches!(
                action,
                AgentAction::ReadFile { .. }
                    | AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ExplainValidationFailure { .. }
                    | AgentAction::SuggestImplementationTargets { .. }
                    | AgentAction::SuggestEditAnchors { .. }
                    | AgentAction::PreviewEdit { .. }
                    | AgentAction::RunValidation { .. }
            ),
            Self::Act => true,
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationPlan {
    #[serde(default)]
    pub fmt: bool,
    #[serde(default)]
    pub clippy: bool,
    #[serde(default)]
    pub workspace_tests: bool,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub custom_commands: Vec<String>,
}

impl ValidationPlan {
    pub fn is_empty(&self) -> bool {
        !self.fmt
            && !self.clippy
            && !self.workspace_tests
            && self.tests.is_empty()
            && self.custom_commands.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.fmt {
            parts.push("fmt".to_string());
        }
        if self.clippy {
            parts.push("clippy".to_string());
        }
        if self.workspace_tests {
            parts.push("workspace_tests".to_string());
        }
        if !self.tests.is_empty() {
            parts.push(format!("tests({})", self.tests.join(", ")));
        }
        if !self.custom_commands.is_empty() {
            parts.push(format!("custom({})", self.custom_commands.len()));
        }
        parts.join(", ")
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentAction {
    RunCommand {
        command: String,
        #[serde(default = "default_run_command_timeout_ms")]
        timeout_ms: u64,
    },
    ReadFile {
        path: String,
        #[serde(default)]
        range: Option<ReadFileRange>,
    },
    ListDirectory {
        path: String,
    },
    SearchText {
        query: String,
        limit: usize,
    },
    SearchSymbols {
        query: String,
        limit: usize,
    },
    GetRepoCapsule {
        query: Option<String>,
        limit: usize,
    },
    ExplainValidationFailure {
        command: String,
        output: String,
    },
    SuggestImplementationTargets {
        command: String,
        output: String,
        #[serde(default)]
        failing_path: Option<String>,
        #[serde(default)]
        failing_line: Option<usize>,
    },
    SuggestEditAnchors {
        path: String,
        #[serde(default)]
        range: Option<ReadFileRange>,
        #[serde(default)]
        search_hint: Option<String>,
    },
    PreviewEdit {
        path: String,
        edit: PreviewEditPayload,
    },
    ReplaceRange {
        path: String,
        range: ReadFileRange,
        expected_hash: String,
        replacement: String,
    },
    ModifyToml {
        path: String,
        expected_hash: String,
        operations: Vec<TomlEditOperation>,
    },
    ApplyPreview {
        preview_id: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    ApplyPatch {
        path: String,
        patch: String,
    },
    ReplaceBlock {
        path: String,
        search_block: String,
        replace_block: String,
        #[serde(default)]
        range: Option<ReadFileRange>,
    },
    SetExecutable {
        path: String,
    },
    McpCallTool {
        server_name: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    RunValidation {
        plan: ValidationPlan,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewEditPayload {
    ApplyPatch {
        patch: String,
    },
    ReplaceBlock {
        search_block: String,
        replace_block: String,
        #[serde(default)]
        range: Option<ReadFileRange>,
    },
    ReplaceRange {
        range: ReadFileRange,
        expected_hash: String,
        replacement: String,
    },
    ModifyToml {
        expected_hash: String,
        operations: Vec<TomlEditOperation>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TomlEditOperation {
    SetDependency {
        table: String,
        name: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        features: Vec<String>,
        #[serde(default)]
        default_features: Option<bool>,
        #[serde(default)]
        optional: Option<bool>,
        #[serde(default)]
        package: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    RemoveDependency {
        table: String,
        name: String,
    },
}

impl PreviewEditPayload {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::ApplyPatch { .. } => "apply_patch",
            Self::ReplaceBlock { .. } => "replace_block",
            Self::ReplaceRange { .. } => "replace_range",
            Self::ModifyToml { .. } => "modify_toml",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadFileRange {
    pub start_line: usize,
    pub end_line: usize,
}

impl ReadFileRange {
    pub fn normalized(self) -> Option<Self> {
        if self.start_line == 0 || self.end_line == 0 {
            return None;
        }
        let (start_line, end_line) = if self.start_line <= self.end_line {
            (self.start_line, self.end_line)
        } else {
            (self.end_line, self.start_line)
        };
        Some(Self {
            start_line,
            end_line,
        })
    }

    pub fn label(self) -> String {
        format!("{}-{}", self.start_line, self.end_line)
    }
}

impl AgentAction {
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::RunCommand { .. } => "run_command",
            Self::ReadFile { .. } => "read_file",
            Self::ListDirectory { .. } => "list_directory",
            Self::SearchText { .. } => "search_text",
            Self::SearchSymbols { .. } => "search_symbols",
            Self::GetRepoCapsule { .. } => "get_repo_capsule",
            Self::ExplainValidationFailure { .. } => "explain_validation_failure",
            Self::SuggestImplementationTargets { .. } => "suggest_implementation_targets",
            Self::SuggestEditAnchors { .. } => "suggest_edit_anchors",
            Self::PreviewEdit { .. } => "preview_edit",
            Self::ReplaceRange { .. } => "replace_range",
            Self::ModifyToml { .. } => "modify_toml",
            Self::ApplyPreview { .. } => "apply_preview",
            Self::WriteFile { .. } => "write_file",
            Self::ApplyPatch { .. } => "apply_patch",
            Self::ReplaceBlock { .. } => "replace_block",
            Self::SetExecutable { .. } => "set_executable",
            Self::McpCallTool { .. } => "mcp_call_tool",
            Self::RunValidation { .. } => "run_validation",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::RunCommand { command, .. } => format!("run: {command}"),
            Self::ReadFile { path, range } => match range.and_then(|value| value.normalized()) {
                Some(range) => format!("read_file {path} lines {}", range.label()),
                None => format!("read_file {path}"),
            },
            Self::ListDirectory { path } => format!("list_directory {path}"),
            Self::SearchText { query, .. } => format!("search_text {query}"),
            Self::SearchSymbols { query, .. } => format!("search_symbols {query}"),
            Self::GetRepoCapsule { query, .. } => match query {
                Some(query) if !query.trim().is_empty() => format!("get_repo_capsule {query}"),
                _ => "get_repo_capsule".to_string(),
            },
            Self::ExplainValidationFailure { command, .. } => {
                format!("explain_validation_failure {}", command.trim())
            }
            Self::SuggestImplementationTargets {
                command,
                failing_path,
                failing_line,
                ..
            } => {
                let location = failing_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|path| {
                        failing_line
                            .map(|line| format!(" {path}:{line}"))
                            .unwrap_or_else(|| format!(" {path}"))
                    })
                    .unwrap_or_default();
                format!(
                    "suggest_implementation_targets {}{}",
                    command.trim(),
                    location
                )
            }
            Self::SuggestEditAnchors {
                path,
                range,
                search_hint,
            } => {
                let range_label = range
                    .and_then(|value| value.normalized())
                    .map(|range| format!(" lines {}", range.label()))
                    .unwrap_or_default();
                let hint_label = search_hint
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!(" hint {}", value))
                    .unwrap_or_default();
                format!("suggest_edit_anchors {path}{range_label}{hint_label}")
            }
            Self::PreviewEdit { path, edit } => {
                format!("preview_edit {} {path}", edit.kind_label())
            }
            Self::ReplaceRange { path, range, .. } => {
                format!("replace_range {path} lines {}", range.label())
            }
            Self::ModifyToml {
                path, operations, ..
            } => {
                format!("modify_toml {path} operations({})", operations.len())
            }
            Self::ApplyPreview { preview_id } => {
                format!("apply_preview {preview_id}")
            }
            Self::WriteFile { path, .. } => format!("write_file {path}"),
            Self::ApplyPatch { path, .. } => format!("apply_patch {path}"),
            Self::ReplaceBlock { path, range, .. } => {
                match range.and_then(|value| value.normalized()) {
                    Some(range) => format!("replace_block {path} lines {}", range.label()),
                    None => format!("replace_block {path}"),
                }
            }
            Self::SetExecutable { path } => format!("set_executable {path}"),
            Self::McpCallTool {
                server_name,
                tool_name,
                ..
            } => format!("mcp_tool {server_name}/{tool_name}"),
            Self::RunValidation { plan } => {
                let summary = plan.summary();
                if summary.is_empty() {
                    "run_validation".to_string()
                } else {
                    format!("run_validation {summary}")
                }
            }
        }
    }

    pub fn approval_policy(&self) -> ActionApprovalPolicy {
        match self {
            Self::ReadFile { .. }
            | Self::ListDirectory { .. }
            | Self::SearchText { .. }
            | Self::SearchSymbols { .. }
            | Self::GetRepoCapsule { .. }
            | Self::ExplainValidationFailure { .. }
            | Self::SuggestImplementationTargets { .. }
            | Self::SuggestEditAnchors { .. }
            | Self::PreviewEdit { .. } => ActionApprovalPolicy::AutoApproveReadOnly,
            Self::RunCommand { .. }
            | Self::ReplaceRange { .. }
            | Self::ModifyToml { .. }
            | Self::ApplyPreview { .. }
            | Self::WriteFile { .. }
            | Self::ApplyPatch { .. }
            | Self::ReplaceBlock { .. }
            | Self::SetExecutable { .. }
            | Self::McpCallTool { .. }
            | Self::RunValidation { .. } => ActionApprovalPolicy::RequireExplicitConfirmation,
        }
    }

    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::ReadFile { .. }
                | Self::ListDirectory { .. }
                | Self::SearchText { .. }
                | Self::SearchSymbols { .. }
                | Self::GetRepoCapsule { .. }
                | Self::ExplainValidationFailure { .. }
                | Self::SuggestImplementationTargets { .. }
                | Self::SuggestEditAnchors { .. }
                | Self::PreviewEdit { .. }
        )
    }

    pub fn is_write_like(&self) -> bool {
        matches!(
            self,
            Self::WriteFile { .. }
                | Self::ReplaceRange { .. }
                | Self::ModifyToml { .. }
                | Self::ApplyPreview { .. }
                | Self::ApplyPatch { .. }
                | Self::ReplaceBlock { .. }
                | Self::SetExecutable { .. }
        )
    }

    pub fn followup_command_label(&self) -> String {
        match self {
            Self::RunCommand { command, .. } => command.clone(),
            Self::McpCallTool {
                server_name,
                tool_name,
                ..
            } => {
                format!("MCP {}/{}", server_name, tool_name)
            }
            Self::ReadFile { path, range } => match range.and_then(|value| value.normalized()) {
                Some(range) => format!("read {}:{}", path, range.label()),
                None => format!("read {}", path),
            },
            Self::ListDirectory { path } => format!("ls {}", path),
            Self::SearchText { query, .. } => format!("search '{}'", query),
            Self::SearchSymbols { query, .. } => format!("symbols '{}'", query),
            Self::GetRepoCapsule { query: Some(q), .. } => format!("capsule '{}'", q),
            Self::GetRepoCapsule { query: None, .. } => "capsule".to_string(),
            Self::ExplainValidationFailure { command, .. } => {
                format!("explain validation '{}'", command)
            }
            Self::SuggestImplementationTargets {
                command,
                failing_path,
                failing_line,
                ..
            } => {
                let location = failing_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|path| {
                        failing_line
                            .map(|line| format!(" at {path}:{line}"))
                            .unwrap_or_else(|| format!(" at {path}"))
                    })
                    .unwrap_or_default();
                format!("target suggestions for '{}'{}", command, location)
            }
            Self::SuggestEditAnchors { path, range, .. } => {
                match range.and_then(|value| value.normalized()) {
                    Some(range) => format!("anchors {}:{}", path, range.label()),
                    None => format!("anchors {}", path),
                }
            }
            Self::PreviewEdit { path, edit } => {
                format!("preview {} {}", edit.kind_label(), path)
            }
            Self::ReplaceRange { path, range, .. } => {
                format!("replace range {}:{}", path, range.label())
            }
            Self::ModifyToml { path, .. } => format!("modify toml {}", path),
            Self::ApplyPreview { preview_id } => format!("apply preview {}", preview_id),
            Self::WriteFile { path, .. } => format!("write {}", path),
            Self::ApplyPatch { path, .. } => format!("patch {}", path),
            Self::ReplaceBlock { path, .. } => format!("replace {}", path),
            Self::SetExecutable { path } => format!("chmod +x {}", path),
            Self::RunValidation { plan } => {
                let s = plan.summary();
                if s.is_empty() {
                    "validate".to_string()
                } else {
                    format!("validate ({})", s)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActionOutcome {
    Success { action: AgentAction, output: String },
    Failure { action: AgentAction, error: String },
}

impl ActionOutcome {
    pub fn action(&self) -> &AgentAction {
        match self {
            Self::Success { action, .. } | Self::Failure { action, .. } => action,
        }
    }

    pub fn output_text(&self) -> &str {
        match self {
            Self::Success { output, .. } => output,
            Self::Failure { error, .. } => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stable_content_hash;

    #[test]
    fn stable_content_hash_normalizes_line_endings() {
        assert_eq!(
            stable_content_hash("one\ntwo\n"),
            stable_content_hash("one\r\ntwo\r\n")
        );
        assert_eq!(
            stable_content_hash("one\ntwo\n"),
            stable_content_hash("one\rtwo\r")
        );
    }
}
