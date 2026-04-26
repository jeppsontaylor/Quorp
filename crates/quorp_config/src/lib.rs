//! JSON settings loader for user and project QUORP configuration.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use quorp_core::{PermissionMode, ProviderProfile, SandboxMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub provider: ProviderProfile,
    pub sandbox: SandboxSettings,
    pub permissions: PermissionSettings,
    pub hooks: HookSettings,
    pub allowed_commands: Vec<String>,
    pub proof_lanes: BTreeMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: ProviderProfile::nvidia_qwen(),
            sandbox: SandboxSettings::default(),
            permissions: PermissionSettings::default(),
            hooks: HookSettings::default(),
            allowed_commands: Vec::new(),
            proof_lanes: BTreeMap::new(),
        }
    }
}

impl Settings {
    fn merge(self, override_settings: Self) -> Self {
        Self {
            provider: override_settings.provider,
            sandbox: override_settings.sandbox,
            permissions: override_settings.permissions,
            hooks: override_settings.hooks,
            allowed_commands: if override_settings.allowed_commands.is_empty() {
                self.allowed_commands
            } else {
                override_settings.allowed_commands
            },
            proof_lanes: if override_settings.proof_lanes.is_empty() {
                self.proof_lanes
            } else {
                override_settings.proof_lanes
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSettings {
    pub mode: SandboxMode,
    pub keep_last_sandbox: bool,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: SandboxMode::TmpCopy,
            keep_last_sandbox: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionSettings {
    pub mode: PermissionMode,
    pub require_clean_git_for_full_permissions: bool,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Ask,
            require_clean_git_for_full_permissions: true,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSources {
    pub user_path: PathBuf,
    pub project_path: PathBuf,
    pub loaded_user: bool,
    pub loaded_project: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSettings {
    pub settings: Settings,
    pub sources: SettingsSources,
}

pub fn user_settings_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve home dir"))?;
    Ok(home.join(".quorp").join("settings.json"))
}

pub fn project_settings_path(project_root: &Path) -> PathBuf {
    project_root.join(".quorp").join("settings.json")
}

pub fn load_settings(project_root: &Path) -> anyhow::Result<LoadedSettings> {
    let user_path = user_settings_path()?;
    let project_path = project_settings_path(project_root);
    load_settings_from_paths(&user_path, &project_path)
}

pub fn load_settings_from_paths(
    user_path: &Path,
    project_path: &Path,
) -> anyhow::Result<LoadedSettings> {
    let user = read_settings_if_exists(user_path)?;
    let project = read_settings_if_exists(project_path)?;
    let settings = match (user, project) {
        (Some(user), Some(project)) => user.merge(project),
        (Some(user), None) => user,
        (None, Some(project)) => Settings::default().merge(project),
        (None, None) => Settings::default(),
    };
    Ok(LoadedSettings {
        settings,
        sources: SettingsSources {
            user_path: user_path.to_path_buf(),
            project_path: project_path.to_path_buf(),
            loaded_user: user_path.exists(),
            loaded_project: project_path.exists(),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_settings_override_user_settings() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let user_path = temp_dir.path().join("user.json");
        let project_path = temp_dir.path().join("project.json");
        fs::write(
            &user_path,
            r#"{
              "provider": {
                "name": "user",
                "base_url": "https://user.example/v1",
                "model": "user-model",
                "api_key_env": "USER_KEY"
              },
              "sandbox": { "mode": "host", "keep_last_sandbox": true },
              "permissions": { "mode": "ask", "require_clean_git_for_full_permissions": true },
              "hooks": { "before_tool": ["rtk"], "after_tool": [], "stop": [] },
              "allowed_commands": ["cargo check"]
            }"#,
        )
        .expect("write user");
        fs::write(
            &project_path,
            r#"{
              "provider": {
                "name": "project",
                "base_url": "https://project.example/v1",
                "model": "project-model",
                "api_key_env": "PROJECT_KEY"
              },
              "sandbox": { "mode": "tmp-copy", "keep_last_sandbox": false },
              "permissions": { "mode": "full-auto", "require_clean_git_for_full_permissions": true },
              "hooks": { "before_tool": [], "after_tool": ["just fast"], "stop": [] }
            }"#,
        )
        .expect("write project");

        let loaded = load_settings_from_paths(&user_path, &project_path).expect("settings");

        assert_eq!(loaded.settings.provider.name, "project");
        assert_eq!(loaded.settings.sandbox.mode, SandboxMode::TmpCopy);
        assert_eq!(loaded.settings.permissions.mode, PermissionMode::FullAuto);
        assert_eq!(loaded.settings.allowed_commands, ["cargo check"]);
        assert_eq!(loaded.settings.hooks.after_tool, ["just fast"]);
    }
}
