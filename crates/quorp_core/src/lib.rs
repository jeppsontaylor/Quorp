//! Small shared types for the agent-first QUORP core.

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_NVIDIA_MODEL: &str = "qwen/qwen3-coder-480b-a35b-instruct";

pub mod skills;
pub mod validation_planner;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    Plan,
    #[default]
    Act,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    Ask,
    FullAuto,
    FullPermissions,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    Host,
    #[default]
    TmpCopy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxRuntimeProfile {
    #[default]
    Local,
    Container,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ContainerEnginePreference {
    #[default]
    Auto,
    Docker,
    Podman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContainerRuntimeSettings {
    pub engine: ContainerEnginePreference,
    pub image: String,
}

impl Default for ContainerRuntimeSettings {
    fn default() -> Self {
        Self {
            engine: ContainerEnginePreference::Auto,
            image: default_container_image(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxRuntimeSettings {
    pub profile: SandboxRuntimeProfile,
    pub container: ContainerRuntimeSettings,
}

impl Default for SandboxRuntimeSettings {
    fn default() -> Self {
        Self {
            profile: SandboxRuntimeProfile::Local,
            container: ContainerRuntimeSettings::default(),
        }
    }
}

fn default_container_image() -> String {
    "docker.io/library/alpine:3.20".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderProfile {
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl ProviderProfile {
    pub fn nvidia_qwen() -> Self {
        Self {
            name: "nvidia-qwen3-coder".to_string(),
            base_url: DEFAULT_NVIDIA_BASE_URL.to_string(),
            model: DEFAULT_NVIDIA_MODEL.to_string(),
            api_key_env: "NVIDIA_API_KEY".to_string(),
        }
    }
}

fn default_api_key_env() -> String {
    "NVIDIA_API_KEY".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ValidationRecord {
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: i32,
    pub raw_log_path: Option<PathBuf>,
    pub raw_log_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RawArtifact {
    pub path: PathBuf,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProofReceipt {
    pub receipt_version: u32,
    pub run_id: String,
    pub source_commit: Option<String>,
    pub source_hash: Option<String>,
    pub sandbox_path: Option<PathBuf>,
    pub changed_files: Vec<PathBuf>,
    pub validation: Vec<ValidationRecord>,
    pub evaluator_result: Option<String>,
    pub raw_artifacts: BTreeMap<String, RawArtifact>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub usage: BTreeMap<String, u64>,
    pub residual_risks: Vec<String>,
}

impl ProofReceipt {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            receipt_version: 1,
            run_id: run_id.into(),
            source_commit: None,
            source_hash: None,
            sandbox_path: None,
            changed_files: Vec::new(),
            validation: Vec::new(),
            evaluator_result: None,
            raw_artifacts: BTreeMap::new(),
            provider: None,
            model: None,
            usage: BTreeMap::new(),
            residual_risks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuorpEvent {
    SessionStarted {
        workspace: PathBuf,
        sandbox: SandboxMode,
        provider: ProviderProfile,
    },
    ModeChanged {
        run_mode: RunMode,
        permission_mode: PermissionMode,
        sandbox: SandboxMode,
    },
    ToolStarted {
        name: String,
        summary: String,
    },
    ToolFinished {
        name: String,
        success: bool,
        raw_log_path: Option<PathBuf>,
    },
    ProofReceiptWritten {
        path: PathBuf,
    },
}
