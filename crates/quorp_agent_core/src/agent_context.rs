use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::agent_protocol::{ActionApprovalPolicy, AgentAction, AgentMode, ValidationPlan};
use crate::mention_links::{collect_file_mention_uris, file_uri_to_project_path};

const REPO_INSTRUCTION_FILES: &[&str] = &[".rules", "AGENTS.md", "QWEN.md", "CLAUDE.md"];

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyProfile {
    Interactive,
    #[default]
    AutonomousHost,
    AutonomousSandboxed,
}

impl AutonomyProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::AutonomousHost => "autonomous_host",
            Self::AutonomousSandboxed => "autonomous_sandboxed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    Standard,
    BenchmarkAutonomous,
}

impl PolicyMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::BenchmarkAutonomous => "benchmark_autonomous",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PolicyAllow {
    pub read_file: bool,
    pub list_directory: bool,
    pub search_text: bool,
    pub search_symbols: bool,
    pub get_repo_capsule: bool,
    pub write_file: bool,
    pub apply_patch: bool,
    pub replace_block: bool,
    pub set_executable: bool,
    pub run_validation: bool,
    pub mcp_call_tool: bool,
    pub network: bool,
    pub run_command: Vec<String>,
}

impl Default for PolicyAllow {
    fn default() -> Self {
        Self {
            read_file: true,
            list_directory: true,
            search_text: true,
            search_symbols: true,
            get_repo_capsule: true,
            write_file: true,
            apply_patch: true,
            replace_block: true,
            set_executable: true,
            run_validation: true,
            mcp_call_tool: false,
            network: false,
            run_command: default_allowed_command_prefixes(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PolicyLimits {
    pub max_command_runtime_seconds: Option<u64>,
    pub max_command_output_bytes: Option<usize>,
}

impl Default for PolicyLimits {
    fn default() -> Self {
        Self {
            max_command_runtime_seconds: Some(120),
            max_command_output_bytes: Some(65_536),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PolicySettings {
    pub mode: PolicyMode,
    pub allow: PolicyAllow,
    pub limits: PolicyLimits,
}

impl Default for PolicySettings {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Standard,
            allow: PolicyAllow::default(),
            limits: PolicyLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AgentConfig {
    pub defaults: AgentDefaults,
    pub autonomy: AutonomySettings,
    pub policy: PolicySettings,
    pub validation: ValidationCommands,
    pub approval_rules: Vec<ApprovalRule>,
    pub extra_instruction_text: Vec<String>,
    pub extra_roots: Vec<String>,
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AgentDefaults {
    pub mode: AgentMode,
    pub default_model_id: Option<String>,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            mode: AgentMode::Act,
            default_model_id: None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AutonomySettings {
    pub profile: AutonomyProfile,
}

impl Default for AutonomySettings {
    fn default() -> Self {
        Self {
            profile: AutonomyProfile::AutonomousHost,
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ValidationCommands {
    pub fmt_command: Option<String>,
    pub clippy_command: Option<String>,
    pub workspace_test_command: Option<String>,
    pub targeted_test_prefix: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApprovalRule {
    pub action: String,
    pub path_prefix: Option<String>,
    pub command_prefix: Option<String>,
    pub mcp_server_name: Option<String>,
    pub mcp_tool_name: Option<String>,
    pub policy: ActionApprovalPolicy,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct InstructionDocument {
    pub path: PathBuf,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AgentInstructionContext {
    pub documents: Vec<InstructionDocument>,
    pub config: AgentConfig,
}

#[derive(Debug, Deserialize)]
struct AgentConfigFile {
    #[serde(default)]
    defaults: DefaultsSection,
    #[serde(default)]
    autonomy: AutonomySection,
    #[serde(default)]
    policy: PolicySection,
    #[serde(default)]
    validation: ValidationSection,
    #[serde(default)]
    approval_rules: Vec<ApprovalRuleFile>,
    #[serde(default)]
    prompt: PromptSection,
    #[serde(default)]
    extra_roots: Vec<String>,
    #[serde(default)]
    mcp_servers: Vec<McpServerFile>,
}

#[derive(Debug, Default, Deserialize)]
struct DefaultsSection {
    #[serde(default)]
    mode: Option<AgentMode>,
    #[serde(default)]
    default_model_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AutonomySection {
    #[serde(default)]
    profile: Option<AutonomyProfile>,
}

#[derive(Debug, Default, Deserialize)]
struct PolicySection {
    #[serde(default)]
    mode: Option<PolicyMode>,
    #[serde(default)]
    allow: PolicyAllowSection,
    #[serde(default)]
    limits: PolicyLimitsSection,
}

#[derive(Debug, Default, Deserialize)]
struct PolicyAllowSection {
    #[serde(default)]
    read_file: Option<bool>,
    #[serde(default)]
    list_directory: Option<bool>,
    #[serde(default)]
    search_text: Option<bool>,
    #[serde(default)]
    search_symbols: Option<bool>,
    #[serde(default)]
    get_repo_capsule: Option<bool>,
    #[serde(default)]
    write_file: Option<bool>,
    #[serde(default)]
    apply_patch: Option<bool>,
    #[serde(default)]
    replace_block: Option<bool>,
    #[serde(default)]
    set_executable: Option<bool>,
    #[serde(default)]
    run_validation: Option<bool>,
    #[serde(default)]
    mcp_call_tool: Option<bool>,
    #[serde(default)]
    network: Option<bool>,
    #[serde(default)]
    run_command: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct PolicyLimitsSection {
    #[serde(default)]
    max_command_runtime_seconds: Option<u64>,
    #[serde(default)]
    max_command_output_bytes: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct ValidationSection {
    #[serde(default)]
    fmt_command: Option<String>,
    #[serde(default)]
    clippy_command: Option<String>,
    #[serde(default)]
    workspace_test_command: Option<String>,
    #[serde(default)]
    targeted_test_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovalRuleFile {
    action: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    command_prefix: Option<String>,
    #[serde(default)]
    mcp_server_name: Option<String>,
    #[serde(default)]
    mcp_tool_name: Option<String>,
    policy: ApprovalPolicyFile,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ApprovalPolicyFile {
    AutoApproveReadOnly,
    RequireExplicitConfirmation,
}

#[derive(Debug, Default, Deserialize)]
struct PromptSection {
    #[serde(default)]
    extra_instructions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum McpServerFile {
    Shorthand(String),
    Structured(McpServerFileStructured),
}

#[derive(Debug, Deserialize)]
struct McpServerFileStructured {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

pub fn load_agent_config(project_root: &Path) -> AgentConfig {
    let config_path = project_root.join(".quorp/agent.toml");
    let Ok(raw) = std::fs::read_to_string(&config_path) else {
        return AgentConfig::default();
    };
    let parsed: AgentConfigFile = match toml::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::error!(
                "failed to parse agent config {}: {error}",
                config_path.display()
            );
            return AgentConfig::default();
        }
    };
    AgentConfig {
        defaults: AgentDefaults {
            mode: parsed.defaults.mode.unwrap_or(AgentMode::Act),
            default_model_id: parsed.defaults.default_model_id,
        },
        autonomy: AutonomySettings {
            profile: parsed.autonomy.profile.unwrap_or_default(),
        },
        policy: PolicySettings {
            mode: parsed.policy.mode.unwrap_or_default(),
            allow: merge_policy_allow(parsed.policy.allow),
            limits: merge_policy_limits(parsed.policy.limits),
        },
        validation: ValidationCommands {
            fmt_command: parsed.validation.fmt_command,
            clippy_command: parsed.validation.clippy_command,
            workspace_test_command: parsed.validation.workspace_test_command,
            targeted_test_prefix: parsed.validation.targeted_test_prefix,
        },
        approval_rules: parsed
            .approval_rules
            .into_iter()
            .map(|rule| ApprovalRule {
                action: rule.action,
                path_prefix: rule.path_prefix,
                command_prefix: rule.command_prefix,
                mcp_server_name: rule.mcp_server_name,
                mcp_tool_name: rule.mcp_tool_name,
                policy: match rule.policy {
                    ApprovalPolicyFile::AutoApproveReadOnly => {
                        ActionApprovalPolicy::AutoApproveReadOnly
                    }
                    ApprovalPolicyFile::RequireExplicitConfirmation => {
                        ActionApprovalPolicy::RequireExplicitConfirmation
                    }
                },
            })
            .collect(),
        extra_instruction_text: parsed.prompt.extra_instructions,
        extra_roots: parsed.extra_roots,
        mcp_servers: parsed
            .mcp_servers
            .into_iter()
            .filter_map(parse_mcp_server_entry)
            .collect(),
    }
}

fn parse_mcp_server_entry(entry: McpServerFile) -> Option<McpServerConfig> {
    match entry {
        McpServerFile::Structured(structured) => {
            let command = structured.command.trim();
            if structured.name.trim().is_empty() || command.is_empty() {
                log::warn!("Ignoring MCP server entry with empty name or command");
                return None;
            }
            Some(McpServerConfig {
                name: structured.name.trim().to_string(),
                command: command.to_string(),
                args: structured.args,
            })
        }
        McpServerFile::Shorthand(raw) => parse_mcp_server_shorthand(&raw),
    }
}

fn parse_mcp_server_shorthand(raw: &str) -> Option<McpServerConfig> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let split_named = |separator: char| -> Option<(String, String)> {
        let (name, command) = trimmed.split_once(separator)?;
        let name = name.trim();
        let command = command.trim();
        if name.is_empty() || command.is_empty() {
            return None;
        }
        Some((name.to_string(), command.to_string()))
    };

    let (name, command_line) = split_named('=')
        .or_else(|| split_named(':'))
        .unwrap_or_else(|| {
            let tokens = shlex::split(trimmed).unwrap_or_default();
            let inferred_name = tokens
                .first()
                .and_then(|token| {
                    std::path::Path::new(token)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                })
                .filter(|name| !name.is_empty())
                .unwrap_or("mcp")
                .to_string();
            (inferred_name, trimmed.to_string())
        });

    let tokens = match shlex::split(&command_line) {
        Some(tokens) if !tokens.is_empty() => tokens,
        _ => {
            log::warn!("Ignoring MCP server entry with invalid shell syntax: {trimmed}");
            return None;
        }
    };

    let command = tokens[0].clone();
    let args = tokens[1..].to_vec();
    Some(McpServerConfig {
        name,
        command,
        args,
    })
}

fn merge_policy_allow(section: PolicyAllowSection) -> PolicyAllow {
    let default_allow = PolicyAllow::default();
    PolicyAllow {
        read_file: section.read_file.unwrap_or(default_allow.read_file),
        list_directory: section
            .list_directory
            .unwrap_or(default_allow.list_directory),
        search_text: section.search_text.unwrap_or(default_allow.search_text),
        search_symbols: section
            .search_symbols
            .unwrap_or(default_allow.search_symbols),
        get_repo_capsule: section
            .get_repo_capsule
            .unwrap_or(default_allow.get_repo_capsule),
        write_file: section.write_file.unwrap_or(default_allow.write_file),
        apply_patch: section.apply_patch.unwrap_or(default_allow.apply_patch),
        replace_block: section.replace_block.unwrap_or(default_allow.replace_block),
        set_executable: section
            .set_executable
            .unwrap_or(default_allow.set_executable),
        run_validation: section
            .run_validation
            .unwrap_or(default_allow.run_validation),
        mcp_call_tool: section.mcp_call_tool.unwrap_or(default_allow.mcp_call_tool),
        network: section.network.unwrap_or(default_allow.network),
        run_command: section.run_command.unwrap_or(default_allow.run_command),
    }
}

fn merge_policy_limits(section: PolicyLimitsSection) -> PolicyLimits {
    let default_limits = PolicyLimits::default();
    PolicyLimits {
        max_command_runtime_seconds: section
            .max_command_runtime_seconds
            .or(default_limits.max_command_runtime_seconds),
        max_command_output_bytes: section
            .max_command_output_bytes
            .or(default_limits.max_command_output_bytes),
    }
}

fn default_allowed_command_prefixes() -> Vec<String> {
    [
        "cargo check",
        "cargo test",
        "cargo fmt",
        "cargo clippy",
        "cargo nextest",
        "cargo run",
        "./",
        "sh ./",
        "bash ./",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn load_instruction_context(project_root: &Path, user_input: &str) -> AgentInstructionContext {
    let config = load_agent_config(project_root);
    let mut documents = Vec::new();

    for path in global_instruction_candidates() {
        if let Some(document) = read_instruction_document(&path, project_root) {
            documents.push(document);
        }
    }

    for name in REPO_INSTRUCTION_FILES {
        if let Some(document) = read_instruction_document(&project_root.join(name), project_root) {
            documents.push(document);
        }
    }

    let touched_paths = touched_paths_from_user_input(project_root, user_input);
    let nested_paths = nested_instruction_paths(project_root, &touched_paths);
    for path in nested_paths {
        if let Some(document) = read_instruction_document(&path, project_root) {
            documents.push(document);
        }
    }

    for (index, text) in config.extra_instruction_text.iter().enumerate() {
        let label = format!("agent.toml prompt override {}", index + 1);
        documents.push(InstructionDocument {
            path: project_root.join(".quorp/agent.toml"),
            label,
            content: text.clone(),
        });
    }

    AgentInstructionContext { documents, config }
}

pub fn render_instruction_context_for_prompt(context: &AgentInstructionContext) -> String {
    if context.documents.is_empty() {
        return String::new();
    }
    let mut rendered = String::from("Follow these repo and user instructions in priority order:\n");
    for document in &context.documents {
        rendered.push_str(&format!(
            "\n--- {} ({}) ---\n{}\n",
            document.label,
            document.path.display(),
            document.content.trim()
        ));
    }
    rendered
}

pub fn effective_approval_policy(
    action: &AgentAction,
    config: &AgentConfig,
) -> ActionApprovalPolicy {
    let default_policy = action.approval_policy();
    if default_policy == ActionApprovalPolicy::AutoApproveReadOnly {
        return default_policy;
    }

    for rule in &config.approval_rules {
        if !rule.action.eq_ignore_ascii_case(action.tool_name()) {
            continue;
        }
        if !rule_matches_action(rule, action, config) {
            continue;
        }
        return rule.policy;
    }

    default_policy
}

pub fn validation_commands_for_plan(config: &AgentConfig, plan: &ValidationPlan) -> Vec<String> {
    let mut commands = Vec::new();
    if plan.fmt
        && let Some(command) = config.validation.fmt_command.as_ref()
    {
        commands.push(command.clone());
    }
    if plan.clippy
        && let Some(command) = config.validation.clippy_command.as_ref()
    {
        commands.push(command.clone());
    }
    if plan.workspace_tests
        && let Some(command) = config.validation.workspace_test_command.as_ref()
    {
        commands.push(command.clone());
    }
    if !plan.tests.is_empty() {
        if let Some(prefix) = config.validation.targeted_test_prefix.as_ref() {
            for test in &plan.tests {
                let test = test.trim();
                if test.is_empty() {
                    continue;
                }
                if looks_like_full_validation_command(test) {
                    commands.push(test.to_string());
                } else {
                    commands.push(format!("{} {}", prefix.trim_end(), test));
                }
            }
        } else if let Some(command) = config.validation.workspace_test_command.as_ref() {
            commands.push(command.clone());
        }
    }
    commands.extend(plan.custom_commands.iter().cloned());
    commands
}

fn looks_like_full_validation_command(command: &str) -> bool {
    let normalized = command.trim_start();
    normalized.starts_with("cargo ")
        || normalized.starts_with("./")
        || normalized.starts_with("bash ")
        || normalized.starts_with("sh ")
}

fn rule_matches_action(rule: &ApprovalRule, action: &AgentAction, config: &AgentConfig) -> bool {
    match action {
        AgentAction::RunCommand { command, .. } => match rule.command_prefix.as_ref() {
            Some(prefix) => command.trim_start().starts_with(prefix),
            None => false,
        },
        AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::SetExecutable { path }
        | AgentAction::ReadFile { path, .. }
        | AgentAction::ListDirectory { path }
        | AgentAction::SuggestEditAnchors { path, .. }
        | AgentAction::PreviewEdit { path, .. } => match rule.path_prefix.as_ref() {
            Some(prefix) => path.starts_with(prefix),
            None => false,
        },
        AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::ApplyPreview { .. } => false,
        AgentAction::McpCallTool {
            server_name,
            tool_name,
            ..
        } => {
            rule.mcp_server_name
                .as_deref()
                .is_none_or(|expected| expected.eq_ignore_ascii_case(server_name))
                && rule
                    .mcp_tool_name
                    .as_deref()
                    .is_none_or(|expected| expected.eq_ignore_ascii_case(tool_name))
        }
        AgentAction::RunValidation { plan } => {
            let commands = validation_commands_for_plan(config, plan);
            match rule.command_prefix.as_ref() {
                Some(prefix) => {
                    !commands.is_empty()
                        && commands
                            .iter()
                            .all(|command| command.trim_start().starts_with(prefix))
                }
                None => false,
            }
        }
    }
}

fn global_instruction_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let base = PathBuf::from(home).join(".config/quorp");
        for name in REPO_INSTRUCTION_FILES {
            paths.push(base.join(name));
        }
    }
    paths
}

fn read_instruction_document(path: &Path, project_root: &Path) -> Option<InstructionDocument> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return None;
    };
    let label = path
        .strip_prefix(project_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string());
    Some(InstructionDocument {
        path: path.to_path_buf(),
        label,
        content,
    })
}

fn touched_paths_from_user_input(project_root: &Path, user_input: &str) -> Vec<PathBuf> {
    collect_file_mention_uris(user_input)
        .into_iter()
        .filter_map(|uri| file_uri_to_project_path(&uri, project_root))
        .collect()
}

fn nested_instruction_paths(project_root: &Path, touched_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    for touched_path in touched_paths {
        let mut current = if touched_path.is_dir() {
            touched_path.clone()
        } else {
            touched_path.parent().unwrap_or(project_root).to_path_buf()
        };
        while current.starts_with(project_root) && current != project_root {
            for name in REPO_INSTRUCTION_FILES {
                let candidate = current.join(name);
                if candidate.exists() {
                    out.insert(candidate);
                }
            }
            let Some(parent) = current.parent() else {
                break;
            };
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_protocol::AgentAction;

    fn base_config() -> AgentConfig {
        AgentConfig {
            validation: ValidationCommands {
                fmt_command: Some("cargo fmt --check".to_string()),
                clippy_command: Some(
                    "cargo clippy --all-targets --no-deps -- -D warnings".to_string(),
                ),
                workspace_test_command: Some("cargo test".to_string()),
                targeted_test_prefix: Some("cargo test ".to_string()),
            },
            ..AgentConfig::default()
        }
    }

    #[test]
    fn mcp_approval_rule_can_scope_to_server_and_tool() {
        let mut config = base_config();
        config.approval_rules.push(ApprovalRule {
            action: "mcp_call_tool".to_string(),
            path_prefix: None,
            command_prefix: None,
            mcp_server_name: Some("docs".to_string()),
            mcp_tool_name: Some("search".to_string()),
            policy: ActionApprovalPolicy::AutoApproveReadOnly,
        });

        let matching = AgentAction::McpCallTool {
            server_name: "docs".to_string(),
            tool_name: "search".to_string(),
            arguments: serde_json::json!({"query":"validation"}),
        };
        let wrong_tool = AgentAction::McpCallTool {
            server_name: "docs".to_string(),
            tool_name: "fetch".to_string(),
            arguments: serde_json::json!({"id":1}),
        };

        assert_eq!(
            effective_approval_policy(&matching, &config),
            ActionApprovalPolicy::AutoApproveReadOnly
        );
        assert_eq!(
            effective_approval_policy(&wrong_tool, &config),
            ActionApprovalPolicy::RequireExplicitConfirmation
        );
    }

    #[test]
    fn mcp_approval_rule_without_tool_matches_entire_server() {
        let mut config = base_config();
        config.approval_rules.push(ApprovalRule {
            action: "mcp_call_tool".to_string(),
            path_prefix: None,
            command_prefix: None,
            mcp_server_name: Some("filesystem".to_string()),
            mcp_tool_name: None,
            policy: ActionApprovalPolicy::AutoApproveReadOnly,
        });

        let matching = AgentAction::McpCallTool {
            server_name: "filesystem".to_string(),
            tool_name: "read_text_file".to_string(),
            arguments: serde_json::json!({"path":"README.md"}),
        };
        let other_server = AgentAction::McpCallTool {
            server_name: "docs".to_string(),
            tool_name: "read_text_file".to_string(),
            arguments: serde_json::json!({"path":"README.md"}),
        };

        assert_eq!(
            effective_approval_policy(&matching, &config),
            ActionApprovalPolicy::AutoApproveReadOnly
        );
        assert_eq!(
            effective_approval_policy(&other_server, &config),
            ActionApprovalPolicy::RequireExplicitConfirmation
        );
    }

    #[test]
    fn validation_commands_do_not_prepend_prefix_to_full_commands() {
        let commands = validation_commands_for_plan(
            &base_config(),
            &ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: false,
                tests: vec!["cargo test -p toy-domain --quiet".to_string()],
                custom_commands: Vec::new(),
            },
        );
        assert_eq!(
            commands,
            vec!["cargo test -p toy-domain --quiet".to_string()]
        );
    }

    #[test]
    fn validation_commands_still_prefix_targeted_selectors() {
        let commands = validation_commands_for_plan(
            &base_config(),
            &ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: false,
                tests: vec!["-p toy-domain --quiet".to_string()],
                custom_commands: Vec::new(),
            },
        );
        assert_eq!(
            commands,
            vec!["cargo test -p toy-domain --quiet".to_string()]
        );
    }
}
