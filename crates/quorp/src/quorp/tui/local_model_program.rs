use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelRole {
    Coding,
    Reasoning,
}

impl LocalModelRole {
    pub fn as_config_value(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Reasoning => "reasoning",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "coding" => Some(Self::Coding),
            "reasoning" => Some(Self::Reasoning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarmStartPolicy {
    SerializedSharedResidency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModelProgram {
    pub registry_id: &'static str,
    pub role: LocalModelRole,
    pub has_think_tokens: bool,
    pub manifest_path: &'static str,
    pub download_script_path: &'static str,
    pub verify_script_path: &'static str,
    pub preferred_compaction_policy: &'static str,
    pub warm_start_policy: WarmStartPolicy,
    pub default_benchmark_specs: &'static [&'static str],
}

const CODER_BENCHMARK_SPECS: &[&str] = &[
    "heavy/specs/qwen3-coder-30b-a3b-json.json",
    "heavy/specs/second-turn-cache-coder.json",
];

const REASONING_BENCHMARK_SPECS: &[&str] = &[
    "heavy/specs/qwen35-35b-a3b-first.json",
    "heavy/specs/second-turn-cache.json",
];

const LOCAL_MODEL_PROGRAMS: &[LocalModelProgram] = &[
    LocalModelProgram {
        registry_id: "qwen35-27b",
        role: LocalModelRole::Coding,
        has_think_tokens: true,
        manifest_path: "heavy/qwen35-27b/manifest.toml",
        download_script_path: "heavy/qwen35-27b/download.sh",
        verify_script_path: "heavy/qwen35-27b/verify.sh",
        preferred_compaction_policy: "last6-ledger768",
        warm_start_policy: WarmStartPolicy::SerializedSharedResidency,
        default_benchmark_specs: CODER_BENCHMARK_SPECS,
    },
    LocalModelProgram {
        registry_id: "qwen3-coder-30b-a3b",
        role: LocalModelRole::Coding,
        has_think_tokens: false,
        manifest_path: "heavy/qwen3-coder-30b-a3b/manifest.toml",
        download_script_path: "heavy/qwen3-coder-30b-a3b/download.sh",
        verify_script_path: "heavy/qwen3-coder-30b-a3b/verify.sh",
        preferred_compaction_policy: "last6-ledger768",
        warm_start_policy: WarmStartPolicy::SerializedSharedResidency,
        default_benchmark_specs: CODER_BENCHMARK_SPECS,
    },
    LocalModelProgram {
        registry_id: "qwen36-27b",
        role: LocalModelRole::Coding,
        has_think_tokens: true,
        manifest_path: "heavy/qwen36-27b/manifest.toml",
        download_script_path: "heavy/qwen36-27b/download.sh",
        verify_script_path: "heavy/qwen36-27b/verify.sh",
        preferred_compaction_policy: "last6-ledger768",
        warm_start_policy: WarmStartPolicy::SerializedSharedResidency,
        default_benchmark_specs: CODER_BENCHMARK_SPECS,
    },
    LocalModelProgram {
        registry_id: "qwen35-35b-a3b",
        role: LocalModelRole::Reasoning,
        has_think_tokens: true,
        manifest_path: "heavy/qwen35-35b-a3b/manifest.toml",
        download_script_path: "heavy/qwen35-35b-a3b/download.sh",
        verify_script_path: "heavy/qwen35-35b-a3b/verify.sh",
        preferred_compaction_policy: "last6-ledger768",
        warm_start_policy: WarmStartPolicy::SerializedSharedResidency,
        default_benchmark_specs: REASONING_BENCHMARK_SPECS,
    },
    LocalModelProgram {
        registry_id: "qwen35-122b-a10b",
        role: LocalModelRole::Reasoning,
        has_think_tokens: true,
        manifest_path: "heavy/qwen35-122b-a10b/manifest.toml",
        download_script_path: "heavy/qwen35-122b-a10b/download.sh",
        verify_script_path: "heavy/qwen35-122b-a10b/verify.sh",
        preferred_compaction_policy: "last6-ledger768",
        warm_start_policy: WarmStartPolicy::SerializedSharedResidency,
        default_benchmark_specs: REASONING_BENCHMARK_SPECS,
    },
];

#[cfg_attr(not(test), allow(dead_code))]
pub fn local_model_programs() -> &'static [LocalModelProgram] {
    LOCAL_MODEL_PROGRAMS
}

pub fn local_model_program(model_id: &str) -> Option<&'static LocalModelProgram> {
    let lowered = model_id.trim().to_ascii_lowercase();
    LOCAL_MODEL_PROGRAMS.iter().find(|program| {
        program.registry_id.eq_ignore_ascii_case(&lowered)
            || format!("ssd_moe/{}", program.registry_id).eq_ignore_ascii_case(&lowered)
    })
}

pub fn preferred_local_registry_id_for_role(role: LocalModelRole) -> &'static str {
    LOCAL_MODEL_PROGRAMS
        .iter()
        .find(|program| program.role == role)
        .map(|program| program.registry_id)
        .unwrap_or("qwen3-coder-30b-a3b")
}
