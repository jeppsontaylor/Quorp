//! Broker-backed SSD-MOE model selection and persisted broker model id.

use crate::quorp::executor::InteractiveProviderKind;
use crate::quorp::tui::local_model_program::{
    LocalModelRole, WarmStartPolicy, local_model_program, preferred_local_registry_id_for_role,
};
use serde::{Deserialize, Serialize};
use ssd_moe_client::{
    ClientConfig, fetch_catalog_models_blocking, fetch_catalog_recommendations_blocking,
};
use ssd_moe_contract::ModelBackendKind;
#[cfg(test)]
use ssd_moe_launch::catalog_recommendations;
use std::io;
use std::sync::{LazyLock, Mutex, MutexGuard};

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static TEST_MODEL_CONFIG_ROOT: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_model_config_root(path: Option<std::path::PathBuf>) {
    TEST_MODEL_CONFIG_ROOT.with(|root| {
        *root.borrow_mut() = path;
    });
}

#[cfg(test)]
pub(crate) struct TestModelConfigGuard {
    previous_root: Option<std::path::PathBuf>,
    _tempdir: Option<tempfile::TempDir>,
}

#[cfg(test)]
impl Drop for TestModelConfigGuard {
    fn drop(&mut self) {
        set_test_model_config_root(self.previous_root.take());
    }
}

#[cfg(test)]
pub(crate) fn push_test_model_config_root(path: std::path::PathBuf) -> TestModelConfigGuard {
    let previous_root = TEST_MODEL_CONFIG_ROOT.with(|root| root.borrow().clone());
    set_test_model_config_root(Some(path));
    TestModelConfigGuard {
        previous_root,
        _tempdir: None,
    }
}

#[cfg(test)]
pub(crate) fn isolated_test_model_config_guard() -> TestModelConfigGuard {
    let previous_root = TEST_MODEL_CONFIG_ROOT.with(|root| root.borrow().clone());
    if previous_root.is_some() {
        return TestModelConfigGuard {
            previous_root,
            _tempdir: None,
        };
    }
    let tempdir = tempfile::tempdir().expect("temp model config dir");
    set_test_model_config_root(Some(tempdir.path().to_path_buf()));
    TestModelConfigGuard {
        previous_root,
        _tempdir: Some(tempdir),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LocalRuntimeBackend {
    FlashMoePrepared {
        model_dir_name: &'static str,
        k_experts: u32,
        think_budget: u32,
        kv_seq: u32,
    },
    TurboHeroOracle {
        profile_key: &'static str,
        mode: &'static str,
        served_model_id: &'static str,
        default_port: u16,
        max_ctx: u32,
        max_output_tokens: u32,
        trust_remote_code: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub hf_repo: &'static str,
    pub estimated_disk_gb: f32,
    pub estimated_ram_gb: f32,
    pub has_think_tokens: bool,
    pub description: &'static str,
    pub runtime_backend: LocalRuntimeBackend,
}

const OLLAMA_RECOMMENDED_MODELS: &[&str] = &[
    "devstral-small-2",
    "gpt-oss:20b",
    "qwen3.5:9b-q8_0",
    "gemma4:e4b",
    "qwen2.5-coder:32b",
];
pub(crate) const NVIDIA_KIMI_K2_5_MODEL_ID: &str = "moonshotai/kimi-k2.5";
pub(crate) const NVIDIA_QWEN3_CODER_480B_MODEL_ID: &str = "qwen/qwen3-coder-480b-a35b-instruct";
const NVIDIA_RECOMMENDED_MODELS: &[&str] =
    &[NVIDIA_KIMI_K2_5_MODEL_ID, NVIDIA_QWEN3_CODER_480B_MODEL_ID];
const VERIFIED_PRIMARY_LOCAL_CODING_MODEL_ID: &str = "ssd_moe/qwen35-27b";
const AUTOSELECT_QUARANTINED_LOCAL_MODEL_IDS: &[&str] = &["ssd_moe/qwen3-coder-30b-a3b"];
const MANAGED_PLANNER_DEFAULT_MODEL_ID: &str = "ollama/devstral-small-2";
const MANAGED_BALANCED_PLANNER_MODEL_ID: &str = "ollama/gpt-oss:20b";
const MANAGED_LIGHTWEIGHT_PLANNER_MODEL_ID: &str = "ollama/qwen3.5:9b-q8_0";
const MANAGED_COMPACT_PLANNER_MODEL_ID: &str = "ollama/gemma4:e4b";
const MANAGED_CODING_MODEL_ID: &str = "ssd_moe/qwen3-coder-30b-a3b";
const MANAGED_REASONING_MODEL_ID: &str = "ssd_moe/qwen35-35b-a3b";
const MANAGED_MANUAL_ONLY_MODEL_ID: &str = "ssd_moe/qwen35-122b-a10b";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedTaskIntent {
    Planning,
    Coding,
    Reasoning,
}

fn leak_string(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn client_config() -> ClientConfig {
    ClientConfig::default()
}

#[derive(Debug, Clone)]
struct CatalogCache {
    entries: Vec<ssd_moe_contract::ModelCatalogEntry>,
    recommended_coding_model_id: Option<String>,
}

impl CatalogCache {
    fn new() -> Self {
        Self {
            entries: built_in_catalog_entries(),
            recommended_coding_model_id: None,
        }
    }
}

static CATALOG_CACHE: LazyLock<Mutex<CatalogCache>> =
    LazyLock::new(|| Mutex::new(CatalogCache::new()));

fn catalog_cache() -> MutexGuard<'static, CatalogCache> {
    CATALOG_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn merge_catalog_entries(
    entries: &mut Vec<ssd_moe_contract::ModelCatalogEntry>,
    incoming: impl IntoIterator<Item = ssd_moe_contract::ModelCatalogEntry>,
) {
    for entry in incoming {
        if !entries.iter().any(|existing| existing.id == entry.id) {
            entries.push(entry);
        }
    }
}

fn built_in_catalog_entries() -> Vec<ssd_moe_contract::ModelCatalogEntry> {
    vec![
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            aliases: vec!["qwen3-coder-30b-a3b".to_string()],
            display_name: "Qwen 3 Coder 30B".to_string(),
            hf_repo: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit".to_string(),
            backend_kind: ModelBackendKind::MlxOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "qwen3-coder-30b-a3b".to_string(),
            estimated_disk_gb: 17.2,
            estimated_active_ram_gb: 17.2,
            context_len: 16384,
            tool_use_rating: 8,
            rust_coding_rating: 9,
            turboquant_priority: 10,
            apple_m_series_tier: "m4_max_safe".to_string(),
            default_role: "quarantined_coder".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: true,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "qwen3_coder_30b_a3b".to_string(),
                mode: "baseline".to_string(),
                default_port: 57025,
                max_ctx: 16384,
                max_output_tokens: 2560,
                trust_remote_code: false,
            }),
        },
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/qwen35-27b".to_string(),
            aliases: vec!["qwen35-27b".to_string(), "qwen3.5-27b".to_string()],
            display_name: "Qwen 3.5 27B".to_string(),
            hf_repo: "mlx-community/Qwen3.5-27B-4bit".to_string(),
            backend_kind: ModelBackendKind::MlxOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "qwen35-27b".to_string(),
            estimated_disk_gb: 16.1,
            estimated_active_ram_gb: 17.0,
            context_len: 32768,
            tool_use_rating: 8,
            rust_coding_rating: 8,
            turboquant_priority: 6,
            apple_m_series_tier: "m4_36gb_candidate".to_string(),
            default_role: "primary_coder".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: false,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "qwen35_27b".to_string(),
                mode: "baseline".to_string(),
                default_port: 5416,
                max_ctx: 32768,
                max_output_tokens: 2560,
                trust_remote_code: false,
            }),
        },
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/qwen36-27b".to_string(),
            aliases: vec![
                "qwen36-27b".to_string(),
                "qwen3.6-27b".to_string(),
                "qwen3.6:27b".to_string(),
            ],
            display_name: "Qwen 3.6 27B".to_string(),
            hf_repo: "mlx-community/Qwen3.6-27B-4bit".to_string(),
            backend_kind: ModelBackendKind::MlxOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "qwen36-27b".to_string(),
            estimated_disk_gb: 16.1,
            estimated_active_ram_gb: 17.0,
            context_len: 32768,
            tool_use_rating: 8,
            rust_coding_rating: 8,
            turboquant_priority: 8,
            apple_m_series_tier: "m4_36gb_candidate".to_string(),
            default_role: "primary_coder".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: false,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "qwen36_27b".to_string(),
                mode: "baseline".to_string(),
                default_port: 57036,
                max_ctx: 32768,
                max_output_tokens: 2560,
                trust_remote_code: false,
            }),
        },
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/qwen35-35b-a3b".to_string(),
            aliases: vec!["qwen35-35b-a3b".to_string(), "qwen3.5-35b-a3b".to_string()],
            display_name: "Qwen 3.5 35B A3B".to_string(),
            hf_repo: "mlx-community/Qwen3.5-35B-A3B-4bit".to_string(),
            backend_kind: ModelBackendKind::MlxOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "qwen35-35b-a3b".to_string(),
            estimated_disk_gb: 20.4,
            estimated_active_ram_gb: 20.0,
            context_len: 32768,
            tool_use_rating: 8,
            rust_coding_rating: 7,
            turboquant_priority: 7,
            apple_m_series_tier: "m4_max_safe_reasoning".to_string(),
            default_role: "primary_reasoner".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: false,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "qwen35_35b_a3b".to_string(),
                mode: "baseline".to_string(),
                default_port: 57029,
                max_ctx: 32768,
                max_output_tokens: 2560,
                trust_remote_code: false,
            }),
        },
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/qwen35-122b-a10b".to_string(),
            aliases: vec![
                "qwen35-122b-a10b".to_string(),
                "qwen3.5-122b-a10b".to_string(),
            ],
            display_name: "Qwen 3.5 122B A10B".to_string(),
            hf_repo: "mlx-community/Qwen3.5-122B-A10B-4bit".to_string(),
            backend_kind: ModelBackendKind::MlxOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "qwen35-122b-a10b".to_string(),
            estimated_disk_gb: 69.6,
            estimated_active_ram_gb: 19.8,
            context_len: 32768,
            tool_use_rating: 9,
            rust_coding_rating: 8,
            turboquant_priority: 9,
            apple_m_series_tier: "m4_max_streamed_reasoning".to_string(),
            default_role: "secondary_reasoner".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: false,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "qwen35_122b_a10b".to_string(),
                mode: "streamed".to_string(),
                default_port: 5415,
                max_ctx: 32768,
                max_output_tokens: 1024,
                trust_remote_code: false,
            }),
        },
        ssd_moe_contract::ModelCatalogEntry {
            id: "ssd_moe/deepseek-coder-v2-lite-turbo".to_string(),
            aliases: vec!["deepseek-coder-v2-lite-turbo".to_string()],
            display_name: "DeepSeek Coder V2 Lite Turbo".to_string(),
            hf_repo: "mlx-community/DeepSeek-Coder-V2-Lite-Instruct-4bit".to_string(),
            backend_kind: ModelBackendKind::TurboheroOracle,
            artifact_mode: ssd_moe_contract::ModelArtifactMode::SnapshotOnly,
            download_patterns: Vec::new(),
            prepare_action: None,
            served_model_id: "deepseek-coder-v2-lite-turbo".to_string(),
            estimated_disk_gb: 12.0,
            estimated_active_ram_gb: 14.0,
            context_len: 32768,
            tool_use_rating: 8,
            rust_coding_rating: 8,
            turboquant_priority: 1,
            apple_m_series_tier: "m3".to_string(),
            default_role: "primary".to_string(),
            runtime_class: ssd_moe_contract::ModelRuntimeClass::PrimaryExclusive,
            supported_for_runtime: true,
            experimental: false,
            flash_moe: None,
            turbohero: Some(ssd_moe_contract::TurboHeroCatalogConfig {
                profile_key: "deepseek-coder-v2-lite".to_string(),
                mode: "mlx".to_string(),
                default_port: 11435,
                max_ctx: 32768,
                max_output_tokens: 4096,
                trust_remote_code: false,
            }),
        },
    ]
}

fn load_catalog_entries() -> Vec<ssd_moe_contract::ModelCatalogEntry> {
    catalog_cache().entries.clone()
}

#[cfg(test)]
fn default_primary_model_id_for_tests() -> Option<String> {
    Some(catalog_recommendations().default_primary_model_id)
}

pub fn refresh_catalog_cache_from_broker() {
    let mut cache = catalog_cache();
    merge_catalog_entries(
        &mut cache.entries,
        fetch_catalog_models_blocking(&client_config()).unwrap_or_default(),
    );
    if let Ok(recommendations) = fetch_catalog_recommendations_blocking(&client_config()) {
        cache.recommended_coding_model_id = Some(recommendations.default_primary_model_id);
    }
}

fn canonical_local_model_id(model_id: &str) -> Option<String> {
    local_moe_spec_for_registry_id(model_id).map(|model| model.id.to_string())
}

pub fn is_auto_selected_local_model_id(model_id: &str) -> bool {
    canonical_local_model_id(model_id).is_some_and(|canonical| {
        !AUTOSELECT_QUARANTINED_LOCAL_MODEL_IDS
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(&canonical))
    })
}

pub fn preferred_verified_local_coding_model_id() -> Option<String> {
    canonical_local_model_id(VERIFIED_PRIMARY_LOCAL_CODING_MODEL_ID)
        .filter(|model_id| is_auto_selected_local_model_id(model_id))
}

pub fn preferred_local_coding_model_id() -> Option<String> {
    if let Some(saved) = get_saved_preferred_coding_model_id()
        && is_auto_selected_local_model_id(&saved)
    {
        return canonical_local_model_id(&saved);
    }
    if let Some(verified) = preferred_verified_local_coding_model_id() {
        return Some(verified);
    }
    if let Some(recommended) = catalog_cache().recommended_coding_model_id.clone()
        && is_auto_selected_local_model_id(&recommended)
    {
        return canonical_local_model_id(&recommended);
    }
    #[cfg(test)]
    {
        default_primary_model_id_for_tests()
            .filter(|recommended| is_auto_selected_local_model_id(recommended))
            .and_then(|recommended| canonical_local_model_id(&recommended))
            .or_else(|| {
                let default_id = preferred_local_registry_id_for_role(LocalModelRole::Coding);
                canonical_local_model_id(default_id)
                    .filter(|model_id| is_auto_selected_local_model_id(model_id))
            })
            .or_else(|| {
                local_moe_catalog()
                    .into_iter()
                    .find(|model| {
                        model.role() == LocalModelRole::Coding
                            && is_auto_selected_local_model_id(model.id)
                    })
                    .map(|model| model.id.to_string())
            })
    }
    #[cfg(not(test))]
    {
        let default_id = preferred_local_registry_id_for_role(LocalModelRole::Coding);
        canonical_local_model_id(default_id)
            .filter(|model_id| is_auto_selected_local_model_id(model_id))
            .or_else(|| {
                local_moe_catalog()
                    .into_iter()
                    .find(|model| {
                        model.role() == LocalModelRole::Coding
                            && is_auto_selected_local_model_id(model.id)
                    })
                    .map(|model| model.id.to_string())
            })
    }
}

pub fn preferred_local_reasoning_model_id() -> Option<String> {
    if let Some(saved) = get_saved_preferred_reasoning_model_id()
        && local_moe_spec_for_registry_id(&saved).is_some()
    {
        return Some(saved);
    }
    let default_id = preferred_local_registry_id_for_role(LocalModelRole::Reasoning);
    local_moe_spec_for_registry_id(default_id).map(|_| default_id.to_string())
}

pub fn preferred_local_model_id_for_role(role: LocalModelRole) -> Option<String> {
    match role {
        LocalModelRole::Coding => preferred_local_coding_model_id(),
        LocalModelRole::Reasoning => preferred_local_reasoning_model_id(),
    }
}

fn model_spec_from_catalog_entry(entry: &ssd_moe_contract::ModelCatalogEntry) -> Option<ModelSpec> {
    if !entry.supported_for_runtime {
        return None;
    }
    let runtime_backend = match entry.backend_kind {
        ModelBackendKind::FlashMoePrepared => {
            let flash = entry.flash_moe.as_ref()?;
            LocalRuntimeBackend::FlashMoePrepared {
                model_dir_name: leak_string(flash.prepared_model_dir_name.clone()),
                k_experts: flash.k_experts,
                think_budget: flash.think_budget,
                kv_seq: flash.kv_seq,
            }
        }
        ModelBackendKind::TurboheroOracle | ModelBackendKind::MlxOracle => {
            let turbohero = entry.turbohero.as_ref()?;
            LocalRuntimeBackend::TurboHeroOracle {
                profile_key: leak_string(turbohero.profile_key.clone()),
                mode: leak_string(turbohero.mode.clone()),
                served_model_id: leak_string(entry.served_model_id.clone()),
                default_port: turbohero.default_port,
                max_ctx: turbohero.max_ctx,
                max_output_tokens: turbohero.max_output_tokens,
                trust_remote_code: turbohero.trust_remote_code,
            }
        }
        _ => return None,
    };
    Some(ModelSpec {
        id: leak_string(entry.id.clone()),
        display_name: leak_string(entry.display_name.clone()),
        hf_repo: leak_string(entry.hf_repo.clone()),
        estimated_disk_gb: entry.estimated_disk_gb,
        estimated_ram_gb: entry.estimated_active_ram_gb,
        has_think_tokens: local_model_program(&entry.id)
            .map(|program| program.has_think_tokens)
            .unwrap_or(false),
        description: leak_string(format!(
            "{} role={} backend={:?}",
            entry.display_name, entry.default_role, entry.backend_kind
        )),
        runtime_backend,
    })
}

impl ModelSpec {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn role(&self) -> LocalModelRole {
        local_model_program(self.id)
            .map(|program| program.role)
            .unwrap_or(LocalModelRole::Coding)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn preferred_compaction_policy(&self) -> &'static str {
        local_model_program(self.id)
            .map(|program| program.preferred_compaction_policy)
            .unwrap_or("last6-ledger768")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn warm_start_policy(&self) -> WarmStartPolicy {
        local_model_program(self.id)
            .map(|program| program.warm_start_policy)
            .unwrap_or(WarmStartPolicy::SerializedSharedResidency)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn default_benchmark_specs(&self) -> &'static [&'static str] {
        local_model_program(self.id)
            .map(|program| program.default_benchmark_specs)
            .unwrap_or(&[])
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_think_tokens(&self) -> bool {
        self.has_think_tokens
    }

    pub fn model_dir_name(&self) -> Option<&'static str> {
        match self.runtime_backend {
            LocalRuntimeBackend::FlashMoePrepared { model_dir_name, .. } => Some(model_dir_name),
            LocalRuntimeBackend::TurboHeroOracle { .. } => None,
        }
    }

    pub fn k_experts(&self) -> u32 {
        match self.runtime_backend {
            LocalRuntimeBackend::FlashMoePrepared { k_experts, .. } => k_experts,
            LocalRuntimeBackend::TurboHeroOracle { .. } => 0,
        }
    }

    pub fn think_budget(&self) -> u32 {
        match self.runtime_backend {
            LocalRuntimeBackend::FlashMoePrepared { think_budget, .. } => think_budget,
            LocalRuntimeBackend::TurboHeroOracle { .. } => 0,
        }
    }

    pub fn kv_seq(&self) -> u32 {
        match self.runtime_backend {
            LocalRuntimeBackend::FlashMoePrepared { kv_seq, .. } => kv_seq,
            LocalRuntimeBackend::TurboHeroOracle { max_ctx, .. } => max_ctx,
        }
    }

    pub fn default_port(&self) -> u16 {
        match self.runtime_backend {
            LocalRuntimeBackend::FlashMoePrepared { .. } => {
                flash_moe_defaults::DEFAULT_INFER_SERVE_PORT
            }
            LocalRuntimeBackend::TurboHeroOracle { default_port, .. } => default_port,
        }
    }

    #[allow(dead_code)]
    pub fn turbohero_profile_key(&self) -> Option<&'static str> {
        match self.runtime_backend {
            LocalRuntimeBackend::TurboHeroOracle { profile_key, .. } => Some(profile_key),
            LocalRuntimeBackend::FlashMoePrepared { .. } => None,
        }
    }

    #[allow(dead_code)]
    pub fn turbohero_mode(&self) -> Option<&'static str> {
        match self.runtime_backend {
            LocalRuntimeBackend::TurboHeroOracle { mode, .. } => Some(mode),
            LocalRuntimeBackend::FlashMoePrepared { .. } => None,
        }
    }

    pub fn served_model_id(&self) -> Option<&'static str> {
        match self.runtime_backend {
            LocalRuntimeBackend::TurboHeroOracle {
                served_model_id, ..
            } => Some(served_model_id),
            LocalRuntimeBackend::FlashMoePrepared { .. } => None,
        }
    }

    #[allow(dead_code)]
    pub fn max_output_tokens(&self) -> Option<u32> {
        match self.runtime_backend {
            LocalRuntimeBackend::TurboHeroOracle {
                max_output_tokens, ..
            } => Some(max_output_tokens),
            LocalRuntimeBackend::FlashMoePrepared { .. } => None,
        }
    }

    #[allow(dead_code)]
    pub fn trust_remote_code(&self) -> bool {
        match self.runtime_backend {
            LocalRuntimeBackend::TurboHeroOracle {
                trust_remote_code, ..
            } => trust_remote_code,
            LocalRuntimeBackend::FlashMoePrepared { .. } => false,
        }
    }
}

pub fn local_moe_catalog() -> Vec<ModelSpec> {
    load_catalog_entries()
        .into_iter()
        .filter_map(|entry| model_spec_from_catalog_entry(&entry))
        .collect()
}

pub fn local_moe_spec_for_registry_id(registry_id: &str) -> Option<ModelSpec> {
    let lowered = registry_id.trim().to_ascii_lowercase();
    for entry in load_catalog_entries() {
        if entry.id.eq_ignore_ascii_case(&lowered)
            || entry
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&lowered))
        {
            return model_spec_from_catalog_entry(&entry);
        }
    }
    None
}

pub fn interactive_chat_catalog(preferred_provider: InteractiveProviderKind) -> Vec<String> {
    let mut models = Vec::new();
    let mut push_unique = |candidate: String| {
        if !models.iter().any(|existing| existing == &candidate) {
            models.push(candidate);
        }
    };

    let mut append_provider_models = |provider: InteractiveProviderKind| match provider {
        InteractiveProviderKind::Local => {
            for model in local_moe_catalog() {
                push_unique(model.id.to_string());
            }
        }
        InteractiveProviderKind::Ollama => {
            for model in OLLAMA_RECOMMENDED_MODELS {
                push_unique(format!("{}/{}", provider.label(), model));
            }
        }
        InteractiveProviderKind::OpenAiCompatible => {
            if let Some(model) = crate::quorp::provider_config::resolved_model_env() {
                push_unique(format!(
                    "{}/{}",
                    provider.label(),
                    chat_model_raw_id(&model)
                ));
            }
        }
        InteractiveProviderKind::Nvidia => {
            for model in NVIDIA_RECOMMENDED_MODELS {
                push_unique(format!("{}/{}", provider.label(), model));
            }
        }
        InteractiveProviderKind::Codex => {
            push_unique(format!(
                "{}/{}",
                provider.label(),
                crate::quorp::codex_executor::default_model_id()
            ));
        }
    };

    append_provider_models(preferred_provider);
    for provider in [
        InteractiveProviderKind::Local,
        InteractiveProviderKind::Ollama,
        InteractiveProviderKind::OpenAiCompatible,
        InteractiveProviderKind::Nvidia,
        InteractiveProviderKind::Codex,
    ] {
        if provider != preferred_provider {
            append_provider_models(provider);
        }
    }

    models
}

pub fn default_interactive_model_id(provider: InteractiveProviderKind) -> Option<String> {
    match provider {
        InteractiveProviderKind::Local => active_heavy_role()
            .and_then(preferred_local_model_id_for_role)
            .or_else(preferred_local_coding_model_id)
            .or_else(|| {
                local_moe_catalog()
                    .into_iter()
                    .next()
                    .map(|model| model.id.to_string())
            }),
        InteractiveProviderKind::Ollama => OLLAMA_RECOMMENDED_MODELS
            .first()
            .map(|model| format!("{}/{}", provider.label(), model)),
        InteractiveProviderKind::OpenAiCompatible => {
            crate::quorp::provider_config::resolved_model_env()
                .map(|model| format!("{}/{}", provider.label(), chat_model_raw_id(&model)))
        }
        InteractiveProviderKind::Nvidia => NVIDIA_RECOMMENDED_MODELS
            .first()
            .map(|model| format!("{}/{}", provider.label(), model)),
        InteractiveProviderKind::Codex => Some(format!(
            "{}/{}",
            provider.label(),
            crate::quorp::codex_executor::default_model_id()
        )),
    }
}

fn managed_planner_model_id(requested_model_id: &str) -> String {
    match chat_model_raw_id(requested_model_id) {
        "gpt-oss:20b" => MANAGED_BALANCED_PLANNER_MODEL_ID.to_string(),
        "qwen3.5:9b-q8_0" => MANAGED_LIGHTWEIGHT_PLANNER_MODEL_ID.to_string(),
        "gemma4:e4b" => MANAGED_COMPACT_PLANNER_MODEL_ID.to_string(),
        _ => MANAGED_PLANNER_DEFAULT_MODEL_ID.to_string(),
    }
}

fn is_manual_opt_in_model(model_id: &str) -> bool {
    matches!(
        chat_model_provider(model_id, InteractiveProviderKind::Local),
        InteractiveProviderKind::Codex
            | InteractiveProviderKind::OpenAiCompatible
            | InteractiveProviderKind::Nvidia
    ) || canonical_local_model_id(chat_model_raw_id(model_id))
        .is_some_and(|id| id == MANAGED_MANUAL_ONLY_MODEL_ID)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn classify_managed_task_intent(text: &str) -> ManagedTaskIntent {
    let normalized = text.to_ascii_lowercase();
    if contains_any(
        &normalized,
        &[
            "review",
            "code review",
            "critique",
            "architecture",
            "design",
            "tradeoff",
            "trade-off",
            "reason through",
            "analyze this",
        ],
    ) {
        return ManagedTaskIntent::Reasoning;
    }
    if contains_any(
        &normalized,
        &[
            "implement",
            "fix",
            "patch",
            "edit ",
            "edit the",
            "write ",
            "refactor",
            "rename",
            "update ",
            "modify ",
            "add test",
            "add a test",
            "make the change",
        ],
    ) {
        return ManagedTaskIntent::Coding;
    }
    ManagedTaskIntent::Planning
}

pub fn managed_chat_model_id(requested_model_id: &str, latest_input: &str) -> String {
    if is_manual_opt_in_model(requested_model_id) {
        return requested_model_id.to_string();
    }
    match classify_managed_task_intent(latest_input) {
        ManagedTaskIntent::Planning => managed_planner_model_id(requested_model_id),
        ManagedTaskIntent::Coding => MANAGED_CODING_MODEL_ID.to_string(),
        ManagedTaskIntent::Reasoning => MANAGED_REASONING_MODEL_ID.to_string(),
    }
}

pub fn managed_command_summary_model_id(requested_model_id: &str) -> String {
    if is_manual_opt_in_model(requested_model_id) {
        return requested_model_id.to_string();
    }
    managed_planner_model_id(requested_model_id)
}

pub fn managed_autonomous_model_id(requested_model_id: &str, _goal: &str) -> String {
    if is_manual_opt_in_model(requested_model_id) {
        return requested_model_id.to_string();
    }
    MANAGED_CODING_MODEL_ID.to_string()
}

pub fn chat_model_provider(
    model_id: &str,
    default_provider: InteractiveProviderKind,
) -> InteractiveProviderKind {
    if let Some(rest) = model_id.strip_prefix("ollama/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::Ollama;
    }
    if let Some(rest) = model_id.strip_prefix("codex/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::Codex;
    }
    if let Some(rest) = model_id.strip_prefix("openai-compatible/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::OpenAiCompatible;
    }
    if let Some(rest) = model_id.strip_prefix("openai/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::OpenAiCompatible;
    }
    if let Some(rest) = model_id.strip_prefix("nvidia/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::Nvidia;
    }
    if let Some(rest) = model_id.strip_prefix("local/")
        && !rest.trim().is_empty()
    {
        return InteractiveProviderKind::Local;
    }
    if model_id.starts_with("ssd_moe/") {
        return InteractiveProviderKind::Local;
    }
    if local_moe_spec_for_registry_id(model_id).is_some() {
        return InteractiveProviderKind::Local;
    }
    default_provider
}

pub fn chat_model_raw_id(model_id: &str) -> &str {
    for prefix in [
        "ollama/",
        "codex/",
        "openai-compatible/",
        "openai/",
        "nvidia/",
        "local/",
        "ssd_moe/",
    ] {
        if let Some(raw) = model_id.strip_prefix(prefix)
            && !raw.trim().is_empty()
        {
            return raw;
        }
    }
    model_id
}

pub fn chat_model_display_label(
    model_id: &str,
    default_provider: InteractiveProviderKind,
) -> String {
    let provider = chat_model_provider(model_id, default_provider);
    let raw = chat_model_raw_id(model_id);
    match provider {
        InteractiveProviderKind::Local if local_moe_spec_for_registry_id(raw).is_some() => {
            raw.to_string()
        }
        _ => format!("{raw} · {}", provider.title()),
    }
}

pub fn chat_model_subtitle(model_id: &str, default_provider: InteractiveProviderKind) -> String {
    let provider = chat_model_provider(model_id, default_provider);
    let raw = chat_model_raw_id(model_id);
    if let Some(spec) = local_moe_spec_for_registry_id(raw) {
        return spec.description.to_string();
    }
    match provider {
        InteractiveProviderKind::Local => "Shared local SSD-MOE runtime model.".to_string(),
        InteractiveProviderKind::Ollama => {
            "Ollama chat model served through the local OpenAI-compatible endpoint.".to_string()
        }
        InteractiveProviderKind::OpenAiCompatible => {
            "Remote OpenAI-compatible chat model served over HTTPS.".to_string()
        }
        InteractiveProviderKind::Nvidia => {
            "NVIDIA NIM hosted model served through the OpenAI-compatible endpoint.".to_string()
        }
        InteractiveProviderKind::Codex => {
            "Codex executor model for interactive Quorp sessions.".to_string()
        }
    }
}

fn read_saved_model_id_from_disk() -> Option<String> {
    #[cfg(test)]
    {
        let test_root = TEST_MODEL_CONFIG_ROOT.with(|root| root.borrow().clone());
        if let Some(root) = test_root.as_ref() {
            let path = root.join(".config/quorp-tui/active_model.txt");
            return std::fs::read_to_string(path).ok();
        }
    }
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".config/quorp-tui/active_model.txt");
    std::fs::read_to_string(path).ok()
}

pub fn get_saved_model_id_raw() -> Option<String> {
    let raw = read_saved_model_id_from_disk()?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn get_saved_model() -> Option<ModelSpec> {
    if let Some(saved) = get_saved_model_id_raw()
        .filter(|trimmed| is_auto_selected_local_model_id(trimmed))
        .and_then(|trimmed| local_moe_spec_for_registry_id(&trimmed))
    {
        return Some(saved);
    }
    if let Some(role_saved_model) = active_heavy_role()
        .and_then(preferred_local_model_id_for_role)
        .and_then(|model_id| local_moe_spec_for_registry_id(&model_id))
    {
        return Some(role_saved_model);
    }
    if let Some(saved_coding_model) = preferred_local_coding_model_id()
        .and_then(|model_id| local_moe_spec_for_registry_id(&model_id))
    {
        return Some(saved_coding_model);
    }
    local_moe_catalog().into_iter().next()
}

fn config_root() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        let test_root = TEST_MODEL_CONFIG_ROOT.with(|root| root.borrow().clone());
        if let Some(root) = test_root.as_ref() {
            return Some(root.clone());
        }
    }
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

fn write_config_file(name: &str, contents: &str) -> io::Result<()> {
    let Some(root) = config_root() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is not set for Quorp model config",
        ));
    };
    let dir = root.join(".config/quorp-tui");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(name), contents)
}

fn read_config_file(name: &str) -> Option<String> {
    let root = config_root()?;
    std::fs::read_to_string(root.join(".config/quorp-tui").join(name)).ok()
}

pub fn save_model(id: &str) -> io::Result<()> {
    write_config_file("active_model.txt", id)?;
    if let Some(model_spec) = local_moe_spec_for_registry_id(id) {
        save_active_heavy_role(model_spec.role())?;
        match model_spec.role() {
            LocalModelRole::Coding => save_preferred_coding_model_id(model_spec.id)?,
            LocalModelRole::Reasoning => save_preferred_reasoning_model_id(model_spec.id)?,
        }
    }
    Ok(())
}

pub fn get_saved_chat_model_id() -> Option<String> {
    let raw = read_config_file("default_chat_model.txt")?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn save_chat_model_id(id: &str) -> io::Result<()> {
    write_config_file("default_chat_model.txt", id)
}

fn read_non_empty_config_file(name: &str) -> Option<String> {
    let raw = read_config_file(name)?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn get_saved_preferred_coding_model_id() -> Option<String> {
    read_non_empty_config_file("preferred_coding_model.txt")
}

pub fn save_preferred_coding_model_id(id: &str) -> io::Result<()> {
    write_config_file("preferred_coding_model.txt", id)
}

pub fn get_saved_preferred_reasoning_model_id() -> Option<String> {
    read_non_empty_config_file("preferred_reasoning_model.txt")
}

pub fn save_preferred_reasoning_model_id(id: &str) -> io::Result<()> {
    write_config_file("preferred_reasoning_model.txt", id)
}

pub fn active_heavy_role() -> Option<LocalModelRole> {
    read_non_empty_config_file("active_heavy_role.txt")
        .as_deref()
        .and_then(LocalModelRole::from_config_value)
}

pub fn save_active_heavy_role(role: LocalModelRole) -> io::Result<()> {
    write_config_file("active_heavy_role.txt", role.as_config_value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_model_persists_role_specific_preferences() {
        let _guard = isolated_test_model_config_guard();
        save_model("qwen35-35b-a3b").expect("save reasoning model");

        assert_eq!(active_heavy_role(), Some(LocalModelRole::Reasoning));
        assert_eq!(
            get_saved_preferred_reasoning_model_id().as_deref(),
            Some("ssd_moe/qwen35-35b-a3b")
        );
        assert_eq!(
            get_saved_model()
                .map(|model| model.id.to_string())
                .as_deref(),
            Some("ssd_moe/qwen35-35b-a3b")
        );
    }

    #[test]
    fn saved_active_role_falls_back_to_role_default_model() {
        let _guard = isolated_test_model_config_guard();
        save_active_heavy_role(LocalModelRole::Reasoning).expect("save active role");

        let model = get_saved_model().expect("reasoning default model");
        assert_eq!(model.id, "ssd_moe/qwen35-35b-a3b");
    }

    #[test]
    fn local_model_programs_match_the_runtime_catalog() {
        for program in crate::quorp::tui::local_model_program::local_model_programs() {
            let model = local_moe_spec_for_registry_id(program.registry_id).unwrap_or_else(|| {
                panic!("missing runtime catalog entry for {}", program.registry_id)
            });
            assert_eq!(model.role(), program.role);
            assert_eq!(
                model.preferred_compaction_policy(),
                program.preferred_compaction_policy
            );
            assert_eq!(model.warm_start_policy(), program.warm_start_policy);
            assert_eq!(
                model.default_benchmark_specs(),
                program.default_benchmark_specs
            );
            assert_eq!(model.has_think_tokens(), program.has_think_tokens);
        }
    }

    #[test]
    fn preferred_local_coding_model_defaults_to_verified_27b() {
        let _guard = isolated_test_model_config_guard();

        let preferred = preferred_local_coding_model_id().expect("preferred coding model");

        assert_eq!(preferred, "ssd_moe/qwen35-27b");
    }

    #[test]
    fn quarantined_coding_model_is_not_auto_selected() {
        let _guard = isolated_test_model_config_guard();
        save_model("qwen3-coder-30b-a3b").expect("save quarantined model");

        let preferred = preferred_local_coding_model_id().expect("preferred coding model");
        let saved = get_saved_model().expect("resolved saved model");

        assert_eq!(preferred, "ssd_moe/qwen35-27b");
        assert_eq!(saved.id, "ssd_moe/qwen35-27b");
        assert!(!is_auto_selected_local_model_id("qwen3-coder-30b-a3b"));
    }

    #[test]
    fn preferred_provider_catalog_includes_remote_entries() {
        let catalog = interactive_chat_catalog(InteractiveProviderKind::Ollama);
        assert_eq!(
            catalog
                .iter()
                .take(5)
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "ollama/devstral-small-2",
                "ollama/gpt-oss:20b",
                "ollama/qwen3.5:9b-q8_0",
                "ollama/gemma4:e4b",
                "ollama/qwen2.5-coder:32b",
            ]
        );
        assert!(
            catalog
                .iter()
                .any(|model| model == "ollama/devstral-small-2")
        );
        assert!(catalog.iter().any(|model| model == "ollama/gpt-oss:20b"));
        assert!(
            catalog
                .iter()
                .any(|model| model == "ollama/qwen3.5:9b-q8_0")
        );
        assert!(catalog.iter().any(|model| model == "ollama/gemma4:e4b"));
        assert!(
            catalog
                .iter()
                .any(|model| model == "ollama/qwen2.5-coder:32b")
        );
        assert!(catalog.iter().any(|model| model.starts_with("codex/")));
        assert!(
            interactive_chat_catalog(InteractiveProviderKind::Local)
                .iter()
                .any(|model| model.starts_with("ssd_moe/"))
        );
        assert!(
            interactive_chat_catalog(InteractiveProviderKind::Nvidia)
                .iter()
                .any(|model| model == "nvidia/moonshotai/kimi-k2.5")
        );
    }

    #[test]
    fn provider_scoped_models_resolve_provider_and_raw_id() {
        assert_eq!(
            chat_model_provider("ollama/qwen2.5-coder:32b", InteractiveProviderKind::Local),
            InteractiveProviderKind::Ollama
        );
        assert_eq!(
            chat_model_raw_id("codex/gpt-5.3-codex-spark"),
            "gpt-5.3-codex-spark"
        );
        assert_eq!(
            chat_model_display_label("ollama/qwen2.5-coder:32b", InteractiveProviderKind::Local),
            "qwen2.5-coder:32b · Ollama"
        );
    }

    #[test]
    fn nvidia_models_resolve_provider_and_raw_id() {
        assert_eq!(
            default_interactive_model_id(InteractiveProviderKind::Nvidia),
            Some("nvidia/moonshotai/kimi-k2.5".to_string())
        );
        assert_eq!(
            chat_model_provider(
                "nvidia/moonshotai/kimi-k2.5",
                InteractiveProviderKind::Local
            ),
            InteractiveProviderKind::Nvidia
        );
        assert_eq!(
            chat_model_raw_id("nvidia/moonshotai/kimi-k2.5"),
            "moonshotai/kimi-k2.5"
        );
        assert_eq!(
            chat_model_display_label(
                "nvidia/moonshotai/kimi-k2.5",
                InteractiveProviderKind::Local
            ),
            "moonshotai/kimi-k2.5 · NVIDIA"
        );
        assert_eq!(
            chat_model_raw_id("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
            "qwen/qwen3-coder-480b-a35b-instruct"
        );
        assert_eq!(
            chat_model_display_label(
                "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
                InteractiveProviderKind::Local
            ),
            "qwen/qwen3-coder-480b-a35b-instruct · NVIDIA"
        );
    }

    #[test]
    fn default_ollama_model_prefers_devstral_small_2() {
        assert_eq!(
            default_interactive_model_id(InteractiveProviderKind::Ollama),
            Some("ollama/devstral-small-2".to_string())
        );
    }

    #[test]
    fn heavy_local_models_allow_benchmark_sized_output_caps() {
        let qwen35_27b =
            local_moe_spec_for_registry_id("ssd_moe/qwen35-27b").expect("27b coder model");
        assert_eq!(qwen35_27b.max_output_tokens(), Some(2560));

        let qwen36_27b =
            local_moe_spec_for_registry_id("qwen3.6:27b").expect("qwen3.6 coder model");
        assert_eq!(qwen36_27b.id, "ssd_moe/qwen36-27b");
        assert_eq!(qwen36_27b.max_output_tokens(), Some(2560));

        let coder = local_moe_spec_for_registry_id("ssd_moe/qwen3-coder-30b-a3b")
            .expect("30b coder model in registry");
        assert_eq!(coder.max_output_tokens(), Some(2560));

        let planner = local_moe_spec_for_registry_id("ssd_moe/qwen35-35b-a3b")
            .expect("35b planner model in registry");
        assert_eq!(planner.max_output_tokens(), Some(2560));
    }

    #[test]
    fn managed_chat_model_defaults_to_devstral_for_planning_turns() {
        assert_eq!(
            managed_chat_model_id("ollama/devstral-small-2", "Please plan the change first."),
            "ollama/devstral-small-2"
        );
        assert_eq!(
            managed_chat_model_id(
                "ollama/gpt-oss:20b",
                "Search the repo and outline the approach."
            ),
            "ollama/gpt-oss:20b"
        );
    }

    #[test]
    fn managed_chat_model_escalates_to_coder_for_implementation_turns() {
        assert_eq!(
            managed_chat_model_id(
                "ollama/devstral-small-2",
                "Please implement the fix and add a test."
            ),
            "ssd_moe/qwen3-coder-30b-a3b"
        );
    }

    #[test]
    fn managed_chat_model_escalates_to_reasoner_for_review_turns() {
        assert_eq!(
            managed_chat_model_id(
                "ollama/devstral-small-2",
                "Review this design and critique the tradeoffs."
            ),
            "ssd_moe/qwen35-35b-a3b"
        );
    }

    #[test]
    fn managed_autonomous_model_prefers_coder_lane() {
        assert_eq!(
            managed_autonomous_model_id("ollama/devstral-small-2", "Implement the accepted plan."),
            "ssd_moe/qwen3-coder-30b-a3b"
        );
    }

    #[test]
    fn manual_opt_in_models_are_preserved() {
        assert_eq!(
            managed_chat_model_id("ssd_moe/qwen35-122b-a10b", "Please plan the migration."),
            "ssd_moe/qwen35-122b-a10b"
        );
        assert_eq!(
            managed_autonomous_model_id("codex/gpt-5.4-mini", "Implement the accepted plan."),
            "codex/gpt-5.4-mini"
        );
    }
}
