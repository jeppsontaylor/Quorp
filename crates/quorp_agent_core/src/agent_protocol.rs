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
                    | AgentAction::FindFiles { .. }
                    | AgentAction::StructuralSearch { .. }
                    | AgentAction::StructuralEditPreview { .. }
                    | AgentAction::CargoDiagnostics { .. }
                    | AgentAction::LspDiagnostics { .. }
                    | AgentAction::LspDefinition { .. }
                    | AgentAction::LspReferences { .. }
                    | AgentAction::LspHover { .. }
                    | AgentAction::LspWorkspaceSymbols { .. }
                    | AgentAction::LspDocumentSymbols { .. }
                    | AgentAction::LspCodeActions { .. }
                    | AgentAction::LspRenamePreview { .. }
                    | AgentAction::McpListTools { .. }
                    | AgentAction::McpListResources { .. }
                    | AgentAction::McpReadResource { .. }
                    | AgentAction::McpListPrompts { .. }
                    | AgentAction::McpGetPrompt { .. }
                    | AgentAction::ProcessRead { .. }
                    | AgentAction::ProcessWaitForPort { .. }
                    | AgentAction::BrowserOpen { .. }
                    | AgentAction::BrowserScreenshot { .. }
                    | AgentAction::BrowserConsoleLogs { .. }
                    | AgentAction::BrowserNetworkErrors { .. }
                    | AgentAction::BrowserAccessibilitySnapshot { .. }
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
                    | AgentAction::FindFiles { .. }
                    | AgentAction::StructuralSearch { .. }
                    | AgentAction::StructuralEditPreview { .. }
                    | AgentAction::CargoDiagnostics { .. }
                    | AgentAction::LspDiagnostics { .. }
                    | AgentAction::LspDefinition { .. }
                    | AgentAction::LspReferences { .. }
                    | AgentAction::LspHover { .. }
                    | AgentAction::LspWorkspaceSymbols { .. }
                    | AgentAction::LspDocumentSymbols { .. }
                    | AgentAction::LspCodeActions { .. }
                    | AgentAction::LspRenamePreview { .. }
                    | AgentAction::McpListTools { .. }
                    | AgentAction::McpListResources { .. }
                    | AgentAction::McpReadResource { .. }
                    | AgentAction::McpListPrompts { .. }
                    | AgentAction::McpGetPrompt { .. }
                    | AgentAction::ProcessRead { .. }
                    | AgentAction::ProcessWaitForPort { .. }
                    | AgentAction::BrowserOpen { .. }
                    | AgentAction::BrowserScreenshot { .. }
                    | AgentAction::BrowserConsoleLogs { .. }
                    | AgentAction::BrowserNetworkErrors { .. }
                    | AgentAction::BrowserAccessibilitySnapshot { .. }
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
    FindFiles {
        query: String,
        limit: usize,
    },
    StructuralSearch {
        pattern: String,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        path: Option<String>,
        limit: usize,
    },
    StructuralEditPreview {
        pattern: String,
        rewrite: String,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    CargoDiagnostics {
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        include_clippy: bool,
    },
    LspDiagnostics {
        path: String,
    },
    LspDefinition {
        path: String,
        symbol: String,
        #[serde(default)]
        line: Option<usize>,
        #[serde(default)]
        character: Option<usize>,
    },
    LspReferences {
        #[serde(default)]
        path: Option<String>,
        symbol: String,
        #[serde(default)]
        line: Option<usize>,
        #[serde(default)]
        character: Option<usize>,
        limit: usize,
    },
    LspHover {
        path: String,
        line: usize,
        character: usize,
    },
    LspWorkspaceSymbols {
        query: String,
        limit: usize,
    },
    LspDocumentSymbols {
        path: String,
    },
    LspCodeActions {
        path: String,
        line: usize,
        character: usize,
    },
    LspRenamePreview {
        path: String,
        old_name: String,
        new_name: String,
        #[serde(default)]
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
    McpListTools {
        server_name: String,
    },
    McpListResources {
        server_name: String,
        #[serde(default)]
        cursor: Option<String>,
    },
    McpReadResource {
        server_name: String,
        uri: String,
    },
    McpListPrompts {
        server_name: String,
        #[serde(default)]
        cursor: Option<String>,
    },
    McpGetPrompt {
        server_name: String,
        name: String,
        #[serde(default)]
        arguments: Option<serde_json::Value>,
    },
    ProcessStart {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    ProcessRead {
        process_id: String,
        #[serde(default = "default_process_tail_lines")]
        tail_lines: usize,
    },
    ProcessWrite {
        process_id: String,
        stdin: String,
    },
    ProcessStop {
        process_id: String,
    },
    ProcessWaitForPort {
        process_id: String,
        host: String,
        port: u16,
        #[serde(default = "default_process_wait_timeout_ms")]
        timeout_ms: u64,
    },
    BrowserOpen {
        url: String,
        #[serde(default)]
        headless: bool,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
    },
    BrowserScreenshot {
        browser_id: String,
    },
    BrowserConsoleLogs {
        browser_id: String,
        #[serde(default = "default_browser_log_limit")]
        limit: usize,
    },
    BrowserNetworkErrors {
        browser_id: String,
        #[serde(default = "default_browser_log_limit")]
        limit: usize,
    },
    BrowserAccessibilitySnapshot {
        browser_id: String,
    },
    BrowserClose {
        browser_id: String,
    },
    RunValidation {
        plan: ValidationPlan,
    },
}

fn default_process_tail_lines() -> usize {
    200
}

fn default_process_wait_timeout_ms() -> u64 {
    60_000
}

fn default_browser_log_limit() -> usize {
    100
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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: usize,
    pub character: usize,
}

impl TextPosition {
    pub fn label(self) -> String {
        format!("{}:{}", self.line, self.character)
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
            Self::FindFiles { .. } => "find_files",
            Self::StructuralSearch { .. } => "structural_search",
            Self::StructuralEditPreview { .. } => "structural_edit_preview",
            Self::CargoDiagnostics { .. } => "cargo_diagnostics",
            Self::LspDiagnostics { .. } => "lsp_diagnostics",
            Self::LspDefinition { .. } => "lsp_definition",
            Self::LspReferences { .. } => "lsp_references",
            Self::LspHover { .. } => "lsp_hover",
            Self::LspWorkspaceSymbols { .. } => "lsp_workspace_symbols",
            Self::LspDocumentSymbols { .. } => "lsp_document_symbols",
            Self::LspCodeActions { .. } => "lsp_code_actions",
            Self::LspRenamePreview { .. } => "lsp_rename_preview",
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
            Self::McpListTools { .. } => "mcp_list_tools",
            Self::McpListResources { .. } => "mcp_list_resources",
            Self::McpReadResource { .. } => "mcp_read_resource",
            Self::McpListPrompts { .. } => "mcp_list_prompts",
            Self::McpGetPrompt { .. } => "mcp_get_prompt",
            Self::ProcessStart { .. } => "process_start",
            Self::ProcessRead { .. } => "process_read",
            Self::ProcessWrite { .. } => "process_write",
            Self::ProcessStop { .. } => "process_stop",
            Self::ProcessWaitForPort { .. } => "process_wait_for_port",
            Self::BrowserOpen { .. } => "browser_open",
            Self::BrowserScreenshot { .. } => "browser_screenshot",
            Self::BrowserConsoleLogs { .. } => "browser_console_logs",
            Self::BrowserNetworkErrors { .. } => "browser_network_errors",
            Self::BrowserAccessibilitySnapshot { .. } => "browser_accessibility_snapshot",
            Self::BrowserClose { .. } => "browser_close",
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
            Self::FindFiles { query, .. } => format!("find_files {query}"),
            Self::StructuralSearch { pattern, path, .. } => {
                let scope = path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!(" in {value}"))
                    .unwrap_or_default();
                format!("structural_search {pattern}{scope}")
            }
            Self::StructuralEditPreview { path, .. } => {
                let scope = path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(".");
                format!("structural_edit_preview {scope}")
            }
            Self::CargoDiagnostics { command, .. } => {
                format!(
                    "cargo_diagnostics {}",
                    command
                        .as_deref()
                        .unwrap_or("cargo check --message-format=json")
                )
            }
            Self::LspDiagnostics { path } => format!("lsp_diagnostics {path}"),
            Self::LspDefinition {
                path,
                symbol,
                line,
                character,
            } => {
                let location = match (line, character) {
                    (Some(line), Some(character)) => format!(" {line}:{character}"),
                    _ => String::new(),
                };
                format!("lsp_definition {path} {symbol}{location}")
            }
            Self::LspReferences {
                path,
                symbol,
                line,
                character,
                ..
            } => {
                let location = match (line, character) {
                    (Some(line), Some(character)) => format!(" {line}:{character}"),
                    _ => String::new(),
                };
                match path {
                    Some(path) => format!("lsp_references {path} {symbol}{location}"),
                    None => format!("lsp_references {symbol}{location}"),
                }
            }
            Self::LspHover {
                path,
                line,
                character,
            } => format!("lsp_hover {path} {line}:{character}"),
            Self::LspWorkspaceSymbols { query, .. } => {
                format!("lsp_workspace_symbols {query}")
            }
            Self::LspDocumentSymbols { path } => format!("lsp_document_symbols {path}"),
            Self::LspCodeActions {
                path,
                line,
                character,
            } => format!("lsp_code_actions {path} {line}:{character}"),
            Self::LspRenamePreview {
                path,
                old_name,
                new_name,
                ..
            } => {
                format!("lsp_rename_preview {path} {old_name} -> {new_name}")
            }
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
            Self::McpListTools { server_name } => format!("mcp_list_tools {server_name}"),
            Self::McpListResources {
                server_name,
                cursor,
            } => match cursor {
                Some(cursor) => format!("mcp_list_resources {server_name} {cursor}"),
                None => format!("mcp_list_resources {server_name}"),
            },
            Self::McpReadResource { server_name, uri } => {
                format!("mcp_read_resource {server_name} {uri}")
            }
            Self::McpListPrompts {
                server_name,
                cursor,
            } => match cursor {
                Some(cursor) => format!("mcp_list_prompts {server_name} {cursor}"),
                None => format!("mcp_list_prompts {server_name}"),
            },
            Self::McpGetPrompt {
                server_name, name, ..
            } => format!("mcp_get_prompt {server_name} {name}"),
            Self::ProcessStart {
                command, args, cwd, ..
            } => {
                let cwd = cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!(" cwd {value}"))
                    .unwrap_or_default();
                let args = if args.is_empty() {
                    String::new()
                } else {
                    format!(" args({})", args.join(" "))
                };
                format!("process_start {command}{args}{cwd}")
            }
            Self::ProcessRead {
                process_id,
                tail_lines,
            } => format!("process_read {process_id} tail {tail_lines}"),
            Self::ProcessWrite { process_id, .. } => format!("process_write {process_id}"),
            Self::ProcessStop { process_id } => format!("process_stop {process_id}"),
            Self::ProcessWaitForPort {
                process_id,
                host,
                port,
                timeout_ms,
            } => format!("process_wait_for_port {process_id} {host}:{port} timeout {timeout_ms}"),
            Self::BrowserOpen {
                url,
                headless,
                width,
                height,
            } => format!(
                "browser_open {url} headless={} viewport={:?}x{:?}",
                headless, width, height
            ),
            Self::BrowserScreenshot { browser_id } => {
                format!("browser_screenshot {browser_id}")
            }
            Self::BrowserConsoleLogs { browser_id, limit } => {
                format!("browser_console_logs {browser_id} limit {limit}")
            }
            Self::BrowserNetworkErrors { browser_id, limit } => {
                format!("browser_network_errors {browser_id} limit {limit}")
            }
            Self::BrowserAccessibilitySnapshot { browser_id } => {
                format!("browser_accessibility_snapshot {browser_id}")
            }
            Self::BrowserClose { browser_id } => format!("browser_close {browser_id}"),
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
            | Self::FindFiles { .. }
            | Self::StructuralSearch { .. }
            | Self::StructuralEditPreview { .. }
            | Self::CargoDiagnostics { .. }
            | Self::LspDiagnostics { .. }
            | Self::LspDefinition { .. }
            | Self::LspReferences { .. }
            | Self::LspHover { .. }
            | Self::LspWorkspaceSymbols { .. }
            | Self::LspDocumentSymbols { .. }
            | Self::LspCodeActions { .. }
            | Self::LspRenamePreview { .. }
            | Self::GetRepoCapsule { .. }
            | Self::ExplainValidationFailure { .. }
            | Self::SuggestImplementationTargets { .. }
            | Self::SuggestEditAnchors { .. }
            | Self::PreviewEdit { .. }
            | Self::McpListTools { .. }
            | Self::McpListResources { .. }
            | Self::McpReadResource { .. }
            | Self::McpListPrompts { .. }
            | Self::McpGetPrompt { .. } => ActionApprovalPolicy::AutoApproveReadOnly,
            Self::ProcessRead { .. }
            | Self::ProcessWaitForPort { .. }
            | Self::BrowserOpen { .. }
            | Self::BrowserScreenshot { .. }
            | Self::BrowserConsoleLogs { .. }
            | Self::BrowserNetworkErrors { .. }
            | Self::BrowserAccessibilitySnapshot { .. } => {
                ActionApprovalPolicy::AutoApproveReadOnly
            }
            Self::RunCommand { .. }
            | Self::ProcessStart { .. }
            | Self::ProcessWrite { .. }
            | Self::ProcessStop { .. }
            | Self::BrowserClose { .. }
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
                | Self::FindFiles { .. }
                | Self::StructuralSearch { .. }
                | Self::StructuralEditPreview { .. }
                | Self::CargoDiagnostics { .. }
                | Self::LspDiagnostics { .. }
                | Self::LspDefinition { .. }
                | Self::LspReferences { .. }
                | Self::LspHover { .. }
                | Self::LspWorkspaceSymbols { .. }
                | Self::LspDocumentSymbols { .. }
                | Self::LspCodeActions { .. }
                | Self::LspRenamePreview { .. }
                | Self::GetRepoCapsule { .. }
                | Self::ExplainValidationFailure { .. }
                | Self::SuggestImplementationTargets { .. }
                | Self::SuggestEditAnchors { .. }
                | Self::PreviewEdit { .. }
                | Self::McpListTools { .. }
                | Self::McpListResources { .. }
                | Self::McpReadResource { .. }
                | Self::McpListPrompts { .. }
                | Self::McpGetPrompt { .. }
                | Self::ProcessRead { .. }
                | Self::ProcessWaitForPort { .. }
                | Self::BrowserOpen { .. }
                | Self::BrowserScreenshot { .. }
                | Self::BrowserConsoleLogs { .. }
                | Self::BrowserNetworkErrors { .. }
                | Self::BrowserAccessibilitySnapshot { .. }
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
            Self::McpListTools { server_name } => format!("MCP {server_name}/tools/list"),
            Self::McpListResources {
                server_name,
                cursor,
            } => match cursor {
                Some(cursor) => format!("MCP {server_name}/resources/list {cursor}"),
                None => format!("MCP {server_name}/resources/list"),
            },
            Self::McpReadResource { server_name, uri } => {
                format!("MCP {server_name}/resources/read {uri}")
            }
            Self::McpListPrompts {
                server_name,
                cursor,
            } => match cursor {
                Some(cursor) => format!("MCP {server_name}/prompts/list {cursor}"),
                None => format!("MCP {server_name}/prompts/list"),
            },
            Self::McpGetPrompt {
                server_name, name, ..
            } => format!("MCP {server_name}/prompts/get {name}"),
            Self::ReadFile { path, range } => match range.and_then(|value| value.normalized()) {
                Some(range) => format!("read {}:{}", path, range.label()),
                None => format!("read {}", path),
            },
            Self::ListDirectory { path } => format!("ls {}", path),
            Self::SearchText { query, .. } => format!("search '{}'", query),
            Self::SearchSymbols { query, .. } => format!("symbols '{}'", query),
            Self::FindFiles { query, .. } => format!("find files '{}'", query),
            Self::StructuralSearch { pattern, .. } => format!("structural search '{}'", pattern),
            Self::StructuralEditPreview { path, .. } => {
                format!("structural edit preview {}", path.as_deref().unwrap_or("."))
            }
            Self::CargoDiagnostics { command, .. } => {
                format!(
                    "cargo diagnostics '{}'",
                    command
                        .as_deref()
                        .unwrap_or("cargo check --message-format=json")
                )
            }
            Self::LspDiagnostics { path } => format!("lsp diagnostics {}", path),
            Self::LspDefinition {
                path,
                symbol,
                line,
                character,
            } => {
                let location = match (line, character) {
                    (Some(line), Some(character)) => format!(" at {}:{}", line, character),
                    _ => String::new(),
                };
                format!("definition '{}' in {}{}", symbol, path, location)
            }
            Self::LspReferences { path, symbol, .. } => match path {
                Some(path) => format!("references '{}' in {}", symbol, path),
                None => format!("references '{}'", symbol),
            },
            Self::LspHover {
                path,
                line,
                character,
            } => format!("hover {}:{} in {}", line, character, path),
            Self::LspWorkspaceSymbols { query, .. } => format!("workspace symbols '{}'", query),
            Self::LspDocumentSymbols { path } => format!("document symbols {}", path),
            Self::LspCodeActions {
                path,
                line,
                character,
            } => {
                format!("code actions {}:{} in {}", line, character, path)
            }
            Self::LspRenamePreview {
                path,
                old_name,
                new_name,
                ..
            } => {
                format!("rename preview {} -> {} in {}", old_name, new_name, path)
            }
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
            Self::ProcessStart { command, .. } => format!("process start {}", command),
            Self::ProcessRead { process_id, .. } => format!("process read {}", process_id),
            Self::ProcessWrite { process_id, .. } => format!("process write {}", process_id),
            Self::ProcessStop { process_id } => format!("process stop {}", process_id),
            Self::ProcessWaitForPort {
                process_id,
                host,
                port,
                ..
            } => {
                format!("process wait {} {}:{}", process_id, host, port)
            }
            Self::BrowserOpen { url, .. } => format!("browser open {}", url),
            Self::BrowserScreenshot { browser_id } => format!("browser screenshot {}", browser_id),
            Self::BrowserConsoleLogs { browser_id, .. } => {
                format!("browser console logs {}", browser_id)
            }
            Self::BrowserNetworkErrors { browser_id, .. } => {
                format!("browser network errors {}", browser_id)
            }
            Self::BrowserAccessibilitySnapshot { browser_id } => {
                format!("browser accessibility snapshot {}", browser_id)
            }
            Self::BrowserClose { browser_id } => format!("browser close {}", browser_id),
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
