use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::agent_protocol::{ActionApprovalPolicy, AgentAction, AgentMode, ValidationPlan};
use crate::mention_links::{collect_file_mention_uris, file_uri_to_project_path};
use quorp_core::skills::{SkillCatalog, discover_skill_catalog};

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
    pub process_control: bool,
    pub browser_control: bool,
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
            process_control: false,
            browser_control: false,
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
    pub agent_tools: AgentToolsSettings,
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
pub struct AgentToolsSettings {
    pub enabled: bool,
    pub fd: ExternalToolSettings,
    pub ast_grep: AstGrepToolSettings,
    pub cargo_diagnostics: CargoDiagnosticsToolSettings,
    pub nextest: NextestToolSettings,
    pub cargo_expand: ExternalToolSettings,
    pub rust_analyzer: ExternalToolSettings,
    pub serena: ExternalToolSettings,
    pub browser: BrowserToolSettings,
}

impl Default for AgentToolsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            fd: ExternalToolSettings {
                enabled: true,
                command: "fd".to_string(),
                max_runtime_seconds: Some(30),
                max_output_bytes: Some(16 * 1024),
            },
            ast_grep: AstGrepToolSettings {
                enabled: true,
                command: "ast-grep".to_string(),
                max_runtime_seconds: Some(30),
                max_output_bytes: Some(32 * 1024),
                allow_rewrite_preview: false,
                allow_apply: false,
            },
            cargo_diagnostics: CargoDiagnosticsToolSettings {
                enabled: true,
                check_command: "cargo check --message-format=json".to_string(),
                clippy_command: Some(
                    "cargo clippy --message-format=json --all-targets --no-deps".to_string(),
                ),
                max_runtime_seconds: Some(120),
                max_output_bytes: Some(128 * 1024),
            },
            nextest: NextestToolSettings {
                enabled: true,
                command: "cargo nextest run".to_string(),
                max_runtime_seconds: Some(120),
                max_output_bytes: Some(64 * 1024),
                prefer_for_workspace_tests: true,
            },
            cargo_expand: ExternalToolSettings {
                enabled: false,
                command: "cargo expand".to_string(),
                max_runtime_seconds: Some(60),
                max_output_bytes: Some(64 * 1024),
            },
            rust_analyzer: ExternalToolSettings {
                enabled: false,
                command: "rust-analyzer".to_string(),
                max_runtime_seconds: Some(30),
                max_output_bytes: Some(32 * 1024),
            },
            serena: ExternalToolSettings {
                enabled: false,
                command: "serena".to_string(),
                max_runtime_seconds: Some(30),
                max_output_bytes: Some(32 * 1024),
            },
            browser: BrowserToolSettings {
                enabled: false,
                command: "node".to_string(),
                args: vec!["-e".to_string()],
                max_runtime_seconds: Some(120),
                max_output_bytes: Some(128 * 1024),
                url_policy: BrowserUrlPolicy::default(),
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExternalToolSettings {
    pub enabled: bool,
    pub command: String,
    pub max_runtime_seconds: Option<u64>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BrowserToolSettings {
    pub enabled: bool,
    pub command: String,
    pub args: Vec<String>,
    pub max_runtime_seconds: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub url_policy: BrowserUrlPolicy,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum BrowserUrlPolicy {
    #[default]
    LocalOnly,
    AllowRemote,
}

impl BrowserUrlPolicy {
    pub fn allows_url(self, url: &str) -> anyhow::Result<()> {
        let parsed_url = Url::parse(url)
            .map_err(|error| anyhow::anyhow!("invalid browser URL `{url}`: {error}"))?;
        match self {
            Self::AllowRemote => Ok(()),
            Self::LocalOnly => {
                let scheme = parsed_url.scheme();
                if matches!(scheme, "file" | "data" | "about") {
                    return Ok(());
                }
                if !matches!(scheme, "http" | "https") {
                    return Err(anyhow::anyhow!(
                        "browser URL scheme `{scheme}` is not allowed by the local-only policy"
                    ));
                }
                if is_local_browser_host(parsed_url.host_str()) {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(
                        "browser URL `{url}` is not allowed by the local-only policy"
                    ))
                }
            }
        }
    }
}

fn is_local_browser_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("127.0.0.1")
        || host.eq_ignore_ascii_case("::1")
        || host.ends_with(".localhost")
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AstGrepToolSettings {
    pub enabled: bool,
    pub command: String,
    pub max_runtime_seconds: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub allow_rewrite_preview: bool,
    pub allow_apply: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CargoDiagnosticsToolSettings {
    pub enabled: bool,
    pub check_command: String,
    pub clippy_command: Option<String>,
    pub max_runtime_seconds: Option<u64>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NextestToolSettings {
    pub enabled: bool,
    pub command: String,
    pub max_runtime_seconds: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub prefer_for_workspace_tests: bool,
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
    pub project_root: PathBuf,
    pub documents: Vec<InstructionDocument>,
    pub config: AgentConfig,
    pub skill_catalog: SkillCatalog,
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
    agent_tools: AgentToolsSection,
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
struct GlobalSettingsFile {
    #[serde(default)]
    agent_tools: AgentToolsSection,
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
    process_control: Option<bool>,
    #[serde(default)]
    browser_control: Option<bool>,
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

#[derive(Debug, Default, Deserialize)]
struct AgentToolsSection {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    tools: AgentToolsFile,
}

#[derive(Debug, Default, Deserialize)]
struct AgentToolsFile {
    #[serde(default)]
    fd: ToolSection,
    #[serde(default)]
    ast_grep: ToolSection,
    #[serde(default)]
    cargo_diagnostics: ToolSection,
    #[serde(default)]
    nextest: ToolSection,
    #[serde(default)]
    cargo_expand: ToolSection,
    #[serde(default)]
    rust_analyzer: ToolSection,
    #[serde(default)]
    serena: ToolSection,
    #[serde(default)]
    browser: BrowserToolSection,
}

#[derive(Debug, Default, Deserialize)]
struct BrowserToolSection {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    max_runtime_seconds: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    #[serde(default)]
    allow_remote_urls: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolSection {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    check_command: Option<String>,
    #[serde(default)]
    clippy_command: Option<String>,
    #[serde(default)]
    max_runtime_seconds: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    #[serde(default)]
    allow_rewrite_preview: Option<bool>,
    #[serde(default)]
    allow_apply: Option<bool>,
    #[serde(default)]
    prefer_for_workspace_tests: Option<bool>,
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
    let mut config = AgentConfig {
        agent_tools: load_global_agent_tools_settings(),
        ..AgentConfig::default()
    };
    let config_path = project_root.join(".quorp/agent.toml");
    let Ok(raw) = std::fs::read_to_string(&config_path) else {
        return config;
    };
    let parsed: AgentConfigFile = match toml::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::error!(
                "failed to parse agent config {}: {error}",
                config_path.display()
            );
            return config;
        }
    };
    config.defaults = AgentDefaults {
        mode: parsed.defaults.mode.unwrap_or(AgentMode::Act),
        default_model_id: parsed.defaults.default_model_id,
    };
    config.autonomy = AutonomySettings {
        profile: parsed.autonomy.profile.unwrap_or_default(),
    };
    config.policy = PolicySettings {
        mode: parsed.policy.mode.unwrap_or_default(),
        allow: merge_policy_allow(parsed.policy.allow),
        limits: merge_policy_limits(parsed.policy.limits),
    };
    config.validation = ValidationCommands {
        fmt_command: parsed.validation.fmt_command,
        clippy_command: parsed.validation.clippy_command,
        workspace_test_command: parsed.validation.workspace_test_command,
        targeted_test_prefix: parsed.validation.targeted_test_prefix,
    };
    apply_agent_tools_section(&mut config.agent_tools, parsed.agent_tools, true);
    config.approval_rules = parsed
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
        .collect();
    config.extra_instruction_text = parsed.prompt.extra_instructions;
    config.extra_roots = parsed.extra_roots;
    config.mcp_servers = parsed
        .mcp_servers
        .into_iter()
        .filter_map(parse_mcp_server_entry)
        .collect();
    config
}

fn load_global_agent_tools_settings() -> AgentToolsSettings {
    let mut settings = AgentToolsSettings::default();
    let Some(path) = global_settings_path() else {
        return settings;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return settings;
    };
    match serde_json::from_str::<GlobalSettingsFile>(&raw) {
        Ok(parsed) => apply_agent_tools_section(&mut settings, parsed.agent_tools, false),
        Err(error) => {
            log::error!(
                "failed to parse global settings {}: {error}",
                path.display()
            );
        }
    }
    settings
}

fn global_settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".quorp/settings.json"))
}

fn apply_agent_tools_section(
    settings: &mut AgentToolsSettings,
    section: AgentToolsSection,
    project_override: bool,
) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    apply_external_tool_section(&mut settings.fd, section.tools.fd);
    apply_ast_grep_tool_section(
        &mut settings.ast_grep,
        section.tools.ast_grep,
        project_override,
    );
    apply_cargo_diagnostics_tool_section(
        &mut settings.cargo_diagnostics,
        section.tools.cargo_diagnostics,
    );
    apply_nextest_tool_section(&mut settings.nextest, section.tools.nextest);
    apply_external_tool_section(&mut settings.cargo_expand, section.tools.cargo_expand);
    apply_external_tool_section(&mut settings.rust_analyzer, section.tools.rust_analyzer);
    apply_external_tool_section(&mut settings.serena, section.tools.serena);
    apply_browser_tool_section(&mut settings.browser, section.tools.browser);
}

fn apply_external_tool_section(settings: &mut ExternalToolSettings, section: ToolSection) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    if let Some(command) = non_empty_string(section.command) {
        settings.command = command;
    }
    if section.max_runtime_seconds.is_some() {
        settings.max_runtime_seconds = section.max_runtime_seconds;
    }
    if section.max_output_bytes.is_some() {
        settings.max_output_bytes = section.max_output_bytes;
    }
}

fn apply_ast_grep_tool_section(
    settings: &mut AstGrepToolSettings,
    section: ToolSection,
    project_override: bool,
) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    if let Some(command) = non_empty_string(section.command) {
        settings.command = command;
    }
    if section.max_runtime_seconds.is_some() {
        settings.max_runtime_seconds = section.max_runtime_seconds;
    }
    if section.max_output_bytes.is_some() {
        settings.max_output_bytes = section.max_output_bytes;
    }
    if let Some(allow_rewrite_preview) = section.allow_rewrite_preview {
        settings.allow_rewrite_preview = if project_override {
            settings.allow_rewrite_preview && allow_rewrite_preview
        } else {
            allow_rewrite_preview
        };
    }
    if let Some(allow_apply) = section.allow_apply {
        settings.allow_apply = if project_override {
            settings.allow_apply && allow_apply
        } else {
            allow_apply
        };
    }
}

fn apply_cargo_diagnostics_tool_section(
    settings: &mut CargoDiagnosticsToolSettings,
    section: ToolSection,
) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    if let Some(command) = non_empty_string(section.check_command.or(section.command)) {
        settings.check_command = command;
    }
    if let Some(command) = section.clippy_command {
        settings.clippy_command = non_empty_string(Some(command));
    }
    if section.max_runtime_seconds.is_some() {
        settings.max_runtime_seconds = section.max_runtime_seconds;
    }
    if section.max_output_bytes.is_some() {
        settings.max_output_bytes = section.max_output_bytes;
    }
}

fn apply_nextest_tool_section(settings: &mut NextestToolSettings, section: ToolSection) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    if let Some(command) = non_empty_string(section.command) {
        settings.command = command;
    }
    if section.max_runtime_seconds.is_some() {
        settings.max_runtime_seconds = section.max_runtime_seconds;
    }
    if section.max_output_bytes.is_some() {
        settings.max_output_bytes = section.max_output_bytes;
    }
    if let Some(prefer_for_workspace_tests) = section.prefer_for_workspace_tests {
        settings.prefer_for_workspace_tests = prefer_for_workspace_tests;
    }
}

fn apply_browser_tool_section(settings: &mut BrowserToolSettings, section: BrowserToolSection) {
    if let Some(enabled) = section.enabled {
        settings.enabled = enabled;
    }
    if let Some(command) = non_empty_string(section.command) {
        settings.command = command;
    }
    if let Some(args) = section.args {
        settings.args = args;
    }
    if section.max_runtime_seconds.is_some() {
        settings.max_runtime_seconds = section.max_runtime_seconds;
    }
    if section.max_output_bytes.is_some() {
        settings.max_output_bytes = section.max_output_bytes;
    }
    if let Some(allow_remote_urls) = section.allow_remote_urls {
        settings.url_policy = if allow_remote_urls {
            BrowserUrlPolicy::AllowRemote
        } else {
            BrowserUrlPolicy::LocalOnly
        };
    }
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
        process_control: section
            .process_control
            .unwrap_or(default_allow.process_control),
        browser_control: section
            .browser_control
            .unwrap_or(default_allow.browser_control),
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
    let skill_catalog = discover_skill_catalog(project_root);
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

    AgentInstructionContext {
        project_root: project_root.to_path_buf(),
        documents,
        config,
        skill_catalog,
    }
}

pub fn render_instruction_context_for_prompt(context: &AgentInstructionContext) -> String {
    let mut rendered = String::new();
    if !context.documents.is_empty() {
        rendered.push_str("Follow these repo and user instructions in priority order:\n");
        for document in &context.documents {
            rendered.push_str(&format!(
                "\n--- {} ({}) ---\n{}\n",
                document.label,
                document.path.display(),
                document.content.trim()
            ));
        }
    }
    let skill_section = context.skill_catalog.render_prompt_section();
    if !skill_section.trim().is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&skill_section);
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
        && let Some(command) = workspace_test_command_for_config(config)
    {
        commands.push(command);
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

fn workspace_test_command_for_config(config: &AgentConfig) -> Option<String> {
    if config.agent_tools.enabled
        && config.agent_tools.nextest.enabled
        && config.agent_tools.nextest.prefer_for_workspace_tests
        && command_is_available(&config.agent_tools.nextest.command)
    {
        return Some(config.agent_tools.nextest.command.clone());
    }
    config.validation.workspace_test_command.clone()
}

pub fn command_is_available(command: &str) -> bool {
    let Some(program) = command_program(command) else {
        return false;
    };
    if program == "cargo"
        && let Some(subcommand_binary) = cargo_subcommand_binary(command)
    {
        return path_contains_executable(&subcommand_binary);
    }
    path_contains_executable(&program)
}

fn path_contains_executable(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file() && is_executable(&candidate)
        })
    })
}

fn cargo_subcommand_binary(command: &str) -> Option<String> {
    let parts = shlex::split(command)?;
    if parts.first().is_some_and(|program| program == "cargo")
        && let Some(subcommand) = parts.get(1)
        && !matches!(
            subcommand.as_str(),
            "build" | "check" | "clippy" | "fmt" | "run" | "test" | "tree"
        )
    {
        return Some(format!("cargo-{subcommand}"));
    }
    None
}

fn command_program(command: &str) -> Option<String> {
    shlex::split(command)
        .and_then(|parts| parts.into_iter().next())
        .map(|program| {
            Path::new(&program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(program.as_str())
                .to_string()
        })
        .filter(|program| !program.is_empty())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
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
        | AgentAction::PreviewEdit { path, .. }
        | AgentAction::LspDiagnostics { path }
        | AgentAction::LspDefinition { path, .. }
        | AgentAction::LspHover { path, .. }
        | AgentAction::LspDocumentSymbols { path }
        | AgentAction::LspCodeActions { path, .. }
        | AgentAction::LspRenamePreview { path, .. } => match rule.path_prefix.as_ref() {
            Some(prefix) => path.starts_with(prefix),
            None => false,
        },
        AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::LspReferences { .. }
        | AgentAction::LspWorkspaceSymbols { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::ProcessStart { .. }
        | AgentAction::ProcessRead { .. }
        | AgentAction::ProcessWrite { .. }
        | AgentAction::ProcessStop { .. }
        | AgentAction::ProcessWaitForPort { .. }
        | AgentAction::BrowserOpen { .. }
        | AgentAction::BrowserScreenshot { .. }
        | AgentAction::BrowserConsoleLogs { .. }
        | AgentAction::BrowserNetworkErrors { .. }
        | AgentAction::BrowserAccessibilitySnapshot { .. }
        | AgentAction::BrowserClose { .. } => false,
        AgentAction::McpListTools { server_name }
        | AgentAction::McpListResources { server_name, .. }
        | AgentAction::McpReadResource { server_name, .. }
        | AgentAction::McpListPrompts { server_name, .. }
        | AgentAction::McpGetPrompt { server_name, .. } => {
            rule.mcp_server_name
                .as_deref()
                .is_none_or(|expected| expected.eq_ignore_ascii_case(server_name))
                && match action {
                    AgentAction::McpListTools { .. } => rule
                        .mcp_tool_name
                        .as_deref()
                        .is_none_or(|expected| expected.eq_ignore_ascii_case("tools/list")),
                    AgentAction::McpListResources { .. } => rule
                        .mcp_tool_name
                        .as_deref()
                        .is_none_or(|expected| expected.eq_ignore_ascii_case("resources/list")),
                    AgentAction::McpReadResource { .. } => rule
                        .mcp_tool_name
                        .as_deref()
                        .is_none_or(|expected| expected.eq_ignore_ascii_case("resources/read")),
                    AgentAction::McpListPrompts { .. } => rule
                        .mcp_tool_name
                        .as_deref()
                        .is_none_or(|expected| expected.eq_ignore_ascii_case("prompts/list")),
                    AgentAction::McpGetPrompt { .. } => rule
                        .mcp_tool_name
                        .as_deref()
                        .is_none_or(|expected| expected.eq_ignore_ascii_case("prompts/get")),
                    _ => false,
                }
        }
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
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous.as_ref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

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

    #[test]
    fn agent_tools_global_settings_parse_and_missing_file_defaults() {
        let _env_lock = env_lock();
        let temp_home = tempfile::tempdir().expect("home");
        let _home_guard = EnvVarGuard::set("HOME", temp_home.path());

        let missing = load_agent_config(temp_home.path());
        assert!(!missing.agent_tools.enabled);

        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("config dir");
        std::fs::write(
            temp_home.path().join(".quorp/settings.json"),
            r#"{
              "agent_tools": {
                "enabled": true,
                "tools": {
                  "fd": {"enabled": true, "command": "fd"},
                  "ast_grep": {
                    "enabled": true,
                    "command": "ast-grep",
                    "allow_rewrite_preview": true,
                    "allow_apply": false
                  },
                  "browser": {
                    "enabled": true,
                    "command": "node",
                    "args": ["-e"],
                    "max_runtime_seconds": 45,
                    "max_output_bytes": 4096
                  },
                  "cargo_diagnostics": {
                    "enabled": true,
                    "check_command": "cargo check --message-format=json"
                  }
                }
              }
            }"#,
        )
        .expect("settings");

        let parsed = load_agent_config(temp_home.path());
        assert!(parsed.agent_tools.enabled);
        assert_eq!(parsed.agent_tools.fd.command, "fd");
        assert!(parsed.agent_tools.ast_grep.allow_rewrite_preview);
        assert!(!parsed.agent_tools.ast_grep.allow_apply);
        assert!(parsed.agent_tools.browser.enabled);
        assert_eq!(parsed.agent_tools.browser.command, "node");
        assert_eq!(parsed.agent_tools.browser.args, vec!["-e".to_string()]);
        assert_eq!(
            parsed.agent_tools.cargo_diagnostics.check_command,
            "cargo check --message-format=json"
        );
    }

    #[test]
    fn agent_tools_project_settings_can_narrow_global_tools() {
        let _env_lock = env_lock();
        let temp_home = tempfile::tempdir().expect("home");
        let project = tempfile::tempdir().expect("project");
        let _home_guard = EnvVarGuard::set("HOME", temp_home.path());
        std::fs::create_dir_all(temp_home.path().join(".quorp")).expect("home config");
        std::fs::write(
            temp_home.path().join(".quorp/settings.json"),
            r#"{
              "agent_tools": {
                "enabled": true,
                "tools": {
                "ast_grep": {
                    "enabled": true,
                    "command": "ast-grep",
                    "allow_rewrite_preview": true,
                    "allow_apply": true
                  },
                  "browser": {
                    "enabled": false,
                    "command": "node"
                  }
                }
              }
            }"#,
        )
        .expect("settings");
        std::fs::create_dir_all(project.path().join(".quorp")).expect("project config");
        std::fs::write(
            project.path().join(".quorp/agent.toml"),
            r#"
[agent_tools]
enabled = true

[agent_tools.tools.ast_grep]
allow_rewrite_preview = false
allow_apply = true

[agent_tools.tools.browser]
enabled = true
command = "node"
args = ["-e"]
"#,
        )
        .expect("agent toml");

        let config = load_agent_config(project.path());
        assert!(config.agent_tools.enabled);
        assert!(!config.agent_tools.ast_grep.allow_rewrite_preview);
        assert!(config.agent_tools.ast_grep.allow_apply);
        assert!(config.agent_tools.browser.enabled);

        std::fs::write(
            project.path().join(".quorp/agent.toml"),
            r#"
[agent_tools]
enabled = false

[agent_tools.tools.browser]
enabled = false
"#,
        )
        .expect("agent toml");
        let narrowed = load_agent_config(project.path());
        assert!(!narrowed.agent_tools.enabled);
        assert!(!narrowed.agent_tools.browser.enabled);
    }

    #[test]
    fn agent_tools_nextest_command_requires_cargo_subcommand_binary() {
        let _env_lock = env_lock();
        let temp_path = tempfile::tempdir().expect("path");
        let cargo = temp_path.path().join("cargo");
        std::fs::write(&cargo, "#!/bin/sh\nexit 0\n").expect("cargo");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let _path_guard = EnvVarGuard::set("PATH", temp_path.path());
        assert!(command_is_available("cargo check --message-format=json"));
        assert!(!command_is_available("cargo nextest run"));
        let cargo_nextest = temp_path.path().join("cargo-nextest");
        std::fs::write(&cargo_nextest, "#!/bin/sh\nexit 0\n").expect("cargo-nextest");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cargo_nextest, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        assert!(command_is_available("cargo nextest run"));
    }
}
