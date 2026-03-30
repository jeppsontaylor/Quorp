//! Local SSD-MOE weight catalog and on-disk hint `~/.config/quorp-tui/active_model.txt` (**local infer
//! weights only**).
//!
//! Model ids stored here are local TUI infer selections only. Do not write provider-style ids such as
//! `provider/model` into `active_model.txt`; [`save_model`] is for local catalog ids and tests.

use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
static TEST_MODEL_CONFIG_ROOT: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// When set, [`save_model`] and [`get_saved_model`] use `ROOT/.config/quorp-tui/active_model.txt`
/// instead of `$HOME`. Reset to `None` after the test (see flow test harness).
#[cfg(test)]
pub(crate) fn set_test_model_config_root(path: Option<std::path::PathBuf>) {
    *TEST_MODEL_CONFIG_ROOT.lock().expect("model config test lock") = path;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub hf_repo: &'static str,
    pub model_dir_name: &'static str,
    pub k_experts: u32,
    pub estimated_disk_gb: f32,
    pub estimated_ram_gb: f32,
    pub has_think_tokens: bool,
    pub description: &'static str,
}

/// Local Flash-MOE / SSD-MOE weight sets (infer `--model` directory, download scripts, etc.).
/// Chat model selection for cloud providers comes from [`language_model::LanguageModelRegistry`]; this
/// catalog is only for driving the local `infer` process.
pub fn local_moe_catalog() -> Vec<ModelSpec> {
    vec![
        ModelSpec {
            id: "qwen3-coder-30b-a3b",
            display_name: "Qwen3 Coder 30B",
            hf_repo: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit",
            model_dir_name: "out_coder_30b",
            k_experts: 8,
            estimated_disk_gb: 12.0,
            estimated_ram_gb: 2.0,
            has_think_tokens: false,
            description: "Agentic coding specialist. 48 layers, K=8. Standard full attention.",
        },
        ModelSpec {
            id: "qwen35-35b-a3b",
            display_name: "Qwen3.5 35B Hybrid",
            hf_repo: "mlx-community/Qwen3.5-35B-A3B-4bit",
            model_dir_name: "out_35b",
            k_experts: 4,
            estimated_disk_gb: 18.4,
            estimated_ram_gb: 2.2,
            has_think_tokens: true,
            description: "Hybrid reasoning model. 30 linear + 10 full attention layers.",
        },
        ModelSpec {
            id: "qwen3-30b-a3b-general",
            display_name: "Qwen3 30B General",
            hf_repo: "Qwen/Qwen3-30B-A3B-Instruct-2507",
            model_dir_name: "out_30b_general",
            k_experts: 8,
            estimated_disk_gb: 12.0,
            estimated_ram_gb: 2.0,
            has_think_tokens: false,
            description: "General-purpose 30B model with 256K context capabilities. Suitable for math, reasoning, and multilingual input.",
        },
    ]
}

/// Map a chat / registry model id (e.g. `ssd_moe/qwen3.5-35b-a3b` or short `qwen35-35b-a3b`) to local MoE layout.
pub fn local_moe_spec_for_registry_id(registry_id: &str) -> Option<ModelSpec> {
    for m in local_moe_catalog() {
        if m.id == registry_id {
            return Some(m.clone());
        }
    }
    match registry_id {
        "ssd_moe/qwen3.5-35b-a3b" => local_moe_catalog()
            .into_iter()
            .find(|m| m.id == "qwen35-35b-a3b"),
        _ => None,
    }
}

fn read_saved_model_id_from_disk() -> Option<String> {
    #[cfg(test)]
    {
        let guard = TEST_MODEL_CONFIG_ROOT.lock().expect("model config test lock");
        if let Some(root) = guard.as_ref() {
            let path = root.join(".config/quorp-tui/active_model.txt");
            return std::fs::read_to_string(path).ok();
        }
    }
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".config/quorp-tui/active_model.txt");
    std::fs::read_to_string(path).ok()
}

/// Raw `active_model.txt` contents (trimmed), if any.
pub fn get_saved_model_id_raw() -> Option<String> {
    let raw = read_saved_model_id_from_disk()?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn get_saved_model() -> ModelSpec {
    let default = local_moe_catalog()[1].clone();
    let Some(trimmed) = get_saved_model_id_raw() else {
        return default;
    };
    local_moe_spec_for_registry_id(&trimmed)
        .or_else(|| local_moe_catalog().into_iter().find(|m| m.id == trimmed.as_str()))
        .unwrap_or(default)
}

pub fn save_model(id: &str) {
    #[cfg(test)]
    {
        let guard = TEST_MODEL_CONFIG_ROOT.lock().expect("model config test lock");
        if let Some(root) = guard.as_ref() {
            let dir = root.join(".config/quorp-tui");
            std::fs::create_dir_all(&dir).ok();
            std::fs::write(dir.join("active_model.txt"), id).ok();
            return;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let dir = std::path::PathBuf::from(home).join(".config/quorp-tui");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("active_model.txt"), id).ok();
    }
}
