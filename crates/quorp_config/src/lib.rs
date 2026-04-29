//! JSON settings loader for user and project QUORP configuration.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use quorp_core::{PermissionMode, ProviderProfile, SandboxMode, SandboxRuntimeSettings};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const SETTINGS_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub provider: ProviderProfile,
    pub sandbox: SandboxSettings,
    pub permissions: PermissionSettings,
    pub hooks: HookSettings,
    pub allowed_commands: Vec<String>,
    pub proof_lanes: BTreeMap<String, Vec<String>>,
    pub trust: TrustSettings,
    pub mcp: McpSettings,
    pub tools: ToolPolicySettings,
    pub proof: ProofSettings,
    pub context: ContextSettings,
    pub memory: MemorySettings,
    pub rules: RuleSettings,
    pub tui: TuiSettings,
    pub evals: EvalSettings,
    pub managed_policy: ManagedPolicySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            provider: ProviderProfile::nvidia_qwen(),
            sandbox: SandboxSettings::default(),
            permissions: PermissionSettings::default(),
            hooks: HookSettings::default(),
            allowed_commands: Vec::new(),
            proof_lanes: BTreeMap::new(),
            trust: TrustSettings::default(),
            mcp: McpSettings::default(),
            tools: ToolPolicySettings::default(),
            proof: ProofSettings::default(),
            context: ContextSettings::default(),
            memory: MemorySettings::default(),
            rules: RuleSettings::default(),
            tui: TuiSettings::default(),
            evals: EvalSettings::default(),
            managed_policy: ManagedPolicySettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSettings {
    pub mode: SandboxMode,
    pub keep_last_sandbox: bool,
    pub runtime: SandboxRuntimeSettings,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: SandboxMode::TmpCopy,
            keep_last_sandbox: false,
            runtime: SandboxRuntimeSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionSettings {
    pub mode: PermissionMode,
    pub require_clean_git_for_full_permissions: bool,
    pub allow_network: bool,
    pub allow_mcp: bool,
    pub allow_browser: bool,
    pub allow_process_control: bool,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Ask,
            require_clean_git_for_full_permissions: true,
            allow_network: false,
            allow_mcp: false,
            allow_browser: false,
            allow_process_control: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HookSettings {
    pub before_tool: Vec<String>,
    pub after_tool: Vec<String>,
    pub stop: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TrustSettings {
    pub project_id: Option<String>,
    pub trusted_projects: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct McpSettings {
    pub enabled: bool,
    pub allowed_servers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPolicySettings {
    pub browser: bool,
    pub mcp: bool,
    pub process_control: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ProofSettings {
    pub default_lane: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextSettings {
    pub prefer_indexed: bool,
    pub max_lexical_hits: u32,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            prefer_indexed: true,
            max_lexical_hits: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySettings {
    pub enabled: bool,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RuleSettings {
    pub enabled: bool,
}

impl Default for RuleSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TuiSettings {
    pub preferred_mode: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EvalSettings {
    pub artifact_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ManagedPolicySettings {
    pub require_trust_for_project_elevation: bool,
    pub full_auto_requires_sandbox: bool,
    pub full_auto_requires_network_off: bool,
}

impl Default for ManagedPolicySettings {
    fn default() -> Self {
        Self {
            require_trust_for_project_elevation: true,
            full_auto_requires_sandbox: true,
            full_auto_requires_network_off: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSources {
    pub user_path: PathBuf,
    pub project_path: PathBuf,
    pub legacy_agent_toml_path: PathBuf,
    pub loaded_user: bool,
    pub loaded_project: bool,
    pub loaded_legacy_agent_toml: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveTrust {
    pub project_id: String,
    pub trusted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub sources: SettingsSources,
    pub trust: EffectiveTrust,
    pub warnings: Vec<String>,
}

pub fn user_settings_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve home dir"))?;
    Ok(home.join(".quorp").join("settings.json"))
}

pub fn project_settings_path(project_root: &Path) -> PathBuf {
    project_root.join(".quorp").join("settings.json")
}

pub fn legacy_agent_toml_path(project_root: &Path) -> PathBuf {
    project_root.join(".quorp").join("agent.toml")
}

pub fn load_settings(project_root: &Path) -> anyhow::Result<LoadedSettings> {
    let user_path = user_settings_path()?;
    let project_path = project_settings_path(project_root);
    let legacy_path = legacy_agent_toml_path(project_root);
    load_settings_from_paths(project_root, &user_path, &project_path, &legacy_path)
}

pub fn load_settings_from_paths(
    project_root: &Path,
    user_path: &Path,
    project_path: &Path,
    legacy_agent_toml_path: &Path,
) -> anyhow::Result<LoadedSettings> {
    let user = read_settings_if_exists(user_path)?;
    let project = read_settings_if_exists(project_path)?;

    let project_id = resolve_project_id(project_root, user.as_ref(), project.as_ref());
    let trusted = user
        .as_ref()
        .map(|settings| is_project_trusted(&settings.trust, &project_id, project_root))
        .unwrap_or(false);

    let mut settings = user.clone().unwrap_or_default();
    if let Some(project_settings) = project.as_ref() {
        settings = merge_settings(settings, project_settings.clone(), trusted);
    }
    settings.version = settings.version.max(SETTINGS_VERSION);

    let mut warnings = Vec::new();
    if legacy_agent_toml_path.exists() {
        warnings.push(format!(
            "legacy compatibility file present: {} (settings.json is canonical)",
            legacy_agent_toml_path.display()
        ));
    }
    if settings.managed_policy.full_auto_requires_sandbox
        && settings.permissions.mode == PermissionMode::FullAuto
        && settings.sandbox.mode == SandboxMode::Host
    {
        warnings.push(
            "full-auto requires a sandboxed workspace; effective permission mode was downgraded to ask"
                .to_string(),
        );
        settings.permissions.mode = PermissionMode::Ask;
    }
    if settings.managed_policy.full_auto_requires_network_off
        && settings.permissions.mode == PermissionMode::FullAuto
        && settings.permissions.allow_network
    {
        warnings.push(
            "full-auto requires network-off by default; effective permission mode was downgraded to ask"
                .to_string(),
        );
        settings.permissions.mode = PermissionMode::Ask;
    }

    Ok(LoadedSettings {
        settings,
        sources: SettingsSources {
            user_path: user_path.to_path_buf(),
            project_path: project_path.to_path_buf(),
            legacy_agent_toml_path: legacy_agent_toml_path.to_path_buf(),
            loaded_user: user_path.exists(),
            loaded_project: project_path.exists(),
            loaded_legacy_agent_toml: legacy_agent_toml_path.exists(),
        },
        trust: EffectiveTrust {
            project_id,
            trusted,
        },
        warnings,
    })
}

pub fn settings_schema_json() -> anyhow::Result<String> {
    let schema = schemars::schema_for!(Settings);
    serde_json::to_string_pretty(&schema).context("failed to render settings schema")
}

fn read_settings_if_exists(path: &Path) -> anyhow::Result<Option<Settings>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn merge_settings(base: Settings, project: Settings, trusted: bool) -> Settings {
    let managed_policy = merge_managed_policy(base.managed_policy, project.managed_policy);
    Settings {
        version: base.version.max(project.version).max(SETTINGS_VERSION),
        provider: project.provider,
        sandbox: merge_sandbox(base.sandbox, project.sandbox, trusted, &managed_policy),
        permissions: merge_permissions(
            base.permissions,
            project.permissions,
            trusted,
            &managed_policy,
        ),
        hooks: project.hooks,
        allowed_commands: merge_string_list(
            base.allowed_commands,
            project.allowed_commands,
            trusted,
        ),
        proof_lanes: if project.proof_lanes.is_empty() {
            base.proof_lanes
        } else {
            project.proof_lanes
        },
        trust: merge_trust(base.trust, project.trust),
        mcp: merge_mcp(base.mcp, project.mcp, trusted),
        tools: merge_tool_policy(base.tools, project.tools, trusted),
        proof: if project.proof.default_lane.is_some() {
            project.proof
        } else {
            base.proof
        },
        context: merge_context(base.context, project.context),
        memory: merge_simple_bool(base.memory, project.memory, trusted),
        rules: merge_simple_bool(base.rules, project.rules, trusted),
        tui: if project.tui.preferred_mode.is_some() {
            project.tui
        } else {
            base.tui
        },
        evals: if project.evals.artifact_root.is_some() {
            project.evals
        } else {
            base.evals
        },
        managed_policy,
    }
}

fn merge_sandbox(
    base: SandboxSettings,
    project: SandboxSettings,
    trusted: bool,
    managed_policy: &ManagedPolicySettings,
) -> SandboxSettings {
    let mode = if trusted || !managed_policy.require_trust_for_project_elevation {
        project.mode
    } else {
        narrower_sandbox_mode(base.mode, project.mode)
    };
    let runtime = if mode == project.mode {
        project.runtime
    } else {
        base.runtime
    };
    SandboxSettings {
        mode,
        keep_last_sandbox: base.keep_last_sandbox && project.keep_last_sandbox,
        runtime,
    }
}

fn merge_permissions(
    base: PermissionSettings,
    project: PermissionSettings,
    trusted: bool,
    managed_policy: &ManagedPolicySettings,
) -> PermissionSettings {
    PermissionSettings {
        mode: if trusted || !managed_policy.require_trust_for_project_elevation {
            project.mode
        } else {
            narrower_permission_mode(base.mode, project.mode)
        },
        require_clean_git_for_full_permissions: base.require_clean_git_for_full_permissions
            || project.require_clean_git_for_full_permissions,
        allow_network: merge_privileged_bool(
            base.allow_network,
            project.allow_network,
            trusted,
            managed_policy.require_trust_for_project_elevation,
        ),
        allow_mcp: merge_privileged_bool(
            base.allow_mcp,
            project.allow_mcp,
            trusted,
            managed_policy.require_trust_for_project_elevation,
        ),
        allow_browser: merge_privileged_bool(
            base.allow_browser,
            project.allow_browser,
            trusted,
            managed_policy.require_trust_for_project_elevation,
        ),
        allow_process_control: merge_privileged_bool(
            base.allow_process_control,
            project.allow_process_control,
            trusted,
            managed_policy.require_trust_for_project_elevation,
        ),
    }
}

fn merge_trust(base: TrustSettings, project: TrustSettings) -> TrustSettings {
    TrustSettings {
        project_id: project.project_id.or(base.project_id),
        trusted_projects: if project.trusted_projects.is_empty() {
            base.trusted_projects
        } else {
            project.trusted_projects
        },
    }
}

fn merge_mcp(base: McpSettings, project: McpSettings, trusted: bool) -> McpSettings {
    McpSettings {
        enabled: merge_privileged_bool(base.enabled, project.enabled, trusted, true),
        allowed_servers: merge_string_list(base.allowed_servers, project.allowed_servers, trusted),
    }
}

fn merge_tool_policy(
    base: ToolPolicySettings,
    project: ToolPolicySettings,
    trusted: bool,
) -> ToolPolicySettings {
    ToolPolicySettings {
        browser: merge_privileged_bool(base.browser, project.browser, trusted, true),
        mcp: merge_privileged_bool(base.mcp, project.mcp, trusted, true),
        process_control: merge_privileged_bool(
            base.process_control,
            project.process_control,
            trusted,
            true,
        ),
    }
}

fn merge_context(base: ContextSettings, project: ContextSettings) -> ContextSettings {
    ContextSettings {
        prefer_indexed: base.prefer_indexed && project.prefer_indexed,
        max_lexical_hits: if project.max_lexical_hits == ContextSettings::default().max_lexical_hits
        {
            base.max_lexical_hits
        } else {
            project.max_lexical_hits
        },
    }
}

fn merge_simple_bool<T>(base: T, project: T, trusted: bool) -> T
where
    T: SimpleEnabled,
{
    if trusted {
        project
    } else {
        T::new(base.enabled() && project.enabled())
    }
}

fn merge_managed_policy(
    base: ManagedPolicySettings,
    project: ManagedPolicySettings,
) -> ManagedPolicySettings {
    ManagedPolicySettings {
        require_trust_for_project_elevation: base.require_trust_for_project_elevation
            || project.require_trust_for_project_elevation,
        full_auto_requires_sandbox: base.full_auto_requires_sandbox
            || project.full_auto_requires_sandbox,
        full_auto_requires_network_off: base.full_auto_requires_network_off
            || project.full_auto_requires_network_off,
    }
}

fn merge_privileged_bool(
    base: bool,
    project: bool,
    trusted: bool,
    require_trust_for_elevation: bool,
) -> bool {
    if trusted || !require_trust_for_elevation {
        project
    } else {
        base && project
    }
}

fn merge_string_list(base: Vec<String>, project: Vec<String>, trusted: bool) -> Vec<String> {
    if project.is_empty() {
        return base;
    }
    if trusted {
        return project;
    }
    if base.is_empty() {
        return Vec::new();
    }
    project
        .into_iter()
        .filter(|value| base.iter().any(|existing| existing == value))
        .collect()
}

fn resolve_project_id(
    project_root: &Path,
    user: Option<&Settings>,
    project: Option<&Settings>,
) -> String {
    project
        .and_then(|settings| settings.trust.project_id.clone())
        .or_else(|| user.and_then(|settings| settings.trust.project_id.clone()))
        .unwrap_or_else(|| canonical_project_key(project_root))
}

fn is_project_trusted(trust: &TrustSettings, project_id: &str, project_root: &Path) -> bool {
    let canonical_path = canonical_project_key(project_root);
    trust
        .trusted_projects
        .iter()
        .any(|candidate| candidate == project_id || candidate == &canonical_path)
}

fn canonical_project_key(project_root: &Path) -> String {
    fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string()
}

fn narrower_permission_mode(base: PermissionMode, project: PermissionMode) -> PermissionMode {
    if permission_mode_rank(project) <= permission_mode_rank(base) {
        project
    } else {
        base
    }
}

fn narrower_sandbox_mode(base: SandboxMode, project: SandboxMode) -> SandboxMode {
    if sandbox_mode_rank(project) <= sandbox_mode_rank(base) {
        project
    } else {
        base
    }
}

fn permission_mode_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Ask => 0,
        PermissionMode::FullAuto => 1,
        PermissionMode::FullPermissions => 2,
    }
}

fn sandbox_mode_rank(mode: SandboxMode) -> u8 {
    match mode {
        SandboxMode::TmpCopy => 0,
        SandboxMode::Host => 1,
    }
}

trait SimpleEnabled {
    fn enabled(&self) -> bool;
    fn new(enabled: bool) -> Self;
}

impl SimpleEnabled for MemorySettings {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl SimpleEnabled for RuleSettings {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}
#[cfg(test)]
#[path = "../../../testing/quorp_config/lib/tests.rs"]
mod tests;
