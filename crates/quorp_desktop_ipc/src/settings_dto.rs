//! Desktop-specific settings that augment Quorp's project-level config.
//!
//! Quorp's CLI/runtime settings live in `~/.quorp/settings.json` and
//! `<workspace>/.quorp/settings.json` and are owned by `quorp_config`.
//! These DTOs describe app-state preferences that only the desktop UI
//! cares about (theme, sidebar widths, window geometry hints) plus
//! summaries of the underlying provider/sandbox config that the UI
//! needs to render. The Rust side is the source of truth for everything
//! sensitive; secrets never appear here.

use serde::{Deserialize, Serialize};

use crate::permission_dto::PermissionModeDto;
use crate::run_request::SandboxModeDto;

/// Top-level settings DTO loaded by the desktop on startup. Persisted
/// in `~/Library/Application Support/Quorp/settings.json` (macOS) by
/// `quorp_desktop_core::settings_store`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopSettingsDto {
    pub general: GeneralSettingsDto,
    pub sandbox: SandboxSettingsDto,
    pub provider: ProviderSummary,
    pub default_permission_mode: PermissionModeDto,
    pub default_sandbox_mode: SandboxModeDto,
    pub run_retention: RunRetentionDto,
}

/// Visual preferences that take effect immediately on save.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettingsDto {
    pub theme: ThemeDto,
    pub font_size: FontSize,
    pub density: Density,
    /// Cap on the number of normalized timeline events held in React
    /// memory. Older events are paged from disk on scroll.
    pub timeline_event_cap: u32,
    pub animations_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeDto {
    Dark,
    HighContrast,
    NoColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontSize {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Comfortable,
    Compact,
}

/// Sandbox-related defaults. Network is off by default and cannot be
/// disabled in v1; the toggle exists for forward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSettingsDto {
    /// How many recent run-temp directories to keep on disk before the
    /// retention sweeper evicts the oldest.
    pub keep_last_n: u32,
    pub network_default: NetworkAllowanceDto,
    /// Disk budget across all retained sandboxes, in bytes. Default is
    /// 25 GiB; the run service refuses new runs when over budget.
    pub disk_budget_bytes: u64,
    /// Base directory for run-temp lifecycles. Default is `/tmp/quorp`.
    pub working_dir_base: String,
    /// Wall-clock budget per run in seconds. The watchdog kills runs
    /// that exceed this.
    pub default_wall_clock_budget_seconds: u64,
}

/// Network allowance levels. Mirrors `AppleSandboxSettings::allow_network`
/// from `quorp_sandbox` on the wire (added in PR3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAllowanceDto {
    /// Default. The sandbox profile denies all network syscalls.
    None,
    /// Allow only `127.0.0.1` traffic (LSP servers, fixture loopbacks).
    LocalhostOnly,
    /// Full outbound access. Requires Trusted workspace + explicit
    /// confirmation.
    All,
}

/// Retention policy for run artifacts under `<workspace>/.quorp/runs/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRetentionDto {
    /// Keep at most this many runs per workspace before compaction.
    pub keep_last_runs: u32,
    /// Total disk budget across all runs, in bytes. The lower of the two
    /// caps wins. Default 5 GiB.
    pub max_total_bytes: u64,
    /// When `true`, older runs are gzipped (events.jsonl.zst) but
    /// summaries/proofs are kept verbatim. When `false`, older runs
    /// are deleted outright.
    pub compact_instead_of_delete: bool,
}

/// Provider summary surfaced in Settings → Provider. The desktop only
/// ever reports a single provider — Quorp uses NVIDIA NIM Qwen3-Coder
/// exclusively. The API key never appears here; only its presence does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSummary {
    /// Always [`crate::DEFAULT_PROVIDER_NAME`].
    pub name: String,
    pub display_name: String,
    /// Read-only base URL. Surface for diagnostics; not user-editable.
    pub base_url: String,
    /// Always [`crate::DEFAULT_MODEL_ID`].
    pub default_model: String,
    /// `true` when an API key is present in the macOS Keychain.
    pub has_key: bool,
}

/// Result of pinging the provider's chat-completions endpoint with the
/// stored key. Returned by `validate_nim_provider`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub ok: bool,
    pub latency_ms: u64,
    /// Model id echoed by the provider response (sanity check).
    pub model_id_echo: Option<String>,
    /// On failure, a redacted error description. Never includes the key.
    pub error: Option<String>,
}

/// Metadata for a benchmark fixture surfaced in the Benchmarks panel.
/// Source path layout: `benchmark/challenges/<set>/<id>/...`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkFixture {
    pub fixture_id: String,
    pub set: String,
    pub display_name: String,
    pub description: String,
    /// Absolute path to the fixture's upstream workspace.
    pub workspace_path: String,
    /// Absolute path to the reference proof directory if present.
    pub reference_proof_path: Option<String>,
    /// `true` when the fixture has a `proof-full/` directory we can
    /// diff against the agent's output.
    pub has_reference_proof: bool,
}
