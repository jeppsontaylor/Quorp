use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::quorp::agent_local::{HeadlessRunOptions, resume_headless_agent, run_headless_agent};
use crate::quorp::codex_executor::{
    CodexCompletionOptions, CodexRunOptions, default_model_id as default_codex_model_id,
    fresh_session_strategy, request_codex_completion, run_codex_agent,
};
use crate::quorp::tui::chat_service::{
    ChatServiceMessage, ChatServiceRole, StreamRequest, request_single_completion_details,
};
use crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle;
use quorp_agent_core::{PromptCompactionPolicy, TranscriptMessage, TranscriptRole};

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const SYNTHETIC_OBJECTIVE_FILE: &str = "QUORP_BENCHMARK_OBJECTIVE.md";
const CHALLENGE_OBJECTIVE_FILE: &str = "QUORP_CHALLENGE_OBJECTIVE.md";
const CHALLENGE_SANDBOX_DIR: &str = "sandbox";
const CHALLENGE_CARGO_CACHE_DIR: &str = ".quorp-cargo-target";
const CHALLENGE_EVALUATION_CARGO_CACHE_DIR: &str = ".quorp-cargo-target-eval";
const SAFE_PROMPT_TOKEN_CAP: u64 = 1800;
const SAFE_LOCAL_BENCHMARK_MODEL_ID: &str = "ssd_moe/qwen35-27b";
const JUDGE_OUTPUT_LINE_LIMIT: usize = 48;
const JUDGE_OUTPUT_CHAR_LIMIT: usize = 6000;
const BENCHMARK_BOOTSTRAP_PROGRESS_FILE: &str = "bootstrap-progress.json";
const PRE_MODEL_BOOTSTRAP_TIMEOUT_SECS: u64 = 120;
const BOOTSTRAP_PHASE_BENCHMARK_STARTED: &str = "benchmark_started";
const BOOTSTRAP_PHASE_WORKSPACE_LAYOUT_RESOLVED: &str = "workspace_layout_resolved";
const BOOTSTRAP_PHASE_BASELINE_RESET_READY: &str = "baseline_reset_ready";
const BOOTSTRAP_PHASE_CHALLENGE_CAPSULE_REHYDRATED: &str = "challenge_capsule_rehydrated";
const BOOTSTRAP_PHASE_FAST_LOOP_CONTRACT_LOADED: &str = "fast_loop_contract_loaded";
const BOOTSTRAP_PHASE_PROMPT_MATERIALIZED: &str = "prompt_materialized";
const BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED: &str = "control_loop_started";
const BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST: &str = "first_task_model_request";
const BOOTSTRAP_STALL_CLASS_PRE_MODEL: &str = "pre_model_bootstrap_stalled";

fn safe_benchmark_model_id() -> anyhow::Result<String> {
    if let Some(model_id) =
        crate::quorp::tui::model_registry::preferred_verified_local_coding_model_id()
    {
        return Ok(model_id);
    }
    if crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(
        SAFE_LOCAL_BENCHMARK_MODEL_ID,
    )
    .is_some()
    {
        return Ok(SAFE_LOCAL_BENCHMARK_MODEL_ID.to_string());
    }
    crate::quorp::tui::model_registry::local_moe_catalog()
        .into_iter()
        .find(|model| !is_heavy_local_model_id(model.id))
        .map(|model| model.id.to_string())
        .or_else(crate::quorp::tui::model_registry::preferred_local_coding_model_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no safe local benchmark model was available; pass --model explicitly if you want a specific native runtime"
            )
        })
}

fn allow_resolved_benchmark_model_without_opt_in(
    requested_model_id: Option<&str>,
    resolved_model_id: &str,
    allow_heavy_local_model: bool,
) -> bool {
    allow_heavy_local_model
        || (requested_model_id.is_none()
            && safe_benchmark_model_id()
                .ok()
                .is_some_and(|default_model| default_model.eq_ignore_ascii_case(resolved_model_id)))
}

fn apply_requested_prompt_compaction_override(
    completion_policy: &mut quorp_agent_core::CompletionPolicy,
    requested_policy: Option<PromptCompactionPolicy>,
) {
    if let Some(policy) = requested_policy {
        completion_policy.prompt_compaction_policy = Some(policy);
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkExecutor {
    #[default]
    Native,
    Codex,
}

impl BenchmarkExecutor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkRunOptions {
    pub path: PathBuf,
    pub executor: BenchmarkExecutor,
    pub model_id: Option<String>,
    pub base_url_override: Option<String>,
    pub briefing_file: Option<PathBuf>,
    pub compaction_policy: Option<PromptCompactionPolicy>,
    pub seed_transcript: Option<PathBuf>,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub result_dir: PathBuf,
    pub autonomy_profile: quorp_agent_core::AutonomyProfile,
    pub max_attempts: Option<usize>,
    pub allow_heavy_local_model: bool,
    pub condition: Option<String>,
    pub keep_sandbox: bool,
}

#[derive(Debug, Clone)]
pub struct BenchmarkResumeOptions {
    pub result_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkPromptBundle {
    resolved: ResolvedBenchmark,
    workspace_dir: PathBuf,
    objective_path: PathBuf,
    model_id: String,
    safety_mode_label: String,
    prompt: String,
    prompt_fingerprint: String,
    prompt_token_estimate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiBenchmarkLaunch {
    pub workspace_dir: PathBuf,
    pub objective_file: PathBuf,
    pub evaluate_command: Option<String>,
    pub objective_metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct BenchmarkBatchRunOptions {
    pub cases_root: PathBuf,
    pub result_dir: PathBuf,
    pub executor: BenchmarkExecutor,
    pub model_id: Option<String>,
    pub base_url_override: Option<String>,
    pub briefing_file: Option<PathBuf>,
    pub compaction_policy: Option<PromptCompactionPolicy>,
    pub seed_transcript: Option<PathBuf>,
    pub max_steps: usize,
    pub max_seconds: Option<u64>,
    pub max_total_tokens: Option<u64>,
    pub max_attempts: Option<usize>,
    pub autonomy_profile: quorp_agent_core::AutonomyProfile,
    pub allow_heavy_local_model: bool,
    pub condition: Option<String>,
    pub keep_sandbox: bool,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BenchmarkScoreOptions {
    pub run_dirs: Vec<PathBuf>,
    pub suite: String,
    pub reports_root: PathBuf,
    pub output_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BenchmarkScoreArtifacts {
    pub output_dir: PathBuf,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkManifest {
    resolved: ResolvedBenchmark,
    #[serde(default)]
    executor: BenchmarkExecutor,
    model_id: String,
    #[serde(default = "default_safe_mode_label")]
    safety_mode_label: String,
    #[serde(default)]
    scenario_label: Option<String>,
    base_url_override: Option<String>,
    briefing_file: Option<PathBuf>,
    #[serde(default)]
    compaction_policy: Option<PromptCompactionPolicy>,
    #[serde(default)]
    seed_transcript: Option<PathBuf>,
    max_steps: usize,
    max_seconds: Option<u64>,
    max_total_tokens: Option<u64>,
    autonomy_profile: String,
    max_attempts: usize,
    #[serde(default)]
    challenge: Option<ChallengeMetadata>,
    #[serde(default)]
    keep_sandbox: bool,
    #[serde(default)]
    completion_policy: quorp_agent_core::CompletionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolvedBenchmark {
    benchmark_root: PathBuf,
    issue_id: String,
    benchmark_name: String,
    issue_dir: Option<PathBuf>,
    workspace_source: PathBuf,
    objective_source: PathBuf,
    visible_evaluator: Option<PathBuf>,
    collector_evaluator: Option<PathBuf>,
    context_files: Vec<PathBuf>,
    repair_artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct WarposBenchmarkRootMarker {
    benchmark: Option<String>,
    issue: String,
    #[allow(dead_code)]
    condition: Option<String>,
    #[allow(dead_code)]
    suite: Option<String>,
    handoff_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChallengeManifest {
    id: String,
    title: String,
    difficulty: String,
    category: String,
    repo_condition: Vec<String>,
    objective_file: String,
    success_file: String,
    reset_command: String,
    evaluate_command: String,
    estimated_minutes: Option<u64>,
    expected_files_touched: Vec<String>,
    #[serde(default)]
    allowed_generated_files: Vec<String>,
    primary_metrics: Vec<String>,
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChallengeMetadata {
    case_root: PathBuf,
    sandbox_root: PathBuf,
    workspace_dir: PathBuf,
    condition: String,
    objective_file: PathBuf,
    success_file: PathBuf,
    #[serde(default)]
    reference_file: Option<PathBuf>,
    reset_command: String,
    evaluate_command: String,
    expected_files_touched: Vec<String>,
    #[serde(default)]
    allowed_generated_files: Vec<String>,
    primary_metrics: Vec<String>,
    tags: Vec<String>,
    capsule_file: PathBuf,
    #[serde(default)]
    capsule: ChallengeCapsule,
}

#[derive(Debug, Clone)]
struct ResolvedChallengeCase {
    case_root: PathBuf,
    manifest: ChallengeManifest,
    condition: String,
    objective_source: PathBuf,
    success_source: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ChallengeCapsule {
    #[serde(default)]
    case_class: String,
    #[serde(default)]
    owner_files: Vec<String>,
    #[serde(default)]
    first_reads: Vec<String>,
    #[serde(default)]
    fast_loop_commands: Vec<String>,
    #[serde(default)]
    expected_touch_targets: Vec<String>,
    #[serde(default)]
    companion_files_required: Vec<String>,
    #[serde(default)]
    strong_hints: Vec<String>,
    #[serde(default)]
    watch_points: Vec<String>,
    #[serde(default)]
    named_tests: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct RustSweCaseProfile {
    case_id: &'static str,
    fast_loop_commands: &'static [&'static str],
    final_eval_command: &'static str,
    likely_owner_files: &'static [&'static str],
    expected_touch_targets: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkBootstrapProgress {
    attempt: usize,
    bootstrap_phase: String,
    #[serde(default)]
    bootstrap_phase_detail: Option<String>,
    started_at_epoch_ms: u64,
    updated_at_epoch_ms: u64,
    #[serde(default)]
    first_task_model_request_seen: bool,
    #[serde(default)]
    bootstrap_elapsed_ms_before_first_task_request: Option<u64>,
    #[serde(default)]
    pre_model_bootstrap_stalled: bool,
    #[serde(default)]
    bootstrap_stall_class: Option<String>,
}

struct BenchmarkBootstrapTracker {
    root_progress_path: PathBuf,
    attempt_progress_path: PathBuf,
    attempt: usize,
    started_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvaluatorOutcome {
    name: String,
    script: PathBuf,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    duration_ms: u64,
    exit_code: i32,
    passed: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptReport {
    attempt: usize,
    #[serde(default)]
    executor: BenchmarkExecutor,
    #[serde(default)]
    model_id: String,
    #[serde(default = "default_safe_mode_label")]
    safety_mode_label: String,
    #[serde(default)]
    scenario_label: Option<String>,
    agent_stop_reason: quorp_agent_core::StopReason,
    agent_error_message: Option<String>,
    total_steps: usize,
    #[serde(default)]
    duration_ms: u64,
    total_billed_tokens: u64,
    #[serde(default)]
    max_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    max_completion_token_cap: Option<u32>,
    #[serde(default)]
    watchdog_near_limit: bool,
    #[serde(default)]
    watchdog_triggered: bool,
    visible_evaluation: Option<EvaluatorOutcome>,
    collector_evaluation: Option<EvaluatorOutcome>,
    evaluation: Option<EvaluatorOutcome>,
    changed_files: Vec<String>,
    #[serde(default)]
    ignored_changed_files: Vec<String>,
    validations: Vec<String>,
    widening_happened: bool,
    attempt_dir: PathBuf,
    workspace_dir: PathBuf,
    agent_result_dir: PathBuf,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    model_requests: usize,
    #[serde(default)]
    first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    first_model_turn_started: bool,
    #[serde(default)]
    first_action_emitted: bool,
    #[serde(default)]
    prompt_token_series_by_turn: Vec<PromptTokenTurnSample>,
    #[serde(default)]
    read_range_observations: Vec<ReadRangeObservation>,
    #[serde(default)]
    read_count: usize,
    #[serde(default)]
    write_count: usize,
    #[serde(default)]
    command_execution_count: usize,
    #[serde(default)]
    parser_recovery_count: usize,
    #[serde(default)]
    repair_invalid_action_streak_max: usize,
    #[serde(default)]
    repair_submode_entered: bool,
    #[serde(default)]
    repair_submode_turns: usize,
    #[serde(default)]
    repair_write_locked: bool,
    #[serde(default)]
    write_phase_action_refusal_count: usize,
    #[serde(default)]
    patch_scaffold_offered: bool,
    #[serde(default)]
    patch_scaffold_honored: bool,
    #[serde(default)]
    preview_apply_locked: bool,
    #[serde(default)]
    preview_apply_action_refusal_count: usize,
    #[serde(default)]
    write_phase_write_emitted: bool,
    #[serde(default)]
    bootstrap_phase: Option<String>,
    #[serde(default)]
    bootstrap_phase_detail: Option<String>,
    #[serde(default)]
    first_task_model_request_seen: bool,
    #[serde(default)]
    bootstrap_elapsed_ms_before_first_task_request: Option<u64>,
    #[serde(default)]
    pre_model_bootstrap_stalled: bool,
    #[serde(default)]
    bootstrap_stall_class: Option<String>,
    #[serde(default)]
    rolled_back_write_count: usize,
    #[serde(default)]
    rolled_back_non_support_edit_count: usize,
    #[serde(default)]
    soft_budget_inefficient: bool,
    #[serde(default)]
    fast_loop_command_seen: bool,
    #[serde(default)]
    agent_final_evaluate_command_seen: bool,
    #[serde(default)]
    final_evaluate_command_seen: bool,
    #[serde(default)]
    host_evaluation_commands_run: usize,
    #[serde(default)]
    non_support_edit_count: usize,
    #[serde(default)]
    repo_capsule_injected: bool,
    #[serde(default)]
    reasoning_enabled: bool,
    #[serde(default)]
    path_resolution_failures: usize,
    #[serde(default)]
    recovery_turns: usize,
    #[serde(default)]
    action_contract_mode: String,
    #[serde(default)]
    action_contract_selected: String,
    #[serde(default)]
    action_contract_fallback_reason: Option<String>,
    #[serde(default)]
    attempt_lineage: Vec<String>,
    #[serde(default)]
    effective_prompt_compaction_policy: Option<String>,
    #[serde(default)]
    fast_loop_validation_status: Option<String>,
    #[serde(default)]
    last_validation_failure: Option<String>,
    #[serde(default)]
    failing_test_names: Vec<String>,
    #[serde(default)]
    primary_failure_test_name: Option<String>,
    #[serde(default)]
    primary_failure_path: Option<String>,
    #[serde(default)]
    primary_failure_line: Option<usize>,
    #[serde(default)]
    assertion_excerpt: Option<String>,
    #[serde(default)]
    diagnostic_class: Option<String>,
    #[serde(default)]
    implementation_target_lease: Option<String>,
    #[serde(default)]
    dependency_candidates: Vec<String>,
    #[serde(default)]
    target_dependency_table: Option<String>,
    #[serde(default)]
    repair_required: bool,
    #[serde(default)]
    repair_phase_terminal: Option<String>,
    #[serde(default)]
    failure_anchor_reread_attempted: bool,
    #[serde(default)]
    failure_anchor_reread_honored: bool,
    #[serde(default)]
    implementation_reread_allowed: bool,
    #[serde(default)]
    implementation_reread_attempted: bool,
    #[serde(default)]
    implementation_reread_honored: bool,
    #[serde(default)]
    repair_phase_invalid_action_count: usize,
    #[serde(default)]
    post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    patch_packet_injected: bool,
    #[serde(default)]
    patch_packet_honored_range: Option<String>,
    #[serde(default)]
    recommended_rerun_command: Option<String>,
    #[serde(default)]
    fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    failed_edit_records: Vec<quorp_agent_core::FailedEditRecord>,
    #[serde(default)]
    local_model_memory: quorp_agent_core::LocalModelMemory,
    #[serde(default)]
    local_agent_scorecard: quorp_agent_core::LocalAgentScorecard,
    #[serde(default)]
    planner_model: Option<String>,
    #[serde(default)]
    executor_model: Option<String>,
    #[serde(default)]
    judge: Option<ChallengeJudgeOutcome>,
    #[serde(default)]
    routing: crate::quorp::agent_local::RoutingSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkReport {
    benchmark_name: String,
    issue_id: String,
    #[serde(default)]
    executor: BenchmarkExecutor,
    #[serde(default)]
    model_id: String,
    #[serde(default = "default_safe_mode_label")]
    safety_mode_label: String,
    #[serde(default)]
    scenario_label: Option<String>,
    #[serde(default)]
    provider_kind: String,
    #[serde(default)]
    provider_base_url: Option<String>,
    #[serde(default)]
    auth_mode: String,
    #[serde(default)]
    usage_source: String,
    #[serde(default)]
    proxy_visible_remote_egress_expected: bool,
    #[serde(default)]
    routing_mode: Option<String>,
    #[serde(default)]
    requested_provider: Option<String>,
    #[serde(default)]
    requested_model: Option<String>,
    #[serde(default)]
    candidate_models: Vec<String>,
    #[serde(default)]
    effective_provider: Option<String>,
    #[serde(default)]
    effective_model: Option<String>,
    #[serde(default)]
    used_local_fallback: bool,
    #[serde(default)]
    fallback_reason: Option<String>,
    #[serde(default)]
    comparable_run: Option<bool>,
    #[serde(default)]
    provider_request_id: Option<String>,
    #[serde(default)]
    routing_status: Option<String>,
    success: bool,
    attempts_run: usize,
    max_attempts: usize,
    total_billed_tokens: u64,
    #[serde(default)]
    wall_clock_ms: u64,
    max_total_tokens: Option<u64>,
    #[serde(default)]
    max_prompt_token_estimate_seen: Option<u64>,
    #[serde(default)]
    max_completion_token_cap_seen: Option<u32>,
    #[serde(default)]
    watchdog_near_limit: bool,
    #[serde(default)]
    watchdog_triggered: bool,
    final_stop_reason: Option<quorp_agent_core::StopReason>,
    changed_files: Vec<String>,
    #[serde(default)]
    ignored_changed_files: Vec<String>,
    widening_happened: bool,
    attempts: Vec<AttemptReport>,
    #[serde(default)]
    reset_outcome: Option<EvaluatorOutcome>,
    #[serde(default)]
    challenge: Option<ChallengeMetadata>,
    #[serde(default)]
    run_dir: PathBuf,
    #[serde(default)]
    sandbox_root: Option<PathBuf>,
    #[serde(default)]
    exit_code: i32,
    #[serde(default)]
    lines_added: u64,
    #[serde(default)]
    lines_removed: u64,
    #[serde(default)]
    mistakes_corrected: usize,
    #[serde(default)]
    validation_commands_run: usize,
    #[serde(default)]
    evaluation_commands_run: usize,
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    run_error: Option<String>,
    #[serde(default)]
    setup_failure_class: Option<String>,
    #[serde(default)]
    total_requests: usize,
    #[serde(default)]
    task_model_call_count: usize,
    #[serde(default)]
    tool_call_count: usize,
    #[serde(default)]
    edit_count: usize,
    #[serde(default)]
    read_count: usize,
    #[serde(default)]
    write_count: usize,
    #[serde(default)]
    command_execution_count: usize,
    #[serde(default)]
    parser_recovery_count: usize,
    #[serde(default)]
    repair_invalid_action_streak_max: usize,
    #[serde(default)]
    repair_submode_entered: bool,
    #[serde(default)]
    repair_submode_turns: usize,
    #[serde(default)]
    repair_write_locked: bool,
    #[serde(default)]
    write_phase_action_refusal_count: usize,
    #[serde(default)]
    patch_scaffold_offered: bool,
    #[serde(default)]
    patch_scaffold_honored: bool,
    #[serde(default)]
    preview_apply_locked: bool,
    #[serde(default)]
    preview_apply_action_refusal_count: usize,
    #[serde(default)]
    write_phase_write_emitted: bool,
    #[serde(default)]
    bootstrap_phase: Option<String>,
    #[serde(default)]
    bootstrap_phase_detail: Option<String>,
    #[serde(default)]
    first_task_model_request_seen: bool,
    #[serde(default)]
    bootstrap_elapsed_ms_before_first_task_request: Option<u64>,
    #[serde(default)]
    pre_model_bootstrap_stalled: bool,
    #[serde(default)]
    bootstrap_stall_class: Option<String>,
    #[serde(default)]
    rolled_back_write_count: usize,
    #[serde(default)]
    rolled_back_non_support_edit_count: usize,
    #[serde(default)]
    soft_budget_inefficient: bool,
    #[serde(default)]
    fast_loop_command_seen: bool,
    #[serde(default)]
    agent_final_evaluate_command_seen: bool,
    #[serde(default)]
    final_evaluate_command_seen: bool,
    #[serde(default)]
    host_evaluation_commands_run: usize,
    #[serde(default)]
    non_support_edit_count: usize,
    #[serde(default)]
    last_failure_class: Option<String>,
    #[serde(default)]
    evaluation_command_seen: bool,
    #[serde(default)]
    text_only_action_failure: bool,
    #[serde(default)]
    first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    first_model_turn_started: bool,
    #[serde(default)]
    first_action_emitted: bool,
    #[serde(default)]
    prompt_token_series_by_turn: Vec<PromptTokenTurnSample>,
    #[serde(default)]
    read_range_observations: Vec<ReadRangeObservation>,
    #[serde(default)]
    repo_capsule_injected: bool,
    #[serde(default)]
    reasoning_enabled: bool,
    #[serde(default)]
    path_resolution_failures: usize,
    #[serde(default)]
    recovery_turns: usize,
    #[serde(default)]
    action_contract_mode: String,
    #[serde(default)]
    action_contract_selected: String,
    #[serde(default)]
    action_contract_fallback_reason: Option<String>,
    #[serde(default)]
    attempt_lineage: Vec<String>,
    #[serde(default)]
    effective_prompt_compaction_policy: Option<String>,
    #[serde(default)]
    fast_loop_validation_status: Option<String>,
    #[serde(default)]
    last_validation_failure: Option<String>,
    #[serde(default)]
    failing_test_names: Vec<String>,
    #[serde(default)]
    primary_failure_test_name: Option<String>,
    #[serde(default)]
    primary_failure_path: Option<String>,
    #[serde(default)]
    primary_failure_line: Option<usize>,
    #[serde(default)]
    assertion_excerpt: Option<String>,
    #[serde(default)]
    diagnostic_class: Option<String>,
    #[serde(default)]
    implementation_target_lease: Option<String>,
    #[serde(default)]
    dependency_candidates: Vec<String>,
    #[serde(default)]
    target_dependency_table: Option<String>,
    #[serde(default)]
    repair_required: bool,
    #[serde(default)]
    repair_phase_terminal: Option<String>,
    #[serde(default)]
    failure_anchor_reread_attempted: bool,
    #[serde(default)]
    failure_anchor_reread_honored: bool,
    #[serde(default)]
    implementation_reread_allowed: bool,
    #[serde(default)]
    implementation_reread_attempted: bool,
    #[serde(default)]
    implementation_reread_honored: bool,
    #[serde(default)]
    repair_phase_invalid_action_count: usize,
    #[serde(default)]
    post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    patch_packet_injected: bool,
    #[serde(default)]
    patch_packet_honored_range: Option<String>,
    #[serde(default)]
    recommended_rerun_command: Option<String>,
    #[serde(default)]
    fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    failed_edit_records: Vec<quorp_agent_core::FailedEditRecord>,
    #[serde(default)]
    local_model_memory: quorp_agent_core::LocalModelMemory,
    #[serde(default)]
    local_agent_scorecard: quorp_agent_core::LocalAgentScorecard,
    #[serde(default)]
    preview_edit_count: usize,
    #[serde(default)]
    preview_edit_success_count: usize,
    #[serde(default)]
    preview_created_count: usize,
    #[serde(default)]
    replace_range_count: usize,
    #[serde(default)]
    replace_range_hash_mismatch_count: usize,
    #[serde(default)]
    modify_toml_count: usize,
    #[serde(default)]
    apply_preview_count: usize,
    #[serde(default)]
    apply_preview_hash_mismatch_count: usize,
    #[serde(default)]
    syntax_preview_count: usize,
    #[serde(default)]
    syntax_preview_failure_count: usize,
    #[serde(default)]
    target_redirect_count: usize,
    #[serde(default)]
    evidence_file_fixation_count: usize,
    #[serde(default)]
    local_agent_final_failure_classification: Option<String>,
    #[serde(default)]
    planner_model: Option<String>,
    #[serde(default)]
    executor_model: Option<String>,
    #[serde(default)]
    deterministic_evaluation_passed: Option<bool>,
    #[serde(default)]
    judge: Option<ChallengeJudgeOutcome>,
    #[serde(default)]
    primary_failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChallengeJudgeOutcome {
    passed: bool,
    summary: String,
    rationale: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    raw_response: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptTokenTurnSample {
    step: usize,
    prompt_token_estimate: u64,
    #[serde(default)]
    raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    completion_token_cap: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReadRangeObservation {
    path: String,
    #[serde(default)]
    requested_range: Option<String>,
    #[serde(default)]
    honored_range: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ActionEvidence {
    read_count: usize,
    write_count: usize,
    command_execution_count: usize,
    fast_loop_command_seen: bool,
    final_evaluate_command_seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CheckpointValidationState {
    #[serde(default)]
    validation_status: Option<String>,
    #[serde(default)]
    last_validation_failure: Option<String>,
    #[serde(default)]
    failing_test_names: Vec<String>,
    #[serde(default)]
    primary_failure_test_name: Option<String>,
    #[serde(default)]
    primary_failure_path: Option<String>,
    #[serde(default)]
    primary_failure_line: Option<usize>,
    #[serde(default)]
    assertion_excerpt: Option<String>,
    #[serde(default)]
    diagnostic_class: Option<String>,
    #[serde(default)]
    implementation_target_lease: Option<String>,
    #[serde(default)]
    dependency_candidates: Vec<String>,
    #[serde(default)]
    target_dependency_table: Option<String>,
    #[serde(default)]
    repair_required: bool,
    #[serde(default)]
    repair_phase_terminal: Option<String>,
    #[serde(default)]
    failure_anchor_reread_attempted: bool,
    #[serde(default)]
    failure_anchor_reread_honored: bool,
    #[serde(default)]
    implementation_reread_allowed: bool,
    #[serde(default)]
    implementation_reread_attempted: bool,
    #[serde(default)]
    implementation_reread_honored: bool,
    #[serde(default)]
    repair_phase_invalid_action_count: usize,
    #[serde(default)]
    post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    patch_packet_injected: bool,
    #[serde(default)]
    patch_packet_honored_range: Option<String>,
    #[serde(default)]
    recommended_rerun_command: Option<String>,
    #[serde(default)]
    fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    failed_edit_records: Vec<quorp_agent_core::FailedEditRecord>,
    #[serde(default)]
    local_model_memory: quorp_agent_core::LocalModelMemory,
    #[serde(default)]
    local_agent_scorecard: quorp_agent_core::LocalAgentScorecard,
}

#[derive(Debug, Clone)]
struct BenchmarkProviderSummary {
    provider_kind: String,
    provider_base_url: Option<String>,
    auth_mode: String,
    usage_source: String,
    proxy_visible_remote_egress_expected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchCaseReport {
    case_id: String,
    case_root: PathBuf,
    objective_path: PathBuf,
    result_dir: PathBuf,
    log_file: PathBuf,
    #[serde(default)]
    executor: BenchmarkExecutor,
    success: bool,
    exit_code: i32,
    wall_clock_ms: u64,
    total_requests: usize,
    total_billed_tokens: u64,
    lines_added: u64,
    lines_removed: u64,
    mistakes_corrected: usize,
    judge_passed: Option<bool>,
    deterministic_evaluation_passed: Option<bool>,
    first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    first_request_compacted_prompt_token_estimate: Option<u64>,
    first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    first_model_turn_started: bool,
    #[serde(default)]
    first_action_emitted: bool,
    final_stop_reason: Option<quorp_agent_core::StopReason>,
    primary_failure: Option<String>,
    #[serde(default)]
    local_agent_final_failure_classification: Option<String>,
    #[serde(default)]
    adaptive_action_mode_retry: bool,
    report_path: PathBuf,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchReport {
    cases_root: PathBuf,
    result_dir: PathBuf,
    cases: Vec<BatchCaseReport>,
    total_requests: usize,
    total_billed_tokens: u64,
    lines_added: u64,
    lines_removed: u64,
    mistakes_corrected: usize,
    successful_cases: usize,
    failed_cases: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunSummaryCase {
    case_id: String,
    success: bool,
    primary_failure: Option<String>,
    local_agent_final_failure_classification: Option<String>,
    final_stop_reason: Option<quorp_agent_core::StopReason>,
    first_valid_write_step: Option<usize>,
    parser_recovery_count: usize,
    redundant_read_count: usize,
    rejected_validation_alias_count: usize,
    target_redirect_count: usize,
    syntax_preview_failure_count: usize,
    adaptive_action_mode_retry: bool,
    report_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunSummary {
    cases_root: PathBuf,
    result_dir: PathBuf,
    cases_run: usize,
    successful_cases: usize,
    failed_cases: usize,
    total_requests: usize,
    total_billed_tokens: u64,
    cases: Vec<RunSummaryCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkScoreReport {
    suite: String,
    generated_at_unix_seconds: u64,
    output_dir: PathBuf,
    run_dirs: Vec<PathBuf>,
    total_cases: usize,
    solved_cases: usize,
    valid_write_cases: usize,
    post_write_validation_cases: usize,
    diagnostic_classified_cases: usize,
    tooling_healthy_cases: usize,
    total_requests: usize,
    total_billed_tokens: u64,
    common_blocker: Option<String>,
    blocker_counts: BTreeMap<String, usize>,
    regressions: Vec<String>,
    cases: Vec<BenchmarkScoreCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkScoreCase {
    case_id: String,
    success: bool,
    progress_score: u8,
    progress_phase: String,
    failure_classification: String,
    primary_failure: Option<String>,
    model_id: Option<String>,
    executor: Option<String>,
    provider_base_url: Option<String>,
    action_contract_selected: Option<String>,
    result_dir: PathBuf,
    report_path: PathBuf,
    first_model_turn_started: bool,
    first_action_emitted: bool,
    diagnostic_class: Option<String>,
    implementation_target_lease: Option<String>,
    first_valid_write_step: Option<usize>,
    post_write_validation: bool,
    parser_recovery_count: usize,
    redundant_read_count: usize,
    rejected_validation_alias_count: usize,
    target_redirect_count: usize,
    syntax_preview_failure_count: usize,
    preview_created_count: usize,
    modify_toml_count: usize,
    replace_range_count: usize,
    apply_preview_count: usize,
    wall_clock_ms: u64,
    total_requests: usize,
    total_billed_tokens: u64,
    lines_added: u64,
    lines_removed: u64,
    general_tooling_gap: Option<String>,
}

struct PreparedBatchRuntime {
    base_url_override: Option<String>,
    stop_after_batch: bool,
}

#[derive(Debug, Clone, Copy)]
struct SingleCaseActionContractOverride {
    native_tool_calls: Option<bool>,
    fallback_reason: Option<&'static str>,
    attempt_lineage: &'static [&'static str],
}

impl SingleCaseActionContractOverride {
    const fn none() -> Self {
        Self {
            native_tool_calls: None,
            fallback_reason: None,
            attempt_lineage: &[],
        }
    }

    const fn json_preselected() -> Self {
        Self {
            native_tool_calls: Some(false),
            fallback_reason: Some("batch_native_schema_failures"),
            attempt_lineage: &["json_actions_preselected"],
        }
    }

    const fn json_retry() -> Self {
        Self {
            native_tool_calls: Some(false),
            fallback_reason: Some("native_schema_failure_before_write"),
            attempt_lineage: &["native_tool_calls_v1", "json_actions_retry"],
        }
    }
}

struct ChallengeJudgeContext<'a> {
    manifest: &'a BenchmarkManifest,
    metadata: &'a ChallengeMetadata,
    attempt_number: usize,
    attempt_dir: &'a Path,
    outcome: &'a quorp_agent_core::AgentRunOutcome,
    evaluation: &'a EvaluatorOutcome,
    changed_files: &'a [String],
    validations: &'a [String],
    metrics: &'a RequestMetricsSummary,
    usage: &'a crate::quorp::agent_local::HeadlessUsageSummary,
}

#[derive(Debug, Clone)]
struct SynthesizedObjective {
    path: PathBuf,
    prompt_token_estimate: u64,
}

#[derive(Debug, Clone, Default)]
struct RequestMetricsSummary {
    max_prompt_token_estimate: Option<u64>,
    max_completion_token_cap: Option<u32>,
    watchdog_near_limit: bool,
    watchdog_triggered: bool,
    first_request_prompt_token_estimate: Option<u64>,
    first_request_raw_prompt_token_estimate: Option<u64>,
    first_request_compacted_prompt_token_estimate: Option<u64>,
    first_request_first_token_latency_ms: Option<u64>,
    first_model_turn_started: bool,
    first_action_emitted: bool,
    prompt_token_series_by_turn: Vec<PromptTokenTurnSample>,
}

#[derive(Debug, Clone, Default)]
struct ControlLoopSummary {
    path_resolution_failures: usize,
    recovery_turns: usize,
}

#[derive(Debug)]
struct BenchmarkRunLock {
    path: PathBuf,
}

pub fn run_benchmark(options: BenchmarkRunOptions) -> anyhow::Result<()> {
    let _run_lock = BenchmarkRunLock::acquire()?;
    fs::create_dir_all(&options.result_dir)?;
    if let Some(challenge) = resolve_challenge_case(&options.path, options.condition.as_deref())? {
        return run_challenge_benchmark(&options, challenge);
    }

    let resolved = resolve_benchmark(&options.path)?;
    let model_id = resolve_benchmark_model_id(options.executor, options.model_id.clone())?;
    if options.executor == BenchmarkExecutor::Native {
        ensure_safe_local_model_selection(
            &model_id,
            allow_resolved_benchmark_model_without_opt_in(
                options.model_id.as_deref(),
                &model_id,
                options.allow_heavy_local_model,
            ),
        )?;
    }
    let safety_mode_label = benchmark_safety_mode_label(options.executor, &model_id);
    let scenario_label = Some(crate::quorp::provider_config::resolved_scenario_label());
    let mut completion_policy =
        benchmark_completion_policy(options.executor, &safety_mode_label, Some(&model_id));
    apply_requested_prompt_compaction_override(&mut completion_policy, options.compaction_policy);
    let manifest = BenchmarkManifest {
        resolved,
        executor: options.executor,
        model_id,
        safety_mode_label,
        scenario_label,
        base_url_override: base_url_override_for_executor(
            options.executor,
            options.base_url_override,
        ),
        briefing_file: options.briefing_file,
        compaction_policy: completion_policy.prompt_compaction_policy,
        seed_transcript: options.seed_transcript,
        max_steps: options.max_steps,
        max_seconds: options.max_seconds,
        max_total_tokens: options.max_total_tokens,
        autonomy_profile: options.autonomy_profile.label().to_string(),
        max_attempts: options.max_attempts.unwrap_or(3).max(1),
        challenge: None,
        keep_sandbox: false,
        completion_policy,
    };
    write_json(
        &options.result_dir.join("benchmark-manifest.json"),
        &manifest,
    )?;
    run_benchmark_from_manifest(&manifest, &options.result_dir, 1)
}

pub fn resume_benchmark(options: BenchmarkResumeOptions) -> anyhow::Result<()> {
    let _run_lock = BenchmarkRunLock::acquire()?;
    let manifest_path = options.result_dir.join("benchmark-manifest.json");
    let mut manifest: BenchmarkManifest = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    normalize_manifest_paths_for_runtime(&mut manifest, &options.result_dir);

    let next_attempt = discover_completed_attempts(&options.result_dir)? + 1;
    let previous_attempt_dir = attempt_dir(&options.result_dir, next_attempt.saturating_sub(1));
    let previous_agent_dir = previous_attempt_dir.join("agent");
    if previous_agent_dir.join("checkpoint.json").exists()
        && previous_agent_dir.join("summary.json").exists()
    {
        let summary_path = previous_agent_dir.join("summary.json");
        let previous_stop_reason = fs::read_to_string(&summary_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|summary| {
                summary
                    .get("stop_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        if previous_stop_reason.as_deref() == Some("success") {
            log_phase(
                "resume",
                ANSI_GREEN,
                format!(
                    "latest attempt already completed successfully at {}",
                    previous_agent_dir.display()
                ),
            );
            return Ok(());
        }
        log_phase(
            "resume",
            ANSI_CYAN,
            format!(
                "resuming latest incomplete agent attempt from {}",
                previous_agent_dir.display()
            ),
        );
        let outcome = resume_headless_agent(previous_agent_dir.clone())?;
        let attempt_report = finalize_attempt(
            &manifest,
            next_attempt.saturating_sub(1),
            &previous_attempt_dir,
            outcome,
        )?;
        let mut attempts = load_existing_attempts(&options.result_dir)?;
        if let Some(index) = attempts
            .iter()
            .position(|attempt| attempt.attempt == attempt_report.attempt)
        {
            attempts[index] = attempt_report;
        } else {
            attempts.push(attempt_report);
        }
        maybe_continue_attempts(&manifest, &options.result_dir, attempts, next_attempt)?;
        return Ok(());
    }

    let existing_attempts = load_existing_attempts(&options.result_dir)?;
    maybe_continue_attempts(
        &manifest,
        &options.result_dir,
        existing_attempts,
        next_attempt,
    )
}

fn normalize_manifest_paths_for_runtime(manifest: &mut BenchmarkManifest, result_dir: &Path) {
    let in_docker = std::env::var("QUORP_IN_DOCKER")
        .ok()
        .is_some_and(|value| value == "1");
    if !in_docker {
        return;
    }
    let host_result_dir = std::env::var("QUORP_DOCKER_HOST_RESULT_DIR")
        .ok()
        .map(PathBuf::from);
    let host_workspace_root = std::env::var("QUORP_DOCKER_HOST_WORKSPACE_ROOT")
        .ok()
        .map(PathBuf::from);
    let container_workspace_root = std::env::var("QUORP_DOCKER_CONTAINER_WORKSPACE_ROOT")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::quorp::docker::CONTAINER_WORKSPACE_ROOT));

    manifest.resolved.benchmark_root = normalize_manifest_path(
        &manifest.resolved.benchmark_root,
        host_result_dir.as_deref(),
        result_dir,
        host_workspace_root.as_deref(),
        &container_workspace_root,
    );
    manifest.resolved.workspace_source = normalize_manifest_path(
        &manifest.resolved.workspace_source,
        host_result_dir.as_deref(),
        result_dir,
        host_workspace_root.as_deref(),
        &container_workspace_root,
    );
    manifest.resolved.objective_source = normalize_manifest_path(
        &manifest.resolved.objective_source,
        host_result_dir.as_deref(),
        result_dir,
        host_workspace_root.as_deref(),
        &container_workspace_root,
    );
    manifest.briefing_file = manifest.briefing_file.as_ref().map(|path| {
        normalize_manifest_path(
            path,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        )
    });
    manifest.resolved.visible_evaluator =
        manifest.resolved.visible_evaluator.as_ref().map(|path| {
            normalize_manifest_path(
                path,
                host_result_dir.as_deref(),
                result_dir,
                host_workspace_root.as_deref(),
                &container_workspace_root,
            )
        });
    manifest.resolved.collector_evaluator =
        manifest.resolved.collector_evaluator.as_ref().map(|path| {
            normalize_manifest_path(
                path,
                host_result_dir.as_deref(),
                result_dir,
                host_workspace_root.as_deref(),
                &container_workspace_root,
            )
        });
    manifest.resolved.context_files = manifest
        .resolved
        .context_files
        .iter()
        .map(|path| {
            normalize_manifest_path(
                path,
                host_result_dir.as_deref(),
                result_dir,
                host_workspace_root.as_deref(),
                &container_workspace_root,
            )
        })
        .collect();
    manifest.resolved.repair_artifacts = manifest
        .resolved
        .repair_artifacts
        .iter()
        .map(|path| {
            normalize_manifest_path(
                path,
                host_result_dir.as_deref(),
                result_dir,
                host_workspace_root.as_deref(),
                &container_workspace_root,
            )
        })
        .collect();
    if let Some(challenge) = manifest.challenge.as_mut() {
        challenge.sandbox_root = normalize_manifest_path(
            &challenge.sandbox_root,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        );
        challenge.workspace_dir = normalize_manifest_path(
            &challenge.workspace_dir,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        );
        challenge.objective_file = normalize_manifest_path(
            &challenge.objective_file,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        );
        challenge.success_file = normalize_manifest_path(
            &challenge.success_file,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        );
        challenge.reference_file = challenge.reference_file.as_ref().map(|path| {
            normalize_manifest_path(
                path,
                host_result_dir.as_deref(),
                result_dir,
                host_workspace_root.as_deref(),
                &container_workspace_root,
            )
        });
        challenge.capsule_file = normalize_manifest_path(
            &challenge.capsule_file,
            host_result_dir.as_deref(),
            result_dir,
            host_workspace_root.as_deref(),
            &container_workspace_root,
        );
    }
}

fn normalize_manifest_path(
    path: &Path,
    host_result_dir: Option<&Path>,
    result_dir: &Path,
    host_workspace_root: Option<&Path>,
    container_workspace_root: &Path,
) -> PathBuf {
    if let Some(host_result_dir) = host_result_dir
        && let Ok(relative) = path.strip_prefix(host_result_dir)
    {
        return result_dir.join(relative);
    }
    if let Some(host_workspace_root) = host_workspace_root
        && let Ok(relative) = path.strip_prefix(host_workspace_root)
    {
        return container_workspace_root.join(relative);
    }
    path.to_path_buf()
}

pub fn parse_prompt_compaction_policy(
    value: Option<&str>,
) -> anyhow::Result<Option<PromptCompactionPolicy>> {
    let Some(value) = value else {
        return Ok(None);
    };
    PromptCompactionPolicy::parse(value)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("unknown compaction policy `{value}`"))
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_benchmark_prompt_bundle(
    path: &Path,
    workspace_dir: &Path,
    executor: BenchmarkExecutor,
    model_id: Option<String>,
    briefing_file: Option<&Path>,
    max_steps: usize,
    max_seconds: Option<u64>,
    max_total_tokens: Option<u64>,
) -> anyhow::Result<BenchmarkPromptBundle> {
    let resolved = resolve_benchmark(path)?;
    let model_id = resolve_benchmark_model_id(executor, model_id)?;
    let safety_mode_label = benchmark_safety_mode_label(executor, &model_id);
    prepare_attempt_workspace(&resolved, workspace_dir)?;
    let helper_briefing = load_benchmark_briefing(briefing_file, &resolved.issue_id)?;
    let objective = synthesize_objective(
        &resolved,
        workspace_dir,
        &safety_mode_label,
        helper_briefing.as_deref(),
    )?;
    let prompt_bundle = crate::quorp::codex_executor::build_benchmark_prompt_bundle(
        workspace_dir,
        &objective.path,
        max_steps,
        max_seconds,
        max_total_tokens,
    )?;
    Ok(BenchmarkPromptBundle {
        resolved,
        workspace_dir: workspace_dir.to_path_buf(),
        objective_path: objective.path,
        model_id,
        safety_mode_label,
        prompt: prompt_bundle.prompt,
        prompt_fingerprint: prompt_bundle.prompt_fingerprint,
        prompt_token_estimate: prompt_bundle.prompt_token_estimate,
    })
}

fn tui_workspace_entries(path: &Path) -> Vec<String> {
    match fs::read_dir(path) {
        Ok(entries) => {
            let mut items = entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| {
                    let file_name = entry.file_name().into_string().ok()?;
                    let metadata = entry.metadata().ok()?;
                    Some(if metadata.is_dir() {
                        format!("{file_name}/")
                    } else {
                        file_name
                    })
                })
                .collect::<Vec<_>>();
            items.sort();
            items.truncate(24);
            items
        }
        Err(_) => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_tui_benchmark_launch(
    path: &Path,
    result_dir: &Path,
    executor: BenchmarkExecutor,
    model_id: Option<String>,
    briefing_file: Option<&Path>,
    max_steps: usize,
    max_seconds: Option<u64>,
    max_total_tokens: Option<u64>,
) -> anyhow::Result<TuiBenchmarkLaunch> {
    if let Some(challenge) = resolve_challenge_case(path, None)? {
        let prepared = prepare_challenge_run(result_dir, &challenge)?;
        let evaluate_command = Some(substitute_condition(
            &prepared.challenge_metadata.evaluate_command,
            &prepared.challenge_metadata.condition,
        ));
        let workspace_dir = prepared.challenge_metadata.workspace_dir.clone();
        let objective_file = prepared.challenge_metadata.objective_file.clone();
        let objective_metadata = serde_json::json!({
            "workspace_root": workspace_dir,
            "challenge_root": prepared.challenge_metadata.sandbox_root,
            "editable_workspace_root": prepared.challenge_metadata.workspace_dir,
            "editable_workspace_relative_root": serde_json::Value::Null,
            "objective_file": objective_file,
            "evaluate_command": evaluate_command,
            "reset_command": substitute_condition(
                &prepared.challenge_metadata.reset_command,
                &prepared.challenge_metadata.condition,
            ),
            "selected_condition": prepared.challenge_metadata.condition,
            "success_file": prepared.challenge_metadata.success_file,
            "context_files": prepared.resolved.context_files,
            "repair_artifacts": prepared.resolved.repair_artifacts,
            "workspace_root_entries": tui_workspace_entries(&prepared.challenge_metadata.workspace_dir),
            "editable_workspace_entries": tui_workspace_entries(&prepared.challenge_metadata.workspace_dir),
            "benchmark_root": prepared.resolved.benchmark_root,
            "benchmark_issue_id": prepared.resolved.issue_id,
            "benchmark_name": prepared.resolved.benchmark_name,
            "expected_files_touched": prepared.challenge_metadata.expected_files_touched,
            "primary_metrics": prepared.challenge_metadata.primary_metrics,
            "tags": prepared.challenge_metadata.tags,
            "warpos_capture_scope": "benchmark_task",
            "warpos_capture_call_class": "task_model_call",
        });
        return Ok(TuiBenchmarkLaunch {
            workspace_dir,
            objective_file,
            evaluate_command,
            objective_metadata,
        });
    }

    let workspace_dir = result_dir.join("workspace");
    let bundle = prepare_benchmark_prompt_bundle(
        path,
        &workspace_dir,
        executor,
        model_id,
        briefing_file,
        max_steps,
        max_seconds,
        max_total_tokens,
    )?;
    let evaluate_command = bundle
        .resolved
        .visible_evaluator
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(|name| format!("./{name}"));
    let objective_metadata = serde_json::json!({
        "workspace_root": bundle.workspace_dir,
        "challenge_root": bundle.workspace_dir,
        "editable_workspace_root": bundle.workspace_dir,
        "editable_workspace_relative_root": serde_json::Value::Null,
        "objective_file": bundle.objective_path,
        "evaluate_command": evaluate_command,
        "reset_command": serde_json::Value::Null,
        "selected_condition": serde_json::Value::Null,
        "success_file": serde_json::Value::Null,
        "context_files": bundle
            .resolved
            .context_files
            .iter()
            .map(|path| rebase_attempt_path(&bundle.resolved, &bundle.workspace_dir, path))
            .collect::<Vec<_>>(),
        "repair_artifacts": bundle
            .resolved
            .repair_artifacts
            .iter()
            .map(|path| rebase_attempt_path(&bundle.resolved, &bundle.workspace_dir, path))
            .collect::<Vec<_>>(),
        "workspace_root_entries": tui_workspace_entries(&bundle.workspace_dir),
        "editable_workspace_entries": tui_workspace_entries(&bundle.workspace_dir),
        "benchmark_root": bundle.resolved.benchmark_root,
        "benchmark_issue_id": bundle.resolved.issue_id,
        "benchmark_name": bundle.resolved.benchmark_name,
        "warpos_capture_scope": "benchmark_task",
        "warpos_capture_call_class": "task_model_call",
    });
    Ok(TuiBenchmarkLaunch {
        workspace_dir: bundle.workspace_dir,
        objective_file: bundle.objective_path,
        evaluate_command,
        objective_metadata,
    })
}

pub fn run_benchmark_batch(options: BenchmarkBatchRunOptions) -> anyhow::Result<()> {
    fs::create_dir_all(&options.result_dir)?;
    let case_roots = discover_challenge_case_roots(&options.cases_root)?;
    if case_roots.is_empty() {
        anyhow::bail!(
            "no challenge cases were found under {}",
            options.cases_root.display()
        );
    }

    let mut batch_cases = Vec::new();
    let mut native_schema_failure_count = 0usize;
    let batch_started_at = std::time::Instant::now();
    let prepared_runtime = prepare_batch_runtime(&options)?;
    write_batch_summary_artifacts(
        &options,
        &batch_cases,
        batch_started_at.elapsed().as_millis() as u64,
    )?;
    for case_root in case_roots {
        let case_manifest_path = case_root.join("benchmark.json");
        let case_manifest: ChallengeManifest = serde_json::from_str(
            &fs::read_to_string(&case_manifest_path)
                .with_context(|| format!("failed to read {}", case_manifest_path.display()))?,
        )
        .with_context(|| format!("failed to parse {}", case_manifest_path.display()))?;
        let objective_path = case_root.join(&case_manifest.objective_file);
        let case_id = case_manifest.id.clone();
        let case_result_dir = options.result_dir.join(&case_id);
        let case_log_dir = options
            .log_dir
            .clone()
            .unwrap_or_else(|| options.result_dir.join("logs"));
        fs::create_dir_all(&case_log_dir)?;
        let case_log_file = case_log_dir.join(format!("{case_id}.log"));
        if case_result_dir.exists() {
            fs::remove_dir_all(&case_result_dir)
                .with_context(|| format!("failed to clear {}", case_result_dir.display()))?;
        }
        fs::create_dir_all(&case_result_dir)?;

        log_phase(
            "batch",
            ANSI_CYAN,
            format!("running case {} from {}", case_id, objective_path.display()),
        );
        let preselect_json_actions =
            options.executor == BenchmarkExecutor::Native && native_schema_failure_count >= 2;
        if preselect_json_actions {
            log_phase(
                "batch",
                ANSI_YELLOW,
                format!(
                    "case {case_id} starting in JSON-envelope action mode after repeated native schema failures"
                ),
            );
        }

        let launch_result = launch_single_case_run(
            &case_root,
            &objective_path,
            &case_result_dir,
            &case_log_file,
            &options,
            prepared_runtime.base_url_override.as_deref(),
            if preselect_json_actions {
                SingleCaseActionContractOverride::json_preselected()
            } else {
                SingleCaseActionContractOverride::none()
            },
        );

        let mut selected_result_dir = case_result_dir.clone();
        let mut selected_log_file = case_log_file.clone();
        let mut report_path = selected_result_dir.join("benchmark-report.json");
        let mut error = None;
        let mut report_summary = None;
        let mut adaptive_action_mode_retry = false;
        let mut status_code = -1;
        match launch_result {
            Ok(status) => {
                status_code = status.code().unwrap_or(-1);
            }
            Err(launch_error) => {
                error = Some(launch_error.to_string());
            }
        }
        if report_path.exists() {
            match read_benchmark_report(&report_path) {
                Ok(report) => report_summary = Some(report),
                Err(read_error) => error = Some(read_error.to_string()),
            }
        }

        if report_summary
            .as_ref()
            .is_some_and(|summary| should_retry_case_with_json_actions(&options, summary))
        {
            native_schema_failure_count = native_schema_failure_count.saturating_add(1);
            adaptive_action_mode_retry = true;
            selected_result_dir = options
                .result_dir
                .join(format!("{case_id}-json-actions-retry"));
            selected_log_file = case_log_dir.join(format!("{case_id}-json-actions-retry.log"));
            if selected_result_dir.exists() {
                fs::remove_dir_all(&selected_result_dir).with_context(|| {
                    format!("failed to clear {}", selected_result_dir.display())
                })?;
            }
            fs::create_dir_all(&selected_result_dir)?;
            log_phase(
                "batch",
                ANSI_YELLOW,
                format!(
                    "case {case_id} hit native-tool action-contract failure before a write; retrying once with JSON-envelope actions"
                ),
            );
            let retry_status = launch_single_case_run(
                &case_root,
                &objective_path,
                &selected_result_dir,
                &selected_log_file,
                &options,
                prepared_runtime.base_url_override.as_deref(),
                SingleCaseActionContractOverride::json_retry(),
            );
            match retry_status {
                Ok(status) => {
                    status_code = status.code().unwrap_or(-1);
                }
                Err(retry_error) => {
                    error = Some(retry_error.to_string());
                }
            }
            report_path = selected_result_dir.join("benchmark-report.json");
            if report_path.exists() {
                match read_benchmark_report(&report_path) {
                    Ok(report) => report_summary = Some(report),
                    Err(read_error) => error = Some(read_error.to_string()),
                }
            }
        } else if report_summary.as_ref().is_some_and(|summary| {
            summary.action_contract_mode == "native_tool_calls_v1"
                && summary
                    .local_agent_final_failure_classification
                    .as_deref()
                    .is_some_and(|classification| classification == "parser_tool_schema")
        }) {
            native_schema_failure_count = native_schema_failure_count.saturating_add(1);
        }

        if report_summary.is_none() {
            let synthetic_error = error.clone().unwrap_or_else(|| {
                format!(
                    "benchmark process exited with status {} without writing a report",
                    status_code
                )
            });
            let synthetic_model_id =
                resolve_benchmark_model_id(options.executor, options.model_id.clone())
                    .unwrap_or_else(|_| "broker-unavailable".to_string());
            if let Err(write_error) = write_synthetic_failure_report(
                &case_manifest,
                &selected_result_dir,
                options.executor,
                options
                    .model_id
                    .as_deref()
                    .unwrap_or(synthetic_model_id.as_str()),
                options.max_attempts.unwrap_or(3).max(1),
                synthetic_error.clone(),
                None,
            ) {
                error = Some(format!(
                    "{synthetic_error}; synthetic report write failed: {write_error}"
                ));
            } else {
                report_summary = read_benchmark_report(&report_path).ok();
            }
        }

        if report_summary.is_none() {
            error = Some(format!(
                "benchmark report was not written to {}",
                report_path.display()
            ));
        }

        if let Some(summary) = report_summary {
            batch_cases.push(BatchCaseReport {
                case_id: case_id.clone(),
                case_root: case_root.clone(),
                objective_path: objective_path.clone(),
                result_dir: selected_result_dir.clone(),
                log_file: selected_log_file.clone(),
                executor: options.executor,
                success: summary.success,
                exit_code: summary.exit_code,
                wall_clock_ms: summary.wall_clock_ms,
                total_requests: summary.total_requests,
                total_billed_tokens: summary.total_billed_tokens,
                lines_added: summary.lines_added,
                lines_removed: summary.lines_removed,
                mistakes_corrected: summary.mistakes_corrected,
                judge_passed: summary.judge.as_ref().map(|judge| judge.passed),
                deterministic_evaluation_passed: summary.deterministic_evaluation_passed,
                first_request_prompt_token_estimate: summary.first_request_prompt_token_estimate,
                first_request_raw_prompt_token_estimate: summary
                    .first_request_raw_prompt_token_estimate,
                first_request_compacted_prompt_token_estimate: summary
                    .first_request_compacted_prompt_token_estimate,
                first_request_first_token_latency_ms: summary.first_request_first_token_latency_ms,
                first_model_turn_started: summary.first_model_turn_started,
                first_action_emitted: summary.first_action_emitted,
                final_stop_reason: summary.final_stop_reason,
                primary_failure: summary.primary_failure.clone(),
                local_agent_final_failure_classification: summary
                    .local_agent_final_failure_classification
                    .clone(),
                adaptive_action_mode_retry,
                report_path: report_path.clone(),
                error: None,
            });
            log_phase(
                "batch",
                if summary.success {
                    ANSI_GREEN
                } else {
                    ANSI_YELLOW
                },
                format!(
                    "case {} finished: success={} judge={} tokens={} requests={} stop={:?} failure={}",
                    case_id,
                    summary.success,
                    summary
                        .judge
                        .as_ref()
                        .map(|judge| judge.passed.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    summary.total_billed_tokens,
                    summary.total_requests,
                    summary.final_stop_reason,
                    summary
                        .primary_failure
                        .clone()
                        .unwrap_or_else(|| "none".to_string())
                ),
            );
        } else {
            batch_cases.push(BatchCaseReport {
                case_id: case_id.clone(),
                case_root: case_root.clone(),
                objective_path: objective_path.clone(),
                result_dir: selected_result_dir.clone(),
                log_file: selected_log_file.clone(),
                executor: options.executor,
                success: false,
                exit_code: status_code,
                wall_clock_ms: 0,
                total_requests: 0,
                total_billed_tokens: 0,
                lines_added: 0,
                lines_removed: 0,
                mistakes_corrected: 0,
                judge_passed: None,
                deterministic_evaluation_passed: None,
                first_request_prompt_token_estimate: None,
                first_request_raw_prompt_token_estimate: None,
                first_request_compacted_prompt_token_estimate: None,
                first_request_first_token_latency_ms: None,
                first_model_turn_started: false,
                first_action_emitted: false,
                final_stop_reason: None,
                primary_failure: Some("launch_failed".to_string()),
                local_agent_final_failure_classification: Some("launch_failed".to_string()),
                adaptive_action_mode_retry,
                report_path: report_path.clone(),
                error: error.clone(),
            });
            log_phase(
                "batch",
                ANSI_YELLOW,
                format!(
                    "case {} finished without a readable benchmark report: {}",
                    case_id,
                    error.unwrap_or_else(|| "unknown error".to_string())
                ),
            );
        }
        write_batch_summary_artifacts(
            &options,
            &batch_cases,
            batch_started_at.elapsed().as_millis() as u64,
        )?;
    }
    if prepared_runtime.stop_after_batch {
        SsdMoeRuntimeHandle::shared_handle().stop();
    }
    let rendered = fs::read_to_string(options.result_dir.join("batch-report.md"))
        .unwrap_or_else(|_| "# Batch Report\n- No report generated.".to_string());
    println!("{rendered}");
    Ok(())
}

fn read_benchmark_report(report_path: &Path) -> anyhow::Result<BenchmarkReport> {
    let raw = fs::read_to_string(report_path)
        .with_context(|| format!("failed to read {}", report_path.display()))?;
    serde_json::from_str::<BenchmarkReport>(&raw)
        .with_context(|| format!("failed to parse {}", report_path.display()))
}

fn should_retry_case_with_json_actions(
    options: &BenchmarkBatchRunOptions,
    report: &BenchmarkReport,
) -> bool {
    options.executor == BenchmarkExecutor::Native
        && !report.success
        && report.action_contract_mode == "native_tool_calls_v1"
        && report
            .local_agent_scorecard
            .first_valid_write_step
            .is_none()
        && report
            .local_agent_final_failure_classification
            .as_deref()
            .is_some_and(|classification| {
                classification == "parser_tool_schema"
                    || classification == "parser_or_action_contract"
            })
        && (report.local_agent_scorecard.parser_recovery_count > 0
            || report.attempts.last().is_some_and(|attempt| {
                attempt
                    .agent_error_message
                    .as_deref()
                    .is_some_and(|message| message.contains("unsupported native tool call"))
            }))
}

#[allow(clippy::disallowed_methods)]
fn launch_single_case_run(
    case_root: &Path,
    objective_path: &Path,
    case_result_dir: &Path,
    case_log_file: &Path,
    options: &BenchmarkBatchRunOptions,
    prepared_base_url_override: Option<&str>,
    action_contract: SingleCaseActionContractOverride,
) -> anyhow::Result<std::process::ExitStatus> {
    let current_exe = std::env::current_exe()
        .with_context(|| "failed to determine current quorp executable".to_string())?;
    let mut command = Command::new(current_exe);
    command
        .arg("benchmark")
        .arg("run")
        .arg("--path")
        .arg(objective_path)
        .arg("--executor")
        .arg(options.executor.label())
        .arg("--result-dir")
        .arg(case_result_dir)
        .arg("--log-file")
        .arg(case_log_file)
        .arg("--max-steps")
        .arg(options.max_steps.to_string())
        .arg("--max-seconds")
        .arg(options.max_seconds.unwrap_or(3600).to_string())
        .arg("--autonomy-profile")
        .arg(options.autonomy_profile.label())
        .current_dir(case_root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if options.allow_heavy_local_model {
        command.arg("--allow-heavy-local-model");
    }
    if options.keep_sandbox {
        command.arg("--keep-sandbox");
    }

    if let Some(model_id) = options.model_id.as_ref() {
        command.arg("--model").arg(model_id);
    }
    if let Some(base_url_override) =
        prepared_base_url_override.or(options.base_url_override.as_deref())
    {
        command.arg("--base-url").arg(base_url_override);
    }
    if let Some(briefing_file) = options.briefing_file.as_ref() {
        command.arg("--briefing-file").arg(briefing_file);
    }
    if let Some(compaction_policy) = options.compaction_policy {
        command
            .arg("--compaction-policy")
            .arg(compaction_policy.as_str());
    }
    if let Some(seed_transcript) = options.seed_transcript.as_ref() {
        command.arg("--seed-transcript").arg(seed_transcript);
    }
    if let Some(token_budget) = options.max_total_tokens {
        command.arg("--token-budget").arg(token_budget.to_string());
    }
    if let Some(max_attempts) = options.max_attempts {
        command.arg("--max-attempts").arg(max_attempts.to_string());
    }
    if let Some(condition) = options.condition.as_ref() {
        command.arg("--condition").arg(condition);
    }
    if let Some(native_tool_calls_override) = action_contract.native_tool_calls {
        command.env(
            "QUORP_BENCH_NATIVE_TOOL_CALLS",
            if native_tool_calls_override {
                "true"
            } else {
                "false"
            },
        );
    }
    if let Some(reason) = action_contract.fallback_reason {
        command.env("QUORP_BENCH_ACTION_CONTRACT_FALLBACK_REASON", reason);
    }
    if !action_contract.attempt_lineage.is_empty() {
        command.env(
            "QUORP_BENCH_ATTEMPT_LINEAGE",
            action_contract.attempt_lineage.join(","),
        );
    }

    let status = command.status().with_context(|| {
        format!(
            "failed to launch benchmark run for {}",
            objective_path.display()
        )
    })?;
    Ok(status)
}

fn prepare_batch_runtime(
    options: &BenchmarkBatchRunOptions,
) -> anyhow::Result<PreparedBatchRuntime> {
    if options.executor == BenchmarkExecutor::Codex {
        return Ok(PreparedBatchRuntime {
            base_url_override: None,
            stop_after_batch: false,
        });
    }
    if let Some(base_url_override) = options.base_url_override.clone() {
        return Ok(PreparedBatchRuntime {
            base_url_override: Some(base_url_override),
            stop_after_batch: false,
        });
    }
    let model_id = if let Some(model_id) = options.model_id.clone() {
        model_id
    } else {
        safe_benchmark_model_id()?
    };
    if native_batch_model_uses_remote_provider(&model_id) {
        return Ok(PreparedBatchRuntime {
            base_url_override: None,
            stop_after_batch: false,
        });
    }
    let Some(model) = crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(&model_id)
    else {
        return Err(anyhow::anyhow!(
            "SSD-MOE broker listed model `{model_id}` as default, but Quorp could not resolve its runtime metadata"
        ));
    };
    let runtime = SsdMoeRuntimeHandle::shared_handle();
    let timeout = Duration::from_secs(90);
    runtime.ensure_running(&options.cases_root, &model);
    if let Err(first_error) = runtime.wait_until_ready(timeout) {
        runtime.stop();
        std::thread::sleep(Duration::from_secs(1));
        runtime.ensure_running(&options.cases_root, &model);
        runtime.wait_until_ready(timeout).map_err(|second_error| {
            anyhow::anyhow!(
                "failed to prewarm shared local runtime for batch; first attempt: {first_error}; second attempt: {second_error}"
            )
        })?;
    }
    Ok(PreparedBatchRuntime {
        base_url_override: Some(runtime.base_url()),
        stop_after_batch: true,
    })
}

fn native_batch_model_uses_remote_provider(model_id: &str) -> bool {
    if is_nvidia_kimi_model_id(model_id) || is_nvidia_qwen_coder_model_id(model_id) {
        return true;
    }
    let provider = crate::quorp::tui::model_registry::chat_model_provider(
        model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    );
    !matches!(
        provider,
        crate::quorp::executor::InteractiveProviderKind::Local
            | crate::quorp::executor::InteractiveProviderKind::Codex
    )
}

fn discover_challenge_case_roots(cases_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut case_roots = Vec::new();
    for entry in fs::read_dir(cases_root)
        .with_context(|| format!("failed to read cases root {}", cases_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let case_root = entry.path();
        if case_root.join("benchmark.json").exists() {
            case_roots.push(case_root);
        }
    }
    case_roots.sort();
    Ok(case_roots)
}

fn summarize_batch_report(
    cases_root: PathBuf,
    result_dir: PathBuf,
    cases: Vec<BatchCaseReport>,
) -> BatchReport {
    let total_requests = cases.iter().map(|case| case.total_requests).sum();
    let total_billed_tokens = cases.iter().map(|case| case.total_billed_tokens).sum();
    let lines_added = cases.iter().map(|case| case.lines_added).sum();
    let lines_removed = cases.iter().map(|case| case.lines_removed).sum();
    let mistakes_corrected = cases.iter().map(|case| case.mistakes_corrected).sum();
    let successful_cases = cases.iter().filter(|case| case.success).count();
    let failed_cases = cases.len().saturating_sub(successful_cases);
    BatchReport {
        cases_root,
        result_dir,
        cases,
        total_requests,
        total_billed_tokens,
        lines_added,
        lines_removed,
        mistakes_corrected,
        successful_cases,
        failed_cases,
    }
}

fn write_batch_summary_artifacts(
    options: &BenchmarkBatchRunOptions,
    cases: &[BatchCaseReport],
    elapsed_ms: u64,
) -> anyhow::Result<()> {
    let batch_summary = summarize_batch_report(
        options.cases_root.clone(),
        options.result_dir.clone(),
        cases.to_vec(),
    );
    write_json(
        &options.result_dir.join("batch-report.json"),
        &batch_summary,
    )?;
    let rendered = render_batch_report(&batch_summary, elapsed_ms);
    fs::write(options.result_dir.join("batch-report.md"), rendered)?;
    let run_summary = summarize_run_report(&batch_summary);
    write_json(&options.result_dir.join("run-summary.json"), &run_summary)?;
    fs::write(
        options.result_dir.join("run-summary.md"),
        render_run_summary(&run_summary),
    )?;
    Ok(())
}

fn summarize_run_report(report: &BatchReport) -> RunSummary {
    RunSummary {
        cases_root: report.cases_root.clone(),
        result_dir: report.result_dir.clone(),
        cases_run: report.cases.len(),
        successful_cases: report.successful_cases,
        failed_cases: report.failed_cases,
        total_requests: report.total_requests,
        total_billed_tokens: report.total_billed_tokens,
        cases: report
            .cases
            .iter()
            .map(|case| {
                let scorecard = read_case_report_scorecard(&case.report_path);
                RunSummaryCase {
                    case_id: case.case_id.clone(),
                    success: case.success,
                    primary_failure: case.primary_failure.clone(),
                    local_agent_final_failure_classification: case
                        .local_agent_final_failure_classification
                        .clone(),
                    final_stop_reason: case.final_stop_reason,
                    first_valid_write_step: scorecard
                        .as_ref()
                        .and_then(|scorecard| scorecard.first_valid_write_step),
                    parser_recovery_count: scorecard
                        .as_ref()
                        .map(|scorecard| scorecard.parser_recovery_count)
                        .unwrap_or_default(),
                    redundant_read_count: scorecard
                        .as_ref()
                        .map(|scorecard| scorecard.redundant_read_count)
                        .unwrap_or_default(),
                    rejected_validation_alias_count: scorecard
                        .as_ref()
                        .map(|scorecard| scorecard.rejected_validation_alias_count)
                        .unwrap_or_default(),
                    target_redirect_count: scorecard
                        .as_ref()
                        .map(|scorecard| scorecard.target_redirect_count)
                        .unwrap_or_default(),
                    syntax_preview_failure_count: scorecard
                        .as_ref()
                        .map(|scorecard| scorecard.syntax_preview_failure_count)
                        .unwrap_or_default(),
                    adaptive_action_mode_retry: case.adaptive_action_mode_retry,
                    report_path: case.report_path.clone(),
                }
            })
            .collect(),
    }
}

fn read_case_report_scorecard(report_path: &Path) -> Option<quorp_agent_core::LocalAgentScorecard> {
    let raw = fs::read_to_string(report_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("local_agent_scorecard")
        .and_then(|scorecard| serde_json::from_value(scorecard.clone()).ok())
}

fn render_run_summary(summary: &RunSummary) -> String {
    let mut lines = vec![
        "# Run Summary".to_string(),
        format!("- Cases root: `{}`", summary.cases_root.display()),
        format!("- Result dir: `{}`", summary.result_dir.display()),
        format!("- Cases run: `{}`", summary.cases_run),
        format!("- Successful cases: `{}`", summary.successful_cases),
        format!("- Failed cases: `{}`", summary.failed_cases),
        format!("- Total requests: `{}`", summary.total_requests),
        format!("- Total billed tokens: `{}`", summary.total_billed_tokens),
        String::new(),
        "## Classifications".to_string(),
    ];
    for case in &summary.cases {
        lines.push(format!(
            "- `{}` success={} primary={} local={} stop={:?} first_write={} parser_recovery={} redundant_reads={} validation_rejects={} target_redirects={} syntax_preview_failures={} adaptive_retry={} report={}",
            case.case_id,
            case.success,
            case.primary_failure
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            case.local_agent_final_failure_classification
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
            case.final_stop_reason,
            case.first_valid_write_step
                .map(|step| step.to_string())
                .unwrap_or_else(|| "none".to_string()),
            case.parser_recovery_count,
            case.redundant_read_count,
            case.rejected_validation_alias_count,
            case.target_redirect_count,
            case.syntax_preview_failure_count,
            case.adaptive_action_mode_retry,
            case.report_path.display()
        ));
    }
    lines.join("\n")
}

pub fn score_benchmark_reports(
    options: BenchmarkScoreOptions,
) -> anyhow::Result<BenchmarkScoreArtifacts> {
    let run_dirs = resolve_score_run_dirs(&options)?;
    let output_root = options.output_root.clone().unwrap_or_else(|| {
        options
            .reports_root
            .join("scoreboards")
            .join(options.suite.trim())
    });
    let previous_score = read_score_report(&output_root.join("latest.json")).ok();
    let output_dir = output_root.join(format!("session-{}", current_unix_timestamp_seconds()));

    let mut best_cases = BTreeMap::<String, BenchmarkScoreCase>::new();
    for run_dir in &run_dirs {
        for case in load_score_cases_from_run_dir(run_dir, &options.suite)? {
            match best_cases.get(&case.case_id) {
                Some(current) if !score_case_is_better(&case, current) => {}
                _ => {
                    best_cases.insert(case.case_id.clone(), case);
                }
            }
        }
    }

    let mut cases = best_cases.into_values().collect::<Vec<_>>();
    cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    let total_cases = cases.len();
    let solved_cases = cases.iter().filter(|case| case.success).count();
    let valid_write_cases = cases
        .iter()
        .filter(|case| case.first_valid_write_step.is_some())
        .count();
    let post_write_validation_cases = cases
        .iter()
        .filter(|case| case.post_write_validation)
        .count();
    let diagnostic_classified_cases = cases
        .iter()
        .filter(|case| case.progress_score >= 3 || case.success)
        .count();
    let tooling_healthy_cases = cases
        .iter()
        .filter(|case| case_tooling_is_healthy(&case.failure_classification))
        .count();
    let total_requests = cases.iter().map(|case| case.total_requests).sum();
    let total_billed_tokens = cases.iter().map(|case| case.total_billed_tokens).sum();
    let blocker_counts = count_blockers(&cases);
    let common_blocker = blocker_counts
        .iter()
        .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(classification, _)| classification.clone());
    let generated_at_unix_seconds = current_unix_timestamp_seconds();
    let mut score = BenchmarkScoreReport {
        suite: options.suite.clone(),
        generated_at_unix_seconds,
        output_dir: output_dir.clone(),
        run_dirs,
        total_cases,
        solved_cases,
        valid_write_cases,
        post_write_validation_cases,
        diagnostic_classified_cases,
        tooling_healthy_cases,
        total_requests,
        total_billed_tokens,
        common_blocker,
        blocker_counts,
        regressions: Vec::new(),
        cases,
    };
    score.regressions = detect_score_regressions(previous_score.as_ref(), &score);
    let markdown = render_scoreboard(&score);

    write_json(&output_dir.join("scoreboard.json"), &score)?;
    fs::write(output_dir.join("scoreboard.md"), &markdown)?;
    write_json(&output_root.join("latest.json"), &score)?;
    fs::write(output_root.join("latest.md"), &markdown)?;

    Ok(BenchmarkScoreArtifacts {
        output_dir,
        markdown,
    })
}

fn resolve_score_run_dirs(options: &BenchmarkScoreOptions) -> anyhow::Result<Vec<PathBuf>> {
    if !options.run_dirs.is_empty() {
        return Ok(options.run_dirs.clone());
    }
    let discovered = discover_score_run_dirs(&options.reports_root, &options.suite)?;
    if discovered.is_empty() {
        anyhow::bail!(
            "no benchmark reports found for suite `{}` under {}",
            options.suite,
            options.reports_root.display()
        );
    }
    Ok(discovered)
}

fn discover_score_run_dirs(reports_root: &Path, suite: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut stack = vec![reports_root.to_path_buf()];
    let mut run_dirs = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", dir.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                if path.join("batch-report.json").exists()
                    && run_dir_has_suite_cases(&path, suite).unwrap_or(false)
                {
                    run_dirs.push(path);
                } else {
                    stack.push(path);
                }
            }
        }
    }
    run_dirs.sort_by(|left, right| {
        path_modified_unix_seconds(right)
            .cmp(&path_modified_unix_seconds(left))
            .then_with(|| left.cmp(right))
    });
    Ok(run_dirs)
}

fn run_dir_has_suite_cases(run_dir: &Path, suite: &str) -> anyhow::Result<bool> {
    Ok(load_score_cases_from_run_dir(run_dir, suite)?
        .into_iter()
        .next()
        .is_some())
}

fn load_score_cases_from_run_dir(
    run_dir: &Path,
    suite: &str,
) -> anyhow::Result<Vec<BenchmarkScoreCase>> {
    let batch_report_path = run_dir.join("batch-report.json");
    if batch_report_path.exists() {
        let raw = fs::read_to_string(&batch_report_path)
            .with_context(|| format!("failed to read {}", batch_report_path.display()))?;
        let report: BatchReport = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", batch_report_path.display()))?;
        return report
            .cases
            .iter()
            .filter_map(|case| match load_score_case_from_batch_case(case, suite) {
                Ok(Some(score_case)) => Some(Ok(score_case)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect();
    }

    let benchmark_report_path = run_dir.join("benchmark-report.json");
    if benchmark_report_path.exists() {
        let case = load_single_score_case(&benchmark_report_path, suite)?;
        return Ok(case.into_iter().collect());
    }

    anyhow::bail!(
        "no batch-report.json or benchmark-report.json found in {}",
        run_dir.display()
    )
}

fn load_score_case_from_batch_case(
    case: &BatchCaseReport,
    suite: &str,
) -> anyhow::Result<Option<BenchmarkScoreCase>> {
    let report_path = if case.report_path.exists() {
        case.report_path.clone()
    } else {
        case.result_dir.join("benchmark-report.json")
    };
    let report = read_benchmark_report(&report_path).ok();
    if !score_case_matches_suite(case, report.as_ref(), suite) {
        return Ok(None);
    }
    Ok(Some(score_case_from_parts(
        case,
        report.as_ref(),
        report_path,
    )))
}

fn load_single_score_case(
    report_path: &Path,
    suite: &str,
) -> anyhow::Result<Option<BenchmarkScoreCase>> {
    let report = read_benchmark_report(report_path)?;
    let case = BatchCaseReport {
        case_id: report.issue_id.clone(),
        case_root: report
            .challenge
            .as_ref()
            .map(|challenge| challenge.case_root.clone())
            .unwrap_or_else(|| report.run_dir.clone()),
        objective_path: PathBuf::new(),
        result_dir: report.run_dir.clone(),
        log_file: report.run_dir.join("benchmark.log"),
        executor: report.executor,
        success: report.success,
        exit_code: report.exit_code,
        wall_clock_ms: report.wall_clock_ms,
        total_requests: report.total_requests,
        total_billed_tokens: report.total_billed_tokens,
        lines_added: report.lines_added,
        lines_removed: report.lines_removed,
        mistakes_corrected: report.mistakes_corrected,
        judge_passed: report.judge.as_ref().map(|judge| judge.passed),
        deterministic_evaluation_passed: report.deterministic_evaluation_passed,
        first_request_prompt_token_estimate: report.first_request_prompt_token_estimate,
        first_request_raw_prompt_token_estimate: report.first_request_raw_prompt_token_estimate,
        first_request_compacted_prompt_token_estimate: report
            .first_request_compacted_prompt_token_estimate,
        first_request_first_token_latency_ms: report.first_request_first_token_latency_ms,
        first_model_turn_started: report.first_model_turn_started,
        first_action_emitted: report.first_action_emitted,
        final_stop_reason: report.final_stop_reason,
        primary_failure: report.primary_failure.clone(),
        local_agent_final_failure_classification: report
            .local_agent_final_failure_classification
            .clone(),
        adaptive_action_mode_retry: false,
        report_path: report_path.to_path_buf(),
        error: report.run_error.clone(),
    };
    if !score_case_matches_suite(&case, Some(&report), suite) {
        return Ok(None);
    }
    Ok(Some(score_case_from_parts(
        &case,
        Some(&report),
        report_path.to_path_buf(),
    )))
}

fn score_case_matches_suite(
    case: &BatchCaseReport,
    report: Option<&BenchmarkReport>,
    suite: &str,
) -> bool {
    let suite = suite.trim();
    if suite.is_empty() {
        return true;
    }
    if path_contains(&case.case_root, suite) || path_contains(&case.result_dir, suite) {
        return true;
    }
    if suite == "rust-swebench-top5" && rust_swebench_top5_case_id(&case.case_id) {
        return true;
    }
    report.is_some_and(|report| {
        report.challenge.as_ref().is_some_and(|challenge| {
            path_contains(&challenge.case_root, suite)
                || challenge
                    .tags
                    .iter()
                    .any(|tag| tag == suite || tag == "rust-swebench")
        })
    })
}

fn rust_swebench_top5_case_id(case_id: &str) -> bool {
    ["01-", "02-", "03-", "04-", "05-"]
        .iter()
        .any(|prefix| case_id.starts_with(prefix))
}

fn path_contains(path: &Path, needle: &str) -> bool {
    path.to_string_lossy().contains(needle)
}

fn score_case_from_parts(
    case: &BatchCaseReport,
    report: Option<&BenchmarkReport>,
    report_path: PathBuf,
) -> BenchmarkScoreCase {
    let scorecard = report
        .map(|report| report.local_agent_scorecard.clone())
        .or_else(|| read_case_report_scorecard(&report_path))
        .unwrap_or_default();
    let first_valid_write_step = scorecard.first_valid_write_step;
    let post_write_validation = first_valid_write_step.is_some()
        && report.is_some_and(|report| {
            report.post_fast_loop_validation_rerun_attempted
                || report.validation_commands_run > 1
                || report.evaluation_commands_run > 0
        });
    let failure_classification = normalize_score_failure_classification(case, report, &scorecard);
    let progress_score = progress_score_for_case(case, report, &scorecard, post_write_validation);
    let general_tooling_gap = general_tooling_gap_for_case(case, &failure_classification);
    BenchmarkScoreCase {
        case_id: case.case_id.clone(),
        success: case.success,
        progress_score,
        progress_phase: progress_phase_label(progress_score).to_string(),
        failure_classification,
        primary_failure: case.primary_failure.clone(),
        model_id: report.map(|report| report.model_id.clone()),
        executor: Some(case.executor.label().to_string()),
        provider_base_url: report.and_then(|report| report.provider_base_url.clone()),
        action_contract_selected: report.and_then(|report| {
            (!report.action_contract_selected.trim().is_empty())
                .then(|| report.action_contract_selected.clone())
        }),
        result_dir: case.result_dir.clone(),
        report_path,
        first_model_turn_started: case.first_model_turn_started,
        first_action_emitted: case.first_action_emitted,
        diagnostic_class: report.and_then(|report| report.diagnostic_class.clone()),
        implementation_target_lease: report
            .and_then(|report| report.implementation_target_lease.clone()),
        first_valid_write_step,
        post_write_validation,
        parser_recovery_count: scorecard.parser_recovery_count,
        redundant_read_count: scorecard.redundant_read_count,
        rejected_validation_alias_count: scorecard.rejected_validation_alias_count,
        target_redirect_count: scorecard.target_redirect_count,
        syntax_preview_failure_count: scorecard.syntax_preview_failure_count,
        preview_created_count: scorecard.preview_created_count,
        modify_toml_count: scorecard.modify_toml_count,
        replace_range_count: scorecard.replace_range_count,
        apply_preview_count: scorecard.apply_preview_count,
        wall_clock_ms: case.wall_clock_ms,
        total_requests: case.total_requests,
        total_billed_tokens: case.total_billed_tokens,
        lines_added: case.lines_added,
        lines_removed: case.lines_removed,
        general_tooling_gap,
    }
}

fn progress_score_for_case(
    case: &BatchCaseReport,
    report: Option<&BenchmarkReport>,
    scorecard: &quorp_agent_core::LocalAgentScorecard,
    post_write_validation: bool,
) -> u8 {
    if case.success {
        return 6;
    }
    if post_write_validation {
        return 5;
    }
    if scorecard.first_valid_write_step.is_some() {
        return 4;
    }
    if report.is_some_and(|report| {
        report.diagnostic_class.is_some()
            || report.last_validation_failure.is_some()
            || !report.failing_test_names.is_empty()
            || report.primary_failure_path.is_some()
    }) || case
        .local_agent_final_failure_classification
        .as_deref()
        .is_some_and(|classification| classification != "n/a")
    {
        return 3;
    }
    if case.first_action_emitted {
        return 2;
    }
    if case.first_model_turn_started || case.total_requests > 0 {
        return 1;
    }
    0
}

fn progress_phase_label(score: u8) -> &'static str {
    match score {
        6 => "solved",
        5 => "post_write_validation",
        4 => "valid_implementation_write",
        3 => "diagnostic_classified",
        2 => "first_action",
        1 => "launch_or_first_turn",
        _ => "no_artifact",
    }
}

fn normalize_score_failure_classification(
    case: &BatchCaseReport,
    report: Option<&BenchmarkReport>,
    scorecard: &quorp_agent_core::LocalAgentScorecard,
) -> String {
    if case.success {
        return "success".to_string();
    }
    let raw = case
        .local_agent_final_failure_classification
        .as_deref()
        .or_else(|| {
            report.and_then(|report| report.local_agent_final_failure_classification.as_deref())
        })
        .or(case.primary_failure.as_deref())
        .unwrap_or("unknown_agent_fatal");
    match raw {
        "success" => "success",
        "launch_failed"
        | "first_token_timeout"
        | "stream_idle_timeout"
        | "model_request_timeout"
        | "runtime_startup_or_inference" => "infra_runtime",
        "context_wander" => "context_management",
        "agent_fatal_error" | "agent_error" => {
            if scorecard.first_valid_write_step.is_some()
                || scorecard.repeated_failed_edit_count > 0
                || report.is_some_and(|report| !report.failed_edit_records.is_empty())
            {
                "model_edit_strategy"
            } else if scorecard.parser_recovery_count > 0 {
                "parser_tool_schema"
            } else if scorecard.rejected_validation_alias_count > 0 {
                "validation_governance"
            } else if case.first_action_emitted || case.first_model_turn_started {
                "context_management"
            } else {
                "infra_runtime"
            }
        }
        other => other,
    }
    .to_string()
}

fn general_tooling_gap_for_case(
    case: &BatchCaseReport,
    failure_classification: &str,
) -> Option<String> {
    if case.primary_failure.as_deref() == Some("agent_fatal_error")
        && failure_classification == "agent_fatal_error"
    {
        return Some("unknown_agent_fatal_without_typed_classification".to_string());
    }
    match failure_classification {
        "infra_runtime" => Some("runtime_or_host_infrastructure".to_string()),
        "parser_tool_schema" => Some("action_contract_or_parser_recovery".to_string()),
        "validation_governance" => Some("validation_command_governance".to_string()),
        "context_management" => Some("context_selection_or_anti_wander".to_string()),
        "diagnostic_targeting" => Some("diagnostic_to_target_mapping".to_string()),
        _ => None,
    }
}

fn score_case_is_better(candidate: &BenchmarkScoreCase, current: &BenchmarkScoreCase) -> bool {
    candidate
        .progress_score
        .cmp(&current.progress_score)
        .then_with(|| candidate.success.cmp(&current.success))
        .then_with(|| {
            path_modified_unix_seconds(&candidate.report_path)
                .cmp(&path_modified_unix_seconds(&current.report_path))
        })
        .is_gt()
}

fn count_blockers(cases: &[BenchmarkScoreCase]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for case in cases.iter().filter(|case| !case.success) {
        *counts
            .entry(case.failure_classification.clone())
            .or_insert(0) += 1;
    }
    counts
}

fn case_tooling_is_healthy(classification: &str) -> bool {
    matches!(
        classification,
        "success"
            | "model_edit_strategy"
            | "model_semantic_quality"
            | "edit_intent_quality"
            | "syntax_patch_quality"
            | "toml_edit_quality"
    )
}

fn detect_score_regressions(
    previous: Option<&BenchmarkScoreReport>,
    current: &BenchmarkScoreReport,
) -> Vec<String> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    let mut regressions = Vec::new();
    if current.solved_cases < previous.solved_cases {
        regressions.push(format!(
            "solved cases decreased from {} to {}",
            previous.solved_cases, current.solved_cases
        ));
    }
    if current.valid_write_cases < previous.valid_write_cases {
        regressions.push(format!(
            "valid implementation writes decreased from {} to {}",
            previous.valid_write_cases, current.valid_write_cases
        ));
    }
    if current.post_write_validation_cases < previous.post_write_validation_cases {
        regressions.push(format!(
            "post-write validation cases decreased from {} to {}",
            previous.post_write_validation_cases, current.post_write_validation_cases
        ));
    }
    let previous_cases = previous
        .cases
        .iter()
        .map(|case| (case.case_id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    for case in &current.cases {
        if let Some(previous_case) = previous_cases.get(case.case_id.as_str())
            && case.progress_score < previous_case.progress_score
        {
            regressions.push(format!(
                "{} regressed from {} to {}",
                case.case_id, previous_case.progress_phase, case.progress_phase
            ));
        }
    }
    regressions
}

fn read_score_report(path: &Path) -> anyhow::Result<BenchmarkScoreReport> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn render_scoreboard(report: &BenchmarkScoreReport) -> String {
    let mut lines = vec![
        "# Rust SWE Scoreboard".to_string(),
        format!("- Suite: `{}`", report.suite),
        format!("- Generated at: `{}`", report.generated_at_unix_seconds),
        format!("- Output dir: `{}`", report.output_dir.display()),
        format!("- Runs scanned: `{}`", report.run_dirs.len()),
        format!(
            "- Solved score: `{}/{}`",
            report.solved_cases, report.total_cases
        ),
        format!(
            "- Valid implementation writes: `{}/{}`",
            report.valid_write_cases, report.total_cases
        ),
        format!(
            "- Post-write validation: `{}/{}`",
            report.post_write_validation_cases, report.total_cases
        ),
        format!(
            "- Diagnostic classified: `{}/{}`",
            report.diagnostic_classified_cases, report.total_cases
        ),
        format!(
            "- Tooling-healthy terminal states: `{}/{}`",
            report.tooling_healthy_cases, report.total_cases
        ),
        format!(
            "- Most common blocker: `{}`",
            report
                .common_blocker
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
        format!("- Total requests: `{}`", report.total_requests),
        format!("- Total billed tokens: `{}`", report.total_billed_tokens),
        String::new(),
        "## Blockers".to_string(),
    ];
    if report.blocker_counts.is_empty() {
        lines.push("- none".to_string());
    } else {
        for (classification, count) in &report.blocker_counts {
            lines.push(format!("- `{classification}`: `{count}`"));
        }
    }
    lines.push(String::new());
    lines.push("## Regressions".to_string());
    if report.regressions.is_empty() {
        lines.push("- none".to_string());
    } else {
        for regression in &report.regressions {
            lines.push(format!("- {regression}"));
        }
    }
    lines.push(String::new());
    lines.push("## Cases".to_string());
    for case in &report.cases {
        lines.push(format!(
            "- `{}` phase=`{}` progress={} success={} class=`{}` model={} first_write={} post_write_validation={} requests={} tokens={} changed=+{}/-{} gap={} report={}",
            case.case_id,
            case.progress_phase,
            case.progress_score,
            case.success,
            case.failure_classification,
            case.model_id
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
            case.first_valid_write_step
                .map(|step| step.to_string())
                .unwrap_or_else(|| "none".to_string()),
            case.post_write_validation,
            case.total_requests,
            case.total_billed_tokens,
            case.lines_added,
            case.lines_removed,
            case.general_tooling_gap
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            case.report_path.display()
        ));
    }
    lines.join("\n")
}

fn current_unix_timestamp_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn path_modified_unix_seconds(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn render_batch_report(report: &BatchReport, elapsed_ms: u64) -> String {
    let mut lines = vec![
        format!("# Batch Report"),
        format!("- Cases root: `{}`", report.cases_root.display()),
        format!("- Result dir: `{}`", report.result_dir.display()),
        format!("- Cases run: `{}`", report.cases.len()),
        format!("- Successful cases: `{}`", report.successful_cases),
        format!("- Failed cases: `{}`", report.failed_cases),
        format!("- Total requests: `{}`", report.total_requests),
        format!("- Total billed tokens: `{}`", report.total_billed_tokens),
        format!("- Lines added: `{}`", report.lines_added),
        format!("- Lines removed: `{}`", report.lines_removed),
        format!("- Mistakes corrected: `{}`", report.mistakes_corrected),
        format!("- Wall clock ms: `{}`", elapsed_ms),
        String::new(),
        "## Cases".to_string(),
    ];
    for case in &report.cases {
        lines.push(format!(
            "- `{}` executor={} success={} judge={} deterministic={} wall_clock_ms={} first_prompt_est={} compacted_prompt_est={} first_turn_started={} first_action_emitted={} first_token_ms={} requests={} tokens={} added={} removed={} mistakes={} stop={:?} failure={} local={} adaptive_retry={} log={} report={}",
            case.case_id,
            case.executor.label(),
            case.success,
            case
                .judge_passed
                .map(|passed| passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case
                .deterministic_evaluation_passed
                .map(|passed| passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case.wall_clock_ms,
            case
                .first_request_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case
                .first_request_compacted_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case.first_model_turn_started,
            case.first_action_emitted,
            case
                .first_request_first_token_latency_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case.total_requests,
            case.total_billed_tokens,
            case.lines_added,
            case.lines_removed,
            case.mistakes_corrected,
            case.final_stop_reason,
            case
                .primary_failure
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            case
                .local_agent_final_failure_classification
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
            case.adaptive_action_mode_retry,
            case.log_file.display(),
            case.report_path.display(),
        ));
    }
    lines.join("\n")
}

fn run_benchmark_from_manifest(
    manifest: &BenchmarkManifest,
    result_dir: &Path,
    starting_attempt: usize,
) -> anyhow::Result<()> {
    maybe_continue_attempts(
        manifest,
        result_dir,
        load_existing_attempts(result_dir)?,
        starting_attempt,
    )
}

fn run_challenge_benchmark(
    options: &BenchmarkRunOptions,
    challenge: ResolvedChallengeCase,
) -> anyhow::Result<()> {
    let model_id = resolve_benchmark_model_id(options.executor, options.model_id.clone())?;
    if options.executor == BenchmarkExecutor::Native {
        ensure_safe_local_model_selection(
            &model_id,
            allow_resolved_benchmark_model_without_opt_in(
                options.model_id.as_deref(),
                &model_id,
                options.allow_heavy_local_model,
            ),
        )?;
    }
    let safety_mode_label = benchmark_safety_mode_label(options.executor, &model_id);
    let scenario_label = Some(crate::quorp::provider_config::resolved_scenario_label());
    let mut completion_policy =
        benchmark_completion_policy(options.executor, &safety_mode_label, Some(&model_id));
    apply_requested_prompt_compaction_override(&mut completion_policy, options.compaction_policy);
    let prepared = match prepare_challenge_run(&options.result_dir, &challenge) {
        Ok(prepared) => prepared,
        Err(error) => {
            let setup_failure_class = "layout_resolution_failed".to_string();
            write_synthetic_failure_report(
                &challenge.manifest,
                &options.result_dir,
                options.executor,
                &model_id,
                options.max_attempts.unwrap_or(1).max(1),
                format!("{setup_failure_class}: {error:#}"),
                Some(setup_failure_class.clone()),
            )
            .with_context(|| {
                format!(
                    "failed to write synthetic setup failure report in {}",
                    options.result_dir.display()
                )
            })?;
            return Err(error).context(setup_failure_class);
        }
    };
    let manifest = BenchmarkManifest {
        resolved: prepared.resolved,
        executor: options.executor,
        model_id,
        safety_mode_label,
        scenario_label,
        base_url_override: base_url_override_for_executor(
            options.executor,
            options.base_url_override.clone(),
        ),
        briefing_file: options.briefing_file.clone(),
        compaction_policy: completion_policy.prompt_compaction_policy,
        seed_transcript: options.seed_transcript.clone(),
        max_steps: options.max_steps,
        max_seconds: options.max_seconds,
        max_total_tokens: options.max_total_tokens,
        autonomy_profile: options.autonomy_profile.label().to_string(),
        max_attempts: options.max_attempts.unwrap_or(0),
        challenge: Some(prepared.challenge_metadata.clone()),
        keep_sandbox: options.keep_sandbox,
        completion_policy,
    };
    write_json(
        &options.result_dir.join("benchmark-manifest.json"),
        &manifest,
    )?;
    if !prepared.reset_outcome.passed {
        write_report(
            &options.result_dir,
            &manifest,
            &[],
            Some(prepared.reset_outcome.clone()),
            Some("challenge workspace reset failed before any attempts could run".to_string()),
        )?;
        if !options.keep_sandbox
            && let Err(error) = fs::remove_dir_all(&prepared.challenge_metadata.sandbox_root)
        {
            log::warn!(
                "failed to remove challenge sandbox {}: {error}",
                prepared.challenge_metadata.sandbox_root.display()
            );
        }
        return Ok(());
    }

    let outcome = maybe_continue_challenge_attempts(
        &manifest,
        &options.result_dir,
        Vec::new(),
        1,
        Some(prepared.reset_outcome.clone()),
    );
    if !options.keep_sandbox
        && let Err(error) = fs::remove_dir_all(&prepared.challenge_metadata.sandbox_root)
    {
        log::warn!(
            "failed to remove challenge sandbox {}: {error}",
            prepared.challenge_metadata.sandbox_root.display()
        );
    }
    outcome
}

#[derive(Debug, Clone)]
struct PreparedChallengeRun {
    resolved: ResolvedBenchmark,
    challenge_metadata: ChallengeMetadata,
    reset_outcome: EvaluatorOutcome,
}

fn prepare_challenge_run(
    result_dir: &Path,
    challenge: &ResolvedChallengeCase,
) -> anyhow::Result<PreparedChallengeRun> {
    let sandbox_root = result_dir.join(CHALLENGE_SANDBOX_DIR);
    if sandbox_root.exists() {
        fs::remove_dir_all(&sandbox_root)
            .with_context(|| format!("failed to clean {}", sandbox_root.display()))?;
    }
    log_phase(
        "sandbox",
        ANSI_BLUE,
        format!(
            "copying challenge bundle {} -> {}",
            challenge.case_root.display(),
            sandbox_root.display()
        ),
    );
    copy_dir_all(&challenge.case_root, &sandbox_root)?;
    maybe_materialize_rustbench_workspace(&sandbox_root, &challenge.condition)?;
    maybe_materialize_flat_challenge_reset_script(result_dir, &sandbox_root)?;

    let objective_path = sandbox_root.join(CHALLENGE_OBJECTIVE_FILE);
    let sandbox_objective_source = challenge
        .objective_source
        .strip_prefix(&challenge.case_root)
        .map(|relative| sandbox_root.join(relative))
        .unwrap_or_else(|_| challenge.objective_source.clone());
    let sandbox_success_source = challenge
        .success_source
        .strip_prefix(&challenge.case_root)
        .map(|relative| sandbox_root.join(relative))
        .unwrap_or_else(|_| challenge.success_source.clone());
    let capsule = compile_challenge_capsule(challenge, &sandbox_root)?;
    write_benchmark_sandbox_cargo_config(&sandbox_root, &challenge.condition)?;
    let reset_command =
        substitute_condition(&challenge.manifest.reset_command, &challenge.condition);
    let reset_outcome = run_shell_command(
        "reset",
        &reset_command,
        &sandbox_root.join("reset.sh"),
        &sandbox_root,
    )?;

    let workspace_dir = resolve_challenge_workspace_dir(&sandbox_root, &challenge.condition)?;
    let workspace_objective_file = workspace_dir.join(
        sandbox_objective_source
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("START_HERE.md")),
    );
    let workspace_success_file = workspace_dir.join(
        sandbox_success_source
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("SUCCESS.md")),
    );
    let workspace_benchmark_file = workspace_dir.join("benchmark.json");
    let sandbox_reference_source = sandbox_root.join("REFERENCE.md");
    let workspace_reference_file = sandbox_reference_source
        .exists()
        .then(|| workspace_dir.join("REFERENCE.md"));
    let capsule_file = workspace_dir.join(".quorp").join("challenge-capsule.json");

    fs::create_dir_all(&workspace_dir)
        .with_context(|| format!("failed to create {}", workspace_dir.display()))?;
    copy_file_if_different(&sandbox_objective_source, &workspace_objective_file).with_context(
        || {
            format!(
                "failed to mirror challenge objective {} into {}",
                sandbox_objective_source.display(),
                workspace_objective_file.display()
            )
        },
    )?;
    copy_file_if_different(&sandbox_success_source, &workspace_success_file).with_context(
        || {
            format!(
                "failed to mirror challenge success file {} into {}",
                sandbox_success_source.display(),
                workspace_success_file.display()
            )
        },
    )?;
    copy_file_if_different(
        &sandbox_root.join("benchmark.json"),
        &workspace_benchmark_file,
    )
    .with_context(|| {
        format!(
            "failed to mirror challenge manifest into {}",
            workspace_benchmark_file.display()
        )
    })?;
    if let Some(workspace_reference_file) = workspace_reference_file.as_ref() {
        copy_file_if_different(&sandbox_reference_source, workspace_reference_file).with_context(
            || {
                format!(
                    "failed to mirror challenge reference file {} into {}",
                    sandbox_reference_source.display(),
                    workspace_reference_file.display()
                )
            },
        )?;
    }

    if let Some(parent) = capsule_file.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json(&capsule_file, &capsule)?;

    let challenge_metadata = ChallengeMetadata {
        case_root: challenge.case_root.clone(),
        sandbox_root: sandbox_root.clone(),
        workspace_dir: workspace_dir.clone(),
        condition: challenge.condition.clone(),
        objective_file: workspace_objective_file,
        success_file: workspace_success_file,
        reference_file: workspace_reference_file,
        reset_command: challenge.manifest.reset_command.clone(),
        evaluate_command: challenge.manifest.evaluate_command.clone(),
        expected_files_touched: challenge.manifest.expected_files_touched.clone(),
        allowed_generated_files: challenge.manifest.allowed_generated_files.clone(),
        primary_metrics: challenge.manifest.primary_metrics.clone(),
        tags: challenge.manifest.tags.clone(),
        capsule_file,
        capsule,
    };
    let objective_text = build_challenge_objective(challenge, &challenge_metadata)?;
    fs::write(&objective_path, objective_text)
        .with_context(|| format!("failed to write {}", objective_path.display()))?;

    let resolved = ResolvedBenchmark {
        benchmark_root: sandbox_root.clone(),
        issue_id: challenge.manifest.id.clone(),
        benchmark_name: challenge.manifest.title.clone(),
        issue_dir: None,
        workspace_source: workspace_dir.clone(),
        objective_source: objective_path,
        visible_evaluator: None,
        collector_evaluator: None,
        context_files: collect_challenge_context_files(&sandbox_root, &challenge_metadata),
        repair_artifacts: collect_repair_artifacts(&workspace_dir),
    };

    if reset_outcome.passed {
        write_workspace_challenge_command_wrappers(&workspace_dir)?;
        ensure_git_baseline(&workspace_dir)?;
        write_benchmark_sandbox_cargo_config(&sandbox_root, &challenge.condition)?;
        write_benchmark_agent_config(&workspace_dir)?;
    }

    Ok(PreparedChallengeRun {
        resolved,
        challenge_metadata,
        reset_outcome,
    })
}

#[derive(Debug, Deserialize)]
struct RustbenchUpstreamMetadata {
    repo: String,
    base_commit: String,
}

fn maybe_materialize_rustbench_workspace(
    sandbox_root: &Path,
    condition: &str,
) -> anyhow::Result<()> {
    let workspace_dir = sandbox_root.join("workspace").join(condition);
    if workspace_dir.exists() {
        return Ok(());
    }
    let metadata_path = sandbox_root.join("upstream").join("metadata.json");
    if !metadata_path.exists() {
        return Ok(());
    }
    let metadata: RustbenchUpstreamMetadata = serde_json::from_str(
        &fs::read_to_string(&metadata_path)
            .with_context(|| format!("failed to read {}", metadata_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", metadata_path.display()))?;
    let parent = workspace_dir.parent().ok_or_else(|| {
        anyhow::anyhow!("workspace path had no parent: {}", workspace_dir.display())
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let repo_url = format!("https://github.com/{}.git", metadata.repo);
    log_phase(
        "sandbox",
        ANSI_BLUE,
        format!(
            "materializing Rustbench workspace {} @ {} -> {}",
            metadata.repo,
            metadata.base_commit,
            workspace_dir.display()
        ),
    );
    run_git_command(
        None,
        &[
            "clone",
            "--quiet",
            "--no-tags",
            "--filter=blob:none",
            repo_url.as_str(),
            workspace_dir.to_str().ok_or_else(|| {
                anyhow::anyhow!("non-utf8 workspace path {}", workspace_dir.display())
            })?,
        ],
    )?;
    run_git_command(
        Some(&workspace_dir),
        &["checkout", "--quiet", &metadata.base_commit],
    )?;
    let test_patch = sandbox_root.join("upstream").join("test.patch");
    if test_patch.exists() {
        run_git_command(
            Some(&workspace_dir),
            &[
                "apply",
                test_patch.to_str().ok_or_else(|| {
                    anyhow::anyhow!("non-utf8 patch path {}", test_patch.display())
                })?,
            ],
        )?;
    }
    run_git_command(Some(&workspace_dir), &["add", "."])?;
    run_git_command(
        Some(&workspace_dir),
        &[
            "-c",
            "user.name=quorp",
            "-c",
            "user.email=quorp@example.com",
            "commit",
            "-qm",
            "Challenge baseline",
        ],
    )?;
    Ok(())
}

fn run_git_command(cwd: Option<&Path>, args: &[&str]) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("git {} failed with status {status}", args.join(" "))
    }
}

fn reset_challenge_workspace_for_attempt(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
) -> anyhow::Result<Option<EvaluatorOutcome>> {
    if attempt_number <= 1 {
        return Ok(None);
    }

    let Some(challenge_metadata) = manifest.challenge.as_ref() else {
        anyhow::bail!("challenge metadata missing from benchmark manifest");
    };
    let reset_command = substitute_condition(
        &challenge_metadata.reset_command,
        &challenge_metadata.condition,
    );
    let reset_outcome = run_shell_command(
        "reset",
        &reset_command,
        &challenge_metadata.sandbox_root.join("reset.sh"),
        &challenge_metadata.sandbox_root,
    )?;
    if !reset_outcome.passed {
        return Ok(Some(reset_outcome));
    }

    if let Some(parent) = challenge_metadata.capsule_file.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json(
        &challenge_metadata.capsule_file,
        &challenge_metadata.capsule,
    )?;
    write_workspace_challenge_command_wrappers(&challenge_metadata.workspace_dir)?;
    ensure_git_baseline(&challenge_metadata.workspace_dir)?;
    write_benchmark_sandbox_cargo_config(
        &challenge_metadata.sandbox_root,
        &challenge_metadata.condition,
    )?;
    if manifest.executor == BenchmarkExecutor::Native {
        write_benchmark_agent_config(&challenge_metadata.workspace_dir)?;
    }
    Ok(Some(reset_outcome))
}

fn maybe_continue_challenge_attempts(
    manifest: &BenchmarkManifest,
    result_dir: &Path,
    mut attempts: Vec<AttemptReport>,
    starting_attempt: usize,
    reset_outcome: Option<EvaluatorOutcome>,
) -> anyhow::Result<()> {
    if manifest.challenge.is_none() {
        anyhow::bail!("challenge metadata missing from benchmark manifest");
    }
    let workspace_dir = &manifest.resolved.workspace_source;
    let max_attempts = if manifest.max_attempts == 0 {
        usize::MAX
    } else {
        manifest.max_attempts
    };
    let starting_attempt = starting_attempt.max(1);
    for attempt_number in starting_attempt..=max_attempts {
        let budget_used: u64 = attempts
            .iter()
            .map(|attempt| attempt.total_billed_tokens)
            .sum();
        if manifest
            .max_total_tokens
            .is_some_and(|budget| budget_used >= budget)
        {
            log_phase(
                "budget",
                ANSI_YELLOW,
                format!(
                    "skipping new attempts because token budget is exhausted: used={} budget={}",
                    budget_used,
                    manifest.max_total_tokens.unwrap_or_default()
                ),
            );
            break;
        }

        let reset_for_attempt = reset_challenge_workspace_for_attempt(manifest, attempt_number)?;
        if let Some(reset_for_attempt) = reset_for_attempt.as_ref()
            && !reset_for_attempt.passed
        {
            write_report(
                result_dir,
                manifest,
                &attempts,
                Some(reset_for_attempt.clone()),
                Some(format!(
                    "challenge workspace reset failed before attempt {attempt_number} could run"
                )),
            )?;
            return Ok(());
        }

        log_phase(
            "attempt",
            ANSI_CYAN,
            format!(
                "starting challenge attempt {} for {}",
                attempt_number, manifest.resolved.benchmark_name
            ),
        );

        let attempt_dir = attempt_dir(result_dir, attempt_number);
        let agent_result_dir = attempt_dir.join("agent");
        let bootstrap_tracker =
            BenchmarkBootstrapTracker::new(result_dir, &attempt_dir, attempt_number)?;
        fs::create_dir_all(&attempt_dir)?;
        if manifest.executor == BenchmarkExecutor::Native {
            write_benchmark_agent_config(workspace_dir)?;
        }
        let challenge_metadata = manifest
            .challenge
            .as_ref()
            .expect("challenge metadata missing after guard");
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_WORKSPACE_LAYOUT_RESOLVED,
            Some(format!(
                "challenge workspace resolved at {}",
                workspace_dir.display()
            )),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_BASELINE_RESET_READY,
            Some(format!(
                "challenge reset baseline prepared for condition {}",
                challenge_metadata.condition
            )),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_CHALLENGE_CAPSULE_REHYDRATED,
            Some(format!(
                "challenge capsule restored at {}",
                challenge_metadata.capsule_file.display()
            )),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_FAST_LOOP_CONTRACT_LOADED,
            Some(
                challenge_metadata
                    .capsule
                    .fast_loop_commands
                    .first()
                    .map(|command| format!("loaded fast loop contract `{command}`"))
                    .unwrap_or_else(|| "loaded challenge fast loop contract".to_string()),
            ),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_PROMPT_MATERIALIZED,
            Some(format!(
                "challenge prompt rooted at {}",
                manifest.resolved.objective_source.display()
            )),
        )?;

        let remaining_budget = manifest
            .max_total_tokens
            .map(|budget| budget.saturating_sub(budget_used));
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED,
            Some(format!(
                "launching challenge control loop in {}",
                agent_result_dir.display()
            )),
        )?;
        let bootstrap_watchdog = BenchmarkBootstrapWatchdog::spawn(
            manifest.clone(),
            result_dir.to_path_buf(),
            attempt_number,
            attempt_dir.clone(),
            workspace_dir.clone(),
            agent_result_dir.clone(),
            reset_outcome.clone(),
            &bootstrap_tracker,
        );
        let outcome = match run_attempt_executor(
            manifest,
            workspace_dir,
            manifest.resolved.objective_source.clone(),
            remaining_budget,
            agent_result_dir,
        ) {
            Ok(outcome) => {
                drop(bootstrap_watchdog);
                if events_file_has_first_task_model_request(&attempt_dir.join("agent/events.jsonl"))
                    .unwrap_or(false)
                {
                    let _ = bootstrap_tracker.mark_first_task_model_request();
                }
                outcome
            }
            Err(error) => {
                drop(bootstrap_watchdog);
                if let Err(report_error) = write_report(
                    result_dir,
                    manifest,
                    &attempts,
                    None,
                    Some(error.to_string()),
                ) {
                    log::error!("failed to write benchmark report after run error: {report_error}");
                }
                return Err(error);
            }
        };

        let Some(challenge_metadata) = manifest.challenge.as_ref() else {
            anyhow::bail!("challenge metadata missing from benchmark manifest");
        };
        let evaluation_target_dir =
            challenge_evaluation_target_dir(challenge_metadata, attempt_number);
        if evaluation_target_dir.exists() {
            fs::remove_dir_all(&evaluation_target_dir).with_context(|| {
                format!(
                    "failed to clean evaluation cargo target {}",
                    evaluation_target_dir.display()
                )
            })?;
        }
        let evaluation_env = challenge_evaluation_env(challenge_metadata, &evaluation_target_dir);
        let evaluation = match run_shell_command_with_env(
            "evaluation",
            &substitute_condition(
                &challenge_metadata.evaluate_command,
                &challenge_metadata.condition,
            ),
            &challenge_metadata.sandbox_root.join("evaluate.sh"),
            &challenge_metadata.sandbox_root,
            &evaluation_env,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Err(report_error) = write_report(
                    result_dir,
                    manifest,
                    &attempts,
                    reset_outcome.clone(),
                    Some(error.to_string()),
                ) {
                    log::error!(
                        "failed to write benchmark report after evaluation error: {report_error}"
                    );
                }
                return Err(error);
            }
        };

        let attempt_report = match finalize_challenge_attempt(
            manifest,
            attempt_number,
            &attempt_dir,
            outcome,
            evaluation.clone(),
        ) {
            Ok(attempt_report) => attempt_report,
            Err(error) => {
                if let Err(report_error) = write_report(
                    result_dir,
                    manifest,
                    &attempts,
                    reset_outcome.clone(),
                    Some(error.to_string()),
                ) {
                    log::error!(
                        "failed to write benchmark report after finalize error: {report_error}"
                    );
                }
                return Err(error);
            }
        };
        write_json(&attempt_dir.join("attempt-report.json"), &attempt_report)?;
        attempts.push(attempt_report.clone());
        write_report(result_dir, manifest, &attempts, reset_outcome.clone(), None)?;

        if attempt_passed(&attempt_report) {
            log_phase(
                "success",
                ANSI_GREEN,
                format!(
                    "challenge completed successfully on attempt {}",
                    attempt_number
                ),
            );
            break;
        }
    }

    write_report(result_dir, manifest, &attempts, reset_outcome, None)?;
    Ok(())
}

fn finalize_challenge_attempt(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
    attempt_dir: &Path,
    outcome: quorp_agent_core::AgentRunOutcome,
    evaluation: EvaluatorOutcome,
) -> anyhow::Result<AttemptReport> {
    let Some(challenge_metadata) = manifest.challenge.as_ref() else {
        anyhow::bail!("challenge metadata missing from benchmark manifest");
    };
    let workspace_dir = &manifest.resolved.workspace_source;
    let agent_result_dir = attempt_dir.join("agent");
    let all_changed_files = git_changed_files(workspace_dir)?;
    let ignored_changed_files = challenge_ignored_changed_files(challenge_metadata, workspace_dir);
    let changed_files = filter_ignored_changed_files(&all_changed_files, &ignored_changed_files);
    let validations = extract_validation_summaries(&agent_result_dir.join("events.jsonl"))?;
    let metrics = extract_request_metrics(&agent_result_dir.join("events.jsonl"))?;
    let control_loop = extract_control_loop_summary(&agent_result_dir.join("events.jsonl"))?;
    let usage = read_headless_usage_summary(&agent_result_dir.join("summary.json"))?;
    let routing = read_headless_routing_summary(&agent_result_dir.join("summary.json"))?;
    let validation_state =
        read_checkpoint_validation_state(&agent_result_dir.join("checkpoint.json"))?;
    let read_range_observations =
        extract_read_range_observations(&agent_result_dir.join("checkpoint.json"))?;
    let bootstrap_progress =
        read_bootstrap_progress(&attempt_bootstrap_progress_path(attempt_dir))?;
    let action_evidence = extract_action_evidence(
        &agent_result_dir.join("checkpoint.json"),
        Some(&challenge_metadata.capsule),
        Some(&challenge_metadata.evaluate_command),
    )?;
    let non_support_edit_count =
        count_non_support_changed_files(&changed_files, &ignored_changed_files);
    let widening_happened = detect_widening_against_expected(
        &changed_files,
        &challenge_metadata.expected_files_touched,
        &challenge_metadata.allowed_generated_files,
    );
    let judge = if evaluation.passed {
        let judge_context = ChallengeJudgeContext {
            manifest,
            metadata: challenge_metadata,
            attempt_number,
            attempt_dir,
            outcome: &outcome,
            evaluation: &evaluation,
            changed_files: &changed_files,
            validations: &validations,
            metrics: &metrics,
            usage: &usage,
        };
        Some(run_challenge_judge(&judge_context))
    } else {
        None
    };
    let soft_budget_inefficient = validation_state
        .local_agent_scorecard
        .first_valid_write_step
        .is_none()
        && (usage.model_requests > 8 || outcome.total_billed_tokens > 50_000);

    Ok(AttemptReport {
        attempt: attempt_number,
        executor: manifest.executor,
        model_id: manifest.model_id.clone(),
        safety_mode_label: manifest.safety_mode_label.clone(),
        scenario_label: manifest.scenario_label.clone(),
        agent_stop_reason: outcome.stop_reason,
        agent_error_message: outcome.error_message,
        total_steps: outcome.total_steps,
        duration_ms: outcome.duration_ms,
        total_billed_tokens: outcome.total_billed_tokens,
        max_prompt_token_estimate: metrics.max_prompt_token_estimate,
        max_completion_token_cap: metrics.max_completion_token_cap,
        watchdog_near_limit: metrics.watchdog_near_limit,
        watchdog_triggered: metrics.watchdog_triggered,
        visible_evaluation: None,
        collector_evaluation: None,
        evaluation: Some(evaluation),
        changed_files,
        ignored_changed_files,
        validations,
        widening_happened,
        attempt_dir: attempt_dir.to_path_buf(),
        workspace_dir: workspace_dir.clone(),
        agent_result_dir,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        model_requests: usage.model_requests,
        first_request_prompt_token_estimate: metrics.first_request_prompt_token_estimate,
        first_request_raw_prompt_token_estimate: metrics.first_request_raw_prompt_token_estimate,
        first_request_compacted_prompt_token_estimate: metrics
            .first_request_compacted_prompt_token_estimate,
        first_request_first_token_latency_ms: metrics.first_request_first_token_latency_ms,
        first_model_turn_started: metrics.first_model_turn_started,
        first_action_emitted: metrics.first_action_emitted,
        prompt_token_series_by_turn: metrics.prompt_token_series_by_turn,
        read_range_observations,
        read_count: action_evidence.read_count,
        write_count: action_evidence.write_count,
        command_execution_count: action_evidence.command_execution_count,
        parser_recovery_count: validation_state.local_agent_scorecard.parser_recovery_count,
        repair_invalid_action_streak_max: validation_state
            .local_agent_scorecard
            .repair_invalid_action_streak_max,
        repair_submode_entered: validation_state
            .local_agent_scorecard
            .repair_submode_entered,
        repair_submode_turns: validation_state.local_agent_scorecard.repair_submode_turns,
        repair_write_locked: validation_state.local_agent_scorecard.repair_write_locked,
        write_phase_action_refusal_count: validation_state
            .local_agent_scorecard
            .write_phase_action_refusal_count,
        patch_scaffold_offered: validation_state
            .local_agent_scorecard
            .patch_scaffold_offered,
        patch_scaffold_honored: validation_state
            .local_agent_scorecard
            .patch_scaffold_honored,
        preview_apply_locked: validation_state.local_agent_scorecard.preview_apply_locked,
        preview_apply_action_refusal_count: validation_state
            .local_agent_scorecard
            .preview_apply_action_refusal_count,
        write_phase_write_emitted: validation_state
            .local_agent_scorecard
            .write_phase_write_emitted,
        bootstrap_phase: bootstrap_progress
            .as_ref()
            .map(|progress| progress.bootstrap_phase.clone()),
        bootstrap_phase_detail: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_phase_detail.clone()),
        first_task_model_request_seen: bootstrap_progress
            .as_ref()
            .is_some_and(|progress| progress.first_task_model_request_seen)
            || metrics.first_model_turn_started,
        bootstrap_elapsed_ms_before_first_task_request: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_elapsed_ms_before_first_task_request),
        pre_model_bootstrap_stalled: bootstrap_progress
            .as_ref()
            .is_some_and(|progress| progress.pre_model_bootstrap_stalled),
        bootstrap_stall_class: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_stall_class.clone()),
        rolled_back_write_count: validation_state
            .local_agent_scorecard
            .rolled_back_write_count,
        rolled_back_non_support_edit_count: validation_state
            .local_agent_scorecard
            .rolled_back_non_support_edit_count,
        soft_budget_inefficient,
        fast_loop_command_seen: action_evidence.fast_loop_command_seen,
        agent_final_evaluate_command_seen: action_evidence.final_evaluate_command_seen,
        final_evaluate_command_seen: action_evidence.final_evaluate_command_seen,
        host_evaluation_commands_run: 1,
        non_support_edit_count,
        repo_capsule_injected: manifest.completion_policy.include_repo_capsule,
        reasoning_enabled: !manifest.completion_policy.disable_reasoning,
        path_resolution_failures: control_loop.path_resolution_failures,
        recovery_turns: control_loop.recovery_turns,
        action_contract_mode: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_selected: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_fallback_reason: std::env::var(
            "QUORP_BENCH_ACTION_CONTRACT_FALLBACK_REASON",
        )
        .ok()
        .filter(|value| !value.trim().is_empty()),
        attempt_lineage: benchmark_attempt_lineage(&manifest.completion_policy),
        effective_prompt_compaction_policy: manifest
            .completion_policy
            .prompt_compaction_policy
            .map(|policy| policy.as_str().to_string()),
        fast_loop_validation_status: validation_state.validation_status,
        last_validation_failure: validation_state.last_validation_failure,
        failing_test_names: validation_state.failing_test_names,
        primary_failure_test_name: validation_state.primary_failure_test_name,
        primary_failure_path: validation_state.primary_failure_path,
        primary_failure_line: validation_state.primary_failure_line,
        assertion_excerpt: validation_state.assertion_excerpt,
        diagnostic_class: validation_state.diagnostic_class,
        implementation_target_lease: validation_state.implementation_target_lease,
        dependency_candidates: validation_state.dependency_candidates,
        target_dependency_table: validation_state.target_dependency_table,
        repair_required: validation_state.repair_required,
        repair_phase_terminal: validation_state.repair_phase_terminal,
        failure_anchor_reread_attempted: validation_state.failure_anchor_reread_attempted,
        failure_anchor_reread_honored: validation_state.failure_anchor_reread_honored,
        implementation_reread_allowed: validation_state.implementation_reread_allowed,
        implementation_reread_attempted: validation_state.implementation_reread_attempted,
        implementation_reread_honored: validation_state.implementation_reread_honored,
        repair_phase_invalid_action_count: validation_state.repair_phase_invalid_action_count,
        post_fast_loop_patch_attempted: validation_state.post_fast_loop_patch_attempted,
        post_fast_loop_validation_rerun_attempted: validation_state
            .post_fast_loop_validation_rerun_attempted,
        patch_packet_injected: validation_state.patch_packet_injected,
        patch_packet_honored_range: validation_state.patch_packet_honored_range,
        recommended_rerun_command: validation_state.recommended_rerun_command,
        fast_loop_rerun_match_kind: validation_state.fast_loop_rerun_match_kind,
        failed_edit_records: validation_state.failed_edit_records,
        local_model_memory: validation_state.local_model_memory,
        local_agent_scorecard: validation_state.local_agent_scorecard,
        planner_model: None,
        executor_model: Some(manifest.model_id.clone()),
        judge,
        routing,
    })
}

fn compile_challenge_capsule(
    challenge: &ResolvedChallengeCase,
    sandbox_root: &Path,
) -> anyhow::Result<ChallengeCapsule> {
    let start_here = fs::read_to_string(&challenge.objective_source)
        .with_context(|| format!("failed to read {}", challenge.objective_source.display()))?;
    let local_repro_path = sandbox_root.join("LOCAL_REPRO.md");
    let local_repro = fs::read_to_string(&local_repro_path)
        .with_context(|| format!("failed to read {}", local_repro_path.display()))?;

    let start_fast_loop =
        extract_markdown_code_blocks(&extract_markdown_section(&start_here, "Fast Loop"));
    let repro_fast_loop =
        extract_markdown_code_blocks(&extract_markdown_section(&local_repro, "Fast Loop"));
    let owner_files =
        extract_path_like_items(&extract_markdown_section(&start_here, "Likely Owners"));
    let first_reads =
        extract_path_like_items(&extract_markdown_section(&local_repro, "First Reads"));
    let expected_touch_targets = challenge.manifest.expected_files_touched.clone();
    let companion_files_required = expected_touch_targets
        .iter()
        .filter(|path| is_companion_file(path))
        .cloned()
        .collect::<Vec<_>>();
    let strong_hints =
        extract_markdown_bullets(&extract_markdown_section(&start_here, "Strong Hints"));
    let watch_points =
        extract_markdown_bullets(&extract_markdown_section(&local_repro, "What To Watch"));
    let named_tests = watch_points
        .iter()
        .chain(strong_hints.iter())
        .flat_map(|item| extract_inline_code_spans(item))
        .filter(|item| !looks_like_path(item))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let case_class = classify_case_class(&expected_touch_targets, &companion_files_required);

    let capsule = ChallengeCapsule {
        case_class,
        owner_files,
        first_reads,
        fast_loop_commands: start_fast_loop
            .into_iter()
            .chain(repro_fast_loop)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        expected_touch_targets,
        companion_files_required,
        strong_hints,
        watch_points,
        named_tests,
    };
    Ok(apply_rust_swe_case_profile(capsule, &challenge.manifest.id))
}

fn rust_swe_case_profile(case_id: &str) -> Option<RustSweCaseProfile> {
    const PROFILES: &[RustSweCaseProfile] = &[
        RustSweCaseProfile {
            case_id: "06-rust-swebench-bincode-serde-decoder-memory",
            fast_loop_commands: &["cargo test --quiet --features serde --test issues issue_474"],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/features/serde/de_owned.rs"],
            expected_touch_targets: &["src/features/serde/de_owned.rs", "Cargo.toml"],
        },
        RustSweCaseProfile {
            case_id: "07-rust-swebench-chrono-epoch-truncation",
            fast_loop_commands: &["cargo test --quiet --lib round::tests::"],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/round.rs"],
            expected_touch_targets: &["src/round.rs"],
        },
        RustSweCaseProfile {
            case_id: "08-rust-swebench-axum-fallback-merge",
            fast_loop_commands: &[
                "cargo test --quiet -p axum --lib --features headers routing::tests::",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["axum/src/routing/mod.rs"],
            expected_touch_targets: &[
                "axum/src/routing/mod.rs",
                "axum/CHANGELOG.md",
                "axum/src/docs/routing/fallback.md",
                "axum/src/docs/routing/merge.md",
                "axum/src/docs/routing/nest.md",
            ],
        },
        RustSweCaseProfile {
            case_id: "09-rust-swebench-cargo-dist-create-release",
            fast_loop_commands: &[
                "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &[
                "cargo-dist/src/backend/ci/github.rs",
                "cargo-dist/src/config.rs",
                "cargo-dist/src/init.rs",
                "cargo-dist/src/tasks.rs",
                "cargo-dist/templates/ci/github_ci.yml.j2",
            ],
            expected_touch_targets: &[
                "cargo-dist/src/backend/ci/github.rs",
                "cargo-dist/src/config.rs",
                "cargo-dist/src/init.rs",
                "cargo-dist/src/tasks.rs",
                "cargo-dist/templates/ci/github_ci.yml.j2",
            ],
        },
        RustSweCaseProfile {
            case_id: "10-rust-swebench-cc-rs-compile-intermediates",
            fast_loop_commands: &[
                "cargo test --quiet compile_intermediates",
                "cargo test --quiet gnu_smoke",
                "cargo test --quiet msvc_smoke",
            ],
            final_eval_command: "./evaluate.sh proof-full",
            likely_owner_files: &["src/lib.rs"],
            expected_touch_targets: &["src/lib.rs"],
        },
    ];
    PROFILES
        .iter()
        .find(|profile| profile.case_id == case_id)
        .copied()
}

fn apply_rust_swe_case_profile(mut capsule: ChallengeCapsule, case_id: &str) -> ChallengeCapsule {
    let Some(profile) = rust_swe_case_profile(case_id) else {
        return capsule;
    };
    extend_unique(
        &mut capsule.fast_loop_commands,
        profile
            .fast_loop_commands
            .iter()
            .map(|value| (*value).to_string()),
    );
    extend_unique(
        &mut capsule.owner_files,
        profile
            .likely_owner_files
            .iter()
            .map(|value| (*value).to_string()),
    );
    extend_unique(
        &mut capsule.expected_touch_targets,
        profile
            .expected_touch_targets
            .iter()
            .map(|value| (*value).to_string()),
    );
    capsule
        .strong_hints
        .push(format!("Final evaluator: `{}`", profile.final_eval_command));
    capsule
}

fn extend_unique(target: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    let mut seen = target.iter().cloned().collect::<BTreeSet<_>>();
    for value in values {
        if seen.insert(value.clone()) {
            target.push(value);
        }
    }
}

fn extract_markdown_section(markdown: &str, heading: &str) -> String {
    let mut capturing = false;
    let mut lines = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            if trimmed.trim_start_matches("## ").trim() == heading {
                capturing = true;
                continue;
            }
            if capturing {
                break;
            }
        }
        if capturing {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_string()
}

fn extract_markdown_bullets(section: &str) -> Vec<String> {
    section
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- ").map(str::trim))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn extract_markdown_code_blocks(section: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut capturing = false;
    let mut current = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if capturing {
                let block = current.join("\n").trim().to_string();
                if !block.is_empty() {
                    blocks.push(block);
                }
                current.clear();
                capturing = false;
            } else {
                capturing = true;
            }
            continue;
        }
        if capturing {
            current.push(trimmed.to_string());
        }
    }
    blocks
}

fn extract_path_like_items(section: &str) -> Vec<String> {
    let mut items = Vec::new();
    for bullet in extract_markdown_bullets(section) {
        let inline_paths = extract_inline_code_spans(&bullet)
            .into_iter()
            .filter(|item| looks_like_path(item))
            .collect::<Vec<_>>();
        if !inline_paths.is_empty() {
            items.extend(inline_paths);
            continue;
        }
        if looks_like_path(&bullet) {
            items.push(normalize_markdown_item(&bullet));
        }
    }
    items
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn extract_inline_code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character == '`' {
            if let Some(open_index) = start.take() {
                let value = text[open_index + 1..index].trim();
                if !value.is_empty() {
                    spans.push(value.to_string());
                }
            } else {
                start = Some(index);
            }
        }
    }
    spans
}

fn looks_like_path(value: &str) -> bool {
    let trimmed = normalize_markdown_item(value);
    trimmed.contains('/')
        || trimmed.ends_with(".rs")
        || trimmed.ends_with(".md")
        || trimmed.ends_with(".toml")
        || trimmed.ends_with(".j2")
        || trimmed.ends_with(".json")
        || trimmed.ends_with(".yml")
}

fn normalize_markdown_item(value: &str) -> String {
    value
        .trim()
        .trim_matches('`')
        .trim_matches('.')
        .trim_matches(',')
        .trim()
        .to_string()
}

fn is_companion_file(path: &str) -> bool {
    path.contains("CHANGELOG")
        || path.contains("book/")
        || path.contains("docs/")
        || path.contains("templates/")
        || path.ends_with(".j2")
        || path.ends_with(".md")
}

fn classify_case_class(
    expected_touch_targets: &[String],
    companion_files_required: &[String],
) -> String {
    if expected_touch_targets.len() <= 2 && companion_files_required.is_empty() {
        "narrow-owner-first".to_string()
    } else if !companion_files_required.is_empty() && expected_touch_targets.len() >= 5 {
        "breadth-heavy-companion".to_string()
    } else if !companion_files_required.is_empty() {
        "companion-sensitive".to_string()
    } else {
        "multi-layer".to_string()
    }
}

fn build_challenge_objective(
    challenge: &ResolvedChallengeCase,
    metadata: &ChallengeMetadata,
) -> anyhow::Result<String> {
    let objective = fs::read_to_string(&challenge.objective_source)
        .with_context(|| format!("failed to read {}", challenge.objective_source.display()))?;
    let success = fs::read_to_string(&challenge.success_source)
        .with_context(|| format!("failed to read {}", challenge.success_source.display()))?;
    let objective_display =
        workspace_relative_display_path(&metadata.workspace_dir, &metadata.objective_file);
    let success_display =
        workspace_relative_display_path(&metadata.workspace_dir, &metadata.success_file);
    let mirrored_briefing_files = [
        Some(objective_display.clone()),
        Some(success_display.clone()),
        metadata
            .reference_file
            .as_ref()
            .map(|path| workspace_relative_display_path(&metadata.workspace_dir, path)),
        Some("benchmark.json".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let mut sections = vec![
        format!(
            "# Quorp Challenge Objective\n\nYou are running challenge `{}`: {}.\nKeep working until the case evaluator passes or you hit a budget stop.",
            challenge.manifest.id, challenge.manifest.title
        ),
        format!(
            "## Workspace\n- Editable workspace root: `.`\n- Condition: `{}`\n- Mirrored briefing files: {}\n- Do not modify files outside the workspace root.",
            metadata.condition, mirrored_briefing_files
        ),
        format!(
            "## Workspace Path Rules\n- All tool paths must be relative to the workspace root.\n- Do not use absolute paths in tool calls.\n- If you need orientation, start with `ListDirectory` on `.`.\n- Prefer the expected touch targets before top-level metadata files.\n- Avoid rereading `AGENTS.md`, `Cargo.lock`, `README.md`, or other root metadata unless the brief explicitly requires them.\n- Workspace root entries:\n{}",
            summarize_workspace_root(&metadata.workspace_dir)
        ),
        format!(
            "## Objective\n- File: `{}`\n- Inline summary:\n{}",
            objective_display,
            summarize_markdown_brief(&objective)
        ),
        format!(
            "## Success Criteria\n- File: `{}`\n- Inline summary:\n{}",
            success_display,
            summarize_markdown_brief(&success)
        ),
        format!(
            "## Commands\n- Reset: `{}`\n- Evaluate: `{}`\n- Stop when the evaluate command reports success.",
            substitute_condition(&challenge.manifest.reset_command, &challenge.condition),
            substitute_condition(&challenge.manifest.evaluate_command, &challenge.condition)
        ),
        format!(
            "## Expected Touch Targets\n{}",
            challenge
                .manifest
                .expected_files_touched
                .iter()
                .map(|path| format!("- `{}`", path))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        format!(
            "## Primary Metrics\n{}",
            challenge
                .manifest
                .primary_metrics
                .iter()
                .map(|metric| format!("- `{metric}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        format!(
            "## Challenge Capsule\n- Case class: `{}`\n- Primary owner files:\n{}\n- First reads:\n{}\n- Fast loop commands:\n{}\n- Companion files required:\n{}\n- Named tests/assertions to keep in view:\n{}\n- Strong hints:\n{}\n- Watch points:\n{}",
            metadata.capsule.case_class,
            render_bullet_list_or_none(&metadata.capsule.owner_files),
            render_bullet_list_or_none(&metadata.capsule.first_reads),
            render_bullet_list_or_none(&metadata.capsule.fast_loop_commands),
            render_bullet_list_or_none(&metadata.capsule.companion_files_required),
            render_bullet_list_or_none(&metadata.capsule.named_tests),
            render_bullet_list_or_none(&metadata.capsule.strong_hints),
            render_bullet_list_or_none(&metadata.capsule.watch_points)
        ),
        format!(
            "## Validation Ladder\n- First prove progress with the fast loop before full evaluation.\n{}\n- After any failed validation, summarize the failing test/assertion, patch or read the next owner file, and rerun the smallest relevant validation before widening.\n- Run `{}` only after the fast loop is green or the failure clearly requires broader validation.",
            metadata
                .capsule
                .fast_loop_commands
                .iter()
                .map(|command| format!("- Fast loop: `{command}`"))
                .collect::<Vec<_>>()
                .join("\n"),
            substitute_condition(&challenge.manifest.evaluate_command, &challenge.condition)
        ),
    ];
    if !metadata.capsule.companion_files_required.is_empty() {
        sections.push(format!(
            "## Companion File Sentinel\n- This case requires companion-file coverage in addition to code changes.\n{}\n- Do not stop before these surfaces are updated or deliberately ruled out by the brief and tests.",
            metadata
                .capsule
                .companion_files_required
                .iter()
                .map(|path| format!("- `{path}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if metadata.capsule.case_class == "narrow-owner-first" {
        sections.push(
            "## Narrow-Case Mode\n- Do not widen beyond the primary owner files and named tests until the fast loop proves the local hypothesis wrong."
                .to_string(),
        );
    }
    if !challenge.manifest.allowed_generated_files.is_empty() {
        sections.push(format!(
            "## Allowed Generated Files\n{}",
            challenge
                .manifest
                .allowed_generated_files
                .iter()
                .map(|path| format!("- `{}`", path))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if let Some(reference_file) = metadata.reference_file.as_ref() {
        let reference_display =
            workspace_relative_display_path(&metadata.workspace_dir, reference_file);
        let reference = fs::read_to_string(reference_file)
            .with_context(|| format!("failed to read {}", reference_file.display()))?;
        sections.push(format!(
            "## Reference\n- File: `{}`\n- Inline summary:\n{}",
            reference_display,
            summarize_markdown_brief(&reference)
        ));
    }
    if !challenge.manifest.tags.is_empty() {
        sections.push(format!(
            "## Tags\n{}",
            challenge
                .manifest
                .tags
                .iter()
                .map(|tag| format!("- `{tag}`"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(sections.join("\n\n"))
}

fn run_challenge_judge(context: &ChallengeJudgeContext<'_>) -> ChallengeJudgeOutcome {
    let judge_prompt = build_challenge_judge_prompt(context);
    let judge_record = serde_json::json!({
        "attempt": context.attempt_number,
        "prompt": judge_prompt,
        "model_id": context.manifest.model_id,
        "workspace": context.metadata.workspace_dir,
        "evaluation": context.evaluation,
        "changed_files": context.changed_files,
        "validations": context.validations,
        "metrics": {
            "prompt_estimate": context.metrics.max_prompt_token_estimate,
            "completion_cap": context.metrics.max_completion_token_cap,
            "watchdog_near_limit": context.metrics.watchdog_near_limit,
            "watchdog_triggered": context.metrics.watchdog_triggered,
        },
        "usage": context.usage,
        "stop_reason": context.outcome.stop_reason,
    });
    if let Err(error) = write_json(
        &context.attempt_dir.join("judge-request.json"),
        &judge_record,
    ) {
        log::warn!(
            "failed to write challenge judge request for attempt {}: {error}",
            context.attempt_number
        );
    }

    let max_judge_attempts = if context.manifest.executor == BenchmarkExecutor::Native {
        3
    } else {
        1
    };
    let mut result = None;
    for judge_attempt in 1..=max_judge_attempts {
        let attempt_result = request_challenge_judge_completion(context, &judge_prompt);
        match attempt_result {
            Ok(completion) => {
                result = Some(Ok(completion));
                break;
            }
            Err(error)
                if judge_attempt < max_judge_attempts
                    && transient_challenge_judge_error(&error) =>
            {
                log::warn!(
                    "challenge judge attempt {judge_attempt}/{max_judge_attempts} failed transiently: {error}"
                );
                std::thread::sleep(Duration::from_secs(2 * judge_attempt as u64));
            }
            Err(error) => {
                result = Some(Err(error));
                break;
            }
        }
    }

    let result = result.unwrap_or_else(|| Err("judge request did not run".to_string()));

    match result {
        Ok((content, raw_response)) => {
            let parsed = parse_challenge_judge_response(&content);
            let mut outcome = parsed.unwrap_or_else(|error| ChallengeJudgeOutcome {
                passed: false,
                summary: "judge response could not be parsed".to_string(),
                rationale: error,
                model_id: context.manifest.model_id.clone(),
                raw_response: serde_json::json!({
                    "content": content,
                    "raw_response": raw_response,
                }),
                error: None,
            });
            outcome.model_id = context.manifest.model_id.clone();
            if let Err(write_error) = write_json(
                &context.attempt_dir.join("judge-response.json"),
                &serde_json::json!({
                    "content": content,
                    "raw_response": raw_response,
                    "parsed": outcome,
                }),
            ) {
                log::warn!(
                    "failed to write challenge judge response for attempt {}: {write_error}",
                    context.attempt_number
                );
            }
            outcome
        }
        Err(error) => {
            if let Err(write_error) = write_json(
                &context.attempt_dir.join("judge-response.json"),
                &serde_json::json!({
                    "error": error,
                }),
            ) {
                log::warn!(
                    "failed to write challenge judge failure for attempt {}: {write_error}",
                    context.attempt_number
                );
            }
            ChallengeJudgeOutcome {
                passed: false,
                summary: "judge request failed".to_string(),
                rationale: error,
                model_id: context.manifest.model_id.clone(),
                raw_response: serde_json::json!({}),
                error: None,
            }
        }
    }
}

fn request_challenge_judge_completion(
    context: &ChallengeJudgeContext<'_>,
    judge_prompt: &str,
) -> Result<(String, serde_json::Value), String> {
    match context.manifest.executor {
        BenchmarkExecutor::Native => {
            let runtime = tokio::runtime::Runtime::new();
            match runtime {
                Ok(runtime) => runtime.block_on(async {
                    let ssd_moe_runtime = SsdMoeRuntimeHandle::shared_handle();
                    let request = StreamRequest {
                        request_id: crate::quorp::tui::diagnostics::next_request_id(),
                        session_id: context.attempt_number,
                        model_id: context.manifest.model_id.clone(),
                        agent_mode: quorp_agent_core::agent_protocol::AgentMode::Ask,
                        latest_input: judge_prompt.to_string(),
                        messages: vec![ChatServiceMessage {
                            role: ChatServiceRole::User,
                            content: judge_prompt.to_string(),
                        }],
                        project_root: context.metadata.workspace_dir.clone(),
                        base_url_override: context.manifest.base_url_override.clone(),
                        max_completion_tokens: Some(512),
                        include_repo_capsule: false,
                        disable_reasoning: true,
                        native_tool_calls: false,
                        watchdog: Some(quorp_agent_core::CompletionWatchdogConfig {
                            first_token_timeout_ms: Some(30_000),
                            idle_timeout_ms: Some(20_000),
                            total_timeout_ms: Some(90_000),
                        }),
                        safety_mode_label: Some(context.manifest.safety_mode_label.clone()),
                        prompt_compaction_policy: None,
                        capture_scope: Some("evaluation".to_string()),
                        capture_call_class: Some("evaluation".to_string()),
                    };
                    request_single_completion_details(&ssd_moe_runtime, &request)
                        .await
                        .map(|completion| (completion.content, completion.raw_response))
                }),
                Err(error) => Err(error.to_string()),
            }
        }
        BenchmarkExecutor::Codex => request_codex_completion(CodexCompletionOptions {
            workspace: context.metadata.workspace_dir.clone(),
            prompt: judge_prompt.to_string(),
            model_id: context.manifest.model_id.clone(),
            max_seconds: Some(180),
            artifact_dir: context.attempt_dir.join("judge-artifacts"),
            session_strategy: fresh_session_strategy(),
        })
        .map(|completion| (completion.content, completion.raw_response))
        .map_err(|error| error.to_string()),
    }
}

fn transient_challenge_judge_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("first token timeout")
        || normalized.contains("timeout")
        || normalized.contains("503")
        || normalized.contains("service unavailable")
        || normalized.contains("resourceexhausted")
        || normalized.contains("workers are busy")
}

fn build_challenge_judge_prompt(context: &ChallengeJudgeContext<'_>) -> String {
    format!(
        r#"You are the final quality judge for a coding challenge run.

Decide whether the agent actually satisfied the challenge objective and success criteria.
Be strict: if the evidence is incomplete, contradictory, or the evaluation result does not support success, mark the run as failed.
Return a strict JSON object only with this schema:
{{"passed":bool,"summary":"short summary","rationale":"short rationale"}}

## Challenge
- Case: `{case_id}` - {title}
- Attempt: `{attempt_number}`
- Condition: `{condition}`
- Workspace: `{workspace}`
- Objective file: `{objective_file}`
- Success file: `{success_file}`

## Evidence
- Agent stop reason: `{stop_reason:?}`
- Agent total steps: `{total_steps}`
- Agent billed tokens: `{agent_tokens}`
- Evaluation passed: `{evaluation_passed}`
- Evaluation command: `{evaluation_command}`
- Evaluation exit code: `{evaluation_exit_code}`
- Evaluation stdout:
{evaluation_stdout}
- Evaluation stderr:
{evaluation_stderr}
- Changed files: `{changed_files}`
- Validations: `{validations}`
- Request metrics: prompt_estimate={prompt_estimate:?} completion_cap={completion_cap:?} watchdog_near_limit={watchdog_near_limit} watchdog_triggered={watchdog_triggered}
- Usage: requests={model_requests} input={input_tokens} output={output_tokens} reasoning={reasoning_tokens} cache_read={cache_read} cache_write={cache_write}

## Success criteria summary
{success_summary}

## Objective summary
{objective_summary}
"#,
        case_id = context.manifest.resolved.issue_id,
        title = context.manifest.resolved.benchmark_name,
        attempt_number = context.attempt_number,
        condition = context.metadata.condition,
        workspace = context.metadata.workspace_dir.display(),
        objective_file = context.metadata.objective_file.display(),
        success_file = context.metadata.success_file.display(),
        stop_reason = context.outcome.stop_reason,
        total_steps = context.outcome.total_steps,
        agent_tokens = context.outcome.total_billed_tokens,
        evaluation_passed = context.evaluation.passed,
        evaluation_command = context.evaluation.command.as_deref().unwrap_or_default(),
        evaluation_exit_code = context.evaluation.exit_code,
        evaluation_stdout = indent_block(&summarize_judge_output(&context.evaluation.stdout)),
        evaluation_stderr = indent_block(&summarize_judge_output(&context.evaluation.stderr)),
        changed_files = context.changed_files.join(", "),
        validations = context.validations.join(" | "),
        prompt_estimate = context.metrics.max_prompt_token_estimate,
        completion_cap = context.metrics.max_completion_token_cap,
        watchdog_near_limit = context.metrics.watchdog_near_limit,
        watchdog_triggered = context.metrics.watchdog_triggered,
        model_requests = context.usage.model_requests,
        input_tokens = context.usage.input_tokens,
        output_tokens = context.usage.output_tokens,
        reasoning_tokens = context.usage.reasoning_tokens,
        cache_read = context.usage.cache_read_input_tokens,
        cache_write = context.usage.cache_write_input_tokens,
        success_summary = indent_block(
            &fs::read_to_string(&context.metadata.success_file)
                .unwrap_or_else(|_| String::from("<unable to read success criteria>"))
        ),
        objective_summary = indent_block(
            &fs::read_to_string(&context.metadata.objective_file)
                .unwrap_or_else(|_| String::from("<unable to read objective>"))
        ),
    )
}

fn parse_challenge_judge_response(content: &str) -> Result<ChallengeJudgeOutcome, String> {
    let trimmed = content.trim();
    let candidate = extract_json_object(trimmed).unwrap_or(trimmed);
    let value: serde_json::Value = serde_json::from_str(candidate)
        .map_err(|error| format!("judge response was not valid JSON: {error}"))?;
    let passed = value
        .get("passed")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "judge response missing `passed`".to_string())?;
    let summary = value
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("no summary provided")
        .to_string();
    let rationale = value
        .get("rationale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("no rationale provided")
        .to_string();
    Ok(ChallengeJudgeOutcome {
        passed,
        summary,
        rationale,
        model_id: String::new(),
        raw_response: value,
        error: None,
    })
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start > end {
        return None;
    }
    Some(&text[start..=end])
}

fn collect_challenge_context_files(
    _sandbox_root: &Path,
    metadata: &ChallengeMetadata,
) -> Vec<PathBuf> {
    vec![
        Some(metadata.workspace_dir.join("benchmark.json")),
        Some(metadata.objective_file.clone()),
        Some(metadata.success_file.clone()),
        metadata.reference_file.clone(),
        Some(metadata.capsule_file.clone()),
        Some(metadata.workspace_dir.join("AGENTS.md")),
        Some(metadata.workspace_dir.join("agent-map.json")),
        Some(metadata.workspace_dir.join("test-map.json")),
        Some(
            metadata
                .workspace_dir
                .join(".witness")
                .join("witness-graph.json"),
        ),
    ]
    .into_iter()
    .flatten()
    .filter(|path| path.exists())
    .collect()
}

fn workspace_relative_display_path(workspace_dir: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_dir)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn run_shell_command(
    name: &str,
    command: &str,
    script: &Path,
    current_dir: &Path,
) -> anyhow::Result<EvaluatorOutcome> {
    run_shell_command_with_env(name, command, script, current_dir, &[])
}

fn run_shell_command_with_env(
    name: &str,
    command: &str,
    script: &Path,
    current_dir: &Path,
    environment: &[(&str, &std::ffi::OsStr)],
) -> anyhow::Result<EvaluatorOutcome> {
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let mut shell = Command::new("bash");
    shell.arg("-lc").arg(command).current_dir(current_dir);
    for (key, value) in environment {
        shell.env(key, value);
    }
    let output = shell
        .output()
        .with_context(|| format!("failed to run {} command `{}`", name, command))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: name.to_string(),
        script: script.to_path_buf(),
        command: Some(command.to_string()),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

fn challenge_evaluation_target_dir(
    challenge_metadata: &ChallengeMetadata,
    attempt_number: usize,
) -> PathBuf {
    challenge_metadata
        .sandbox_root
        .parent()
        .unwrap_or(&challenge_metadata.sandbox_root)
        .join(CHALLENGE_EVALUATION_CARGO_CACHE_DIR)
        .join(&challenge_metadata.condition)
        .join(format!("attempt-{attempt_number:03}"))
}

fn challenge_evaluation_env<'a>(
    challenge_metadata: &ChallengeMetadata,
    evaluation_target_dir: &'a Path,
) -> Vec<(&'static str, &'a OsStr)> {
    let mut env = Vec::new();
    if challenge_evaluation_needs_sdkroot_override(challenge_metadata) {
        env.push(("SDKROOT", Path::new("/").as_os_str()));
    }
    if challenge_evaluation_is_cargo_dist_snapshot_sensitive(challenge_metadata) {
        env
    } else {
        env.push(("CARGO_TARGET_DIR", evaluation_target_dir.as_os_str()));
        env
    }
}

fn challenge_evaluation_is_cargo_dist_snapshot_sensitive(
    challenge_metadata: &ChallengeMetadata,
) -> bool {
    challenge_metadata
        .allowed_generated_files
        .iter()
        .any(|path| path == "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap")
}

fn challenge_evaluation_needs_sdkroot_override(challenge_metadata: &ChallengeMetadata) -> bool {
    challenge_metadata
        .case_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "05-cc-rs-compile-intermediates")
        || (challenge_metadata.tags.iter().any(|tag| tag == "cc-rs")
            && challenge_metadata
                .expected_files_touched
                .iter()
                .any(|path| path == "src/lib.rs"))
}

fn evaluator_passed(exit_success: bool, stdout: &str) -> bool {
    if let Some(summary) = parse_benchmark_summary_value(stdout)
        && let Some(success) = summary.get("success").and_then(serde_json::Value::as_bool)
    {
        return exit_success && success;
    }
    exit_success
}

fn parse_benchmark_summary_value(stdout: &str) -> Option<serde_json::Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let candidate = trimmed.get(start..)?.trim();
    serde_json::from_str::<serde_json::Value>(candidate).ok()
}

fn read_headless_usage_summary(
    path: &Path,
) -> anyhow::Result<crate::quorp::agent_local::HeadlessUsageSummary> {
    if !path.exists() {
        return Ok(crate::quorp::agent_local::HeadlessUsageSummary::default());
    }
    let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let usage = summary
        .get("usage")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::from_value(usage).unwrap_or_default())
}

fn read_headless_routing_summary(
    path: &Path,
) -> anyhow::Result<crate::quorp::agent_local::RoutingSummary> {
    if !path.exists() {
        return Ok(crate::quorp::agent_local::RoutingSummary::default());
    }
    let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let routing = summary
        .get("routing")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::from_value(routing).unwrap_or_default())
}

fn load_seed_context(path: Option<&Path>) -> anyhow::Result<Vec<TranscriptMessage>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", path.display()))?;
    let messages = value
        .get("checkpoints")
        .and_then(serde_json::Value::as_array)
        .and_then(|checkpoints| checkpoints.last())
        .and_then(|checkpoint| checkpoint.get("messages"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("seed transcript {} is missing checkpoints", path.display())
        })?;
    let mut transcript = Vec::new();
    for message in messages {
        let role = match message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("user")
        {
            "system" => TranscriptRole::System,
            "assistant" => TranscriptRole::Assistant,
            _ => TranscriptRole::User,
        };
        let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        transcript.push(TranscriptMessage {
            role,
            content: content.to_string(),
        });
    }
    Ok(transcript)
}

fn substitute_condition(command: &str, condition: &str) -> String {
    command.replace("<condition>", condition)
}

fn run_attempt_executor(
    manifest: &BenchmarkManifest,
    workspace: &Path,
    objective_file: PathBuf,
    remaining_budget: Option<u64>,
    result_dir: PathBuf,
) -> anyhow::Result<quorp_agent_core::AgentRunOutcome> {
    let seed_context = load_seed_context(manifest.seed_transcript.as_deref())?;
    match manifest.executor {
        BenchmarkExecutor::Native => run_headless_agent(HeadlessRunOptions {
            workspace: workspace.to_path_buf(),
            objective_file,
            executor: crate::quorp::executor::QuorpExecutor::Native,
            codex_session_strategy: fresh_session_strategy(),
            model_id: manifest.model_id.clone(),
            base_url_override: manifest.base_url_override.clone(),
            max_steps: manifest.max_steps,
            max_seconds: manifest.max_seconds,
            max_total_tokens: remaining_budget,
            result_dir,
            autonomy_profile: parse_autonomy_profile(&manifest.autonomy_profile)?,
            completion_policy: manifest.completion_policy.clone(),
            objective_metadata: serde_json::json!({
                "benchmark_mode": true,
                "benchmark_transcript_compression": true,
                "objective_file": manifest.resolved.objective_source.clone(),
                "evaluate_command": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.evaluate_command.clone())
                    .or_else(|| manifest.resolved.visible_evaluator.as_ref().map(|path| path.display().to_string())),
                "context_files": manifest.resolved.context_files.clone(),
                "repair_artifacts": manifest.resolved.repair_artifacts.clone(),
                "benchmark_name": manifest.resolved.benchmark_name.clone(),
                "issue_id": manifest.resolved.issue_id.clone(),
                "repo_capsule_injected": manifest.completion_policy.include_repo_capsule,
                "reasoning_enabled": !manifest.completion_policy.disable_reasoning,
                "action_contract_mode": benchmark_action_contract_mode(&manifest.completion_policy),
                "prompt_compaction_policy": manifest
                    .completion_policy
                    .prompt_compaction_policy
                    .map(PromptCompactionPolicy::as_str),
                "benchmark_case_class": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.case_class.clone()),
                "benchmark_owner_files": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.owner_files.clone())
                    .unwrap_or_default(),
                "benchmark_fast_loop_commands": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.fast_loop_commands.clone())
                    .unwrap_or_default(),
                "benchmark_expected_touch_targets": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.expected_touch_targets.clone())
                    .unwrap_or_default(),
                "benchmark_companion_files_required": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.companion_files_required.clone())
                    .unwrap_or_default(),
                "benchmark_named_tests": manifest
                    .challenge
                    .as_ref()
                    .map(|challenge| challenge.capsule.named_tests.clone())
                    .unwrap_or_default(),
                "warpos_capture_scope": "benchmark_task",
                "warpos_capture_call_class": "task_model_call",
                "planner_model": serde_json::Value::Null,
                "executor_model": manifest.model_id.clone(),
            }),
            seed_context,
        }),
        BenchmarkExecutor::Codex => run_codex_agent(CodexRunOptions {
            workspace: workspace.to_path_buf(),
            objective_file,
            model_id: manifest.model_id.clone(),
            max_steps: manifest.max_steps,
            max_seconds: manifest.max_seconds,
            max_total_tokens: remaining_budget,
            result_dir,
            session_strategy: fresh_session_strategy(),
        }),
    }
}

fn events_file_has_first_task_model_request(events_path: &Path) -> anyhow::Result<bool> {
    if !events_path.exists() {
        return Ok(false);
    }
    let events = fs::read_to_string(events_path)
        .with_context(|| format!("failed to read {}", events_path.display()))?;
    Ok(events.contains(r#""event":"model_request_started""#))
}

fn attempt_report_for_bootstrap_stall(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
    attempt_dir: &Path,
    workspace_dir: &Path,
    agent_result_dir: &Path,
    progress: &BenchmarkBootstrapProgress,
) -> AttemptReport {
    let bootstrap_stall_class = progress
        .bootstrap_stall_class
        .clone()
        .unwrap_or_else(|| BOOTSTRAP_STALL_CLASS_PRE_MODEL.to_string());
    let agent_error_message = format!(
        "{bootstrap_stall_class}: phase={} detail={}",
        progress.bootstrap_phase,
        progress
            .bootstrap_phase_detail
            .as_deref()
            .unwrap_or("no additional detail")
    );
    AttemptReport {
        attempt: attempt_number,
        executor: manifest.executor,
        model_id: manifest.model_id.clone(),
        safety_mode_label: manifest.safety_mode_label.clone(),
        scenario_label: manifest.scenario_label.clone(),
        agent_stop_reason: quorp_agent_core::StopReason::FatalError,
        agent_error_message: Some(agent_error_message),
        total_steps: 0,
        duration_ms: progress
            .bootstrap_elapsed_ms_before_first_task_request
            .unwrap_or_default(),
        total_billed_tokens: 0,
        max_prompt_token_estimate: None,
        max_completion_token_cap: None,
        watchdog_near_limit: false,
        watchdog_triggered: false,
        visible_evaluation: None,
        collector_evaluation: None,
        evaluation: None,
        changed_files: Vec::new(),
        ignored_changed_files: Vec::new(),
        validations: Vec::new(),
        widening_happened: false,
        attempt_dir: attempt_dir.to_path_buf(),
        workspace_dir: workspace_dir.to_path_buf(),
        agent_result_dir: agent_result_dir.to_path_buf(),
        input_tokens: 0,
        output_tokens: 0,
        reasoning_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        model_requests: 0,
        first_request_prompt_token_estimate: None,
        first_request_raw_prompt_token_estimate: None,
        first_request_compacted_prompt_token_estimate: None,
        first_request_first_token_latency_ms: None,
        first_model_turn_started: false,
        first_action_emitted: false,
        prompt_token_series_by_turn: Vec::new(),
        read_range_observations: Vec::new(),
        read_count: 0,
        write_count: 0,
        command_execution_count: 0,
        parser_recovery_count: 0,
        repair_invalid_action_streak_max: 0,
        repair_submode_entered: false,
        repair_submode_turns: 0,
        repair_write_locked: false,
        write_phase_action_refusal_count: 0,
        patch_scaffold_offered: false,
        patch_scaffold_honored: false,
        preview_apply_locked: false,
        preview_apply_action_refusal_count: 0,
        write_phase_write_emitted: false,
        bootstrap_phase: Some(progress.bootstrap_phase.clone()),
        bootstrap_phase_detail: progress.bootstrap_phase_detail.clone(),
        first_task_model_request_seen: progress.first_task_model_request_seen,
        bootstrap_elapsed_ms_before_first_task_request: progress
            .bootstrap_elapsed_ms_before_first_task_request,
        pre_model_bootstrap_stalled: progress.pre_model_bootstrap_stalled,
        bootstrap_stall_class: progress.bootstrap_stall_class.clone(),
        rolled_back_write_count: 0,
        rolled_back_non_support_edit_count: 0,
        soft_budget_inefficient: false,
        fast_loop_command_seen: false,
        agent_final_evaluate_command_seen: false,
        final_evaluate_command_seen: false,
        host_evaluation_commands_run: 0,
        non_support_edit_count: 0,
        repo_capsule_injected: manifest.completion_policy.include_repo_capsule,
        reasoning_enabled: !manifest.completion_policy.disable_reasoning,
        path_resolution_failures: 0,
        recovery_turns: 0,
        action_contract_mode: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_selected: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_fallback_reason: None,
        attempt_lineage: vec![
            benchmark_action_contract_mode(&manifest.completion_policy).to_string(),
        ],
        effective_prompt_compaction_policy: manifest
            .completion_policy
            .prompt_compaction_policy
            .map(|policy| policy.as_str().to_string()),
        fast_loop_validation_status: None,
        last_validation_failure: None,
        failing_test_names: Vec::new(),
        primary_failure_test_name: None,
        primary_failure_path: None,
        primary_failure_line: None,
        assertion_excerpt: None,
        diagnostic_class: None,
        implementation_target_lease: None,
        dependency_candidates: Vec::new(),
        target_dependency_table: None,
        repair_required: false,
        repair_phase_terminal: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        repair_phase_invalid_action_count: 0,
        post_fast_loop_patch_attempted: false,
        post_fast_loop_validation_rerun_attempted: false,
        patch_packet_injected: false,
        patch_packet_honored_range: None,
        recommended_rerun_command: None,
        fast_loop_rerun_match_kind: None,
        failed_edit_records: Vec::new(),
        local_model_memory: quorp_agent_core::LocalModelMemory::default(),
        local_agent_scorecard: quorp_agent_core::LocalAgentScorecard::default(),
        planner_model: None,
        executor_model: Some(manifest.model_id.clone()),
        judge: None,
        routing: crate::quorp::agent_local::RoutingSummary::default(),
    }
}

struct BenchmarkBootstrapWatchdog {
    stop_flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BenchmarkBootstrapWatchdog {
    fn spawn(
        manifest: BenchmarkManifest,
        result_dir: PathBuf,
        attempt_number: usize,
        attempt_dir: PathBuf,
        workspace_dir: PathBuf,
        agent_result_dir: PathBuf,
        reset_outcome: Option<EvaluatorOutcome>,
        tracker: &BenchmarkBootstrapTracker,
    ) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let thread_flag = Arc::clone(&stop_flag);
        let root_progress_path = tracker.root_progress_path.clone();
        let attempt_progress_path = tracker.attempt_progress_path.clone();
        let started_at = tracker.started_at;
        let handle = std::thread::spawn(move || {
            loop {
                if thread_flag.load(Ordering::Relaxed) {
                    return;
                }
                if events_file_has_first_task_model_request(&agent_result_dir.join("events.jsonl"))
                    .unwrap_or(false)
                {
                    if let Ok(Some(mut progress)) = read_bootstrap_progress(&attempt_progress_path)
                        && !progress.first_task_model_request_seen
                    {
                        progress.bootstrap_phase =
                            BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST.to_string();
                        progress.bootstrap_phase_detail =
                            Some("first benchmark task model request started".to_string());
                        progress.updated_at_epoch_ms = epoch_time_ms();
                        progress.first_task_model_request_seen = true;
                        progress.bootstrap_elapsed_ms_before_first_task_request =
                            Some(started_at.elapsed().as_millis() as u64);
                        let _ = write_bootstrap_progress_files(
                            &root_progress_path,
                            &attempt_progress_path,
                            &progress,
                        );
                    }
                    return;
                }
                if started_at.elapsed() >= Duration::from_secs(PRE_MODEL_BOOTSTRAP_TIMEOUT_SECS) {
                    let mut progress = read_bootstrap_progress(&attempt_progress_path)
                    .ok()
                    .flatten()
                    .unwrap_or(BenchmarkBootstrapProgress {
                        attempt: attempt_number,
                        bootstrap_phase: BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED.to_string(),
                        bootstrap_phase_detail: Some(
                            "benchmark control loop started but no bootstrap detail was recorded"
                                .to_string(),
                        ),
                        started_at_epoch_ms: epoch_time_ms(),
                        updated_at_epoch_ms: epoch_time_ms(),
                        first_task_model_request_seen: false,
                        bootstrap_elapsed_ms_before_first_task_request: None,
                        pre_model_bootstrap_stalled: false,
                        bootstrap_stall_class: None,
                    });
                    progress.pre_model_bootstrap_stalled = true;
                    progress.bootstrap_stall_class =
                        Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL.to_string());
                    progress.updated_at_epoch_ms = epoch_time_ms();
                    if progress.bootstrap_phase_detail.is_none() {
                        progress.bootstrap_phase_detail = Some(format!(
                            "timed out after {}s before the first benchmark task model request",
                            PRE_MODEL_BOOTSTRAP_TIMEOUT_SECS
                        ));
                    }
                    let _ = write_bootstrap_progress_files(
                        &root_progress_path,
                        &attempt_progress_path,
                        &progress,
                    );
                    let attempt_report = attempt_report_for_bootstrap_stall(
                        &manifest,
                        attempt_number,
                        &attempt_dir,
                        &workspace_dir,
                        &agent_result_dir,
                        &progress,
                    );
                    let _ = write_json(&attempt_dir.join("attempt-report.json"), &attempt_report);
                    let _ = write_report(
                        &result_dir,
                        &manifest,
                        &[attempt_report],
                        reset_outcome.clone(),
                        None,
                    );
                    std::process::exit(124);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        Self {
            stop_flag,
            handle: Some(handle),
        }
    }
}

impl Drop for BenchmarkBootstrapWatchdog {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn detect_widening_against_expected(
    changed_files: &[String],
    expected_files_touched: &[String],
    allowed_generated_files: &[String],
) -> bool {
    if expected_files_touched.is_empty() {
        return detect_widening(changed_files);
    }
    let expected = expected_files_touched
        .iter()
        .chain(allowed_generated_files.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    changed_files.iter().any(|file| !expected.contains(file))
}

fn maybe_continue_attempts(
    manifest: &BenchmarkManifest,
    result_dir: &Path,
    mut attempts: Vec<AttemptReport>,
    starting_attempt: usize,
) -> anyhow::Result<()> {
    for attempt_number in starting_attempt..=manifest.max_attempts {
        let budget_used: u64 = attempts
            .iter()
            .map(|attempt| attempt.total_billed_tokens)
            .sum();
        if manifest
            .max_total_tokens
            .is_some_and(|budget| budget_used >= budget)
        {
            log_phase(
                "budget",
                ANSI_YELLOW,
                format!(
                    "skipping new attempts because token budget is exhausted: used={} budget={}",
                    budget_used,
                    manifest.max_total_tokens.unwrap_or_default()
                ),
            );
            break;
        }

        log_phase(
            "attempt",
            ANSI_CYAN,
            format!(
                "starting attempt {} of {} for {}",
                attempt_number, manifest.max_attempts, manifest.resolved.benchmark_name
            ),
        );

        let attempt_dir = attempt_dir(result_dir, attempt_number);
        let workspace_dir = attempt_dir.join("workspace");
        let agent_result_dir = attempt_dir.join("agent");
        let bootstrap_tracker =
            BenchmarkBootstrapTracker::new(result_dir, &attempt_dir, attempt_number)?;
        prepare_attempt_workspace(&manifest.resolved, &workspace_dir)?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_WORKSPACE_LAYOUT_RESOLVED,
            Some(format!("workspace prepared at {}", workspace_dir.display())),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_BASELINE_RESET_READY,
            Some("standard benchmark workspace prepared without a reset script".to_string()),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_CHALLENGE_CAPSULE_REHYDRATED,
            Some("standard benchmark run has no challenge capsule rehydration".to_string()),
        )?;
        let objective = synthesize_objective(
            &manifest.resolved,
            &workspace_dir,
            &manifest.safety_mode_label,
            load_benchmark_briefing(
                manifest.briefing_file.as_deref(),
                &manifest.resolved.issue_id,
            )?
            .as_deref(),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_FAST_LOOP_CONTRACT_LOADED,
            Some("standard benchmark objective and validation ladder loaded".to_string()),
        )?;
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_PROMPT_MATERIALIZED,
            Some(format!(
                "objective materialized at {}",
                objective.path.display()
            )),
        )?;
        if manifest.executor == BenchmarkExecutor::Native {
            write_benchmark_agent_config(&workspace_dir)?;
        }
        log_phase(
            "preflight",
            if manifest.safety_mode_label == "heavy_local" {
                ANSI_YELLOW
            } else {
                ANSI_GREEN
            },
            format!(
                "risk={} model={} prompt_est={} max_tokens={} repo_capsule={}",
                manifest.safety_mode_label,
                manifest.model_id,
                objective.prompt_token_estimate,
                manifest
                    .completion_policy
                    .first_turn_max_completion_tokens
                    .or(manifest.completion_policy.later_turn_max_completion_tokens)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "default".to_string()),
                manifest.completion_policy.include_repo_capsule
            ),
        );

        let remaining_budget = manifest
            .max_total_tokens
            .map(|budget| budget.saturating_sub(budget_used));
        bootstrap_tracker.update(
            BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED,
            Some(format!(
                "launching native benchmark control loop in {}",
                agent_result_dir.display()
            )),
        )?;
        let bootstrap_watchdog = BenchmarkBootstrapWatchdog::spawn(
            manifest.clone(),
            result_dir.to_path_buf(),
            attempt_number,
            attempt_dir.clone(),
            workspace_dir.clone(),
            agent_result_dir.clone(),
            None,
            &bootstrap_tracker,
        );
        let outcome = match run_attempt_executor(
            manifest,
            &workspace_dir,
            objective.path,
            remaining_budget,
            agent_result_dir,
        ) {
            Ok(outcome) => {
                drop(bootstrap_watchdog);
                if events_file_has_first_task_model_request(&attempt_dir.join("agent/events.jsonl"))
                    .unwrap_or(false)
                {
                    let _ = bootstrap_tracker.mark_first_task_model_request();
                }
                outcome
            }
            Err(error) => {
                drop(bootstrap_watchdog);
                if let Err(report_error) = write_report(
                    result_dir,
                    manifest,
                    &attempts,
                    None,
                    Some(error.to_string()),
                ) {
                    log::error!("failed to write benchmark report after run error: {report_error}");
                }
                return Err(error);
            }
        };

        let attempt_report = match finalize_attempt(manifest, attempt_number, &attempt_dir, outcome)
        {
            Ok(report) => report,
            Err(error) => {
                if let Err(report_error) = write_report(
                    result_dir,
                    manifest,
                    &attempts,
                    None,
                    Some(error.to_string()),
                ) {
                    log::error!(
                        "failed to write benchmark report after finalize error: {report_error}"
                    );
                }
                return Err(error);
            }
        };
        write_json(&attempt_dir.join("attempt-report.json"), &attempt_report)?;
        attempts.push(attempt_report.clone());
        write_report(result_dir, manifest, &attempts, None, None)?;

        if attempt_report
            .visible_evaluation
            .as_ref()
            .is_some_and(|outcome| !outcome.passed)
        {
            log_phase(
                "retry",
                ANSI_YELLOW,
                format!("visible evaluation failed on attempt {}", attempt_number),
            );
        } else if attempt_report
            .collector_evaluation
            .as_ref()
            .is_some_and(|outcome| !outcome.passed)
        {
            log_phase(
                "retry",
                ANSI_YELLOW,
                format!("collector evaluation failed on attempt {}", attempt_number),
            );
        } else if attempt_report
            .judge
            .as_ref()
            .is_some_and(|judge| !judge.passed)
        {
            log_phase(
                "retry",
                ANSI_YELLOW,
                format!("judge failed on attempt {}", attempt_number),
            );
        } else if matches!(
            attempt_report.agent_stop_reason,
            quorp_agent_core::StopReason::Success
        ) {
            log_phase(
                "success",
                ANSI_GREEN,
                format!(
                    "benchmark completed successfully on attempt {}",
                    attempt_number
                ),
            );
            break;
        }
    }

    write_report(result_dir, manifest, &attempts, None, None)?;
    Ok(())
}

fn finalize_attempt(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
    attempt_dir: &Path,
    outcome: quorp_agent_core::AgentRunOutcome,
) -> anyhow::Result<AttemptReport> {
    let resolved = &manifest.resolved;
    let workspace_dir = attempt_dir.join("workspace");
    let agent_result_dir = attempt_dir.join("agent");
    let visible_evaluation = match resolved.visible_evaluator.as_ref() {
        Some(script) => Some(run_visible_evaluator(script, &workspace_dir)?),
        None => None,
    };
    let collector_evaluation = match resolved.collector_evaluator.as_ref() {
        Some(script) => Some(run_collector_evaluator(
            script,
            &workspace_dir,
            attempt_dir,
        )?),
        None => None,
    };
    if let Some(outcome) = visible_evaluation.as_ref() {
        write_json(&attempt_dir.join("visible-evaluation.json"), outcome)?;
    }
    if let Some(outcome) = collector_evaluation.as_ref() {
        write_json(&attempt_dir.join("collector-evaluation.json"), outcome)?;
    }
    let changed_files = git_changed_files(&workspace_dir)?;
    let validations = extract_validation_summaries(&agent_result_dir.join("events.jsonl"))?;
    let widening_happened = detect_widening(&changed_files);
    let metrics = extract_request_metrics(&agent_result_dir.join("events.jsonl"))?;
    let control_loop = extract_control_loop_summary(&agent_result_dir.join("events.jsonl"))?;
    let usage = read_headless_usage_summary(&agent_result_dir.join("summary.json"))?;
    let routing = read_headless_routing_summary(&agent_result_dir.join("summary.json"))?;
    let validation_state =
        read_checkpoint_validation_state(&agent_result_dir.join("checkpoint.json"))?;
    let read_range_observations =
        extract_read_range_observations(&agent_result_dir.join("checkpoint.json"))?;
    let bootstrap_progress =
        read_bootstrap_progress(&attempt_bootstrap_progress_path(attempt_dir))?;
    let action_evidence = extract_action_evidence(
        &agent_result_dir.join("checkpoint.json"),
        manifest
            .challenge
            .as_ref()
            .map(|challenge| &challenge.capsule),
        manifest
            .challenge
            .as_ref()
            .map(|challenge| challenge.evaluate_command.as_str()),
    )?;
    let non_support_edit_count =
        manifest
            .challenge
            .as_ref()
            .map_or(changed_files.len(), |metadata| {
                let ignored_changed_files =
                    challenge_ignored_changed_files(metadata, &workspace_dir);
                count_non_support_changed_files(&changed_files, &ignored_changed_files)
            });
    let soft_budget_inefficient = validation_state
        .local_agent_scorecard
        .first_valid_write_step
        .is_none()
        && (usage.model_requests > 8 || outcome.total_billed_tokens > 50_000);
    let host_evaluation_commands_run =
        usize::from(visible_evaluation.is_some()) + usize::from(collector_evaluation.is_some());

    Ok(AttemptReport {
        attempt: attempt_number,
        executor: manifest.executor,
        model_id: manifest.model_id.clone(),
        safety_mode_label: manifest.safety_mode_label.clone(),
        scenario_label: manifest.scenario_label.clone(),
        agent_stop_reason: outcome.stop_reason,
        agent_error_message: outcome.error_message,
        total_steps: outcome.total_steps,
        duration_ms: outcome.duration_ms,
        total_billed_tokens: outcome.total_billed_tokens,
        max_prompt_token_estimate: metrics.max_prompt_token_estimate,
        max_completion_token_cap: metrics.max_completion_token_cap,
        watchdog_near_limit: metrics.watchdog_near_limit,
        watchdog_triggered: metrics.watchdog_triggered,
        visible_evaluation,
        collector_evaluation,
        changed_files,
        ignored_changed_files: Vec::new(),
        validations,
        widening_happened,
        attempt_dir: attempt_dir.to_path_buf(),
        workspace_dir,
        agent_result_dir,
        evaluation: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        model_requests: usage.model_requests,
        first_request_prompt_token_estimate: metrics.first_request_prompt_token_estimate,
        first_request_raw_prompt_token_estimate: metrics.first_request_raw_prompt_token_estimate,
        first_request_compacted_prompt_token_estimate: metrics
            .first_request_compacted_prompt_token_estimate,
        first_request_first_token_latency_ms: metrics.first_request_first_token_latency_ms,
        first_model_turn_started: metrics.first_model_turn_started,
        first_action_emitted: metrics.first_action_emitted,
        prompt_token_series_by_turn: metrics.prompt_token_series_by_turn,
        read_range_observations,
        read_count: action_evidence.read_count,
        write_count: action_evidence.write_count,
        command_execution_count: action_evidence.command_execution_count,
        parser_recovery_count: validation_state.local_agent_scorecard.parser_recovery_count,
        repair_invalid_action_streak_max: validation_state
            .local_agent_scorecard
            .repair_invalid_action_streak_max,
        repair_submode_entered: validation_state
            .local_agent_scorecard
            .repair_submode_entered,
        repair_submode_turns: validation_state.local_agent_scorecard.repair_submode_turns,
        repair_write_locked: validation_state.local_agent_scorecard.repair_write_locked,
        write_phase_action_refusal_count: validation_state
            .local_agent_scorecard
            .write_phase_action_refusal_count,
        patch_scaffold_offered: validation_state
            .local_agent_scorecard
            .patch_scaffold_offered,
        patch_scaffold_honored: validation_state
            .local_agent_scorecard
            .patch_scaffold_honored,
        preview_apply_locked: validation_state.local_agent_scorecard.preview_apply_locked,
        preview_apply_action_refusal_count: validation_state
            .local_agent_scorecard
            .preview_apply_action_refusal_count,
        write_phase_write_emitted: validation_state
            .local_agent_scorecard
            .write_phase_write_emitted,
        bootstrap_phase: bootstrap_progress
            .as_ref()
            .map(|progress| progress.bootstrap_phase.clone()),
        bootstrap_phase_detail: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_phase_detail.clone()),
        first_task_model_request_seen: bootstrap_progress
            .as_ref()
            .is_some_and(|progress| progress.first_task_model_request_seen)
            || metrics.first_model_turn_started,
        bootstrap_elapsed_ms_before_first_task_request: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_elapsed_ms_before_first_task_request),
        pre_model_bootstrap_stalled: bootstrap_progress
            .as_ref()
            .is_some_and(|progress| progress.pre_model_bootstrap_stalled),
        bootstrap_stall_class: bootstrap_progress
            .as_ref()
            .and_then(|progress| progress.bootstrap_stall_class.clone()),
        rolled_back_write_count: validation_state
            .local_agent_scorecard
            .rolled_back_write_count,
        rolled_back_non_support_edit_count: validation_state
            .local_agent_scorecard
            .rolled_back_non_support_edit_count,
        soft_budget_inefficient,
        fast_loop_command_seen: action_evidence.fast_loop_command_seen,
        agent_final_evaluate_command_seen: action_evidence.final_evaluate_command_seen,
        final_evaluate_command_seen: action_evidence.final_evaluate_command_seen,
        host_evaluation_commands_run,
        non_support_edit_count,
        repo_capsule_injected: manifest.completion_policy.include_repo_capsule,
        reasoning_enabled: !manifest.completion_policy.disable_reasoning,
        path_resolution_failures: control_loop.path_resolution_failures,
        recovery_turns: control_loop.recovery_turns,
        action_contract_mode: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_selected: benchmark_action_contract_mode(&manifest.completion_policy)
            .to_string(),
        action_contract_fallback_reason: std::env::var(
            "QUORP_BENCH_ACTION_CONTRACT_FALLBACK_REASON",
        )
        .ok()
        .filter(|value| !value.trim().is_empty()),
        attempt_lineage: benchmark_attempt_lineage(&manifest.completion_policy),
        effective_prompt_compaction_policy: manifest
            .completion_policy
            .prompt_compaction_policy
            .map(|policy| policy.as_str().to_string()),
        fast_loop_validation_status: validation_state.validation_status,
        last_validation_failure: validation_state.last_validation_failure,
        failing_test_names: validation_state.failing_test_names,
        primary_failure_test_name: validation_state.primary_failure_test_name,
        primary_failure_path: validation_state.primary_failure_path,
        primary_failure_line: validation_state.primary_failure_line,
        assertion_excerpt: validation_state.assertion_excerpt,
        diagnostic_class: validation_state.diagnostic_class,
        implementation_target_lease: validation_state.implementation_target_lease,
        dependency_candidates: validation_state.dependency_candidates,
        target_dependency_table: validation_state.target_dependency_table,
        repair_required: validation_state.repair_required,
        repair_phase_terminal: validation_state.repair_phase_terminal,
        failure_anchor_reread_attempted: validation_state.failure_anchor_reread_attempted,
        failure_anchor_reread_honored: validation_state.failure_anchor_reread_honored,
        implementation_reread_allowed: validation_state.implementation_reread_allowed,
        implementation_reread_attempted: validation_state.implementation_reread_attempted,
        implementation_reread_honored: validation_state.implementation_reread_honored,
        repair_phase_invalid_action_count: validation_state.repair_phase_invalid_action_count,
        post_fast_loop_patch_attempted: validation_state.post_fast_loop_patch_attempted,
        post_fast_loop_validation_rerun_attempted: validation_state
            .post_fast_loop_validation_rerun_attempted,
        patch_packet_injected: validation_state.patch_packet_injected,
        patch_packet_honored_range: validation_state.patch_packet_honored_range,
        recommended_rerun_command: validation_state.recommended_rerun_command,
        fast_loop_rerun_match_kind: validation_state.fast_loop_rerun_match_kind,
        failed_edit_records: validation_state.failed_edit_records,
        local_model_memory: validation_state.local_model_memory,
        local_agent_scorecard: validation_state.local_agent_scorecard,
        planner_model: None,
        executor_model: Some(manifest.model_id.clone()),
        judge: None,
        routing,
    })
}

fn write_report(
    result_dir: &Path,
    manifest: &BenchmarkManifest,
    attempts: &[AttemptReport],
    reset_outcome: Option<EvaluatorOutcome>,
    run_error: Option<String>,
) -> anyhow::Result<()> {
    let last_attempt = attempts.last();
    let changed_files = attempts
        .iter()
        .flat_map(|attempt| attempt.changed_files.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let ignored_changed_files = attempts
        .iter()
        .flat_map(|attempt| attempt.ignored_changed_files.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let total_billed_tokens = attempts
        .iter()
        .map(|attempt| attempt.total_billed_tokens)
        .sum();
    let wall_clock_ms = attempts
        .iter()
        .map(|attempt| attempt.duration_ms)
        .sum::<u64>()
        .saturating_add(
            attempts
                .iter()
                .filter_map(|attempt| attempt.visible_evaluation.as_ref())
                .map(|outcome| outcome.duration_ms)
                .sum::<u64>(),
        )
        .saturating_add(
            attempts
                .iter()
                .filter_map(|attempt| attempt.collector_evaluation.as_ref())
                .map(|outcome| outcome.duration_ms)
                .sum::<u64>(),
        )
        .saturating_add(
            attempts
                .iter()
                .filter_map(|attempt| attempt.evaluation.as_ref())
                .map(|outcome| outcome.duration_ms)
                .sum::<u64>(),
        )
        .saturating_add(
            reset_outcome
                .as_ref()
                .map(|outcome| outcome.duration_ms)
                .unwrap_or_default(),
        );
    let prompt_tokens = attempts.iter().map(|attempt| attempt.input_tokens).sum();
    let completion_tokens = attempts.iter().map(|attempt| attempt.output_tokens).sum();
    let reasoning_tokens = attempts
        .iter()
        .map(|attempt| attempt.reasoning_tokens)
        .sum();
    let cache_read_input_tokens = attempts
        .iter()
        .map(|attempt| attempt.cache_read_input_tokens)
        .sum();
    let cache_write_input_tokens = attempts
        .iter()
        .map(|attempt| attempt.cache_write_input_tokens)
        .sum();
    let max_prompt_token_estimate_seen = attempts
        .iter()
        .filter_map(|attempt| attempt.max_prompt_token_estimate)
        .max();
    let max_completion_token_cap_seen = attempts
        .iter()
        .filter_map(|attempt| attempt.max_completion_token_cap)
        .max();
    let success = last_attempt.is_some_and(attempt_passed);
    let evaluation_commands_run: usize = attempts.iter().map(count_evaluation_commands).sum();
    let validation_commands_run: usize = attempts
        .iter()
        .map(|attempt| attempt.validations.len())
        .sum();
    let mistakes_corrected = count_mistakes_corrected(attempts);
    let total_requests: usize = attempts.iter().map(|attempt| attempt.model_requests).sum();
    let task_model_call_count = total_requests;
    let read_count = attempts
        .iter()
        .map(|attempt| attempt.read_count)
        .sum::<usize>();
    let write_count = attempts
        .iter()
        .map(|attempt| attempt.write_count)
        .sum::<usize>();
    let command_execution_count = attempts
        .iter()
        .map(|attempt| attempt.command_execution_count)
        .sum::<usize>()
        .max(validation_commands_run.saturating_add(evaluation_commands_run));
    let fast_loop_command_seen = attempts
        .iter()
        .any(|attempt| attempt.fast_loop_command_seen);
    let agent_final_evaluate_command_seen = attempts
        .iter()
        .any(|attempt| attempt.agent_final_evaluate_command_seen);
    let final_evaluate_command_seen = agent_final_evaluate_command_seen;
    let host_evaluation_commands_run = evaluation_commands_run;
    let non_support_edit_count = attempts
        .iter()
        .map(|attempt| attempt.non_support_edit_count)
        .sum::<usize>();
    let scorecard_tool_call_count = validation_commands_run
        .saturating_add(evaluation_commands_run)
        .saturating_add(
            attempts
                .iter()
                .map(|attempt| {
                    attempt
                        .local_agent_scorecard
                        .preview_edit_count
                        .saturating_add(attempt.local_agent_scorecard.replace_range_count)
                        .saturating_add(attempt.local_agent_scorecard.modify_toml_count)
                        .saturating_add(attempt.local_agent_scorecard.apply_preview_count)
                })
                .sum::<usize>(),
        );
    let action_tool_call_count = read_count
        .saturating_add(write_count)
        .saturating_add(command_execution_count);
    let tool_call_count = action_tool_call_count.max(scorecard_tool_call_count);
    let edit_count = non_support_edit_count;
    let text_only_action_failure = last_attempt.is_some_and(|attempt| {
        attempt.first_model_turn_started
            && !attempt.first_action_emitted
            && attempt.model_requests > 0
            && !success
    });
    let sandbox_root = manifest
        .challenge
        .as_ref()
        .map(|challenge| challenge.sandbox_root.clone())
        .or_else(|| last_attempt.map(|attempt| attempt.workspace_dir.clone()));
    let workspace_for_diff = manifest
        .challenge
        .as_ref()
        .map(|challenge| challenge.workspace_dir.clone())
        .or_else(|| last_attempt.map(|attempt| attempt.workspace_dir.clone()));
    let (lines_added, lines_removed) = if let Some(workspace) = workspace_for_diff.as_ref() {
        match git_numstat(workspace) {
            Ok(values) => values,
            Err(error) => {
                log::warn!(
                    "failed to compute git numstat in {}: {error}",
                    workspace.display()
                );
                (0, 0)
            }
        }
    } else {
        (0, 0)
    };
    let judge = attempts
        .iter()
        .rev()
        .find_map(|attempt| attempt.judge.clone());
    let first_request_prompt_token_estimate = attempts
        .iter()
        .find_map(|attempt| attempt.first_request_prompt_token_estimate);
    let first_request_raw_prompt_token_estimate = attempts
        .iter()
        .find_map(|attempt| attempt.first_request_raw_prompt_token_estimate)
        .or(first_request_prompt_token_estimate);
    let first_request_compacted_prompt_token_estimate = attempts
        .iter()
        .find_map(|attempt| attempt.first_request_compacted_prompt_token_estimate);
    let first_request_first_token_latency_ms = attempts
        .iter()
        .find_map(|attempt| attempt.first_request_first_token_latency_ms);
    let first_model_turn_started = attempts
        .iter()
        .any(|attempt| attempt.first_model_turn_started);
    let first_action_emitted = attempts.iter().any(|attempt| {
        attempt.first_action_emitted
            || attempt.read_count > 0
            || attempt.write_count > 0
            || attempt.command_execution_count > 0
    });
    let repo_capsule_injected = attempts.iter().any(|attempt| attempt.repo_capsule_injected);
    let reasoning_enabled = attempts.iter().any(|attempt| attempt.reasoning_enabled);
    let path_resolution_failures = attempts
        .iter()
        .map(|attempt| attempt.path_resolution_failures)
        .sum();
    let recovery_turns = attempts.iter().map(|attempt| attempt.recovery_turns).sum();
    let action_contract_mode = attempts
        .iter()
        .map(|attempt| attempt.action_contract_mode.as_str())
        .find(|value| !value.is_empty())
        .unwrap_or("strict_json_v1")
        .to_string();
    let planner_model = attempts
        .iter()
        .find_map(|attempt| attempt.planner_model.clone());
    let executor_model = attempts
        .iter()
        .find_map(|attempt| attempt.executor_model.clone())
        .or_else(|| Some(manifest.model_id.clone()));
    let deterministic_evaluation_passed = last_attempt.and_then(|attempt| {
        attempt
            .evaluation
            .as_ref()
            .map(|outcome| outcome.passed)
            .or_else(|| {
                attempt
                    .visible_evaluation
                    .as_ref()
                    .map(|outcome| outcome.passed)
            })
            .or_else(|| {
                attempt
                    .collector_evaluation
                    .as_ref()
                    .map(|outcome| outcome.passed)
            })
    });
    let provider_summary = benchmark_provider_summary(
        manifest.executor,
        &manifest.model_id,
        manifest.base_url_override.as_deref(),
    );
    let mut routing_summary = crate::quorp::agent_local::RoutingSummary::default();
    for attempt in attempts {
        if routing_summary.routing_mode.is_none() {
            routing_summary.routing_mode = attempt.routing.routing_mode.clone();
        }
        if routing_summary.requested_provider.is_none() {
            routing_summary.requested_provider = attempt.routing.requested_provider.clone();
        }
        if routing_summary.requested_model.is_none() {
            routing_summary.requested_model = attempt.routing.requested_model.clone();
        }
        if routing_summary.candidate_models.is_empty()
            && !attempt.routing.candidate_models.is_empty()
        {
            routing_summary.candidate_models = attempt.routing.candidate_models.clone();
        }
        if attempt.routing.effective_provider.is_some() {
            routing_summary.effective_provider = attempt.routing.effective_provider.clone();
        }
        if attempt.routing.effective_model.is_some() {
            routing_summary.effective_model = attempt.routing.effective_model.clone();
        }
        if routing_summary.provider_base_url.is_none() {
            routing_summary.provider_base_url = attempt.routing.provider_base_url.clone();
        }
        if routing_summary.auth_mode.is_none() {
            routing_summary.auth_mode = attempt.routing.auth_mode.clone();
        }
        routing_summary.proxy_visible_remote_egress_expected |=
            attempt.routing.proxy_visible_remote_egress_expected;
        if routing_summary.provider_request_id.is_none() {
            routing_summary.provider_request_id = attempt.routing.provider_request_id.clone();
        }
        if routing_summary.routing_status.is_none() {
            routing_summary.routing_status = attempt.routing.routing_status.clone();
        }
        routing_summary.used_local_fallback |= attempt.routing.used_local_fallback;
        if routing_summary.fallback_reason.is_none() {
            routing_summary.fallback_reason = attempt.routing.fallback_reason.clone();
        }
        routing_summary.comparable = Some(
            routing_summary.comparable.unwrap_or(true)
                && attempt.routing.comparable.unwrap_or(true),
        );
    }
    let report = BenchmarkReport {
        benchmark_name: manifest.resolved.benchmark_name.clone(),
        issue_id: manifest.resolved.issue_id.clone(),
        executor: manifest.executor,
        model_id: manifest.model_id.clone(),
        safety_mode_label: manifest.safety_mode_label.clone(),
        scenario_label: manifest.scenario_label.clone(),
        provider_kind: provider_summary.provider_kind,
        provider_base_url: provider_summary.provider_base_url,
        auth_mode: provider_summary.auth_mode,
        usage_source: provider_summary.usage_source,
        proxy_visible_remote_egress_expected: provider_summary.proxy_visible_remote_egress_expected,
        routing_mode: routing_summary.routing_mode,
        requested_provider: routing_summary.requested_provider,
        requested_model: routing_summary.requested_model,
        candidate_models: routing_summary.candidate_models,
        effective_provider: routing_summary.effective_provider,
        effective_model: routing_summary.effective_model,
        used_local_fallback: routing_summary.used_local_fallback,
        fallback_reason: routing_summary.fallback_reason,
        comparable_run: routing_summary.comparable,
        provider_request_id: routing_summary.provider_request_id,
        routing_status: routing_summary.routing_status,
        success,
        attempts_run: attempts.len(),
        max_attempts: manifest.max_attempts,
        total_billed_tokens,
        wall_clock_ms,
        max_total_tokens: manifest.max_total_tokens,
        max_prompt_token_estimate_seen,
        max_completion_token_cap_seen,
        watchdog_near_limit: attempts.iter().any(|attempt| attempt.watchdog_near_limit),
        watchdog_triggered: attempts.iter().any(|attempt| attempt.watchdog_triggered),
        final_stop_reason: last_attempt.map(|attempt| attempt.agent_stop_reason),
        changed_files,
        ignored_changed_files,
        widening_happened: attempts.iter().any(|attempt| attempt.widening_happened),
        attempts: attempts.to_vec(),
        reset_outcome,
        challenge: manifest.challenge.clone(),
        run_dir: result_dir.to_path_buf(),
        sandbox_root,
        exit_code: if success { 0 } else { 1 },
        lines_added,
        lines_removed,
        mistakes_corrected,
        validation_commands_run,
        evaluation_commands_run,
        prompt_tokens,
        completion_tokens,
        reasoning_tokens,
        cache_read_input_tokens,
        cache_write_input_tokens,
        run_error,
        setup_failure_class: None,
        total_requests,
        task_model_call_count,
        tool_call_count,
        edit_count,
        read_count,
        write_count,
        command_execution_count,
        parser_recovery_count: last_attempt
            .map(|attempt| attempt.parser_recovery_count)
            .unwrap_or(0),
        repair_invalid_action_streak_max: last_attempt
            .map(|attempt| attempt.repair_invalid_action_streak_max)
            .unwrap_or(0),
        repair_submode_entered: last_attempt.is_some_and(|attempt| attempt.repair_submode_entered),
        repair_submode_turns: last_attempt
            .map(|attempt| attempt.repair_submode_turns)
            .unwrap_or(0),
        repair_write_locked: last_attempt.is_some_and(|attempt| attempt.repair_write_locked),
        write_phase_action_refusal_count: last_attempt
            .map(|attempt| attempt.write_phase_action_refusal_count)
            .unwrap_or(0),
        patch_scaffold_offered: last_attempt.is_some_and(|attempt| attempt.patch_scaffold_offered),
        patch_scaffold_honored: last_attempt.is_some_and(|attempt| attempt.patch_scaffold_honored),
        preview_apply_locked: last_attempt.is_some_and(|attempt| attempt.preview_apply_locked),
        preview_apply_action_refusal_count: last_attempt
            .map(|attempt| attempt.preview_apply_action_refusal_count)
            .unwrap_or(0),
        write_phase_write_emitted: last_attempt
            .is_some_and(|attempt| attempt.write_phase_write_emitted),
        bootstrap_phase: last_attempt.and_then(|attempt| attempt.bootstrap_phase.clone()),
        bootstrap_phase_detail: last_attempt
            .and_then(|attempt| attempt.bootstrap_phase_detail.clone()),
        first_task_model_request_seen: attempts
            .iter()
            .any(|attempt| attempt.first_task_model_request_seen),
        bootstrap_elapsed_ms_before_first_task_request: last_attempt
            .and_then(|attempt| attempt.bootstrap_elapsed_ms_before_first_task_request),
        pre_model_bootstrap_stalled: attempts
            .iter()
            .any(|attempt| attempt.pre_model_bootstrap_stalled),
        bootstrap_stall_class: last_attempt
            .and_then(|attempt| attempt.bootstrap_stall_class.clone()),
        rolled_back_write_count: attempts
            .iter()
            .map(|attempt| attempt.rolled_back_write_count)
            .sum(),
        rolled_back_non_support_edit_count: attempts
            .iter()
            .map(|attempt| attempt.rolled_back_non_support_edit_count)
            .sum(),
        soft_budget_inefficient: attempts
            .iter()
            .any(|attempt| attempt.soft_budget_inefficient),
        fast_loop_command_seen,
        agent_final_evaluate_command_seen,
        final_evaluate_command_seen,
        host_evaluation_commands_run,
        non_support_edit_count,
        last_failure_class: None,
        evaluation_command_seen: final_evaluate_command_seen || host_evaluation_commands_run > 0,
        text_only_action_failure,
        first_request_prompt_token_estimate,
        first_request_raw_prompt_token_estimate,
        first_request_compacted_prompt_token_estimate,
        first_request_first_token_latency_ms,
        first_model_turn_started,
        first_action_emitted,
        prompt_token_series_by_turn: last_attempt
            .map(|attempt| attempt.prompt_token_series_by_turn.clone())
            .unwrap_or_default(),
        read_range_observations: last_attempt
            .map(|attempt| attempt.read_range_observations.clone())
            .unwrap_or_default(),
        repo_capsule_injected,
        reasoning_enabled,
        path_resolution_failures,
        recovery_turns,
        action_contract_selected: action_contract_mode.clone(),
        action_contract_fallback_reason: std::env::var(
            "QUORP_BENCH_ACTION_CONTRACT_FALLBACK_REASON",
        )
        .ok()
        .filter(|value| !value.trim().is_empty()),
        attempt_lineage: std::env::var("QUORP_BENCH_ATTEMPT_LINEAGE")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| vec![action_contract_mode.clone()]),
        action_contract_mode,
        effective_prompt_compaction_policy: manifest
            .completion_policy
            .prompt_compaction_policy
            .map(|policy| policy.as_str().to_string()),
        fast_loop_validation_status: last_attempt
            .and_then(|attempt| attempt.fast_loop_validation_status.clone()),
        last_validation_failure: last_attempt
            .and_then(|attempt| attempt.last_validation_failure.clone()),
        failing_test_names: last_attempt
            .map(|attempt| attempt.failing_test_names.clone())
            .unwrap_or_default(),
        primary_failure_test_name: last_attempt
            .and_then(|attempt| attempt.primary_failure_test_name.clone()),
        primary_failure_path: last_attempt.and_then(|attempt| attempt.primary_failure_path.clone()),
        primary_failure_line: last_attempt.and_then(|attempt| attempt.primary_failure_line),
        assertion_excerpt: last_attempt.and_then(|attempt| attempt.assertion_excerpt.clone()),
        diagnostic_class: last_attempt.and_then(|attempt| attempt.diagnostic_class.clone()),
        implementation_target_lease: last_attempt
            .and_then(|attempt| attempt.implementation_target_lease.clone()),
        dependency_candidates: last_attempt
            .map(|attempt| attempt.dependency_candidates.clone())
            .unwrap_or_default(),
        target_dependency_table: last_attempt
            .and_then(|attempt| attempt.target_dependency_table.clone()),
        repair_required: last_attempt.is_some_and(|attempt| attempt.repair_required),
        repair_phase_terminal: last_attempt
            .and_then(|attempt| attempt.repair_phase_terminal.clone()),
        failure_anchor_reread_attempted: last_attempt
            .is_some_and(|attempt| attempt.failure_anchor_reread_attempted),
        failure_anchor_reread_honored: last_attempt
            .is_some_and(|attempt| attempt.failure_anchor_reread_honored),
        implementation_reread_allowed: last_attempt
            .is_some_and(|attempt| attempt.implementation_reread_allowed),
        implementation_reread_attempted: last_attempt
            .is_some_and(|attempt| attempt.implementation_reread_attempted),
        implementation_reread_honored: last_attempt
            .is_some_and(|attempt| attempt.implementation_reread_honored),
        repair_phase_invalid_action_count: last_attempt
            .map(|attempt| attempt.repair_phase_invalid_action_count)
            .unwrap_or(0),
        post_fast_loop_patch_attempted: last_attempt
            .is_some_and(|attempt| attempt.post_fast_loop_patch_attempted),
        post_fast_loop_validation_rerun_attempted: last_attempt
            .is_some_and(|attempt| attempt.post_fast_loop_validation_rerun_attempted),
        patch_packet_injected: last_attempt.is_some_and(|attempt| attempt.patch_packet_injected),
        patch_packet_honored_range: last_attempt
            .and_then(|attempt| attempt.patch_packet_honored_range.clone()),
        recommended_rerun_command: last_attempt
            .and_then(|attempt| attempt.recommended_rerun_command.clone()),
        fast_loop_rerun_match_kind: last_attempt
            .and_then(|attempt| attempt.fast_loop_rerun_match_kind.clone()),
        failed_edit_records: last_attempt
            .map(|attempt| attempt.failed_edit_records.clone())
            .unwrap_or_default(),
        local_model_memory: last_attempt
            .map(|attempt| attempt.local_model_memory.clone())
            .unwrap_or_default(),
        local_agent_scorecard: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.clone())
            .unwrap_or_default(),
        preview_edit_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.preview_edit_count)
            .unwrap_or(0),
        preview_edit_success_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.preview_edit_success_count)
            .unwrap_or(0),
        preview_created_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.preview_created_count)
            .unwrap_or(0),
        replace_range_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.replace_range_count)
            .unwrap_or(0),
        replace_range_hash_mismatch_count: last_attempt
            .map(|attempt| {
                attempt
                    .local_agent_scorecard
                    .replace_range_hash_mismatch_count
            })
            .unwrap_or(0),
        modify_toml_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.modify_toml_count)
            .unwrap_or(0),
        apply_preview_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.apply_preview_count)
            .unwrap_or(0),
        apply_preview_hash_mismatch_count: last_attempt
            .map(|attempt| {
                attempt
                    .local_agent_scorecard
                    .apply_preview_hash_mismatch_count
            })
            .unwrap_or(0),
        syntax_preview_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.syntax_preview_count)
            .unwrap_or(0),
        syntax_preview_failure_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.syntax_preview_failure_count)
            .unwrap_or(0),
        target_redirect_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.target_redirect_count)
            .unwrap_or(0),
        evidence_file_fixation_count: last_attempt
            .map(|attempt| attempt.local_agent_scorecard.evidence_file_fixation_count)
            .unwrap_or(0),
        local_agent_final_failure_classification: None,
        planner_model,
        executor_model,
        deterministic_evaluation_passed,
        judge,
        primary_failure: None,
    };
    let primary_failure = classify_primary_failure(&report);
    let local_agent_final_failure_classification =
        classify_local_agent_failure(&report, primary_failure.as_deref());
    let last_failure_class = local_agent_final_failure_classification
        .clone()
        .or_else(|| primary_failure.clone());
    let report = BenchmarkReport {
        primary_failure,
        last_failure_class,
        local_agent_final_failure_classification,
        ..report
    };
    write_json(&result_dir.join("benchmark-report.json"), &report)?;
    fs::write(
        result_dir.join("benchmark-report.md"),
        render_report_markdown(&report),
    )?;
    log_phase(
        "report",
        if report.success {
            ANSI_GREEN
        } else {
            ANSI_YELLOW
        },
        format!(
            "attempts={} success={} billed_tokens={}",
            report.attempts_run, report.success, report.total_billed_tokens
        ),
    );
    Ok(())
}

fn write_synthetic_failure_report(
    case_manifest: &ChallengeManifest,
    result_dir: &Path,
    executor: BenchmarkExecutor,
    model_id: &str,
    max_attempts: usize,
    run_error: String,
    setup_failure_class: Option<String>,
) -> anyhow::Result<()> {
    let safety_mode_label = benchmark_safety_mode_label(executor, model_id);
    let provider_summary = benchmark_provider_summary(executor, model_id, None);
    let completion_policy =
        benchmark_completion_policy(executor, &safety_mode_label, Some(model_id));
    let report = BenchmarkReport {
        benchmark_name: case_manifest.title.clone(),
        issue_id: case_manifest.id.clone(),
        executor,
        model_id: model_id.to_string(),
        safety_mode_label,
        scenario_label: Some(crate::quorp::provider_config::resolved_scenario_label()),
        provider_kind: provider_summary.provider_kind,
        provider_base_url: provider_summary.provider_base_url,
        auth_mode: provider_summary.auth_mode,
        usage_source: provider_summary.usage_source,
        proxy_visible_remote_egress_expected: provider_summary.proxy_visible_remote_egress_expected,
        routing_mode: None,
        requested_provider: None,
        requested_model: None,
        candidate_models: Vec::new(),
        effective_provider: None,
        effective_model: None,
        used_local_fallback: false,
        fallback_reason: None,
        comparable_run: None,
        provider_request_id: None,
        routing_status: None,
        success: false,
        attempts_run: 0,
        max_attempts,
        total_billed_tokens: 0,
        wall_clock_ms: 0,
        max_total_tokens: None,
        max_prompt_token_estimate_seen: None,
        max_completion_token_cap_seen: None,
        watchdog_near_limit: false,
        watchdog_triggered: false,
        final_stop_reason: None,
        changed_files: Vec::new(),
        ignored_changed_files: Vec::new(),
        widening_happened: false,
        attempts: Vec::new(),
        reset_outcome: None,
        challenge: None,
        run_dir: result_dir.to_path_buf(),
        sandbox_root: None,
        exit_code: 1,
        lines_added: 0,
        lines_removed: 0,
        mistakes_corrected: 0,
        validation_commands_run: 0,
        evaluation_commands_run: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        reasoning_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        run_error: Some(run_error),
        setup_failure_class: setup_failure_class.clone(),
        total_requests: 0,
        task_model_call_count: 0,
        tool_call_count: 0,
        edit_count: 0,
        read_count: 0,
        write_count: 0,
        command_execution_count: 0,
        parser_recovery_count: 0,
        repair_invalid_action_streak_max: 0,
        repair_submode_entered: false,
        repair_submode_turns: 0,
        repair_write_locked: false,
        write_phase_action_refusal_count: 0,
        patch_scaffold_offered: false,
        patch_scaffold_honored: false,
        preview_apply_locked: false,
        preview_apply_action_refusal_count: 0,
        write_phase_write_emitted: false,
        bootstrap_phase: None,
        bootstrap_phase_detail: None,
        first_task_model_request_seen: false,
        bootstrap_elapsed_ms_before_first_task_request: None,
        pre_model_bootstrap_stalled: false,
        bootstrap_stall_class: None,
        rolled_back_write_count: 0,
        rolled_back_non_support_edit_count: 0,
        soft_budget_inefficient: false,
        fast_loop_command_seen: false,
        agent_final_evaluate_command_seen: false,
        final_evaluate_command_seen: false,
        host_evaluation_commands_run: 0,
        non_support_edit_count: 0,
        last_failure_class: setup_failure_class
            .clone()
            .or_else(|| Some("launch_failed".to_string())),
        evaluation_command_seen: false,
        text_only_action_failure: false,
        first_request_prompt_token_estimate: None,
        first_request_raw_prompt_token_estimate: None,
        first_request_compacted_prompt_token_estimate: None,
        first_request_first_token_latency_ms: None,
        first_model_turn_started: false,
        first_action_emitted: false,
        prompt_token_series_by_turn: Vec::new(),
        read_range_observations: Vec::new(),
        repo_capsule_injected: completion_policy.include_repo_capsule,
        reasoning_enabled: !completion_policy.disable_reasoning,
        path_resolution_failures: 0,
        recovery_turns: 0,
        action_contract_mode: benchmark_action_contract_mode(&completion_policy).to_string(),
        action_contract_selected: benchmark_action_contract_mode(&completion_policy).to_string(),
        action_contract_fallback_reason: None,
        attempt_lineage: vec![benchmark_action_contract_mode(&completion_policy).to_string()],
        effective_prompt_compaction_policy: completion_policy
            .prompt_compaction_policy
            .map(|policy| policy.as_str().to_string()),
        fast_loop_validation_status: None,
        last_validation_failure: None,
        failing_test_names: Vec::new(),
        primary_failure_test_name: None,
        primary_failure_path: None,
        primary_failure_line: None,
        assertion_excerpt: None,
        diagnostic_class: None,
        implementation_target_lease: None,
        dependency_candidates: Vec::new(),
        target_dependency_table: None,
        repair_required: false,
        repair_phase_terminal: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        repair_phase_invalid_action_count: 0,
        post_fast_loop_patch_attempted: false,
        post_fast_loop_validation_rerun_attempted: false,
        patch_packet_injected: false,
        patch_packet_honored_range: None,
        recommended_rerun_command: None,
        fast_loop_rerun_match_kind: None,
        failed_edit_records: Vec::new(),
        local_model_memory: quorp_agent_core::LocalModelMemory::default(),
        local_agent_scorecard: quorp_agent_core::LocalAgentScorecard::default(),
        preview_edit_count: 0,
        preview_edit_success_count: 0,
        preview_created_count: 0,
        replace_range_count: 0,
        replace_range_hash_mismatch_count: 0,
        modify_toml_count: 0,
        apply_preview_count: 0,
        apply_preview_hash_mismatch_count: 0,
        syntax_preview_count: 0,
        syntax_preview_failure_count: 0,
        target_redirect_count: 0,
        evidence_file_fixation_count: 0,
        local_agent_final_failure_classification: Some(
            setup_failure_class
                .clone()
                .unwrap_or_else(|| "launch_failed".to_string()),
        ),
        planner_model: None,
        executor_model: Some(model_id.to_string()),
        deterministic_evaluation_passed: None,
        judge: None,
        primary_failure: Some(setup_failure_class.unwrap_or_else(|| "launch_failed".to_string())),
    };
    write_json(&result_dir.join("benchmark-report.json"), &report)?;
    fs::write(
        result_dir.join("benchmark-report.md"),
        render_report_markdown(&report),
    )?;
    Ok(())
}

fn classify_primary_failure(report: &BenchmarkReport) -> Option<String> {
    if let Some(setup_failure_class) = report.setup_failure_class.as_ref() {
        return Some(setup_failure_class.clone());
    }
    if report.pre_model_bootstrap_stalled {
        return report
            .bootstrap_stall_class
            .clone()
            .or_else(|| Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL.to_string()));
    }
    if report.text_only_action_failure {
        return Some("text_only_action_failure".to_string());
    }
    if report.run_error.is_some() {
        return Some("run_error".to_string());
    }
    let agent_error = report
        .attempts
        .last()
        .and_then(|attempt| attempt.agent_error_message.as_deref())
        .unwrap_or_default();
    if agent_error.contains("source_patch_refusal") {
        return Some("source_patch_refusal".to_string());
    }
    if agent_error.contains("repair loop stalled")
        || agent_error.contains("without a concrete repair action")
        || agent_error.contains("repeated invalid repair-phase actions")
        || agent_error.contains("repeating redundant inspection")
    {
        return Some("repair_loop_stalled".to_string());
    }
    if agent_error.contains("write_phase_action_refusal") {
        return Some("write_phase_action_refusal".to_string());
    }
    if agent_error.contains("parser recovery stalled")
        || agent_error.contains("without changing validation state")
    {
        return Some("parser_recovery_stalled".to_string());
    }
    if let Some(stop_reason) = report.final_stop_reason {
        match stop_reason {
            quorp_agent_core::StopReason::FatalError => {
                return Some("agent_fatal_error".to_string());
            }
            quorp_agent_core::StopReason::BudgetExhausted => {
                return Some("budget_exhausted".to_string());
            }
            quorp_agent_core::StopReason::MaxIterations => {
                return Some("max_iterations".to_string());
            }
            quorp_agent_core::StopReason::PendingValidation => {
                return Some("pending_validation".to_string());
            }
            quorp_agent_core::StopReason::TimeBudgetExhausted => {
                return Some("time_budget_exhausted".to_string());
            }
            quorp_agent_core::StopReason::Cancelled => {
                return Some("cancelled".to_string());
            }
            quorp_agent_core::StopReason::FirstTokenTimeout => {
                return Some("first_token_timeout".to_string());
            }
            quorp_agent_core::StopReason::StreamIdleTimeout => {
                return Some("stream_idle_timeout".to_string());
            }
            quorp_agent_core::StopReason::ModelRequestTimeout => {
                return Some("model_request_timeout".to_string());
            }
            quorp_agent_core::StopReason::Stalled => {
                return Some("stalled".to_string());
            }
            quorp_agent_core::StopReason::Success => {}
        }
    }
    let last_attempt = report.attempts.last()?;
    if last_attempt
        .visible_evaluation
        .as_ref()
        .is_some_and(|outcome| !outcome.passed)
    {
        return Some("visible_evaluation_failed".to_string());
    }
    if last_attempt
        .collector_evaluation
        .as_ref()
        .is_some_and(|outcome| !outcome.passed)
    {
        return Some("collector_evaluation_failed".to_string());
    }
    if last_attempt
        .evaluation
        .as_ref()
        .is_some_and(|outcome| !outcome.passed)
    {
        return Some("evaluation_failed".to_string());
    }
    if last_attempt
        .judge
        .as_ref()
        .is_some_and(judge_blocks_deterministic_success)
    {
        return Some("judge_failed".to_string());
    }
    if deterministic_evaluation_passed(last_attempt) {
        return None;
    }
    if last_attempt.agent_error_message.is_some() {
        return Some("agent_error".to_string());
    }
    None
}

fn classify_local_agent_failure(
    report: &BenchmarkReport,
    primary_failure: Option<&str>,
) -> Option<String> {
    if report.success {
        return Some("success".to_string());
    }
    if report.pre_model_bootstrap_stalled {
        return Some("infra_runtime".to_string());
    }
    let scorecard = &report.local_agent_scorecard;
    if matches!(
        primary_failure,
        Some("first_token_timeout" | "stream_idle_timeout" | "model_request_timeout")
    ) {
        return Some("runtime_startup_or_inference".to_string());
    }
    let agent_error = report
        .attempts
        .last()
        .and_then(|attempt| attempt.agent_error_message.as_deref())
        .unwrap_or_default();
    if agent_error.contains("unsupported native tool call") {
        return if scorecard.first_valid_write_step.is_none() {
            Some("parser_tool_schema".to_string())
        } else {
            Some("model_edit_strategy".to_string())
        };
    }
    if scorecard.syntax_preview_failure_count > 0 {
        return Some("syntax_patch_quality".to_string());
    }
    if scorecard.target_redirect_count > 0 || scorecard.evidence_file_fixation_count > 0 {
        return Some("diagnostic_targeting".to_string());
    }
    if report.diagnostic_class.as_deref() == Some("rust_parse_error")
        && scorecard.first_valid_write_step.is_some()
    {
        return Some("syntax_patch_quality".to_string());
    }
    if agent_error.contains("source_patch_refusal") {
        return Some("source_patch_refusal".to_string());
    }
    if agent_error.contains("needs_baseline_validation")
        || agent_error.contains("repeating redundant inspection")
        || agent_error.contains("without a validation anchor")
        || agent_error.contains("repeated non-patch inspection")
    {
        return Some("context_management".to_string());
    }
    if agent_error.contains("repair loop stalled")
        || agent_error.contains("without a concrete repair action")
        || agent_error.contains("repeated invalid repair-phase actions")
    {
        return Some("repair_loop_stalled".to_string());
    }
    if agent_error.contains("write_phase_action_refusal") {
        return Some("model_edit_strategy".to_string());
    }
    if agent_error.contains("parser recovery stalled")
        || agent_error.contains("without changing validation state")
    {
        return Some("parser_recovery_stalled".to_string());
    }
    if agent_error.contains("needs_patch")
        && agent_error.contains("invalid repair-phase actions")
        && scorecard.first_valid_write_step.is_none()
    {
        return Some("model_edit_strategy".to_string());
    }
    if agent_error.contains("non-allowlisted shell command")
        || agent_error.contains("repeated validation before any repair write")
        || scorecard.rejected_validation_alias_count > 0
    {
        return Some("validation_governance".to_string());
    }
    if agent_error.contains("needs_failure_anchor_read")
        || report.repair_phase_terminal.as_deref() == Some("needs_failure_anchor_read")
    {
        return Some("context_management".to_string());
    }
    if report
        .final_stop_reason
        .is_some_and(|reason| reason == quorp_agent_core::StopReason::FatalError)
        && scorecard.parser_recovery_count > 0
        && scorecard.first_valid_write_step.is_none()
    {
        return Some("parser_tool_schema".to_string());
    }
    if report.repair_required
        && scorecard.redundant_read_count >= 2
        && scorecard.first_valid_write_step.is_none()
    {
        return Some("context_wander".to_string());
    }
    if report.repair_required
        && report.repair_phase_terminal.as_deref() == Some("needs_patch")
        && scorecard.first_valid_write_step.is_none()
    {
        return Some("model_edit_strategy".to_string());
    }
    if report.repair_required
        && (!report.failed_edit_records.is_empty()
            || scorecard.repeated_failed_edit_count > 0
            || scorecard.first_valid_write_step.is_some())
    {
        return Some("model_edit_strategy".to_string());
    }
    if primary_failure == Some("agent_fatal_error") {
        if scorecard.first_valid_write_step.is_some() {
            return Some("model_edit_strategy".to_string());
        }
        if scorecard.parser_recovery_count > 0 {
            return Some("parser_tool_schema".to_string());
        }
        if scorecard.rejected_validation_alias_count > 0 {
            return Some("validation_governance".to_string());
        }
        if report.first_action_emitted || report.first_model_turn_started {
            return Some("context_management".to_string());
        }
        return Some("infra_runtime".to_string());
    }
    primary_failure
        .map(str::to_string)
        .or_else(|| Some("model_semantic_quality".to_string()))
}

fn truncate_report_text(value: &str, char_limit: usize) -> String {
    let mut output = String::new();
    let mut characters = value.chars();
    for _ in 0..char_limit {
        let Some(character) = characters.next() else {
            return value.to_string();
        };
        output.push(character);
    }
    if characters.next().is_some() {
        output.push_str("...");
    }
    output
}

fn render_failed_edit_records_for_report(records: &[quorp_agent_core::FailedEditRecord]) -> String {
    records
        .iter()
        .rev()
        .take(4)
        .map(|record| {
            let lines = if record.matching_line_numbers.is_empty() {
                "lines=unknown".to_string()
            } else {
                format!(
                    "lines={}",
                    record
                        .matching_line_numbers
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            format!(
                "{} {} attempts={} {}",
                record.action_kind, record.path, record.attempts, lines
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn attempt_passed(attempt: &AttemptReport) -> bool {
    let agent_succeeded = matches!(
        attempt.agent_stop_reason,
        quorp_agent_core::StopReason::Success
    );
    (agent_succeeded || deterministic_evaluation_passed(attempt))
        && evaluations_all_passed(attempt)
        && attempt
            .judge
            .as_ref()
            .is_none_or(|judge| !judge_blocks_deterministic_success(judge))
}

fn deterministic_evaluation_passed(attempt: &AttemptReport) -> bool {
    (attempt.visible_evaluation.is_some()
        || attempt.collector_evaluation.is_some()
        || attempt.evaluation.is_some())
        && evaluations_all_passed(attempt)
}

fn evaluations_all_passed(attempt: &AttemptReport) -> bool {
    attempt
        .visible_evaluation
        .as_ref()
        .is_none_or(|outcome| outcome.passed)
        && attempt
            .collector_evaluation
            .as_ref()
            .is_none_or(|outcome| outcome.passed)
        && attempt
            .evaluation
            .as_ref()
            .is_none_or(|outcome| outcome.passed)
}

fn judge_blocks_deterministic_success(judge: &ChallengeJudgeOutcome) -> bool {
    if judge.passed {
        return false;
    }
    !matches!(
        judge.summary.as_str(),
        "judge request failed" | "judge runtime could not start"
    )
}

fn count_evaluation_commands(attempt: &AttemptReport) -> usize {
    (if attempt.visible_evaluation.is_some() {
        1
    } else {
        0
    }) + (if attempt.collector_evaluation.is_some() {
        1
    } else {
        0
    }) + (if attempt.evaluation.is_some() { 1 } else { 0 })
}

fn count_mistakes_corrected(attempts: &[AttemptReport]) -> usize {
    let last_success_index = attempts.iter().rposition(attempt_passed);
    match last_success_index {
        Some(success_index) => attempts
            .iter()
            .enumerate()
            .filter(|(index, attempt)| *index < success_index && !attempt_passed(attempt))
            .count(),
        None => 0,
    }
}

fn git_numstat(workspace_dir: &Path) -> anyhow::Result<(u64, u64)> {
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_dir)
        .arg("diff")
        .arg("--numstat")
        .output()
        .with_context(|| format!("failed to run git numstat in {}", workspace_dir.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git numstat failed in {} with status {}",
            workspace_dir.display(),
            output.status
        );
    }
    let mut lines_added = 0u64;
    let mut lines_removed = 0u64;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let added = parts.next().unwrap_or("0");
        let removed = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or_default();
        if !is_reportable_changed_file(path) {
            continue;
        }
        if let Ok(value) = added.parse::<u64>() {
            lines_added = lines_added.saturating_add(value);
        }
        if let Ok(value) = removed.parse::<u64>() {
            lines_removed = lines_removed.saturating_add(value);
        }
    }
    Ok((lines_added, lines_removed))
}

fn resolve_benchmark(path: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve benchmark path {}", path.display()))?;
    if looks_like_warpos_staged_workspace(&canonical) {
        return resolve_from_warpos_staged_workspace(&canonical);
    }
    if looks_like_proof_full_workspace(&canonical) {
        return resolve_from_workspace_root(&canonical);
    }
    if looks_like_issue_dir(&canonical) {
        return resolve_from_issue_dir(&canonical);
    }
    anyhow::bail!(
        "benchmark path `{}` was not recognized as an issue brief directory or proof-full workspace root",
        canonical.display()
    );
}

fn resolve_challenge_case(
    path: &Path,
    explicit_condition: Option<&str>,
) -> anyhow::Result<Option<ResolvedChallengeCase>> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve challenge path {}", path.display()))?;
    let Some(case_root) = find_ancestor_with_file(&canonical, "benchmark.json") else {
        return Ok(None);
    };
    let manifest_path = case_root.join("benchmark.json");
    let manifest: ChallengeManifest = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let condition =
        resolve_challenge_condition(&canonical, &case_root, &manifest, explicit_condition)?;
    let declared_objective = case_root.join(&manifest.objective_file);
    let objective_source = if canonical == declared_objective {
        canonical.clone()
    } else if canonical.is_dir()
        || canonical.starts_with(case_root.join("workspace"))
        || looks_like_proof_full_workspace(&case_root)
    {
        declared_objective.clone()
    } else if canonical.starts_with(&case_root) {
        anyhow::bail!(
            "provided challenge path {} does not match the declared objective file {}; pass the case root, the objective markdown, or a workspace file",
            canonical.display(),
            declared_objective.display()
        );
    } else {
        declared_objective.clone()
    };
    if !objective_source.exists() {
        anyhow::bail!(
            "failed to locate challenge objective file at {}",
            objective_source.display()
        );
    }
    let success_source = case_root.join(&manifest.success_file);
    if !success_source.exists() {
        anyhow::bail!(
            "failed to locate challenge success file at {}",
            success_source.display()
        );
    }
    Ok(Some(ResolvedChallengeCase {
        case_root,
        manifest,
        condition,
        objective_source,
        success_source,
    }))
}

fn resolve_challenge_condition(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
    explicit_condition: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(explicit) = explicit_condition {
        if manifest
            .repo_condition
            .iter()
            .any(|condition| condition == explicit)
        {
            return Ok(explicit.to_string());
        }
        anyhow::bail!(
            "challenge condition `{}` is not listed in benchmark.json repo_condition",
            explicit
        );
    }

    if let Some(inferred) = infer_condition_from_workspace_path(canonical, case_root, manifest) {
        return Ok(inferred);
    }

    if manifest
        .repo_condition
        .iter()
        .any(|condition| condition == "proof-full")
    {
        return Ok("proof-full".to_string());
    }

    manifest
        .repo_condition
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("benchmark.json did not list any repo_condition values"))
}

fn infer_condition_from_workspace_path(
    canonical: &Path,
    case_root: &Path,
    manifest: &ChallengeManifest,
) -> Option<String> {
    let workspace_root = case_root.join("workspace");
    if !canonical.starts_with(&workspace_root) {
        return None;
    }
    let relative = canonical.strip_prefix(&workspace_root).ok()?;
    let inferred = relative
        .components()
        .next()?
        .as_os_str()
        .to_str()?
        .to_string();
    manifest
        .repo_condition
        .iter()
        .any(|condition| condition == &inferred)
        .then_some(inferred)
}

fn find_ancestor_with_file(path: &Path, file_name: &str) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.join(file_name).exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn resolve_from_warpos_staged_workspace(
    workspace_root: &Path,
) -> anyhow::Result<ResolvedBenchmark> {
    let marker = read_warpos_benchmark_root_marker(workspace_root)?;
    let handoff_root = resolve_marker_handoff_root(workspace_root, &marker);
    let benchmark_root = find_warpos_benchmarks_root(&handoff_root).unwrap_or(handoff_root.clone());
    let issue_dir = find_warpos_issue_dir(&benchmark_root, &marker.issue);
    Ok(ResolvedBenchmark {
        benchmark_root,
        issue_id: marker.issue.clone(),
        benchmark_name: marker
            .benchmark
            .clone()
            .unwrap_or_else(|| marker.issue.clone()),
        issue_dir: issue_dir.clone(),
        workspace_source: workspace_root.to_path_buf(),
        objective_source: [
            workspace_root.join("START_HERE.md"),
            workspace_root.join("README.md"),
        ]
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow::anyhow!("failed to locate benchmark objective file"))?,
        visible_evaluator: [
            workspace_root.join("evaluate.sh"),
            workspace_root.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: issue_dir
            .as_ref()
            .and_then(|path| find_collector_script(path)),
        context_files: collect_context_files(workspace_root),
        repair_artifacts: collect_repair_artifacts(workspace_root),
    })
}

fn read_warpos_benchmark_root_marker(
    workspace_root: &Path,
) -> anyhow::Result<WarposBenchmarkRootMarker> {
    let marker_path = workspace_root.join(".benchmark-root.json");
    serde_json::from_str::<WarposBenchmarkRootMarker>(
        &fs::read_to_string(&marker_path)
            .with_context(|| format!("failed to read {}", marker_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", marker_path.display()))
}

fn resolve_marker_handoff_root(
    workspace_root: &Path,
    marker: &WarposBenchmarkRootMarker,
) -> PathBuf {
    let handoff_root = if marker.handoff_root.is_absolute() {
        marker.handoff_root.clone()
    } else {
        workspace_root.join(&marker.handoff_root)
    };
    fs::canonicalize(&handoff_root).unwrap_or(handoff_root)
}

fn find_warpos_benchmarks_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find_map(|ancestor| {
        (ancestor.file_name().and_then(|name| name.to_str()) == Some("benchmarks"))
            .then(|| ancestor.to_path_buf())
    })
}

fn find_warpos_issue_dir(benchmarks_root: &Path, issue_id: &str) -> Option<PathBuf> {
    [
        benchmarks_root.join("issues").join(issue_id),
        benchmarks_root
            .join("exhaustive")
            .join("issues")
            .join(issue_id),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn resolve_from_workspace_root(workspace_root: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let issue_id = workspace_root
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!("failed to infer issue id from {}", workspace_root.display())
        })?
        .to_string();
    let benchmark_root = find_benchmark_root(workspace_root)?;
    let issue_dir = benchmark_root
        .join("exhaustive")
        .join("issues")
        .join(&issue_id);
    let issue_dir = issue_dir.exists().then_some(issue_dir);
    Ok(ResolvedBenchmark {
        benchmark_root: benchmark_root.clone(),
        issue_id: issue_id.clone(),
        benchmark_name: issue_id.clone(),
        issue_dir: issue_dir.clone(),
        workspace_source: workspace_root.to_path_buf(),
        objective_source: issue_dir
            .as_ref()
            .map(|dir| dir.join("README.md"))
            .filter(|path| path.exists())
            .or_else(|| {
                [
                    workspace_root.join("START_HERE.md"),
                    workspace_root.join("README.md"),
                ]
                .into_iter()
                .find(|path| path.exists())
            })
            .ok_or_else(|| anyhow::anyhow!("failed to locate benchmark objective file"))?,
        visible_evaluator: [
            workspace_root.join("evaluate.sh"),
            workspace_root.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: issue_dir
            .as_ref()
            .and_then(|path| find_collector_script(path)),
        context_files: collect_context_files(workspace_root),
        repair_artifacts: collect_repair_artifacts(workspace_root),
    })
}

fn resolve_from_issue_dir(issue_dir: &Path) -> anyhow::Result<ResolvedBenchmark> {
    let issue_id = issue_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("failed to infer issue id from {}", issue_dir.display()))?
        .to_string();
    let benchmark_root = find_benchmark_root(issue_dir)?;
    let handoffs_root = benchmark_root.join("handoffs");
    let workspace_source =
        find_workspace_for_issue(&handoffs_root, &issue_id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "failed to find proof-full workspace for issue `{}` under {}",
                issue_id,
                handoffs_root.display()
            )
        })?;
    Ok(ResolvedBenchmark {
        benchmark_root,
        issue_id: issue_id.clone(),
        benchmark_name: issue_id.clone(),
        issue_dir: Some(issue_dir.to_path_buf()),
        workspace_source: workspace_source.clone(),
        objective_source: issue_dir.join("README.md"),
        visible_evaluator: [
            workspace_source.join("evaluate.sh"),
            workspace_source.join("evaluate_visible.sh"),
        ]
        .into_iter()
        .find(|path| path.exists()),
        collector_evaluator: find_collector_script(issue_dir),
        context_files: collect_context_files(&workspace_source),
        repair_artifacts: collect_repair_artifacts(&workspace_source),
    })
}

fn find_collector_script(issue_dir: &Path) -> Option<PathBuf> {
    [
        issue_dir.join("evaluate.sh"),
        issue_dir.join(".hidden").join("evaluate_hidden.sh"),
        issue_dir.join("hidden").join("check.sh"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn find_workspace_for_issue(
    handoffs_root: &Path,
    issue_id: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if !handoffs_root.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(handoffs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(issue_id).join("proof-full");
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn collect_context_files(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root.join(".benchmark-root.json"),
        workspace_root.join("issue.json"),
        workspace_root.join("START_HERE.md"),
        workspace_root.join("YOU_ARE_HERE.txt"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn collect_repair_artifacts(workspace_root: &Path) -> Vec<PathBuf> {
    [
        workspace_root
            .join("target")
            .join("agent")
            .join("repair-bundle.json"),
        workspace_root
            .join("target")
            .join("agent")
            .join("last-failure.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn rebase_attempt_path(
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
    original_path: &Path,
) -> PathBuf {
    original_path
        .strip_prefix(&resolved.workspace_source)
        .map(|relative| workspace_dir.join(relative))
        .unwrap_or_else(|_| original_path.to_path_buf())
}

fn prepare_attempt_workspace(
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
) -> anyhow::Result<()> {
    if workspace_dir.exists() {
        fs::remove_dir_all(workspace_dir)?;
    }
    log_phase(
        "sandbox",
        ANSI_BLUE,
        format!("copying workspace to {}", workspace_dir.display()),
    );
    copy_dir_all(&resolved.workspace_source, workspace_dir)?;
    ensure_git_baseline(workspace_dir)?;
    Ok(())
}

fn synthesize_objective(
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
    safety_mode_label: &str,
    helper_briefing: Option<&str>,
) -> anyhow::Result<SynthesizedObjective> {
    let objective_text =
        build_benchmark_objective(resolved, workspace_dir, safety_mode_label, helper_briefing)?;
    let objective_path = workspace_dir.join(SYNTHETIC_OBJECTIVE_FILE);
    fs::write(&objective_path, objective_text)?;
    Ok(SynthesizedObjective {
        prompt_token_estimate: estimate_token_count(&fs::read_to_string(&objective_path)?),
        path: objective_path,
    })
}

fn build_benchmark_objective(
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
    safety_mode_label: &str,
    helper_briefing: Option<&str>,
) -> anyhow::Result<String> {
    let objective = fs::read_to_string(&resolved.objective_source)
        .with_context(|| format!("failed to read {}", resolved.objective_source.display()))?;
    let rebased_objective_path =
        rebase_attempt_path(resolved, workspace_dir, &resolved.objective_source);
    let mut sections = vec![
        format!(
            "# Quorp Benchmark Objective\n\nYou are running benchmark `{}` for issue `{}`.\nWork autonomously until the issue is fixed, the evaluators pass, or you hit a stop condition.",
            resolved.benchmark_name, resolved.issue_id
        ),
        format!(
            "## Workspace\n- Editable workspace root: `.`\n- Safety mode: `{safety_mode_label}`\n- Repo capsule injection is enabled for benchmark turns.\n- Workspace root entries:\n{}",
            summarize_workspace_root(workspace_dir)
        ),
        "## Workspace Path Rules\n- All tool paths must be relative to the workspace root.\n- Do not use absolute paths in tool actions.\n- If the brief names fields, symbols, endpoints, structs, or tests, use `SearchText`, `SearchSymbols`, or `GetRepoCapsule` before guessing filenames.\n- Prefer likely owner crates and touch targets before rereading root metadata.\n- Avoid rereading `AGENTS.md`, `Cargo.lock`, `README.md`, or other root metadata unless the brief explicitly requires them.".to_string(),
        format!(
            "## Authoritative Brief\n- File: `{}`\n- Inline summary:\n{}",
            rebased_objective_path.display(),
            summarize_markdown_brief(&objective)
        ),
        "## First Turn Requirements\n- First turn must produce a short execution plan before edits.\n- Name the likely target files or crates, the first search/query steps, and the validation plan.\n- Use `task_updates` and `verifier_plan` to record that plan.\n- If the brief mentions a symbol or field, search for it before opening guessed file paths.\n- Keep the first turn compact: no repeated reads, and inspect at most four files before either editing or validating.".to_string(),
        "## Required Operating Rules\n- Start from the owning crate or nearest local owner.\n- Validate locally first and widen only when forced by the dependency graph or public contract.\n- Continue after the first visible green run when collector validation still fails.\n- Include files changed, validation commands, widening, and attempt count in the final report.".to_string(),
        format!(
            "## Validation Commands\n{}",
            [
                resolved
                    .visible_evaluator
                    .as_ref()
                    .map(|path| format!("- Visible evaluator: `{}`", rebase_attempt_path(resolved, workspace_dir, path).display())),
                resolved
                    .collector_evaluator
                    .as_ref()
                    .map(|path| format!("- Collector evaluator: `{}`", rebase_attempt_path(resolved, workspace_dir, path).display())),
                Some("- Prefer `RunValidation` for fmt, clippy, and tests before raw shell commands.".to_string()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n")
        ),
        format!(
            "## Required Files To Read\n{}",
            resolved
                .context_files
                .iter()
                .map(|path| format!("- `{}`", rebase_attempt_path(resolved, workspace_dir, path).display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    ];
    if let Some(briefing_text) = helper_briefing
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.insert(2, format!("## Helper Briefing\n{briefing_text}"));
    }
    if !resolved.context_files.is_empty() {
        sections.push(format!(
            "## File Summaries\n{}",
            resolved
                .context_files
                .iter()
                .map(|path| summarize_context_file(&rebase_attempt_path(
                    resolved,
                    workspace_dir,
                    path
                )))
                .collect::<Result<Vec<_>, _>>()?
                .join("\n")
        ));
    }
    if !resolved.repair_artifacts.is_empty() {
        sections.push(format!(
            "## Repair Artifacts\n{}",
            resolved
                .repair_artifacts
                .iter()
                .map(|path| format!(
                    "- Read repair artifact `{}` before widening.",
                    rebase_attempt_path(resolved, workspace_dir, path).display()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    let mut prompt = sections.join("\n\n");
    if estimate_token_count(&prompt) > SAFE_PROMPT_TOKEN_CAP {
        prompt = trim_prompt_to_safe_cap(prompt, resolved, workspace_dir);
    }
    Ok(prompt)
}

fn load_benchmark_briefing(
    briefing_file: Option<&Path>,
    issue_id: &str,
) -> anyhow::Result<Option<String>> {
    let Some(path) = briefing_file else {
        return Ok(None);
    };
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read briefing file {}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .with_context(|| format!("failed to parse briefing JSON {}", path.display()))?;
        return Ok(select_benchmark_briefing_text(&value, issue_id));
    }
    Ok(Some(raw))
}

fn select_benchmark_briefing_text(value: &serde_json::Value, issue_id: &str) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Object(object) => object
            .get(issue_id)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                object
                    .get("default")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            }),
        _ => None,
    }
}

fn summarize_markdown_brief(markdown: &str) -> String {
    markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .map(|line| {
            if line.starts_with('#') || line.starts_with('-') {
                format!("- {}", line.trim_start_matches('#').trim())
            } else {
                format!("- {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_bullet_list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "- [none]".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- `{}`", item))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn indent_block(text: &str) -> String {
    if text.trim().is_empty() {
        return "<empty>".to_string();
    }
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_judge_output(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }

    let lines = trimmed.lines().collect::<Vec<_>>();
    let within_line_limit = lines.len() <= JUDGE_OUTPUT_LINE_LIMIT;
    let within_char_limit = trimmed.chars().count() <= JUDGE_OUTPUT_CHAR_LIMIT;
    if within_line_limit && within_char_limit {
        return trimmed.to_string();
    }

    let head_count = (JUDGE_OUTPUT_LINE_LIMIT / 2).max(1);
    let tail_count = JUDGE_OUTPUT_LINE_LIMIT.saturating_sub(head_count);
    let head = lines
        .iter()
        .take(head_count)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let tail = lines
        .iter()
        .rev()
        .take(tail_count)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{head}\n... truncated {} lines / {} chars ...\n{tail}",
        lines.len(),
        trimmed.chars().count()
    )
}

fn summarize_context_file(path: &Path) -> anyhow::Result<String> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("context");
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read context file {}", path.display()))?;
    let summary = match name {
        "AGENTS.md" | "START_HERE.md" | "YOU_ARE_HERE.txt" => {
            summarize_plaintext_lines(&content, 4)
        }
        "challenge-capsule.json" => summarize_challenge_capsule(&content)?,
        "agent-map.json" => summarize_agent_map(&content)?,
        "test-map.json" => summarize_test_map(&content)?,
        "witness-graph.json" => summarize_witness_graph(&content)?,
        _ => summarize_plaintext_lines(&content, 3),
    };
    Ok(format!("- `{}`: {}", path.display(), summary))
}

fn summarize_plaintext_lines(content: &str, limit: usize) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn summarize_agent_map(content: &str) -> anyhow::Result<String> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    let owners = value["owners"]
        .as_array()
        .into_iter()
        .flatten()
        .take(3)
        .map(|owner| {
            let crate_name = owner["crate"].as_str().unwrap_or("unknown");
            let paths = owner["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            let validation = owner["validation"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            format!("owner `{crate_name}` paths [{paths}] validate [{validation}]")
        })
        .collect::<Vec<_>>();
    Ok(owners.join(" | "))
}

fn summarize_test_map(content: &str) -> anyhow::Result<String> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    let crates = value["crates"]
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
        .map(|crate_entry| {
            let crate_name = crate_entry["crate"].as_str().unwrap_or("unknown");
            let tests = crate_entry["tests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            format!("crate `{crate_name}` tests [{tests}]")
        })
        .collect::<Vec<_>>();
    Ok(crates.join(" | "))
}

fn summarize_witness_graph(content: &str) -> anyhow::Result<String> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    let node_count = value["nodes"]
        .as_array()
        .map(|nodes| nodes.len())
        .unwrap_or_default();
    let edge_count = value["edges"]
        .as_array()
        .map(|edges| edges.len())
        .unwrap_or_default();
    let node_labels = value["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
        .filter_map(|node| node["id"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "nodes={node_count} edges={edge_count} ids=[{node_labels}]"
    ))
}

fn summarize_challenge_capsule(content: &str) -> anyhow::Result<String> {
    let capsule: ChallengeCapsule = serde_json::from_str(content)?;
    Ok(format!(
        "class={} owners=[{}] fast_loop=[{}] companion=[{}]",
        capsule.case_class,
        capsule.owner_files.join(", "),
        capsule.fast_loop_commands.join(" | "),
        capsule.companion_files_required.join(", ")
    ))
}

fn trim_prompt_to_safe_cap(
    prompt: String,
    resolved: &ResolvedBenchmark,
    workspace_dir: &Path,
) -> String {
    let rebased_objective_path =
        rebase_attempt_path(resolved, workspace_dir, &resolved.objective_source);
    let mut sections = vec![
        format!(
            "# Quorp Benchmark Objective\n\nYou are running benchmark `{}` for issue `{}`.",
            resolved.benchmark_name, resolved.issue_id
        ),
        format!(
            "## Brief\n- Read `{}` first.\n- Fix only what the brief requires.\n- Validate locally before widening.",
            rebased_objective_path.display()
        ),
        format!(
            "## Files To Read\n{}",
            resolved
                .context_files
                .iter()
                .map(|path| format!(
                    "- `{}`",
                    rebase_attempt_path(resolved, workspace_dir, path).display()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    ];
    if !resolved.repair_artifacts.is_empty() {
        sections.push(format!(
            "## Repair Artifacts\n{}",
            resolved
                .repair_artifacts
                .iter()
                .map(|path| format!(
                    "- `{}`",
                    rebase_attempt_path(resolved, workspace_dir, path).display()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let trimmed = sections.join("\n\n");
    if estimate_token_count(&trimmed) <= SAFE_PROMPT_TOKEN_CAP {
        trimmed
    } else {
        prompt
            .chars()
            .take((SAFE_PROMPT_TOKEN_CAP.saturating_mul(4)) as usize)
            .collect()
    }
}

fn write_benchmark_agent_config(workspace_dir: &Path) -> anyhow::Result<()> {
    let config_dir = workspace_dir.join(".quorp");
    fs::create_dir_all(&config_dir)?;
    fs::write(
        config_dir.join("agent.toml"),
        r#"[defaults]
mode = "act"

[autonomy]
profile = "autonomous_host"

[policy]
mode = "benchmark_autonomous"

[policy.allow]
read_file = true
list_directory = true
search_text = true
search_symbols = true
get_repo_capsule = true
write_file = true
apply_patch = true
replace_block = true
set_executable = true
run_validation = true
mcp_call_tool = false
network = false
run_command = ["cargo ", "./evaluate.sh", "bash ./evaluate.sh", "sh ./evaluate.sh", "./evaluate_visible.sh", "bash ./evaluate_visible.sh", "sh ./evaluate_visible.sh"]

[policy.limits]
max_command_runtime_seconds = 180
max_command_output_bytes = 131072

[validation]
fmt_command = "cargo fmt --all"
clippy_command = "cargo clippy --all-targets --no-deps -- -D warnings"
workspace_test_command = "cargo test --quiet"
targeted_test_prefix = "cargo test --quiet "

[prompt]
extra_instructions = [
  "Benchmark mode. Act, do not narrate.",
  "Read `.quorp/challenge-capsule.json` first. Keep owner files, fast loop, touch targets, and named tests in mind.",
  "Use the smallest tool that works. Search first, then read the owner slice, then patch.",
  "Stay on owner files and named tests until the fast loop says the local guess is wrong.",
  "After a failed fast loop, reread the failure anchor, patch an owner file, or rerun the exact fast loop. Do not spend a turn planning.",
  "Use workspace-relative paths only.",
  "Prefer ReplaceBlock for tiny edits, ApplyPatch for multi-file changes, WriteFile for new files, and SetExecutable for scripts.",
  "Do not invent names that were not visible in read context.",
  "Use RunValidation for fmt, clippy, and tests when possible.",
  "After any meaningful edit, run the smallest fast loop, then run `./evaluate.sh proof-full` before stopping.",
  "If companion files exist, update them or explicitly rule them out.",
]
"#,
    )?;
    Ok(())
}

fn write_benchmark_sandbox_cargo_config(
    sandbox_root: &Path,
    condition: &str,
) -> anyhow::Result<()> {
    let cargo_dir = sandbox_root.join(".cargo");
    fs::create_dir_all(&cargo_dir)?;
    fs::write(
        cargo_dir.join("config.toml"),
        format!(
            "[build]\ntarget-dir = \"../{}/{}\"\n",
            CHALLENGE_CARGO_CACHE_DIR, condition
        ),
    )?;
    Ok(())
}

fn write_workspace_challenge_command_wrappers(workspace_dir: &Path) -> anyhow::Result<()> {
    for file_name in ["evaluate.sh", "reset.sh"] {
        let wrapper_path = workspace_dir.join(file_name);
        if wrapper_path.exists() {
            continue;
        }
        fs::write(
            &wrapper_path,
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\ncd \"$(dirname \"$0\")/../..\"\nexec ./{file_name} \"$@\"\n"
            ),
        )?;
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&wrapper_path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&wrapper_path, permissions)?;
        }
    }
    Ok(())
}

fn summarize_workspace_root(workspace_dir: &Path) -> String {
    match fs::read_dir(workspace_dir) {
        Ok(entries) => {
            let mut names = entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let mut name = entry.file_name().into_string().ok()?;
                    let metadata = entry.metadata().ok()?;
                    if metadata.is_dir() {
                        name.push('/');
                    }
                    Some(name)
                })
                .collect::<Vec<_>>();
            names.sort();
            if names.is_empty() {
                "- [empty]".to_string()
            } else {
                names
                    .into_iter()
                    .take(12)
                    .map(|name| format!("- `{name}`"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Err(_) => "- [unavailable]".to_string(),
    }
}

fn run_visible_evaluator(script: &Path, workspace_dir: &Path) -> anyhow::Result<EvaluatorOutcome> {
    log_phase(
        "visible",
        ANSI_BLUE,
        format!("running visible evaluator {}", script.display()),
    );
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let output = Command::new(script)
        .current_dir(workspace_dir)
        .output()
        .with_context(|| format!("failed to run visible evaluator {}", script.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: "visible".to_string(),
        script: script.to_path_buf(),
        command: Some(script.display().to_string()),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

fn run_collector_evaluator(
    script: &Path,
    workspace_dir: &Path,
    attempt_dir: &Path,
) -> anyhow::Result<EvaluatorOutcome> {
    log_phase(
        "collector",
        ANSI_BLUE,
        format!("running collector evaluator {}", script.display()),
    );
    let started_at = std::time::Instant::now();
    #[allow(clippy::disallowed_methods)]
    let output = Command::new(script)
        .arg(workspace_dir)
        .env("QUORP_BENCHMARK_WORKSPACE", workspace_dir)
        .env("QUORP_BENCHMARK_ATTEMPT_DIR", attempt_dir)
        .current_dir(script.parent().unwrap_or_else(|| Path::new("/")))
        .output()
        .with_context(|| format!("failed to run collector evaluator {}", script.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(EvaluatorOutcome {
        name: "collector".to_string(),
        script: script.to_path_buf(),
        command: Some(format!("{} {}", script.display(), workspace_dir.display())),
        duration_ms: started_at.elapsed().as_millis() as u64,
        exit_code: output.status.code().unwrap_or(-1),
        passed: evaluator_passed(output.status.success(), &stdout),
        stdout,
        stderr,
    })
}

fn git_changed_files(workspace_dir: &Path) -> anyhow::Result<Vec<String>> {
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_dir)
        .arg("diff")
        .arg("--name-only")
        .output();
    match output {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .filter(|line| is_reportable_changed_file(line))
            .map(str::to_string)
            .collect()),
        _ => Ok(Vec::new()),
    }
}

fn challenge_ignored_changed_files(
    metadata: &ChallengeMetadata,
    workspace_dir: &Path,
) -> Vec<String> {
    let mut ignored = BTreeSet::new();
    let benchmark_manifest_path = workspace_dir.join("benchmark.json");
    let mut paths = vec![
        metadata.objective_file.as_path(),
        metadata.success_file.as_path(),
        metadata.capsule_file.as_path(),
        benchmark_manifest_path.as_path(),
    ];
    if let Some(reference_file) = metadata.reference_file.as_deref() {
        paths.push(reference_file);
    }
    for path in paths {
        if let Ok(relative) = path.strip_prefix(workspace_dir) {
            let value = relative.to_string_lossy().replace('\\', "/");
            if !value.trim().is_empty() {
                ignored.insert(value);
            }
        }
    }
    ignored.into_iter().collect()
}

fn filter_ignored_changed_files(
    changed_files: &[String],
    ignored_changed_files: &[String],
) -> Vec<String> {
    let ignored = ignored_changed_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    changed_files
        .iter()
        .filter(|path| !ignored.contains(path.as_str()))
        .cloned()
        .collect()
}

fn count_non_support_changed_files(
    changed_files: &[String],
    ignored_changed_files: &[String],
) -> usize {
    filter_ignored_changed_files(changed_files, ignored_changed_files)
        .into_iter()
        .filter(|path| is_reportable_changed_file(path))
        .filter(|path| !is_support_or_generated_changed_file(path))
        .count()
}

fn is_reportable_changed_file(path: &str) -> bool {
    let normalized = path.trim();
    !normalized.is_empty()
        && !normalized.starts_with("target/")
        && !normalized.starts_with(".warpos-capture-probe/")
        && !normalized.starts_with(".quorp/")
}

fn is_support_or_generated_changed_file(path: &str) -> bool {
    let normalized = path.trim().trim_start_matches("./");
    if normalized.is_empty() {
        return true;
    }
    if normalized.starts_with("target/")
        || normalized.starts_with(".git/")
        || normalized.starts_with(".quorp/")
        || normalized.starts_with(".warpos-capture-probe/")
    {
        return true;
    }
    matches!(
        normalized,
        "START_HERE.md"
            | "SUCCESS.md"
            | "REFERENCE.md"
            | "LOCAL_REPRO.md"
            | "RUNNER_FEEDBACK.md"
            | "CONTEXT_WARNING.md"
            | "benchmark.json"
            | "issue.json"
            | "evaluation.json"
            | "hidden-evaluation.json"
            | "visible-evaluation.json"
            | "collector-evaluation.json"
            | "benchmark-report.json"
            | "benchmark-report.md"
    )
}

fn read_checkpoint_validation_state(
    checkpoint_path: &Path,
) -> anyhow::Result<CheckpointValidationState> {
    if !checkpoint_path.exists() {
        return Ok(CheckpointValidationState::default());
    }
    let checkpoint: serde_json::Value = serde_json::from_str(&fs::read_to_string(checkpoint_path)?)
        .with_context(|| format!("failed to parse {}", checkpoint_path.display()))?;
    let ledger = checkpoint
        .get("snapshot")
        .and_then(|value| value.get("benchmark_case_ledger"));
    let snapshot_failed_edit_records = checkpoint
        .get("snapshot")
        .and_then(|value| value.get("failed_edit_records"))
        .and_then(|value| {
            serde_json::from_value::<Vec<quorp_agent_core::FailedEditRecord>>(value.clone()).ok()
        })
        .unwrap_or_default();
    let local_model_memory = checkpoint
        .get("snapshot")
        .and_then(|value| value.get("local_model_memory"))
        .and_then(|value| {
            serde_json::from_value::<quorp_agent_core::LocalModelMemory>(value.clone()).ok()
        })
        .unwrap_or_default();
    let local_agent_scorecard = local_model_memory.scorecard.clone();
    let validation_status = ledger
        .and_then(|value| value.get("validation_status"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let last_validation_failure = ledger
        .and_then(|value| value.get("last_validation_failure"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let validation_details = ledger.and_then(|value| value.get("validation_details"));
    let failing_test_names = validation_details
        .and_then(|value| value.get("failing_test_names"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let primary_failure_test_name = validation_details
        .and_then(|value| value.get("primary_failure_test_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let primary_failure_path = validation_details
        .and_then(|value| value.get("primary_failure_path"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let primary_failure_line = validation_details
        .and_then(|value| value.get("primary_failure_line"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let assertion_excerpt = validation_details
        .and_then(|value| value.get("assertion_excerpt"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let diagnostic_class = validation_details
        .and_then(|value| value.get("diagnostic_class"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| local_model_memory.diagnostic_class.clone());
    let implementation_target_lease = validation_details
        .and_then(|value| value.get("implementation_target_lease"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| local_model_memory.implementation_target_lease.clone());
    let dependency_candidates = validation_details
        .and_then(|value| value.get("dependency_candidates"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
        .unwrap_or_else(|| local_model_memory.dependency_candidates.clone());
    let target_dependency_table = validation_details
        .and_then(|value| value.get("target_dependency_table"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| local_model_memory.target_dependency_table.clone());
    let repair_required = validation_details
        .and_then(|value| value.get("repair_required"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let repair_phase_terminal = validation_details
        .and_then(|value| value.get("repair_phase_terminal"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let failure_anchor_reread_attempted = validation_details
        .and_then(|value| value.get("failure_anchor_reread_attempted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let failure_anchor_reread_honored = validation_details
        .and_then(|value| value.get("failure_anchor_reread_honored"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let implementation_reread_allowed = validation_details
        .and_then(|value| value.get("implementation_reread_allowed"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let implementation_reread_attempted = validation_details
        .and_then(|value| value.get("implementation_reread_attempted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let implementation_reread_honored = validation_details
        .and_then(|value| value.get("implementation_reread_honored"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let repair_phase_invalid_action_count = validation_details
        .and_then(|value| value.get("repair_phase_invalid_action_count"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let post_fast_loop_patch_attempted = validation_details
        .and_then(|value| value.get("post_fast_loop_patch_attempted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let post_fast_loop_validation_rerun_attempted = validation_details
        .and_then(|value| value.get("post_fast_loop_validation_rerun_attempted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let patch_packet_injected = validation_details
        .and_then(|value| value.get("patch_packet_injected"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let patch_packet_honored_range = validation_details
        .and_then(|value| value.get("patch_packet_honored_range"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let recommended_rerun_command = validation_details
        .and_then(|value| value.get("recommended_rerun_command"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let fast_loop_rerun_match_kind = validation_details
        .and_then(|value| value.get("fast_loop_rerun_match_kind"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let mut failed_edit_records = validation_details
        .and_then(|value| value.get("failed_edit_records"))
        .and_then(|value| {
            serde_json::from_value::<Vec<quorp_agent_core::FailedEditRecord>>(value.clone()).ok()
        })
        .unwrap_or_default();
    if failed_edit_records.is_empty() {
        failed_edit_records = snapshot_failed_edit_records;
    }
    Ok(CheckpointValidationState {
        validation_status,
        last_validation_failure,
        failing_test_names,
        primary_failure_test_name,
        primary_failure_path,
        primary_failure_line,
        assertion_excerpt,
        diagnostic_class,
        implementation_target_lease,
        dependency_candidates,
        target_dependency_table,
        repair_required,
        repair_phase_terminal,
        failure_anchor_reread_attempted,
        failure_anchor_reread_honored,
        implementation_reread_allowed,
        implementation_reread_attempted,
        implementation_reread_honored,
        repair_phase_invalid_action_count,
        post_fast_loop_patch_attempted,
        post_fast_loop_validation_rerun_attempted,
        patch_packet_injected,
        patch_packet_honored_range,
        recommended_rerun_command,
        fast_loop_rerun_match_kind,
        failed_edit_records,
        local_model_memory,
        local_agent_scorecard,
    })
}

fn extract_validation_summaries(events_path: &Path) -> anyhow::Result<Vec<String>> {
    if !events_path.exists() {
        return Ok(Vec::new());
    }
    let mut validations = Vec::new();
    for line in fs::read_to_string(events_path)?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value["payload"]["event"] == "validation_started"
            && let Some(summary) = value["payload"]["summary"].as_str()
        {
            validations.push(summary.to_string());
        }
    }
    Ok(validations)
}

fn extract_request_metrics(events_path: &Path) -> anyhow::Result<RequestMetricsSummary> {
    if !events_path.exists() {
        return Ok(RequestMetricsSummary::default());
    }
    let mut summary = RequestMetricsSummary::default();
    for line in fs::read_to_string(events_path)?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        match value["payload"]["event"].as_str() {
            Some("model_request_started") => {
                let step = value["payload"]["step"].as_u64();
                let prompt_estimate = value["payload"]["prompt_token_estimate"].as_u64();
                let raw_prompt_estimate = value["payload"]["raw_prompt_token_estimate"]
                    .as_u64()
                    .or(prompt_estimate);
                let compacted_prompt_estimate =
                    value["payload"]["compacted_prompt_token_estimate"].as_u64();
                if step == Some(1) && summary.first_request_prompt_token_estimate.is_none() {
                    summary.first_request_prompt_token_estimate = prompt_estimate;
                }
                if step == Some(1) && summary.first_request_raw_prompt_token_estimate.is_none() {
                    summary.first_request_raw_prompt_token_estimate = raw_prompt_estimate;
                }
                if step == Some(1)
                    && summary
                        .first_request_compacted_prompt_token_estimate
                        .is_none()
                {
                    summary.first_request_compacted_prompt_token_estimate =
                        compacted_prompt_estimate;
                }
                if let Some(step_number) = step.and_then(|value| usize::try_from(value).ok()) {
                    summary
                        .prompt_token_series_by_turn
                        .push(PromptTokenTurnSample {
                            step: step_number,
                            prompt_token_estimate: prompt_estimate.unwrap_or(0),
                            raw_prompt_token_estimate: raw_prompt_estimate,
                            compacted_prompt_token_estimate: compacted_prompt_estimate,
                            completion_token_cap: value["payload"]["completion_token_cap"]
                                .as_u64()
                                .map(|value| value as u32),
                        });
                }
                summary.max_prompt_token_estimate =
                    summary.max_prompt_token_estimate.max(prompt_estimate);
                summary.max_completion_token_cap = summary.max_completion_token_cap.max(
                    value["payload"]["completion_token_cap"]
                        .as_u64()
                        .map(|value| value as u32),
                );
            }
            Some("model_request_finished") => {
                let step = value["payload"]["step"].as_u64();
                let first_token_latency_ms =
                    value["payload"]["watchdog"]["first_token_latency_ms"].as_u64();
                if step == Some(1) && summary.first_request_first_token_latency_ms.is_none() {
                    summary.first_request_first_token_latency_ms = first_token_latency_ms;
                }
                if step == Some(1) {
                    summary.first_model_turn_started = true;
                }
                summary.watchdog_near_limit |= value["payload"]["watchdog"]["near_limit"]
                    .as_bool()
                    .unwrap_or(false);
                summary.watchdog_triggered |= value["payload"]["watchdog"]["triggered_reason"]
                    .as_str()
                    .is_some();
            }
            Some("tool_call_started") => {
                if value["payload"]["step"].as_u64() == Some(1) {
                    summary.first_action_emitted = true;
                }
            }
            Some("assistant_turn_summary") => {
                if value["payload"]["step"].as_u64() == Some(1)
                    && value["payload"]["actions"]
                        .as_array()
                        .is_some_and(|actions| !actions.is_empty())
                {
                    summary.first_action_emitted = true;
                }
            }
            _ => {}
        }
    }
    Ok(summary)
}

fn extract_read_range_observations(
    checkpoint_path: &Path,
) -> anyhow::Result<Vec<ReadRangeObservation>> {
    if !checkpoint_path.exists() {
        return Ok(Vec::new());
    }
    let checkpoint: quorp_agent_core::AgentCheckpoint =
        serde_json::from_str(&fs::read_to_string(checkpoint_path)?)
            .with_context(|| format!("failed to parse {}", checkpoint_path.display()))?;
    let mut observations = Vec::new();
    for message in checkpoint.transcript {
        if message.role != quorp_agent_core::TranscriptRole::User {
            continue;
        }
        let text = message.content;
        if !text.starts_with("[Tool Output]") || !text.contains("action: read_file") {
            continue;
        }
        let mut path = None;
        let mut requested_range = None;
        let mut honored_range = None;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("path:") {
                path = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("requested_range:") {
                requested_range = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("honored_range:") {
                honored_range = Some(
                    value
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        if let Some(path) = path {
            observations.push(ReadRangeObservation {
                path,
                requested_range,
                honored_range,
            });
        }
    }
    Ok(observations)
}

fn extract_action_evidence(
    checkpoint_path: &Path,
    capsule: Option<&ChallengeCapsule>,
    evaluate_command: Option<&str>,
) -> anyhow::Result<ActionEvidence> {
    if !checkpoint_path.exists() {
        return Ok(ActionEvidence::default());
    }
    let checkpoint: quorp_agent_core::AgentCheckpoint =
        serde_json::from_str(&fs::read_to_string(checkpoint_path)?)
            .with_context(|| format!("failed to parse {}", checkpoint_path.display()))?;
    let fast_loop_commands = capsule
        .map(|capsule| capsule.fast_loop_commands.as_slice())
        .unwrap_or(&[]);
    let mut evidence = ActionEvidence::default();
    for message in checkpoint.transcript {
        if message.role != quorp_agent_core::TranscriptRole::User {
            continue;
        }
        let text = message.content;
        if !text.starts_with("[Tool Output]") {
            continue;
        }
        let Some(action) = extract_tool_output_action(&text) else {
            continue;
        };
        if is_read_action(&action) {
            evidence.read_count = evidence.read_count.saturating_add(1);
        }
        if is_write_action(&action) {
            evidence.write_count = evidence.write_count.saturating_add(1);
        }
        if is_command_action(&action) {
            evidence.command_execution_count = evidence.command_execution_count.saturating_add(1);
            evidence.fast_loop_command_seen |= fast_loop_commands
                .iter()
                .any(|command| command_matches(&action, command));
            evidence.final_evaluate_command_seen |= evaluate_command
                .is_some_and(|command| command_matches(&action, command))
                || command_matches(&action, "./evaluate.sh proof-full")
                || command_matches(&action, "bash ./evaluate.sh proof-full")
                || command_matches(&action, "sh ./evaluate.sh proof-full");
        }
    }
    Ok(evidence)
}

fn extract_tool_output_action(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("action:").map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn is_read_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("read_file")
        || normalized.starts_with("list_directory")
        || normalized.starts_with("search_text")
        || normalized.starts_with("search_symbols")
        || normalized.starts_with("get_repo_capsule")
}

fn is_write_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("write_file")
        || normalized.starts_with("apply_patch")
        || normalized.starts_with("replace_block")
        || normalized.starts_with("modify_toml")
        || normalized.starts_with("set_executable")
        || normalized.starts_with("apply_preview")
}

fn is_command_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("run:")
        || normalized.starts_with("run ")
        || normalized.starts_with("run_validation")
        || normalized.contains("cargo test")
        || normalized.contains("./evaluate.sh")
}

fn command_matches(actual: &str, expected: &str) -> bool {
    let actual = normalize_command_for_match(actual);
    let expected = normalize_command_for_match(expected);
    !expected.is_empty() && actual.contains(&expected)
}

fn normalize_command_for_match(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches("action:")
        .trim_start_matches("run:")
        .trim()
        .to_ascii_lowercase()
}

fn extract_control_loop_summary(events_path: &Path) -> anyhow::Result<ControlLoopSummary> {
    if !events_path.exists() {
        return Ok(ControlLoopSummary::default());
    }
    let mut summary = ControlLoopSummary::default();
    for line in fs::read_to_string(events_path)?.lines() {
        let value: serde_json::Value = serde_json::from_str(line)?;
        match value["payload"]["event"].as_str() {
            Some("agent.path_resolution_failed") => {
                summary.path_resolution_failures += 1;
            }
            Some("agent.recovery_turn_queued") | Some("agent.parse_recovery_queued") => {
                summary.recovery_turns += 1;
            }
            _ => {}
        }
    }
    Ok(summary)
}

fn detect_widening(changed_files: &[String]) -> bool {
    let roots = changed_files
        .iter()
        .filter_map(|path| {
            let mut parts = path.split('/');
            let first = parts.next()?;
            let second = parts.next();
            Some(match (first, second) {
                ("crates", Some(name)) => format!("crates/{name}"),
                (first, _) => first.to_string(),
            })
        })
        .collect::<BTreeSet<_>>();
    roots.len() > 1
}

fn render_report_markdown(report: &BenchmarkReport) -> String {
    let mut lines = vec![
        format!("# Benchmark Report: {}", report.benchmark_name),
        format!("- Issue: `{}`", report.issue_id),
        format!("- Executor: `{}`", report.executor.label()),
        format!("- Model: `{}`", report.model_id),
        format!("- Safety mode: `{}`", report.safety_mode_label),
        format!(
            "- Scenario label: `{}`",
            report
                .scenario_label
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Routing mode: `{}`",
            report
                .routing_mode
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Requested provider: `{}`",
            report
                .requested_provider
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Requested model: `{}`",
            report
                .requested_model
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Effective provider: `{}`",
            report
                .effective_provider
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Effective model: `{}`",
            report
                .effective_model
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("- Used local fallback: `{}`", report.used_local_fallback),
        format!(
            "- Comparable run: `{}`",
            report
                .comparable_run
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Provider request id: `{}`",
            report
                .provider_request_id
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Routing status: `{}`",
            report
                .routing_status
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Repo capsule injected: `{}`",
            report.repo_capsule_injected
        ),
        format!("- Reasoning enabled: `{}`", report.reasoning_enabled),
        format!(
            "- Path resolution failures: `{}`",
            report.path_resolution_failures
        ),
        format!("- Recovery turns: `{}`", report.recovery_turns),
        format!("- Action contract: `{}`", report.action_contract_mode),
        format!(
            "- Action contract selected: `{}`",
            report.action_contract_selected
        ),
        format!(
            "- Action contract fallback reason: `{}`",
            report
                .action_contract_fallback_reason
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Attempt lineage: `{}`",
            if report.attempt_lineage.is_empty() {
                "n/a".to_string()
            } else {
                report.attempt_lineage.join(" -> ")
            }
        ),
        format!(
            "- Preview edits: `{}` / `{}` successful",
            report.preview_edit_success_count, report.preview_edit_count
        ),
        format!(
            "- Intent edits: replace_range=`{}` (hash_mismatch=`{}`), modify_toml=`{}`, previews_created=`{}`, apply_preview=`{}` (hash_mismatch=`{}`)",
            report.replace_range_count,
            report.replace_range_hash_mismatch_count,
            report.modify_toml_count,
            report.preview_created_count,
            report.apply_preview_count,
            report.apply_preview_hash_mismatch_count
        ),
        format!(
            "- Effective prompt compaction: `{}`",
            report
                .effective_prompt_compaction_policy
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "- Fast-loop validation status: `{}`",
            report
                .fast_loop_validation_status
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("- Success: `{}`", report.success),
        format!(
            "- Attempts run: `{}` / `{}`",
            report.attempts_run, report.max_attempts
        ),
        format!("- Total requests: `{}`", report.total_requests),
        format!("- Wall clock ms: `{}`", report.wall_clock_ms),
        format!("- Total billed tokens: `{}`", report.total_billed_tokens),
        format!(
            "- Input tokens (provider billed): `{}`",
            report.prompt_tokens
        ),
        format!("- Completion tokens: `{}`", report.completion_tokens),
        format!("- Reasoning tokens: `{}`", report.reasoning_tokens),
        format!(
            "- Cache read input tokens: `{}`",
            report.cache_read_input_tokens
        ),
        format!(
            "- Cache write input tokens: `{}`",
            report.cache_write_input_tokens
        ),
        format!(
            "- Max prompt estimate seen: `{}`",
            report
                .max_prompt_token_estimate_seen
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Max completion cap seen: `{}`",
            report
                .max_completion_token_cap_seen
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First request prompt estimate: `{}`",
            report
                .first_request_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First request raw prompt estimate: `{}`",
            report
                .first_request_raw_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First request compacted prompt estimate: `{}`",
            report
                .first_request_compacted_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First request first-token ms: `{}`",
            report
                .first_request_first_token_latency_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First model turn started: `{}`",
            report.first_model_turn_started
        ),
        format!(
            "- Bootstrap phase: `{}`",
            report
                .bootstrap_phase
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Bootstrap phase detail: `{}`",
            report
                .bootstrap_phase_detail
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- First task model request seen: `{}`",
            report.first_task_model_request_seen
        ),
        format!(
            "- Bootstrap elapsed ms before first task request: `{}`",
            report
                .bootstrap_elapsed_ms_before_first_task_request
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "- Pre-model bootstrap stalled: `{}`",
            report.pre_model_bootstrap_stalled
        ),
        format!(
            "- Bootstrap stall class: `{}`",
            report
                .bootstrap_stall_class
                .clone()
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("- First action emitted: `{}`", report.first_action_emitted),
        format!(
            "- Task model call count: `{}`",
            report.task_model_call_count
        ),
        format!("- Tool call count: `{}`", report.tool_call_count),
        format!("- Edit count: `{}`", report.edit_count),
        format!("- Read count: `{}`", report.read_count),
        format!("- Write count: `{}`", report.write_count),
        format!(
            "- Rolled-back write count: `{}`",
            report.rolled_back_write_count
        ),
        format!(
            "- Command execution count: `{}`",
            report.command_execution_count
        ),
        format!(
            "- Non-support edit count: `{}`",
            report.non_support_edit_count
        ),
        format!(
            "- Rolled-back non-support edit count: `{}`",
            report.rolled_back_non_support_edit_count
        ),
        format!(
            "- Fast loop command seen: `{}`",
            report.fast_loop_command_seen
        ),
        format!(
            "- Agent final evaluate command seen: `{}`",
            report.agent_final_evaluate_command_seen
        ),
        format!(
            "- Final evaluate command seen: `{}`",
            report.final_evaluate_command_seen
        ),
        format!(
            "- Evaluation command seen: `{}`",
            report.evaluation_command_seen
        ),
        format!(
            "- Host evaluation commands run: `{}`",
            report.host_evaluation_commands_run
        ),
        format!(
            "- Text-only action failure: `{}`",
            report.text_only_action_failure
        ),
        format!("- Watchdog near limit: `{}`", report.watchdog_near_limit),
        format!("- Watchdog triggered: `{}`", report.watchdog_triggered),
        format!("- Widening happened: `{}`", report.widening_happened),
        format!("- Lines added: `{}`", report.lines_added),
        format!("- Lines removed: `{}`", report.lines_removed),
        format!("- Mistakes corrected: `{}`", report.mistakes_corrected),
        format!(
            "- Validation commands run: `{}`",
            report.validation_commands_run
        ),
        format!(
            "- Evaluation commands run: `{}`",
            report.evaluation_commands_run
        ),
        format!(
            "- Deterministic evaluation passed: `{}`",
            report
                .deterministic_evaluation_passed
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("- Run dir: `{}`", report.run_dir.display()),
        format!(
            "- Sandbox root: `{}`",
            report
                .sandbox_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("- Exit code: `{}`", report.exit_code),
        format!(
            "- Primary failure: `{}`",
            report
                .primary_failure
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "- Setup failure class: `{}`",
            report
                .setup_failure_class
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
        format!(
            "- Last failure class: `{}`",
            report
                .last_failure_class
                .clone()
                .unwrap_or_else(|| "none".to_string())
        ),
    ];
    if let Some(judge) = &report.judge {
        lines.push(format!(
            "- Judge: passed={} model={} summary={}",
            judge.passed, judge.model_id, judge.summary
        ));
        lines.push(format!("- Judge rationale: {}", judge.rationale));
    }
    if let Some(reset_outcome) = &report.reset_outcome {
        lines.push(format!(
            "- Reset outcome: passed={} exit_code={} duration_ms={}",
            reset_outcome.passed, reset_outcome.exit_code, reset_outcome.duration_ms
        ));
    }
    if let Some(run_error) = &report.run_error {
        lines.push(format!("- Run error: `{run_error}`"));
    }
    if !report.candidate_models.is_empty() {
        lines.push(format!(
            "- Candidate models: `{}`",
            report.candidate_models.join(", ")
        ));
    }
    if let Some(fallback_reason) = &report.fallback_reason {
        lines.push(format!("- Fallback reason: `{fallback_reason}`"));
    }
    if let Some(challenge) = &report.challenge {
        lines.push(format!(
            "- Challenge: `{}` condition=`{}` workspace=`{}`",
            challenge.case_root.display(),
            challenge.condition,
            challenge.workspace_dir.display()
        ));
    }
    if !report.prompt_token_series_by_turn.is_empty() {
        let series = report
            .prompt_token_series_by_turn
            .iter()
            .map(|sample| {
                format!(
                    "step{}={} raw={} compacted={} cap={}",
                    sample.step,
                    sample.prompt_token_estimate,
                    sample
                        .raw_prompt_token_estimate
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    sample
                        .compacted_prompt_token_estimate
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    sample
                        .completion_token_cap
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "n/a".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("- Prompt token series by turn: {series}"));
    }
    if !report.read_range_observations.is_empty() {
        let observations = report
            .read_range_observations
            .iter()
            .map(|observation| {
                format!(
                    "{} requested={} honored={}",
                    observation.path,
                    observation
                        .requested_range
                        .clone()
                        .unwrap_or_else(|| "none".to_string()),
                    observation
                        .honored_range
                        .clone()
                        .unwrap_or_else(|| "none".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("- Read range observations: {observations}"));
    }
    if !report.failing_test_names.is_empty() {
        lines.push(format!(
            "- Failing tests: {}",
            report.failing_test_names.join(", ")
        ));
    }
    if let Some(test_name) = report.primary_failure_test_name.as_ref() {
        lines.push(format!("- Primary failure test: `{test_name}`"));
    }
    if let Some(path) = report.primary_failure_path.as_ref() {
        let line = report
            .primary_failure_line
            .map(|value| format!(":{value}"))
            .unwrap_or_default();
        lines.push(format!("- Primary failure location: `{path}{line}`"));
    }
    if let Some(assertion_excerpt) = report.assertion_excerpt.as_ref() {
        lines.push(format!(
            "- Assertion excerpt: `{}`",
            truncate_report_text(assertion_excerpt, 180)
        ));
    }
    lines.push(format!("- Repair required: `{}`", report.repair_required));
    if let Some(phase) = report.repair_phase_terminal.as_ref() {
        lines.push(format!("- Repair phase terminal: `{phase}`"));
    }
    if let Some(diagnostic_class) = report.diagnostic_class.as_ref() {
        lines.push(format!("- Diagnostic class: `{diagnostic_class}`"));
    }
    if let Some(target_lease) = report.implementation_target_lease.as_ref() {
        lines.push(format!("- Implementation target lease: `{target_lease}`"));
    }
    if let Some(target_dependency_table) = report.target_dependency_table.as_ref() {
        lines.push(format!(
            "- Target dependency table: `[{target_dependency_table}]`"
        ));
    }
    if !report.dependency_candidates.is_empty() {
        lines.push(format!(
            "- Dependency candidates: `{}`",
            report.dependency_candidates.join(", ")
        ));
    }
    lines.push(format!(
        "- Failure-anchor reread: attempted=`{}` honored=`{}`",
        report.failure_anchor_reread_attempted, report.failure_anchor_reread_honored
    ));
    lines.push(format!(
        "- Implementation reread: allowed=`{}` attempted=`{}` honored=`{}`",
        report.implementation_reread_allowed,
        report.implementation_reread_attempted,
        report.implementation_reread_honored
    ));
    lines.push(format!(
        "- Patch packet injected: `{}`",
        report.patch_packet_injected
    ));
    if let Some(range) = report.patch_packet_honored_range.as_ref() {
        lines.push(format!("- Patch packet honored range: `{range}`"));
    }
    if let Some(command) = report.recommended_rerun_command.as_ref() {
        lines.push(format!(
            "- Recommended rerun command: `{}`",
            truncate_report_text(command, 220)
        ));
    }
    if let Some(match_kind) = report.fast_loop_rerun_match_kind.as_ref() {
        lines.push(format!("- Fast-loop rerun match kind: `{match_kind}`"));
    }
    if !report.failed_edit_records.is_empty() {
        lines.push(format!(
            "- Failed edit memory: `{}`",
            render_failed_edit_records_for_report(&report.failed_edit_records)
        ));
    }
    lines.push(format!(
        "- Local-agent scorecard: parser_recovery=`{}` line_tools=`{}` controller_reads=`{}` redundant_reads=`{}` first_write=`{}` repeated_edits=`{}` validation_rejects=`{}` test_edit_rejects=`{}` target_redirects=`{}` evidence_fixations=`{}` anchors=`{}` syntax_previews=`{}`/`{}` classification=`{}`",
        report.local_agent_scorecard.parser_recovery_count,
        report.local_agent_scorecard.line_oriented_parse_count,
        report.local_agent_scorecard.controller_injected_read_count,
        report.local_agent_scorecard.redundant_read_count,
        report
            .local_agent_scorecard
            .first_valid_write_step
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        report.local_agent_scorecard.repeated_failed_edit_count,
        report.local_agent_scorecard.rejected_validation_alias_count,
        report.local_agent_scorecard.test_edit_rejection_count,
        report.local_agent_scorecard.target_redirect_count,
        report.local_agent_scorecard.evidence_file_fixation_count,
        report.local_agent_scorecard.anchor_suggestion_count,
        report.local_agent_scorecard.syntax_preview_failure_count,
        report.local_agent_scorecard.syntax_preview_count,
        report
            .local_agent_final_failure_classification
            .clone()
            .unwrap_or_else(|| "n/a".to_string())
    ));
    lines.push(format!(
        "- Repair submode: entered=`{}` turns=`{}` invalid_streak_max=`{}` write_locked=`{}` write_refusals=`{}` scaffold_offered=`{}` scaffold_honored=`{}` write_emitted=`{}` soft_budget_inefficient=`{}`",
        report.repair_submode_entered,
        report.repair_submode_turns,
        report.repair_invalid_action_streak_max,
        report.repair_write_locked,
        report.write_phase_action_refusal_count,
        report.patch_scaffold_offered,
        report.patch_scaffold_honored,
        report.write_phase_write_emitted,
        report.soft_budget_inefficient
    ));
    lines.push(format!(
        "- Repair-phase invalid action count: `{}`",
        report.repair_phase_invalid_action_count
    ));
    lines.push(format!(
        "- Post-fast-loop patch attempted: `{}`",
        report.post_fast_loop_patch_attempted
    ));
    lines.push(format!(
        "- Post-fast-loop validation rerun attempted: `{}`",
        report.post_fast_loop_validation_rerun_attempted
    ));
    lines.push(String::new());
    lines.push("## Attempts".to_string());
    for attempt in &report.attempts {
        lines.push(format!(
            "- Attempt {}: executor={}, stop={:?}, tokens={}, requests={}, prompt_est={}, max_tokens={}, visible={}, collector={}, evaluation={}, judge={}",
            attempt.attempt,
            attempt.executor.label(),
            attempt.agent_stop_reason,
            attempt.total_billed_tokens,
            attempt.model_requests,
            attempt
                .max_prompt_token_estimate
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            attempt
                .max_completion_token_cap
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            attempt
                .visible_evaluation
                .as_ref()
                .map(|outcome| outcome.passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            attempt
                .collector_evaluation
                .as_ref()
                .map(|outcome| outcome.passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            attempt
                .evaluation
                .as_ref()
                .map(|outcome| outcome.passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            attempt
                .judge
                .as_ref()
                .map(|judge| judge.passed.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
        ));
        lines.push(format!(
            "  - Tokens: input={} output={} reasoning={} cache_read={} cache_write={}",
            attempt.input_tokens,
            attempt.output_tokens,
            attempt.reasoning_tokens,
            attempt.cache_read_input_tokens,
            attempt.cache_write_input_tokens
        ));
        if !attempt.changed_files.is_empty() {
            lines.push(format!(
                "  - Files changed: {}",
                attempt.changed_files.join(", ")
            ));
        }
        if !attempt.ignored_changed_files.is_empty() {
            lines.push(format!(
                "  - Ignored support-file changes: {}",
                attempt.ignored_changed_files.join(", ")
            ));
        }
        if !attempt.validations.is_empty() {
            lines.push(format!(
                "  - Validations: {}",
                attempt.validations.join(" | ")
            ));
        }
        if let Some(status) = attempt.fast_loop_validation_status.as_ref() {
            lines.push(format!("  - Fast-loop validation status: {status}"));
        }
        if let Some(failure) = attempt.last_validation_failure.as_ref() {
            lines.push(format!("  - Last validation failure: {failure}"));
        }
        if !attempt.failing_test_names.is_empty() {
            lines.push(format!(
                "  - Failing tests: {}",
                attempt.failing_test_names.join(", ")
            ));
        }
        if let Some(test_name) = attempt.primary_failure_test_name.as_ref() {
            lines.push(format!("  - Primary failure test: {test_name}"));
        }
        if let Some(path) = attempt.primary_failure_path.as_ref() {
            let line = attempt
                .primary_failure_line
                .map(|value| format!(":{value}"))
                .unwrap_or_default();
            lines.push(format!("  - Primary failure location: {path}{line}"));
        }
        if let Some(assertion_excerpt) = attempt.assertion_excerpt.as_ref() {
            lines.push(format!(
                "  - Assertion excerpt: {}",
                truncate_report_text(assertion_excerpt, 180)
            ));
        }
        lines.push(format!("  - Repair required: {}", attempt.repair_required));
        if let Some(phase) = attempt.repair_phase_terminal.as_ref() {
            lines.push(format!("  - Repair phase terminal: {phase}"));
        }
        lines.push(format!(
            "  - Failure-anchor reread: attempted={} honored={}",
            attempt.failure_anchor_reread_attempted, attempt.failure_anchor_reread_honored
        ));
        lines.push(format!(
            "  - Implementation reread: allowed={} attempted={} honored={}",
            attempt.implementation_reread_allowed,
            attempt.implementation_reread_attempted,
            attempt.implementation_reread_honored
        ));
        lines.push(format!(
            "  - Patch packet injected: {}",
            attempt.patch_packet_injected
        ));
        if let Some(range) = attempt.patch_packet_honored_range.as_ref() {
            lines.push(format!("  - Patch packet honored range: {range}"));
        }
        if let Some(command) = attempt.recommended_rerun_command.as_ref() {
            lines.push(format!(
                "  - Recommended rerun command: {}",
                truncate_report_text(command, 220)
            ));
        }
        if let Some(match_kind) = attempt.fast_loop_rerun_match_kind.as_ref() {
            lines.push(format!("  - Fast-loop rerun match kind: {match_kind}"));
        }
        if let Some(diagnostic_class) = attempt.diagnostic_class.as_ref() {
            lines.push(format!("  - Diagnostic class: {diagnostic_class}"));
        }
        if let Some(target_lease) = attempt.implementation_target_lease.as_ref() {
            lines.push(format!("  - Implementation target lease: {target_lease}"));
        }
        if let Some(target_dependency_table) = attempt.target_dependency_table.as_ref() {
            lines.push(format!(
                "  - Target dependency table: [{target_dependency_table}]"
            ));
        }
        if !attempt.dependency_candidates.is_empty() {
            lines.push(format!(
                "  - Dependency candidates: {}",
                attempt.dependency_candidates.join(", ")
            ));
        }
        if !attempt.failed_edit_records.is_empty() {
            lines.push(format!(
                "  - Failed edit memory: {}",
                render_failed_edit_records_for_report(&attempt.failed_edit_records)
            ));
        }
        lines.push(format!(
            "  - Local-agent scorecard: parser_recovery={} line_tools={} controller_reads={} redundant_reads={} first_write={} repeated_edits={} validation_rejects={} test_edit_rejects={} target_redirects={} evidence_fixations={} anchors={} syntax_previews={}/{}",
            attempt.local_agent_scorecard.parser_recovery_count,
            attempt.local_agent_scorecard.line_oriented_parse_count,
            attempt.local_agent_scorecard.controller_injected_read_count,
            attempt.local_agent_scorecard.redundant_read_count,
            attempt
                .local_agent_scorecard
                .first_valid_write_step
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            attempt.local_agent_scorecard.repeated_failed_edit_count,
            attempt.local_agent_scorecard.rejected_validation_alias_count,
            attempt.local_agent_scorecard.test_edit_rejection_count,
            attempt.local_agent_scorecard.target_redirect_count,
            attempt.local_agent_scorecard.evidence_file_fixation_count,
            attempt.local_agent_scorecard.anchor_suggestion_count,
            attempt.local_agent_scorecard.syntax_preview_failure_count,
            attempt.local_agent_scorecard.syntax_preview_count
        ));
        lines.push(format!(
            "  - Repair submode: entered={} turns={} invalid_streak_max={} write_locked={} write_refusals={} scaffold_offered={} scaffold_honored={} write_emitted={} rolled_back_writes={} rolled_back_non_support={} soft_budget_inefficient={}",
            attempt.repair_submode_entered,
            attempt.repair_submode_turns,
            attempt.repair_invalid_action_streak_max,
            attempt.repair_write_locked,
            attempt.write_phase_action_refusal_count,
            attempt.patch_scaffold_offered,
            attempt.patch_scaffold_honored,
            attempt.write_phase_write_emitted,
            attempt.rolled_back_write_count,
            attempt.rolled_back_non_support_edit_count,
            attempt.soft_budget_inefficient
        ));
        lines.push(format!(
            "  - Repair-phase invalid action count: {}",
            attempt.repair_phase_invalid_action_count
        ));
        lines.push(format!(
            "  - Post-fast-loop patch attempted: {}",
            attempt.post_fast_loop_patch_attempted
        ));
        lines.push(format!(
            "  - Post-fast-loop validation rerun attempted: {}",
            attempt.post_fast_loop_validation_rerun_attempted
        ));
        if !attempt.prompt_token_series_by_turn.is_empty() {
            let series = attempt
                .prompt_token_series_by_turn
                .iter()
                .map(|sample| format!("step{}={}", sample.step, sample.prompt_token_estimate))
                .collect::<Vec<_>>()
                .join(" | ");
            lines.push(format!("  - Prompt token series: {series}"));
        }
        if !attempt.read_range_observations.is_empty() {
            let observations = attempt
                .read_range_observations
                .iter()
                .map(|observation| {
                    format!(
                        "{} [{} -> {}]",
                        observation.path,
                        observation
                            .requested_range
                            .clone()
                            .unwrap_or_else(|| "none".to_string()),
                        observation
                            .honored_range
                            .clone()
                            .unwrap_or_else(|| "none".to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            lines.push(format!("  - Read ranges: {observations}"));
        }
        lines.push(format!(
            "  - Safety: {} watchdog_near_limit={} watchdog_triggered={}",
            attempt.safety_mode_label, attempt.watchdog_near_limit, attempt.watchdog_triggered
        ));
    }
    lines.join("\n")
}

fn ensure_safe_local_model_selection(
    model_id: &str,
    allow_heavy_local_model: bool,
) -> anyhow::Result<()> {
    if is_heavy_local_model_id(model_id) && !allow_heavy_local_model {
        anyhow::bail!(
            "benchmark run refused heavy local model `{}` without --allow-heavy-local-model",
            model_id
        );
    }
    Ok(())
}

fn resolve_benchmark_model_id(
    executor: BenchmarkExecutor,
    requested_model: Option<String>,
) -> anyhow::Result<String> {
    if let Some(model_id) = requested_model {
        return Ok(model_id);
    }
    match executor {
        BenchmarkExecutor::Native => {
            crate::quorp::tui::model_registry::refresh_catalog_cache_from_broker();
            let model_id = safe_benchmark_model_id()?;
            let resolved_provider = crate::quorp::tui::model_registry::chat_model_provider(
                &model_id,
                crate::quorp::executor::InteractiveProviderKind::Local,
            );
            if !matches!(
                resolved_provider,
                crate::quorp::executor::InteractiveProviderKind::Local
            ) {
                anyhow::bail!(
                    "Quorp benchmark/native runs are local-only in this build; resolved provider `{}` for model `{}`",
                    resolved_provider.label(),
                    model_id
                );
            }
            Ok(model_id)
        }
        BenchmarkExecutor::Codex => Ok(default_codex_model_id()),
    }
}

fn base_url_override_for_executor(
    executor: BenchmarkExecutor,
    base_url_override: Option<String>,
) -> Option<String> {
    match executor {
        BenchmarkExecutor::Native => base_url_override,
        BenchmarkExecutor::Codex => None,
    }
}

fn benchmark_provider_summary(
    executor: BenchmarkExecutor,
    model_id: &str,
    base_url_override: Option<&str>,
) -> BenchmarkProviderSummary {
    if executor == BenchmarkExecutor::Codex {
        return BenchmarkProviderSummary {
            provider_kind: "codex".to_string(),
            provider_base_url: None,
            auth_mode: "codex_managed".to_string(),
            usage_source: "codex_output".to_string(),
            proxy_visible_remote_egress_expected: false,
        };
    }

    let provider = crate::quorp::tui::model_registry::chat_model_provider(
        model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    );
    match provider {
        crate::quorp::executor::InteractiveProviderKind::Local => BenchmarkProviderSummary {
            provider_kind: provider.label().to_string(),
            provider_base_url: base_url_override
                .map(str::to_string)
                .or_else(crate::quorp::provider_config::resolved_local_base_url_env),
            auth_mode: "local_bearer".to_string(),
            usage_source: "provider_response".to_string(),
            proxy_visible_remote_egress_expected: base_url_override
                .map(str::to_string)
                .or_else(crate::quorp::provider_config::resolved_local_base_url_env)
                .is_some_and(|base_url| {
                    !crate::quorp::provider_config::is_loopback_base_url(&base_url)
                }),
        },
        crate::quorp::executor::InteractiveProviderKind::Ollama => {
            let normalized = base_url_override.and_then(|value| {
                crate::quorp::provider_config::normalize_remote_base_url(value, true).ok()
            });
            let proxy_visible = normalized.as_deref().is_some_and(|base_url| {
                !crate::quorp::provider_config::is_loopback_base_url(base_url)
            });
            BenchmarkProviderSummary {
                provider_kind: provider.label().to_string(),
                provider_base_url: normalized,
                auth_mode: "none".to_string(),
                usage_source: "provider_response".to_string(),
                proxy_visible_remote_egress_expected: proxy_visible,
            }
        }
        crate::quorp::executor::InteractiveProviderKind::OpenAiCompatible => {
            match crate::quorp::provider_config::resolve_openai_compatible_runtime(
                base_url_override,
            ) {
                Ok(config) => BenchmarkProviderSummary {
                    provider_kind: provider.label().to_string(),
                    provider_base_url: Some(config.base_url),
                    auth_mode: config.auth_mode,
                    usage_source: "provider_response".to_string(),
                    proxy_visible_remote_egress_expected: config
                        .proxy_visible_remote_egress_expected,
                },
                Err(_) => BenchmarkProviderSummary {
                    provider_kind: provider.label().to_string(),
                    provider_base_url: base_url_override.map(str::to_string),
                    auth_mode: "missing".to_string(),
                    usage_source: "provider_response".to_string(),
                    proxy_visible_remote_egress_expected: base_url_override.is_some_and(
                        |base_url| !crate::quorp::provider_config::is_loopback_base_url(base_url),
                    ),
                },
            }
        }
        crate::quorp::executor::InteractiveProviderKind::Nvidia => {
            match crate::quorp::provider_config::resolve_nvidia_runtime(base_url_override) {
                Ok(config) => BenchmarkProviderSummary {
                    provider_kind: provider.label().to_string(),
                    provider_base_url: Some(config.base_url),
                    auth_mode: config.auth_mode,
                    usage_source: "provider_response".to_string(),
                    proxy_visible_remote_egress_expected: config
                        .proxy_visible_remote_egress_expected,
                },
                Err(_) => BenchmarkProviderSummary {
                    provider_kind: provider.label().to_string(),
                    provider_base_url: base_url_override.map(str::to_string),
                    auth_mode: "missing".to_string(),
                    usage_source: "provider_response".to_string(),
                    proxy_visible_remote_egress_expected: base_url_override.is_some_and(
                        |base_url| !crate::quorp::provider_config::is_loopback_base_url(base_url),
                    ),
                },
            }
        }
        crate::quorp::executor::InteractiveProviderKind::Codex => BenchmarkProviderSummary {
            provider_kind: provider.label().to_string(),
            provider_base_url: None,
            auth_mode: "codex_managed".to_string(),
            usage_source: "codex_output".to_string(),
            proxy_visible_remote_egress_expected: false,
        },
    }
}

fn benchmark_safety_mode_label(executor: BenchmarkExecutor, model_id: &str) -> String {
    match executor {
        BenchmarkExecutor::Codex => "codex".to_string(),
        BenchmarkExecutor::Native if is_nvidia_kimi_model_id(model_id) => {
            "nvidia_kimi_benchmark".to_string()
        }
        BenchmarkExecutor::Native if is_nvidia_qwen_coder_model_id(model_id) => {
            "nvidia_qwen_benchmark".to_string()
        }
        BenchmarkExecutor::Native if is_heavy_local_model_id(model_id) => "heavy_local".to_string(),
        BenchmarkExecutor::Native => "safe_local".to_string(),
    }
}

fn is_nvidia_kimi_model_id(model_id: &str) -> bool {
    let normalized = model_id.to_ascii_lowercase();
    normalized == "nvidia/moonshotai/kimi-k2.5"
        || normalized == "moonshotai/kimi-k2.5"
        || normalized.starts_with("nvidia/moonshotai/kimi-k2.")
}

fn is_nvidia_qwen_coder_model_id(model_id: &str) -> bool {
    let normalized = model_id.to_ascii_lowercase();
    normalized == "nvidia/qwen/qwen3-coder-480b-a35b-instruct"
        || normalized == "qwen/qwen3-coder-480b-a35b-instruct"
}

fn is_heavy_local_model_id(model_id: &str) -> bool {
    crate::quorp::tui::local_model_program::local_model_program(model_id).is_some()
}

fn benchmark_completion_policy(
    executor: BenchmarkExecutor,
    safety_mode_label: &str,
    model_id: Option<&str>,
) -> quorp_agent_core::CompletionPolicy {
    let mut completion_policy = if executor == BenchmarkExecutor::Codex {
        quorp_agent_core::CompletionPolicy {
            include_repo_capsule: false,
            first_turn_max_completion_tokens: None,
            later_turn_max_completion_tokens: None,
            disable_reasoning: true,
            native_tool_calls: false,
            watchdog: None,
            safety_mode_label: Some("codex".to_string()),
            prompt_compaction_policy: None,
        }
    } else {
        match safety_mode_label {
            "heavy_local" => quorp_agent_core::CompletionPolicy {
                include_repo_capsule: true,
                first_turn_max_completion_tokens: Some(6144),
                later_turn_max_completion_tokens: Some(4096),
                disable_reasoning: false,
                native_tool_calls: true,
                watchdog: Some(quorp_agent_core::CompletionWatchdogConfig {
                    first_token_timeout_ms: Some(180_000),
                    idle_timeout_ms: Some(30_000),
                    total_timeout_ms: Some(420_000),
                }),
                safety_mode_label: Some("heavy_local".to_string()),
                prompt_compaction_policy: Some(PromptCompactionPolicy::CurrentDefault),
            },
            _ => quorp_agent_core::CompletionPolicy {
                include_repo_capsule: true,
                first_turn_max_completion_tokens: Some(1536),
                later_turn_max_completion_tokens: Some(2048),
                disable_reasoning: false,
                native_tool_calls: true,
                watchdog: Some(quorp_agent_core::CompletionWatchdogConfig {
                    first_token_timeout_ms: Some(120_000),
                    idle_timeout_ms: Some(30_000),
                    total_timeout_ms: Some(360_000),
                }),
                safety_mode_label: Some("safe_local".to_string()),
                prompt_compaction_policy: Some(PromptCompactionPolicy::BenchmarkStatePacket),
            },
        }
    };
    apply_model_specific_benchmark_policy_defaults(model_id, &mut completion_policy);
    apply_benchmark_completion_policy_env_overrides(&mut completion_policy);
    completion_policy
}

fn apply_model_specific_benchmark_policy_defaults(
    model_id: Option<&str>,
    completion_policy: &mut quorp_agent_core::CompletionPolicy,
) {
    let Some(model_id) = model_id else {
        return;
    };
    if is_nvidia_kimi_model_id(model_id) {
        completion_policy.include_repo_capsule = true;
        completion_policy.disable_reasoning = true;
        completion_policy.native_tool_calls = false;
        completion_policy.first_turn_max_completion_tokens = Some(768);
        completion_policy.later_turn_max_completion_tokens = Some(1536);
        completion_policy.prompt_compaction_policy =
            Some(PromptCompactionPolicy::BenchmarkStatePacket);
        completion_policy.watchdog = Some(quorp_agent_core::CompletionWatchdogConfig {
            first_token_timeout_ms: Some(120_000),
            idle_timeout_ms: Some(30_000),
            total_timeout_ms: Some(360_000),
        });
        completion_policy.safety_mode_label = Some("nvidia_kimi_benchmark".to_string());
        return;
    }
    if is_nvidia_qwen_coder_model_id(model_id) {
        completion_policy.include_repo_capsule = true;
        completion_policy.disable_reasoning = true;
        completion_policy.native_tool_calls = false;
        completion_policy.first_turn_max_completion_tokens = Some(4096);
        completion_policy.later_turn_max_completion_tokens = Some(3072);
        completion_policy.prompt_compaction_policy =
            Some(PromptCompactionPolicy::BenchmarkStatePacket);
        completion_policy.watchdog = Some(quorp_agent_core::CompletionWatchdogConfig {
            first_token_timeout_ms: Some(120_000),
            idle_timeout_ms: Some(30_000),
            total_timeout_ms: Some(360_000),
        });
        completion_policy.safety_mode_label = Some("nvidia_qwen_benchmark".to_string());
        return;
    }
    let Some(model_spec) =
        crate::quorp::tui::model_registry::local_moe_spec_for_registry_id(model_id)
    else {
        return;
    };
    if model_spec.id.eq_ignore_ascii_case("ssd_moe/qwen35-27b")
        || model_spec.id.eq_ignore_ascii_case("ssd_moe/qwen36-27b")
    {
        completion_policy.disable_reasoning = true;
        completion_policy.native_tool_calls = false;
        completion_policy.prompt_compaction_policy = Some(PromptCompactionPolicy::Last6Ledger768);
    }
    if model_spec
        .id
        .eq_ignore_ascii_case("ssd_moe/qwen3-coder-30b-a3b")
    {
        completion_policy.disable_reasoning = true;
        completion_policy.native_tool_calls = false;
        completion_policy.first_turn_max_completion_tokens = Some(4096);
        completion_policy.later_turn_max_completion_tokens = Some(3072);
        completion_policy.prompt_compaction_policy = Some(PromptCompactionPolicy::Last6Ledger768);
    }
    if model_spec.id.eq_ignore_ascii_case("ssd_moe/qwen36-27b") {
        completion_policy.first_turn_max_completion_tokens = Some(3072);
        completion_policy.later_turn_max_completion_tokens = Some(1536);
    }
}

fn apply_benchmark_completion_policy_env_overrides(
    completion_policy: &mut quorp_agent_core::CompletionPolicy,
) {
    if let Some(value) = env_override_u32("QUORP_BENCH_FIRST_TURN_MAX_COMPLETION_TOKENS") {
        completion_policy.first_turn_max_completion_tokens = Some(value);
    }
    if let Some(value) = env_override_u32("QUORP_BENCH_LATER_TURN_MAX_COMPLETION_TOKENS") {
        completion_policy.later_turn_max_completion_tokens = Some(value);
    }
    if let Some(value) = env_override_bool("QUORP_BENCH_DISABLE_REASONING") {
        completion_policy.disable_reasoning = value;
    }
    if let Some(value) = env_override_bool("QUORP_BENCH_NATIVE_TOOL_CALLS") {
        completion_policy.native_tool_calls = value;
    }
    if let Some(value) =
        env_override_prompt_compaction_policy("QUORP_BENCH_PROMPT_COMPACTION_POLICY")
    {
        completion_policy.prompt_compaction_policy = Some(value);
    }
}

fn env_override_u32(key: &str) -> Option<u32> {
    let raw = env::var(key).ok()?;
    let parsed = raw.trim().parse::<u32>().ok()?;
    Some(parsed)
}

fn env_override_bool(key: &str) -> Option<bool> {
    let raw = env::var(key).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn env_override_prompt_compaction_policy(key: &str) -> Option<PromptCompactionPolicy> {
    let raw = env::var(key).ok()?;
    PromptCompactionPolicy::parse(raw.trim())
}

fn benchmark_action_contract_mode(
    completion_policy: &quorp_agent_core::CompletionPolicy,
) -> &'static str {
    if completion_policy.native_tool_calls {
        "native_tool_calls_v1"
    } else {
        "strict_json_v1"
    }
}

fn benchmark_attempt_lineage(
    completion_policy: &quorp_agent_core::CompletionPolicy,
) -> Vec<String> {
    env::var("QUORP_BENCH_ATTEMPT_LINEAGE")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![benchmark_action_contract_mode(completion_policy).to_string()])
}

fn estimate_token_count(text: &str) -> u64 {
    let char_count = text.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

fn default_safe_mode_label() -> String {
    "safe_local".to_string()
}

fn discover_completed_attempts(result_dir: &Path) -> anyhow::Result<usize> {
    let mut attempts = 0usize;
    if !result_dir.exists() {
        return Ok(0);
    }
    for entry in fs::read_dir(result_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("attempt-"))
        {
            attempts += 1;
        }
    }
    Ok(attempts)
}

fn load_existing_attempts(result_dir: &Path) -> anyhow::Result<Vec<AttemptReport>> {
    let mut attempts = Vec::new();
    if !result_dir.exists() {
        return Ok(attempts);
    }
    for entry in fs::read_dir(result_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let report_path = entry.path().join("attempt-report.json");
        if report_path.exists() {
            attempts.push(serde_json::from_str(&fs::read_to_string(&report_path)?)?);
        }
    }
    attempts.sort_by_key(|attempt| attempt.attempt);
    Ok(attempts)
}

fn looks_like_proof_full_workspace(path: &Path) -> bool {
    path.join("AGENTS.md").exists()
        && path.join("agent-map.json").exists()
        && path.join("test-map.json").exists()
}

fn looks_like_warpos_staged_workspace(path: &Path) -> bool {
    path.join(".benchmark-root.json").exists()
        && path.join("issue.json").exists()
        && path.join("Cargo.toml").exists()
        && path.join("evaluate.sh").exists()
        && (path.join("START_HERE.md").exists() || path.join("README.md").exists())
}

fn looks_like_flat_challenge_workspace(path: &Path) -> bool {
    path.join("benchmark.json").is_file()
        && path.join("evaluate.sh").is_file()
        && (path.join("START_HERE.md").is_file() || path.join("README.md").is_file())
        && (path.join("SUCCESS.md").is_file() || path.join("expected").exists())
}

fn resolve_challenge_workspace_dir(
    sandbox_root: &Path,
    condition: &str,
) -> anyhow::Result<PathBuf> {
    let legacy_workspace = sandbox_root.join("workspace").join(condition);
    if legacy_workspace.exists() {
        return Ok(legacy_workspace);
    }
    if looks_like_flat_challenge_workspace(sandbox_root)
        || looks_like_warpos_staged_workspace(sandbox_root)
    {
        return Ok(sandbox_root.to_path_buf());
    }
    anyhow::bail!(
        "failed to locate challenge workspace for condition `{condition}`; expected `{}` or a flat WarpOS challenge bundle at `{}`",
        legacy_workspace.display(),
        sandbox_root.display()
    )
}

fn maybe_materialize_flat_challenge_reset_script(
    result_dir: &Path,
    sandbox_root: &Path,
) -> anyhow::Result<()> {
    if sandbox_root.join("reset.sh").exists() || !looks_like_flat_challenge_workspace(sandbox_root)
    {
        return Ok(());
    }

    let baseline_root = result_dir.join(".quorp-flat-baseline");
    if baseline_root.exists() {
        fs::remove_dir_all(&baseline_root)
            .with_context(|| format!("failed to clear {}", baseline_root.display()))?;
    }
    let quoted_baseline = shell_single_quote(&baseline_root.display().to_string());
    let reset_script = format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         baseline={quoted_baseline}\n\
         if [[ ! -d \"${{baseline}}\" ]]; then\n\
           echo \"missing flat challenge reset baseline: ${{baseline}}\" >&2\n\
           exit 1\n\
         fi\n\
         find . -mindepth 1 -maxdepth 1 -exec rm -rf -- {{}} +\n\
         cp -a \"${{baseline}}/.\" .\n"
    );
    let reset_path = sandbox_root.join("reset.sh");
    fs::write(&reset_path, reset_script)
        .with_context(|| format!("failed to write {}", reset_path.display()))?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&reset_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&reset_path, permissions)?;
    }
    copy_dir_all(sandbox_root, &baseline_root).with_context(|| {
        format!(
            "failed to snapshot flat challenge baseline {} -> {}",
            sandbox_root.display(),
            baseline_root.display()
        )
    })?;
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn looks_like_issue_dir(path: &Path) -> bool {
    path.join("README.md").exists()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("ISSUE-"))
}

fn find_benchmark_root(path: &Path) -> anyhow::Result<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some("benchmark") {
            return Ok(ancestor.to_path_buf());
        }
    }
    anyhow::bail!(
        "failed to find enclosing `benchmark` directory for {}",
        path.display()
    )
}

fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &destination)?;
            let permissions = fs::metadata(entry.path())?.permissions();
            fs::set_permissions(&destination, permissions)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &destination)?;
        }
    }
    Ok(())
}

fn copy_file_if_different(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if src == dst {
        return Ok(());
    }
    if src.exists()
        && dst.exists()
        && let (Ok(src_canonical), Ok(dst_canonical)) =
            (fs::canonicalize(src), fs::canonicalize(dst))
        && src_canonical == dst_canonical
    {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(src, dst)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

fn ensure_git_baseline(workspace_dir: &Path) -> anyhow::Result<()> {
    if workspace_dir.join(".git").exists() {
        return Ok(());
    }
    #[allow(clippy::disallowed_methods)]
    let init_status = Command::new("git")
        .arg("init")
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !init_status.success() {
        anyhow::bail!("failed to initialize git in {}", workspace_dir.display());
    }
    #[allow(clippy::disallowed_methods)]
    let add_status = Command::new("git")
        .args(["add", "."])
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !add_status.success() {
        anyhow::bail!(
            "failed to stage sandbox baseline in {}",
            workspace_dir.display()
        );
    }
    #[allow(clippy::disallowed_methods)]
    let commit_status = Command::new("git")
        .args([
            "-c",
            "user.name=quorp",
            "-c",
            "user.email=quorp@example.com",
            "commit",
            "-qm",
            "Benchmark baseline",
        ])
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !commit_status.success() {
        anyhow::bail!(
            "failed to commit sandbox baseline in {}",
            workspace_dir.display()
        );
    }
    Ok(())
}

fn parse_autonomy_profile(value: &str) -> anyhow::Result<quorp_agent_core::AutonomyProfile> {
    match value.trim() {
        "interactive" => Ok(quorp_agent_core::AutonomyProfile::Interactive),
        "autonomous_host" => Ok(quorp_agent_core::AutonomyProfile::AutonomousHost),
        "autonomous_sandboxed" => Ok(quorp_agent_core::AutonomyProfile::AutonomousSandboxed),
        other => Err(anyhow::anyhow!("unknown autonomy profile `{other}`")),
    }
}

fn attempt_dir(result_dir: &Path, attempt: usize) -> PathBuf {
    result_dir.join(format!("attempt-{attempt:03}"))
}

fn benchmark_bootstrap_progress_path(result_dir: &Path) -> PathBuf {
    result_dir.join(BENCHMARK_BOOTSTRAP_PROGRESS_FILE)
}

fn attempt_bootstrap_progress_path(attempt_dir: &Path) -> PathBuf {
    attempt_dir.join(BENCHMARK_BOOTSTRAP_PROGRESS_FILE)
}

fn epoch_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn read_bootstrap_progress(path: &Path) -> anyhow::Result<Option<BenchmarkBootstrapProgress>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let progress = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(progress))
}

fn write_bootstrap_progress_files(
    root_progress_path: &Path,
    attempt_progress_path: &Path,
    progress: &BenchmarkBootstrapProgress,
) -> anyhow::Result<()> {
    write_json(root_progress_path, progress)?;
    write_json(attempt_progress_path, progress)?;
    Ok(())
}

impl BenchmarkBootstrapTracker {
    fn new(result_dir: &Path, attempt_dir: &Path, attempt: usize) -> anyhow::Result<Self> {
        let tracker = Self {
            root_progress_path: benchmark_bootstrap_progress_path(result_dir),
            attempt_progress_path: attempt_bootstrap_progress_path(attempt_dir),
            attempt,
            started_at: Instant::now(),
        };
        tracker.update(BOOTSTRAP_PHASE_BENCHMARK_STARTED, None)?;
        Ok(tracker)
    }

    fn update(&self, phase: &str, detail: Option<String>) -> anyhow::Result<()> {
        let mut progress = read_bootstrap_progress(&self.attempt_progress_path)?.unwrap_or(
            BenchmarkBootstrapProgress {
                attempt: self.attempt,
                bootstrap_phase: phase.to_string(),
                bootstrap_phase_detail: None,
                started_at_epoch_ms: epoch_time_ms(),
                updated_at_epoch_ms: epoch_time_ms(),
                first_task_model_request_seen: false,
                bootstrap_elapsed_ms_before_first_task_request: None,
                pre_model_bootstrap_stalled: false,
                bootstrap_stall_class: None,
            },
        );
        progress.attempt = self.attempt;
        progress.bootstrap_phase = phase.to_string();
        progress.bootstrap_phase_detail = detail;
        progress.updated_at_epoch_ms = epoch_time_ms();
        write_bootstrap_progress_files(
            &self.root_progress_path,
            &self.attempt_progress_path,
            &progress,
        )
    }

    fn mark_first_task_model_request(&self) -> anyhow::Result<()> {
        let mut progress = read_bootstrap_progress(&self.attempt_progress_path)?.unwrap_or(
            BenchmarkBootstrapProgress {
                attempt: self.attempt,
                bootstrap_phase: BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST.to_string(),
                bootstrap_phase_detail: None,
                started_at_epoch_ms: epoch_time_ms(),
                updated_at_epoch_ms: epoch_time_ms(),
                first_task_model_request_seen: false,
                bootstrap_elapsed_ms_before_first_task_request: None,
                pre_model_bootstrap_stalled: false,
                bootstrap_stall_class: None,
            },
        );
        if progress.first_task_model_request_seen {
            return Ok(());
        }
        progress.attempt = self.attempt;
        progress.bootstrap_phase = BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST.to_string();
        progress.bootstrap_phase_detail =
            Some("first benchmark task model request started".to_string());
        progress.updated_at_epoch_ms = epoch_time_ms();
        progress.first_task_model_request_seen = true;
        progress.bootstrap_elapsed_ms_before_first_task_request =
            Some(self.started_at.elapsed().as_millis() as u64);
        write_bootstrap_progress_files(
            &self.root_progress_path,
            &self.attempt_progress_path,
            &progress,
        )
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn log_phase(label: &str, color: &str, message: String) {
    eprintln!("{ANSI_BOLD}{color}[{label}]{ANSI_RESET} {message}");
}

impl BenchmarkRunLock {
    fn acquire() -> anyhow::Result<Self> {
        Self::acquire_at(benchmark_run_lock_path()?)
    }

    fn acquire_at(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let metadata = serde_json::json!({
            "pid": std::process::id(),
            "created_at": format!("{:?}", std::time::SystemTime::now()),
        });
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                file.write_all(serde_json::to_string_pretty(&metadata)?.as_bytes())?;
                Ok(Self { path })
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if lock_is_stale(&path) {
                    fs::remove_file(&path)?;
                    return Self::acquire_at(path);
                }
                let detail =
                    fs::read_to_string(&path).unwrap_or_else(|_| "<unreadable lock>".to_string());
                anyhow::bail!(
                    "another headless benchmark run already holds the local benchmark lock at {}: {}",
                    path.display(),
                    detail
                );
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn lock_is_stale(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return true;
    };
    let Some(pid) = value.get("pid").and_then(serde_json::Value::as_u64) else {
        return true;
    };
    let probe = unsafe { libc::kill(pid as i32, 0) };
    if probe == 0 {
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

impl Drop for BenchmarkRunLock {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            log::debug!(
                "failed to remove benchmark lock file {}: {error}",
                self.path.display()
            );
        }
    }
}

fn benchmark_run_lock_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set for benchmark lockfile")?;
    Ok(benchmark_run_lock_path_for_home(Path::new(&home)))
}

fn benchmark_run_lock_path_for_home(home: &Path) -> PathBuf {
    home.join(".config")
        .join("quorp")
        .join("locks")
        .join("benchmark-run.lock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant};

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_env_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_benchmark_completion_policy_env_overrides() {
        unsafe {
            std::env::remove_var("QUORP_BENCH_FIRST_TURN_MAX_COMPLETION_TOKENS");
            std::env::remove_var("QUORP_BENCH_LATER_TURN_MAX_COMPLETION_TOKENS");
            std::env::remove_var("QUORP_BENCH_DISABLE_REASONING");
            std::env::remove_var("QUORP_BENCH_NATIVE_TOOL_CALLS");
            std::env::remove_var("QUORP_BENCH_PROMPT_COMPACTION_POLICY");
        }
    }

    #[test]
    fn detects_proof_full_workspace_path() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        fs::write(temp_dir.path().join("AGENTS.md"), "rules").expect("agents");
        fs::write(temp_dir.path().join("agent-map.json"), "{}").expect("agent-map");
        fs::write(temp_dir.path().join("test-map.json"), "{}").expect("test-map");
        assert!(looks_like_proof_full_workspace(temp_dir.path()));
    }

    #[test]
    fn detects_warpos_staged_workspace_path() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp_dir.path().join(".benchmark-root.json"),
            serde_json::json!({
                "benchmark": "atlas-billing",
                "issue": "ISSUE-00-toy",
                "handoff_root": temp_dir.path().display().to_string(),
            })
            .to_string(),
        )
        .expect("marker");
        fs::write(temp_dir.path().join("issue.json"), "{}").expect("issue");
        fs::write(temp_dir.path().join("Cargo.toml"), "[workspace]\n").expect("cargo");
        fs::write(temp_dir.path().join("evaluate.sh"), "#!/usr/bin/env bash\n").expect("eval");
        fs::write(temp_dir.path().join("START_HERE.md"), "# Objective\n").expect("brief");
        assert!(looks_like_warpos_staged_workspace(temp_dir.path()));
    }

    #[test]
    fn detects_issue_directory_path() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let issue_dir = temp_dir.path().join("ISSUE-00-toy");
        fs::create_dir_all(&issue_dir).expect("mkdir");
        fs::write(issue_dir.join("README.md"), "brief").expect("readme");
        assert!(looks_like_issue_dir(&issue_dir));
    }

    #[test]
    fn resolves_warpos_staged_workspace_from_marker() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let benchmarks_root = temp_dir.path().join("benchmarks");
        let issue_id = "ISSUE-00-toy";
        let handoff_root = benchmarks_root
            .join("handoffs")
            .join("atlas-billing")
            .join(issue_id)
            .join("bare");
        let issue_root = benchmarks_root.join("issues").join(issue_id);
        let hidden_dir = issue_root.join("hidden");

        fs::create_dir_all(&handoff_root).expect("handoff root");
        fs::create_dir_all(&hidden_dir).expect("hidden");
        fs::write(hidden_dir.join("check.sh"), "#!/usr/bin/env bash\n").expect("collector");

        let session_workspace = temp_dir.path().join("session").join("workspace");
        fs::create_dir_all(&session_workspace).expect("session workspace");
        fs::write(
            session_workspace.join(".benchmark-root.json"),
            serde_json::json!({
                "benchmark": "atlas-billing",
                "issue": issue_id,
                "condition": "bare",
                "suite": "psd-prod",
                "handoff_root": handoff_root.display().to_string(),
            })
            .to_string(),
        )
        .expect("marker");
        fs::write(session_workspace.join("issue.json"), "{}").expect("issue");
        fs::write(session_workspace.join("Cargo.toml"), "[workspace]\n").expect("cargo");
        fs::write(
            session_workspace.join("evaluate.sh"),
            "#!/usr/bin/env bash\n",
        )
        .expect("eval");
        fs::write(session_workspace.join("START_HERE.md"), "# Objective\n").expect("brief");
        fs::write(
            session_workspace.join("YOU_ARE_HERE.txt"),
            "owner: billing-domain\n",
        )
        .expect("you are here");

        let resolved = resolve_benchmark(&session_workspace).expect("resolved benchmark");
        assert_eq!(resolved.issue_id, issue_id);
        assert_eq!(resolved.benchmark_name, "atlas-billing");
        assert_eq!(
            resolved.workspace_source,
            fs::canonicalize(&session_workspace).expect("canonical workspace")
        );
        assert_eq!(
            resolved.visible_evaluator,
            Some(
                fs::canonicalize(session_workspace.join("evaluate.sh"))
                    .expect("canonical visible evaluator"),
            )
        );
        assert_eq!(
            resolved.collector_evaluator,
            Some(fs::canonicalize(hidden_dir.join("check.sh")).expect("canonical collector"))
        );
        assert!(
            resolved.context_files.contains(
                &fs::canonicalize(session_workspace.join(".benchmark-root.json"))
                    .expect("canonical benchmark marker")
            )
        );
        assert!(
            resolved.context_files.contains(
                &fs::canonicalize(session_workspace.join("issue.json"))
                    .expect("canonical issue marker")
            )
        );
    }

    #[test]
    fn widening_detection_flags_multiple_roots() {
        assert!(detect_widening(&[
            "crates/a/src/lib.rs".to_string(),
            "crates/b/src/lib.rs".to_string(),
        ]));
        assert!(!detect_widening(&[
            "crates/a/src/lib.rs".to_string(),
            "crates/a/tests/visible.rs".to_string(),
        ]));
    }

    #[test]
    fn parse_prompt_compaction_policy_accepts_known_values() {
        assert_eq!(
            parse_prompt_compaction_policy(Some("current-default")).expect("parse"),
            Some(PromptCompactionPolicy::CurrentDefault)
        );
        assert_eq!(
            parse_prompt_compaction_policy(Some("last6-ledger768")).expect("parse"),
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert_eq!(
            parse_prompt_compaction_policy(Some("benchmark-repair-minimal")).expect("parse"),
            Some(PromptCompactionPolicy::BenchmarkRepairMinimal)
        );
        assert_eq!(
            parse_prompt_compaction_policy(Some("benchmark-state-packet")).expect("parse"),
            Some(PromptCompactionPolicy::BenchmarkStatePacket)
        );
        assert!(
            parse_prompt_compaction_policy(Some("unknown-policy"))
                .expect_err("invalid policy should fail")
                .to_string()
                .contains("unknown compaction policy")
        );
    }

    #[test]
    fn load_seed_context_reads_latest_checkpoint_messages() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let session_path = temp_dir.path().join("session-0001.json");
        fs::write(
            &session_path,
            serde_json::json!({
                "checkpoints": [
                    {
                        "messages": [
                            {"role": "user", "content": "old user"},
                            {"role": "assistant", "content": "old assistant"}
                        ]
                    },
                    {
                        "messages": [
                            {"role": "system", "content": "seed ledger"},
                            {"role": "assistant", "content": "assistant context"},
                            {"role": "user", "content": "active objective context"},
                            {"role": "assistant", "content": "   "}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .expect("write session");

        let messages = load_seed_context(Some(&session_path)).expect("load seed context");
        assert_eq!(
            messages,
            vec![
                TranscriptMessage {
                    role: TranscriptRole::System,
                    content: "seed ledger".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::Assistant,
                    content: "assistant context".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "active objective context".to_string(),
                },
            ]
        );
    }

    fn create_challenge_case_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let case_root = temp_dir.path().join("01-sample-case");
        fs::create_dir_all(case_root.join("expected")).expect("expected");
        fs::create_dir_all(case_root.join("workspace").join("proof-full").join("src"))
            .expect("workspace");
        fs::write(
            case_root.join("START_HERE.md"),
            "# Objective\n\nFix the sample challenge.\n",
        )
        .expect("objective");
        fs::write(
            case_root.join("LOCAL_REPRO.md"),
            "# Local Repro\n\n- `cargo test --quiet`\n",
        )
        .expect("local repro");
        fs::write(
            case_root.join("REFERENCE.md"),
            "# Reference\n\n- sample provenance\n",
        )
        .expect("reference");
        fs::write(
            case_root.join("expected").join("success-criteria.md"),
            "# Success\n\nThe sample challenge passes.\n",
        )
        .expect("success");
        fs::write(
            case_root
                .join("workspace")
                .join("proof-full")
                .join("src")
                .join("lib.rs"),
            "pub fn sample() -> u32 { 1 }\n",
        )
        .expect("workspace file");
        fs::write(
            case_root.join("benchmark.json"),
            serde_json::json!({
                "id": "01-sample-case",
                "title": "Sample challenge",
                "difficulty": "easy",
                "category": "sample",
                "repo_condition": ["bare", "proof-core", "proof-full"],
                "objective_file": "START_HERE.md",
                "success_file": "expected/success-criteria.md",
                "reset_command": "./reset.sh <condition>",
                "evaluate_command": "./evaluate.sh <condition>",
                "estimated_minutes": 1,
                "expected_files_touched": ["src/lib.rs"],
                "primary_metrics": ["total_tokens"],
                "tags": ["rust", "sample"],
            })
            .to_string(),
        )
        .expect("benchmark");
        (temp_dir, case_root)
    }

    fn create_toy_preview_benchmark_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let benchmark_root = temp_dir.path().join("benchmark");
        let issue_dir = benchmark_root
            .join("exhaustive")
            .join("issues")
            .join("ISSUE-00-toy-preview");
        let workspace_dir = benchmark_root
            .join("handoffs")
            .join("proof")
            .join("ISSUE-00-toy-preview")
            .join("proof-full");
        fs::create_dir_all(workspace_dir.join("crates/toy-domain/src")).expect("workspace");
        fs::create_dir_all(issue_dir.join(".hidden")).expect("issue");
        fs::write(
            issue_dir.join("README.md"),
            "# Toy Preview\n\nChange delayed preview behavior to scheduled_at_period_end.\n",
        )
        .expect("readme");
        fs::write(
            issue_dir.join(".hidden").join("evaluate_hidden.sh"),
            r#"#!/usr/bin/env bash
set -euo pipefail
workspace="${1:?workspace}"
grep -q 'scheduled_at_period_end' "$workspace/crates/toy-domain/src/lib.rs"
"#,
        )
        .expect("hidden evaluator");
        fs::write(
            workspace_dir.join("evaluate_visible.sh"),
            r#"#!/usr/bin/env bash
set -euo pipefail
grep -q 'scheduled_at_period_end' crates/toy-domain/src/lib.rs
"#,
        )
        .expect("visible evaluator");
        fs::write(
            workspace_dir.join("START_HERE.md"),
            "# Objective\n\nPatch the toy preview change reason.\n",
        )
        .expect("objective");
        fs::write(
            workspace_dir.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/toy-domain"]
resolver = "2"
"#,
        )
        .expect("workspace cargo manifest");
        fs::write(
            workspace_dir.join("crates/toy-domain/Cargo.toml"),
            r#"[package]
name = "toy-domain"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        )
        .expect("toy cargo manifest");
        fs::write(
            workspace_dir.join("crates/toy-domain/src/lib.rs"),
            "pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n    if delayed_change {\n        \"immediate\"\n    } else {\n        \"immediate\"\n    }\n}\n",
        )
        .expect("toy source");
        for script in [
            issue_dir.join(".hidden").join("evaluate_hidden.sh"),
            workspace_dir.join("evaluate_visible.sh"),
        ] {
            let mut permissions = fs::metadata(&script).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(script, permissions).expect("chmod");
        }
        (temp_dir, issue_dir)
    }

    fn rust_swebench_top5_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("benchmark")
            .join("challenges")
            .join("rust-swebench-top5")
    }

    fn rust_swebench_top5_case_roots() -> Vec<PathBuf> {
        discover_challenge_case_roots(&rust_swebench_top5_root()).expect("discover rust cohort")
    }

    fn copy_case_root_to_temp(case_root: &Path) -> (tempfile::TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let copied_root = temp_dir.path().join(
            case_root
                .file_name()
                .expect("case root file name should exist"),
        );
        copy_dir_all(case_root, &copied_root).expect("copy case root");
        (temp_dir, copied_root)
    }

    fn create_retry_reset_fixture() -> (tempfile::TempDir, BenchmarkManifest, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sandbox_root = temp_dir.path().join("sandbox");
        let workspace_dir = sandbox_root.join("workspace").join("proof-full");
        fs::create_dir_all(workspace_dir.join("src")).expect("workspace");
        fs::write(
            workspace_dir.join("Cargo.toml"),
            r#"[package]
name = "retry-reset-fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        )
        .expect("cargo manifest");
        fs::write(
            workspace_dir.join("src").join("lib.rs"),
            "pub fn sample() -> u32 { 1 }\n",
        )
        .expect("workspace file");
        fs::write(
            sandbox_root.join("START_HERE.md"),
            "# Objective\n\nRestore the clean workspace before each attempt.\n",
        )
        .expect("objective");
        fs::write(
            sandbox_root.join("SUCCESS.md"),
            "# Success\n\nThe retry reset restores the workspace baseline.\n",
        )
        .expect("success");
        fs::write(
            sandbox_root.join("reset.sh"),
            r#"#!/usr/bin/env bash
set -euo pipefail

condition="${1:-proof-full}"
workspace="workspace/${condition}"

rm -rf "$workspace/.git" "$workspace/.quorp"
mkdir -p "$workspace/src"
cat <<'EOF' > "$workspace/Cargo.toml"
[package]
name = "retry-reset-fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
EOF
cat <<'EOF' > "$workspace/src/lib.rs"
pub fn sample() -> u32 { 1 }
EOF
"#,
        )
        .expect("reset");
        #[cfg(unix)]
        {
            let permissions = fs::Permissions::from_mode(0o755);
            fs::set_permissions(sandbox_root.join("reset.sh"), permissions)
                .expect("set reset executable");
        }

        let manifest = BenchmarkManifest {
            resolved: ResolvedBenchmark {
                benchmark_root: sandbox_root.clone(),
                issue_id: "retry-reset-fixture".to_string(),
                benchmark_name: "Retry reset fixture".to_string(),
                issue_dir: None,
                workspace_source: workspace_dir.clone(),
                objective_source: sandbox_root.join("START_HERE.md"),
                visible_evaluator: None,
                collector_evaluator: None,
                context_files: Vec::new(),
                repair_artifacts: Vec::new(),
            },
            executor: BenchmarkExecutor::Native,
            model_id: "fixture-model".to_string(),
            safety_mode_label: default_safe_mode_label(),
            scenario_label: None,
            base_url_override: None,
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 1,
            max_seconds: Some(30),
            max_total_tokens: None,
            autonomy_profile: "autonomous_host".to_string(),
            max_attempts: 2,
            challenge: Some(ChallengeMetadata {
                case_root: sandbox_root.clone(),
                sandbox_root: sandbox_root.clone(),
                workspace_dir: workspace_dir.clone(),
                condition: "proof-full".to_string(),
                objective_file: sandbox_root.join("START_HERE.md"),
                success_file: sandbox_root.join("SUCCESS.md"),
                reference_file: Some(sandbox_root.join("REFERENCE.md")),
                reset_command: "./reset.sh <condition>".to_string(),
                evaluate_command: "cargo test --quiet".to_string(),
                expected_files_touched: vec!["src/lib.rs".to_string()],
                allowed_generated_files: Vec::new(),
                primary_metrics: vec!["evaluate_passed".to_string()],
                tags: vec!["rust".to_string(), "fixture".to_string()],
                capsule_file: workspace_dir.join(".quorp").join("challenge-capsule.json"),
                capsule: ChallengeCapsule::default(),
            }),
            keep_sandbox: true,
            completion_policy: quorp_agent_core::CompletionPolicy::default(),
        };

        (temp_dir, manifest, workspace_dir)
    }

    fn apply_patch_in_workspace(workspace_root: &Path, patch_path: &Path, reverse: bool) -> bool {
        let mut command = Command::new("git");
        command.arg("-C").arg(workspace_root).arg("apply");
        if reverse {
            command.arg("-R");
        }
        let status = command
            .arg("--whitespace=nowarn")
            .arg(patch_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("apply patch");
        status.success()
    }

    fn workspace_probe_path(case_root: &Path) -> PathBuf {
        let workspace_root = case_root.join("workspace").join("proof-full");
        let cargo_manifest = workspace_root.join("Cargo.toml");
        if cargo_manifest.exists() {
            return cargo_manifest;
        }

        let mut stack = vec![workspace_root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir).expect("read dir");
            for entry in entries {
                let entry = entry.expect("entry");
                let path = entry.path();
                let file_type = entry.file_type().expect("file type");
                if file_type.is_dir() {
                    stack.push(path);
                } else {
                    return path;
                }
            }
        }

        panic!("no workspace file found under {}", workspace_root.display());
    }

    #[test]
    fn challenge_resolution_accepts_case_root_objective_and_workspace_paths() {
        let (_temp_dir, case_root) = create_challenge_case_fixture();
        let expected_objective =
            fs::canonicalize(case_root.join("START_HERE.md")).expect("canonical objective");

        let resolved_from_root = resolve_challenge_case(&case_root, None)
            .expect("resolve from case root")
            .expect("challenge case");
        assert_eq!(resolved_from_root.condition, "proof-full");
        assert_eq!(resolved_from_root.objective_source, expected_objective);

        let resolved_from_objective =
            resolve_challenge_case(&case_root.join("START_HERE.md"), None)
                .expect("resolve from objective")
                .expect("challenge case");
        assert_eq!(resolved_from_objective.condition, "proof-full");
        assert_eq!(resolved_from_objective.objective_source, expected_objective);

        let resolved_from_workspace = resolve_challenge_case(
            &case_root
                .join("workspace")
                .join("proof-full")
                .join("src")
                .join("lib.rs"),
            Some("bare"),
        )
        .expect("resolve from workspace path")
        .expect("challenge case");
        assert_eq!(resolved_from_workspace.condition, "bare");
        assert_eq!(resolved_from_workspace.objective_source, expected_objective);
    }

    #[test]
    fn challenge_resolution_rejects_mismatched_objective_markdown() {
        let (_temp_dir, case_root) = create_challenge_case_fixture();
        fs::write(case_root.join("README.md"), "alternate brief").expect("readme");
        let error = resolve_challenge_case(&case_root.join("README.md"), None)
            .expect_err("mismatched markdown should be rejected");
        assert!(
            error
                .to_string()
                .contains("does not match the declared objective file")
        );
    }

    #[test]
    fn challenge_case_discovery_finds_case_roots() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        for case_name in ["01-a", "02-b", "03-c", "04-d"] {
            let case_root = temp_dir.path().join(case_name);
            fs::create_dir_all(&case_root).expect("case dir");
            fs::write(case_root.join("benchmark.json"), "{}").expect("benchmark");
        }
        let case_roots = discover_challenge_case_roots(temp_dir.path()).expect("discover cases");
        assert_eq!(case_roots.len(), 4);
        assert!(case_roots.iter().any(|path| path.ends_with("01-a")));
        assert!(case_roots.iter().any(|path| path.ends_with("04-d")));
    }

    #[test]
    fn rust_swebench_top5_structure_and_resolution() {
        let case_roots = rust_swebench_top5_case_roots();
        assert_eq!(case_roots.len(), 5);

        for case_root in case_roots {
            for relative in [
                "benchmark.json",
                "START_HERE.md",
                "SUCCESS.md",
                "REFERENCE.md",
                "reset.sh",
                "evaluate.sh",
                "upstream/metadata.json",
                "upstream/problem_statement.md",
                "upstream/fix.patch",
                "upstream/test.patch",
            ] {
                assert!(
                    case_root.join(relative).exists(),
                    "missing `{relative}` for {}",
                    case_root.display()
                );
            }

            let manifest_path = case_root.join("benchmark.json");
            let manifest: ChallengeManifest =
                serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
                    .expect("parse challenge manifest");
            assert_eq!(manifest.repo_condition, vec!["proof-full".to_string()]);
            assert!(!manifest.expected_files_touched.is_empty());

            let resolved_from_root = resolve_challenge_case(&case_root, None)
                .expect("resolve from case root")
                .expect("challenge case");
            assert_eq!(resolved_from_root.condition, "proof-full");

            let resolved_from_objective =
                resolve_challenge_case(&case_root.join("START_HERE.md"), None)
                    .expect("resolve from objective")
                    .expect("challenge case");
            assert_eq!(resolved_from_objective.condition, "proof-full");

            let workspace_root = case_root.join("workspace").join("proof-full");
            if !workspace_root.exists() {
                eprintln!(
                    "skipping optional unpacked workspace checks for {}",
                    case_root.display()
                );
                continue;
            }

            for relative in [
                "AGENTS.md",
                "agent-map.json",
                "test-map.json",
                ".witness/witness-graph.json",
            ] {
                assert!(
                    workspace_root.join(relative).exists(),
                    "missing workspace fixture `{relative}` in {}",
                    workspace_root.display()
                );
            }

            let probe_path = workspace_probe_path(&case_root);
            let resolved_from_workspace = resolve_challenge_case(&probe_path, None)
                .expect("resolve from workspace path")
                .expect("challenge case");
            assert_eq!(resolved_from_workspace.condition, "proof-full");

            assert!(
                !workspace_root.join("target").exists(),
                "vendored cargo target should not exist in {}",
                workspace_root.display()
            );
            for expected in &manifest.expected_files_touched {
                assert!(
                    workspace_root.join(expected).exists(),
                    "missing expected touch target `{expected}` in {}",
                    workspace_root.display()
                );
            }
        }
    }

    #[test]
    #[ignore = "expensive real benchmark validation"]
    fn rust_swebench_top5_gold_patch_validation() {
        for case_root in rust_swebench_top5_case_roots() {
            let (_temp_dir, copied_root) = copy_case_root_to_temp(&case_root);
            let manifest: ChallengeManifest = serde_json::from_str(
                &fs::read_to_string(copied_root.join("benchmark.json")).expect("read manifest"),
            )
            .expect("parse manifest");

            let reset = run_shell_command(
                "reset",
                "./reset.sh proof-full",
                &copied_root.join("reset.sh"),
                &copied_root,
            )
            .expect("reset challenge workspace");
            assert!(reset.passed, "reset failed for {}", copied_root.display());

            let baseline = run_shell_command(
                "evaluation",
                "./evaluate.sh proof-full",
                &copied_root.join("evaluate.sh"),
                &copied_root,
            )
            .expect("run baseline evaluation");
            assert!(
                !baseline.passed,
                "baseline unexpectedly passed for {}",
                copied_root.display()
            );

            let workspace_root = copied_root.join("workspace").join("proof-full");
            for expected in &manifest.expected_files_touched {
                assert!(
                    workspace_root.join(expected).exists(),
                    "missing expected touch target `{expected}` in {}",
                    workspace_root.display()
                );
            }

            assert!(
                apply_patch_in_workspace(
                    &workspace_root,
                    &copied_root.join("upstream").join("fix.patch"),
                    false,
                ),
                "fix patch failed to apply for {}",
                copied_root.display()
            );

            let gold = run_shell_command(
                "evaluation",
                "./evaluate.sh proof-full",
                &copied_root.join("evaluate.sh"),
                &copied_root,
            )
            .expect("run gold evaluation");
            assert!(
                gold.passed,
                "gold patch failed for {}: stdout={} stderr={}",
                copied_root.display(),
                gold.stdout,
                gold.stderr
            );

            let (_replay_temp_dir, replay_root) = copy_case_root_to_temp(&case_root);
            let replay_reset = run_shell_command(
                "reset",
                "./reset.sh proof-full",
                &replay_root.join("reset.sh"),
                &replay_root,
            )
            .expect("reset replay workspace");
            assert!(
                replay_reset.passed,
                "replay reset failed for {}",
                replay_root.display()
            );

            let replay_workspace = replay_root.join("workspace").join("proof-full");
            assert!(
                apply_patch_in_workspace(
                    &replay_workspace,
                    &replay_root.join("upstream").join("test.patch"),
                    true,
                ),
                "reverse test patch failed for {}",
                replay_root.display()
            );
            assert!(
                apply_patch_in_workspace(
                    &replay_workspace,
                    &replay_root.join("upstream").join("test.patch"),
                    false,
                ),
                "test patch failed to apply for {}",
                replay_root.display()
            );
            assert!(
                apply_patch_in_workspace(
                    &replay_workspace,
                    &replay_root.join("upstream").join("fix.patch"),
                    false,
                ),
                "fix patch replay failed for {}",
                replay_root.display()
            );

            let replay_gold = run_shell_command(
                "evaluation",
                "./evaluate.sh proof-full",
                &replay_root.join("evaluate.sh"),
                &replay_root,
            )
            .expect("run replay gold evaluation");
            assert!(
                replay_gold.passed,
                "replayed test+fix patch failed for {}: stdout={} stderr={}",
                replay_root.display(),
                replay_gold.stdout,
                replay_gold.stderr
            );
        }
    }

    #[test]
    fn rust_swebench_retry_reset_restores_clean_workspace() {
        let (_temp_dir, manifest, workspace_dir) = create_retry_reset_fixture();
        let first_attempt = reset_challenge_workspace_for_attempt(&manifest, 1)
            .expect("attempt one should not fail");
        assert!(first_attempt.is_none());
        let second_attempt = reset_challenge_workspace_for_attempt(&manifest, 2)
            .expect("attempt two reset")
            .expect("attempt two should run reset");
        assert!(second_attempt.passed, "initial reset should succeed");
        let baseline_status = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .current_dir(&workspace_dir)
            .output()
            .expect("git status after initial reset");
        assert!(
            baseline_status.status.success(),
            "initial git status should succeed"
        );
        let baseline_status_stdout = String::from_utf8_lossy(&baseline_status.stdout).to_string();
        assert!(
            workspace_dir.join(".quorp").join("agent.toml").exists(),
            "initial reset should recreate the agent config"
        );

        fs::write(
            workspace_dir.join("src").join("lib.rs"),
            "pub fn sample() -> u32 { 99 }\n",
        )
        .expect("mutate workspace");
        fs::create_dir_all(workspace_dir.join(".quorp")).expect("seed .quorp directory");
        fs::write(workspace_dir.join(".quorp").join("stale.txt"), "stale")
            .expect("seed stale config");

        let third_attempt = reset_challenge_workspace_for_attempt(&manifest, 3)
            .expect("attempt three reset")
            .expect("attempt three should run reset");
        assert!(third_attempt.passed, "reset should succeed");
        assert_eq!(
            fs::read_to_string(workspace_dir.join("src").join("lib.rs"))
                .expect("read restored file"),
            "pub fn sample() -> u32 { 1 }\n"
        );
        assert!(
            workspace_dir.join(".git").exists(),
            "git baseline should be restored"
        );
        let agent_config = workspace_dir.join(".quorp").join("agent.toml");
        assert!(
            agent_config.exists(),
            "agent config should be rewritten after reset"
        );
        assert!(
            fs::read_to_string(&agent_config)
                .expect("read agent config")
                .contains("[defaults]"),
            "agent config should contain benchmark defaults"
        );
        let capsule_file = workspace_dir.join(".quorp").join("challenge-capsule.json");
        assert!(
            capsule_file.exists(),
            "challenge capsule should be rewritten after reset"
        );

        let status = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .current_dir(&workspace_dir)
            .output()
            .expect("git status");
        assert!(status.status.success(), "git status should succeed");
        assert!(
            String::from_utf8_lossy(&status.stdout) == baseline_status_stdout,
            "workspace should match the initial attempt state after reset"
        );
    }

    #[test]
    fn judge_response_parser_accepts_strict_json() {
        let parsed = parse_challenge_judge_response(
            r#"{"passed":true,"summary":"looks good","rationale":"objective was satisfied"}"#,
        )
        .expect("parse judge");
        assert!(parsed.passed);
        assert_eq!(parsed.summary, "looks good");
        assert_eq!(parsed.rationale, "objective was satisfied");
    }

    #[test]
    fn batch_report_sums_case_metrics() {
        let report = summarize_batch_report(
            PathBuf::from("/tmp/cases"),
            PathBuf::from("/tmp/results"),
            vec![
                BatchCaseReport {
                    case_id: "case-a".to_string(),
                    case_root: PathBuf::from("/tmp/cases/case-a"),
                    objective_path: PathBuf::from("/tmp/cases/case-a/START_HERE.md"),
                    result_dir: PathBuf::from("/tmp/results/case-a"),
                    log_file: PathBuf::from("/tmp/results/logs/case-a.log"),
                    executor: BenchmarkExecutor::Native,
                    success: true,
                    exit_code: 0,
                    wall_clock_ms: 100,
                    total_requests: 3,
                    total_billed_tokens: 12,
                    lines_added: 4,
                    lines_removed: 1,
                    mistakes_corrected: 1,
                    judge_passed: Some(true),
                    deterministic_evaluation_passed: Some(true),
                    first_request_prompt_token_estimate: Some(1200),
                    first_request_raw_prompt_token_estimate: Some(1200),
                    first_request_compacted_prompt_token_estimate: Some(700),
                    first_request_first_token_latency_ms: Some(800),
                    first_model_turn_started: true,
                    first_action_emitted: true,
                    final_stop_reason: Some(quorp_agent_core::StopReason::Success),
                    primary_failure: None,
                    local_agent_final_failure_classification: Some("success".to_string()),
                    adaptive_action_mode_retry: false,
                    report_path: PathBuf::from("/tmp/results/case-a/benchmark-report.json"),
                    error: None,
                },
                BatchCaseReport {
                    case_id: "case-b".to_string(),
                    case_root: PathBuf::from("/tmp/cases/case-b"),
                    objective_path: PathBuf::from("/tmp/cases/case-b/START_HERE.md"),
                    result_dir: PathBuf::from("/tmp/results/case-b"),
                    log_file: PathBuf::from("/tmp/results/logs/case-b.log"),
                    executor: BenchmarkExecutor::Native,
                    success: false,
                    exit_code: 1,
                    wall_clock_ms: 200,
                    total_requests: 2,
                    total_billed_tokens: 8,
                    lines_added: 2,
                    lines_removed: 3,
                    mistakes_corrected: 0,
                    judge_passed: Some(false),
                    deterministic_evaluation_passed: Some(false),
                    first_request_prompt_token_estimate: Some(1400),
                    first_request_raw_prompt_token_estimate: Some(1400),
                    first_request_compacted_prompt_token_estimate: None,
                    first_request_first_token_latency_ms: Some(900),
                    first_model_turn_started: false,
                    first_action_emitted: false,
                    final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                    primary_failure: Some("agent_fatal_error".to_string()),
                    local_agent_final_failure_classification: Some(
                        "parser_tool_schema".to_string(),
                    ),
                    adaptive_action_mode_retry: false,
                    report_path: PathBuf::from("/tmp/results/case-b/benchmark-report.json"),
                    error: Some("failed".to_string()),
                },
            ],
        );
        assert_eq!(report.total_requests, 5);
        assert_eq!(report.total_billed_tokens, 20);
        assert_eq!(report.lines_added, 6);
        assert_eq!(report.lines_removed, 4);
        assert_eq!(report.mistakes_corrected, 1);
        assert_eq!(report.successful_cases, 1);
        assert_eq!(report.failed_cases, 1);
    }

    #[test]
    fn synthetic_failure_report_marks_launch_failures() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let case_manifest = ChallengeManifest {
            id: "01-sample-case".to_string(),
            title: "Sample challenge".to_string(),
            difficulty: "easy".to_string(),
            category: "sample".to_string(),
            repo_condition: vec!["proof-full".to_string()],
            objective_file: "START_HERE.md".to_string(),
            success_file: "expected/success-criteria.md".to_string(),
            reset_command: "./reset.sh <condition>".to_string(),
            evaluate_command: "./evaluate.sh <condition>".to_string(),
            estimated_minutes: Some(1),
            expected_files_touched: Vec::new(),
            allowed_generated_files: Vec::new(),
            primary_metrics: Vec::new(),
            tags: Vec::new(),
        };

        write_synthetic_failure_report(
            &case_manifest,
            temp_dir.path(),
            BenchmarkExecutor::Native,
            &safe_benchmark_model_id().expect("broker default benchmark model"),
            3,
            "runtime never became ready".to_string(),
            None,
        )
        .expect("write synthetic report");

        let report: BenchmarkReport = serde_json::from_str(
            &fs::read_to_string(temp_dir.path().join("benchmark-report.json"))
                .expect("read report"),
        )
        .expect("parse report");
        assert!(!report.success);
        assert_eq!(report.primary_failure.as_deref(), Some("launch_failed"));
        assert_eq!(
            report.run_error.as_deref(),
            Some("runtime never became ready")
        );
    }

    #[test]
    fn challenge_setup_failure_writes_benchmark_report() {
        let (_temp_dir, case_root) = create_challenge_case_fixture();
        fs::write(
            case_root.join("reset.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\ncondition=\"$1\"\nrm -rf \"workspace/${condition}\"\n",
        )
        .expect("reset script");
        let mut permissions = fs::metadata(case_root.join("reset.sh"))
            .expect("reset metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(case_root.join("reset.sh"), permissions).expect("chmod reset");
        let challenge =
            resolve_challenge_case(&case_root.join("START_HERE.md"), Some("proof-full"))
                .expect("resolve challenge")
                .expect("challenge case");
        let result_dir = tempfile::tempdir().expect("result dir");
        let options = BenchmarkRunOptions {
            path: case_root.join("START_HERE.md"),
            executor: BenchmarkExecutor::Codex,
            model_id: Some("test-model".to_string()),
            base_url_override: None,
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 1,
            max_seconds: Some(1),
            max_total_tokens: None,
            result_dir: result_dir.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousSandboxed,
            max_attempts: Some(1),
            allow_heavy_local_model: false,
            condition: Some("proof-full".to_string()),
            keep_sandbox: true,
        };

        let error = run_challenge_benchmark(&options, challenge).expect_err("setup failure");
        assert!(error.to_string().contains("layout_resolution_failed"));

        let report: BenchmarkReport = serde_json::from_str(
            &fs::read_to_string(result_dir.path().join("benchmark-report.json"))
                .expect("read report"),
        )
        .expect("parse report");
        assert!(!report.success);
        assert_eq!(
            report.setup_failure_class.as_deref(),
            Some("layout_resolution_failed")
        );
        assert_eq!(
            report.primary_failure.as_deref(),
            Some("layout_resolution_failed")
        );
    }

    #[test]
    fn bootstrap_tracker_records_progress_and_first_task_request() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let attempt_dir = temp_dir.path().join("attempt-001");
        fs::create_dir_all(&attempt_dir).expect("attempt dir");

        let tracker = BenchmarkBootstrapTracker::new(temp_dir.path(), &attempt_dir, 1)
            .expect("create tracker");
        tracker
            .update(
                BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED,
                Some("benchmark control loop entered".to_string()),
            )
            .expect("update phase");
        tracker
            .mark_first_task_model_request()
            .expect("mark first request");

        let progress = read_bootstrap_progress(&attempt_bootstrap_progress_path(&attempt_dir))
            .expect("read progress")
            .expect("progress exists");
        assert_eq!(
            progress.bootstrap_phase,
            BOOTSTRAP_PHASE_FIRST_TASK_MODEL_REQUEST
        );
        assert!(progress.first_task_model_request_seen);
        assert!(
            progress
                .bootstrap_elapsed_ms_before_first_task_request
                .is_some()
        );
    }

    #[test]
    fn write_report_preserves_pre_model_bootstrap_stall_fields() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace_dir = temp_dir.path().join("workspace");
        let attempt_dir = temp_dir.path().join("attempt-001");
        let agent_result_dir = attempt_dir.join("agent");
        fs::create_dir_all(&workspace_dir).expect("workspace");
        fs::create_dir_all(&agent_result_dir).expect("agent");

        let manifest = BenchmarkManifest {
            resolved: ResolvedBenchmark {
                benchmark_root: temp_dir.path().join("benchmark-root"),
                issue_id: "06-rust-swebench-bincode-serde-decoder-memory".to_string(),
                benchmark_name: "Bootstrap stall case".to_string(),
                issue_dir: None,
                workspace_source: workspace_dir.clone(),
                objective_source: workspace_dir.join("START_HERE.md"),
                visible_evaluator: None,
                collector_evaluator: None,
                context_files: Vec::new(),
                repair_artifacts: Vec::new(),
            },
            executor: BenchmarkExecutor::Native,
            model_id: "ssd_moe/qwen3-coder-30b-a3b".to_string(),
            safety_mode_label: "safe_local".to_string(),
            scenario_label: Some("QuorpLocal".to_string()),
            base_url_override: Some("http://127.0.0.1:49919".to_string()),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(5_000),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost
                .label()
                .to_string(),
            max_attempts: 1,
            challenge: None,
            keep_sandbox: false,
            completion_policy: benchmark_completion_policy(
                BenchmarkExecutor::Native,
                "safe_local",
                Some("ssd_moe/qwen3-coder-30b-a3b"),
            ),
        };
        let progress = BenchmarkBootstrapProgress {
            attempt: 1,
            bootstrap_phase: BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED.to_string(),
            bootstrap_phase_detail: Some(
                "benchmark control loop started but never reached the model".to_string(),
            ),
            started_at_epoch_ms: 1,
            updated_at_epoch_ms: 2,
            first_task_model_request_seen: false,
            bootstrap_elapsed_ms_before_first_task_request: None,
            pre_model_bootstrap_stalled: true,
            bootstrap_stall_class: Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL.to_string()),
        };
        let attempt = attempt_report_for_bootstrap_stall(
            &manifest,
            1,
            &attempt_dir,
            &workspace_dir,
            &agent_result_dir,
            &progress,
        );

        write_report(temp_dir.path(), &manifest, &[attempt], None, None).expect("write report");

        let report: BenchmarkReport = serde_json::from_str(
            &fs::read_to_string(temp_dir.path().join("benchmark-report.json"))
                .expect("read report"),
        )
        .expect("parse report");
        assert!(report.pre_model_bootstrap_stalled);
        assert_eq!(
            report.bootstrap_phase.as_deref(),
            Some(BOOTSTRAP_PHASE_CONTROL_LOOP_STARTED)
        );
        assert!(!report.first_task_model_request_seen);
        assert_eq!(
            report.primary_failure.as_deref(),
            Some(BOOTSTRAP_STALL_CLASS_PRE_MODEL)
        );
    }

    #[test]
    fn partial_batch_summary_is_persisted() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let options = BenchmarkBatchRunOptions {
            cases_root: PathBuf::from("/tmp/cases"),
            result_dir: temp_dir.path().to_path_buf(),
            executor: BenchmarkExecutor::Native,
            model_id: None,
            base_url_override: None,
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(60),
            max_total_tokens: Some(1000),
            max_attempts: Some(2),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousSandboxed,
            allow_heavy_local_model: false,
            condition: None,
            keep_sandbox: false,
            log_dir: None,
        };
        let cases = vec![BatchCaseReport {
            case_id: "case-a".to_string(),
            case_root: PathBuf::from("/tmp/cases/case-a"),
            objective_path: PathBuf::from("/tmp/cases/case-a/START_HERE.md"),
            result_dir: PathBuf::from("/tmp/results/case-a"),
            log_file: PathBuf::from("/tmp/results/logs/case-a.log"),
            executor: BenchmarkExecutor::Native,
            success: false,
            exit_code: 1,
            wall_clock_ms: 77,
            total_requests: 1,
            total_billed_tokens: 42,
            lines_added: 0,
            lines_removed: 0,
            mistakes_corrected: 0,
            judge_passed: None,
            deterministic_evaluation_passed: None,
            first_request_prompt_token_estimate: None,
            first_request_raw_prompt_token_estimate: None,
            first_request_compacted_prompt_token_estimate: None,
            first_request_first_token_latency_ms: None,
            first_model_turn_started: false,
            first_action_emitted: false,
            final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
            primary_failure: Some("agent_fatal_error".to_string()),
            local_agent_final_failure_classification: Some("parser_tool_schema".to_string()),
            adaptive_action_mode_retry: false,
            report_path: PathBuf::from("/tmp/results/case-a/benchmark-report.json"),
            error: Some("fatal".to_string()),
        }];

        write_batch_summary_artifacts(&options, &cases, 123).expect("write partial batch summary");

        let report: BatchReport = serde_json::from_str(
            &fs::read_to_string(temp_dir.path().join("batch-report.json")).expect("read report"),
        )
        .expect("parse report");
        assert_eq!(report.cases.len(), 1);
        assert_eq!(report.total_billed_tokens, 42);
        let rendered =
            fs::read_to_string(temp_dir.path().join("batch-report.md")).expect("read markdown");
        assert!(rendered.contains("failure=agent_fatal_error"));
        let run_summary =
            fs::read_to_string(temp_dir.path().join("run-summary.md")).expect("read summary");
        assert!(run_summary.contains("local=parser_tool_schema"));
    }

    #[test]
    fn score_benchmark_reports_writes_session_scoreboard() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let run_dir = temp_dir.path().join("run");
        let case_a_dir = run_dir.join("01-case-a");
        let case_b_dir = run_dir.join("02-case-b");
        fs::create_dir_all(&case_a_dir).expect("case a dir");
        fs::create_dir_all(&case_b_dir).expect("case b dir");
        let case_a_report = case_a_dir.join("benchmark-report.json");
        let case_b_report = case_b_dir.join("benchmark-report.json");
        write_json(
            &case_a_report,
            &serde_json::json!({
                "benchmark_name": "Case A",
                "issue_id": "01-case-a",
                "model_id": "ollama/local",
                "success": false,
                "attempts_run": 1,
                "max_attempts": 1,
                "total_billed_tokens": 100,
                "changed_files": ["Cargo.toml"],
                "widening_happened": false,
                "attempts": [],
                "run_dir": case_a_dir,
                "wall_clock_ms": 10,
                "total_requests": 2,
                "lines_added": 1,
                "lines_removed": 0,
                "first_model_turn_started": true,
                "first_action_emitted": true,
                "validation_commands_run": 2,
                "post_fast_loop_validation_rerun_attempted": true,
                "local_agent_final_failure_classification": "model_edit_strategy",
                "local_agent_scorecard": {
                    "first_valid_write_step": 4,
                    "modify_toml_count": 1
                }
            }),
        )
        .expect("write case a report");
        write_json(
            &case_b_report,
            &serde_json::json!({
                "benchmark_name": "Case B",
                "issue_id": "02-case-b",
                "model_id": "ollama/local",
                "success": false,
                "attempts_run": 1,
                "max_attempts": 1,
                "total_billed_tokens": 50,
                "changed_files": [],
                "widening_happened": false,
                "attempts": [],
                "run_dir": case_b_dir,
                "wall_clock_ms": 20,
                "total_requests": 1,
                "first_model_turn_started": true,
                "first_action_emitted": true,
                "primary_failure": "agent_fatal_error",
                "local_agent_final_failure_classification": "parser_tool_schema",
                "local_agent_scorecard": {
                    "parser_recovery_count": 2
                }
            }),
        )
        .expect("write case b report");
        let batch = summarize_batch_report(
            PathBuf::from("/tmp/rust-swebench-top5"),
            run_dir.clone(),
            vec![
                BatchCaseReport {
                    case_id: "01-case-a".to_string(),
                    case_root: PathBuf::from("/tmp/rust-swebench-top5/01-case-a"),
                    objective_path: PathBuf::from(
                        "/tmp/rust-swebench-top5/01-case-a/START_HERE.md",
                    ),
                    result_dir: case_a_dir.clone(),
                    log_file: run_dir.join("logs/01-case-a.log"),
                    executor: BenchmarkExecutor::Native,
                    success: false,
                    exit_code: 1,
                    wall_clock_ms: 10,
                    total_requests: 2,
                    total_billed_tokens: 100,
                    lines_added: 1,
                    lines_removed: 0,
                    mistakes_corrected: 0,
                    judge_passed: None,
                    deterministic_evaluation_passed: None,
                    first_request_prompt_token_estimate: None,
                    first_request_raw_prompt_token_estimate: None,
                    first_request_compacted_prompt_token_estimate: None,
                    first_request_first_token_latency_ms: None,
                    first_model_turn_started: true,
                    first_action_emitted: true,
                    final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                    primary_failure: Some("agent_fatal_error".to_string()),
                    local_agent_final_failure_classification: Some(
                        "model_edit_strategy".to_string(),
                    ),
                    adaptive_action_mode_retry: false,
                    report_path: case_a_report,
                    error: None,
                },
                BatchCaseReport {
                    case_id: "02-case-b".to_string(),
                    case_root: PathBuf::from("/tmp/rust-swebench-top5/02-case-b"),
                    objective_path: PathBuf::from(
                        "/tmp/rust-swebench-top5/02-case-b/START_HERE.md",
                    ),
                    result_dir: case_b_dir,
                    log_file: run_dir.join("logs/02-case-b.log"),
                    executor: BenchmarkExecutor::Native,
                    success: false,
                    exit_code: 1,
                    wall_clock_ms: 20,
                    total_requests: 1,
                    total_billed_tokens: 50,
                    lines_added: 0,
                    lines_removed: 0,
                    mistakes_corrected: 0,
                    judge_passed: None,
                    deterministic_evaluation_passed: None,
                    first_request_prompt_token_estimate: None,
                    first_request_raw_prompt_token_estimate: None,
                    first_request_compacted_prompt_token_estimate: None,
                    first_request_first_token_latency_ms: None,
                    first_model_turn_started: true,
                    first_action_emitted: true,
                    final_stop_reason: Some(quorp_agent_core::StopReason::FatalError),
                    primary_failure: Some("agent_fatal_error".to_string()),
                    local_agent_final_failure_classification: Some(
                        "parser_tool_schema".to_string(),
                    ),
                    adaptive_action_mode_retry: true,
                    report_path: case_b_report,
                    error: Some("fatal".to_string()),
                },
            ],
        );
        write_json(&run_dir.join("batch-report.json"), &batch).expect("write batch report");

        let output_root = temp_dir.path().join("scoreboards");
        let artifacts = score_benchmark_reports(BenchmarkScoreOptions {
            run_dirs: vec![run_dir],
            suite: "rust-swebench-top5".to_string(),
            reports_root: temp_dir.path().join("reports"),
            output_root: Some(output_root.clone()),
        })
        .expect("score reports");

        assert!(artifacts.markdown.contains("Solved score: `0/2`"));
        assert!(
            artifacts
                .markdown
                .contains("Valid implementation writes: `1/2`")
        );
        assert!(artifacts.markdown.contains("Post-write validation: `1/2`"));
        assert!(artifacts.output_dir.join("scoreboard.json").exists());
        assert!(output_root.join("latest.md").exists());
        let score: BenchmarkScoreReport = serde_json::from_str(
            &fs::read_to_string(output_root.join("latest.json")).expect("read latest score"),
        )
        .expect("parse score");
        assert_eq!(score.valid_write_cases, 1);
        assert_eq!(score.post_write_validation_cases, 1);
        assert_eq!(score.blocker_counts.get("parser_tool_schema"), Some(&1));
    }

    #[test]
    fn git_numstat_counts_added_and_removed_lines() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::write(workspace.join("sample.txt"), "alpha\nbeta\ngamma\n").expect("baseline file");
        ensure_git_baseline(&workspace).expect("baseline git repo");
        fs::write(
            workspace.join("sample.txt"),
            "alpha\nbeta updated\ngamma\ndelta\n",
        )
        .expect("modified file");

        let (lines_added, lines_removed) = git_numstat(&workspace).expect("git numstat");
        assert_eq!(lines_added, 2);
        assert_eq!(lines_removed, 1);
    }

    #[test]
    fn reportable_changed_files_ignore_target_artifacts() {
        assert!(is_reportable_changed_file("crates/toy-domain/src/lib.rs"));
        assert!(!is_reportable_changed_file("target/.rustc_info.json"));
        assert!(!is_reportable_changed_file(".quorp/challenge-capsule.json"));
        assert!(!is_reportable_changed_file(
            ".warpos-capture-probe/events.jsonl"
        ));
        assert!(is_support_or_generated_changed_file("START_HERE.md"));
        assert!(is_support_or_generated_changed_file(
            "benchmark-report.json"
        ));
        assert!(!is_support_or_generated_changed_file("src/lib.rs"));
    }

    #[test]
    fn challenge_ignored_changed_files_exclude_benchmark_support_files() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace_dir = temp_dir.path().join("workspace");
        let quorp_dir = workspace_dir.join(".quorp");
        fs::create_dir_all(&quorp_dir).expect("mkdir");
        let objective_file = workspace_dir.join("START_HERE.md");
        let success_file = workspace_dir.join("SUCCESS.md");
        let reference_file = workspace_dir.join("REFERENCE.md");
        let benchmark_manifest = workspace_dir.join("benchmark.json");
        let capsule_file = quorp_dir.join("challenge-capsule.json");
        for path in [
            &objective_file,
            &success_file,
            &reference_file,
            &benchmark_manifest,
            &capsule_file,
        ] {
            fs::write(path, "placeholder").expect("write support file");
        }
        let metadata = ChallengeMetadata {
            case_root: temp_dir.path().join("case"),
            sandbox_root: temp_dir.path().join("sandbox"),
            workspace_dir: workspace_dir.clone(),
            condition: "proof-full".to_string(),
            objective_file,
            success_file,
            reference_file: Some(reference_file),
            reset_command: "./reset.sh proof-full".to_string(),
            evaluate_command: "./evaluate.sh proof-full".to_string(),
            expected_files_touched: vec!["src/lib.rs".to_string()],
            allowed_generated_files: Vec::new(),
            primary_metrics: Vec::new(),
            tags: Vec::new(),
            capsule_file,
            capsule: ChallengeCapsule::default(),
        };

        let ignored = challenge_ignored_changed_files(&metadata, &workspace_dir);
        let changed = vec![
            "START_HERE.md".to_string(),
            "SUCCESS.md".to_string(),
            "REFERENCE.md".to_string(),
            "benchmark.json".to_string(),
            ".quorp/challenge-capsule.json".to_string(),
            "src/lib.rs".to_string(),
        ];

        assert_eq!(
            filter_ignored_changed_files(&changed, &ignored),
            vec!["src/lib.rs".to_string()]
        );
    }

    #[test]
    fn extract_read_range_observations_from_checkpoint_transcript() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let checkpoint_path = temp_dir.path().join("checkpoint.json");
        let checkpoint = quorp_agent_core::AgentCheckpoint {
            snapshot: quorp_agent_core::AgentTaskStateSnapshot {
                current_mode: quorp_agent_core::AgentMode::Act,
                acceptance_criteria: Vec::new(),
                working_set: BTreeSet::new(),
                last_tool_summary: None,
                last_failing_verifier: None,
                last_safe_checkpoint: None,
                last_parse_error: None,
                stall_count: 0,
                redundant_inspection_turns: 0,
                recoverable_inspection_failures: 0,
                parser_recovery_failures: 0,
                parser_recovery_validation_fingerprint: None,
                parser_recovery_same_validation_streak: 0,
                has_mutating_change: false,
                verified_green: false,
                validation_queue: std::collections::VecDeque::new(),
                total_billed_tokens: 0,
                last_failed_tool_error: None,
                repair_recovery_turns_remaining: 0,
                benchmark_case_ledger: None,
                repair_requirement: None,
                last_successful_write_action: None,
                benchmark_repair_state: None,
                failed_edit_records: Vec::new(),
                local_model_memory: quorp_agent_core::LocalModelMemory::default(),
            },
            transcript: vec![TranscriptMessage {
                role: TranscriptRole::User,
                content: "[Tool Output]\nstatus: success\naction: read_file src/round.rs lines 390-450\npath: src/round.rs\nrequested_range: 390-450\nhonored_range: 390-450\nround excerpt".to_string(),
            }],
            step: 2,
            request_counter: 1,
        };
        write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");

        let observations =
            extract_read_range_observations(&checkpoint_path).expect("read observations");

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].path, "src/round.rs");
        assert_eq!(observations[0].requested_range.as_deref(), Some("390-450"));
        assert_eq!(observations[0].honored_range.as_deref(), Some("390-450"));
    }

    #[test]
    fn extract_action_evidence_counts_reads_writes_and_gate_commands() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let checkpoint_path = temp_dir.path().join("checkpoint.json");
        let checkpoint = quorp_agent_core::AgentCheckpoint {
            snapshot: quorp_agent_core::AgentTaskStateSnapshot {
                current_mode: quorp_agent_core::AgentMode::Act,
                acceptance_criteria: Vec::new(),
                working_set: BTreeSet::new(),
                last_tool_summary: None,
                last_failing_verifier: None,
                last_safe_checkpoint: None,
                last_parse_error: None,
                stall_count: 0,
                redundant_inspection_turns: 0,
                recoverable_inspection_failures: 0,
                parser_recovery_failures: 0,
                parser_recovery_validation_fingerprint: None,
                parser_recovery_same_validation_streak: 0,
                has_mutating_change: false,
                verified_green: false,
                validation_queue: std::collections::VecDeque::new(),
                total_billed_tokens: 0,
                last_failed_tool_error: None,
                repair_recovery_turns_remaining: 0,
                benchmark_case_ledger: None,
                repair_requirement: None,
                last_successful_write_action: None,
                benchmark_repair_state: None,
                failed_edit_records: Vec::new(),
                local_model_memory: quorp_agent_core::LocalModelMemory::default(),
            },
            transcript: vec![
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: read_file src/round.rs lines 1-20\npath: src/round.rs\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: replace_block src/round.rs lines 10-12\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: failure\naction: run: cargo test --quiet --lib round::tests::\n".to_string(),
                },
                TranscriptMessage {
                    role: TranscriptRole::User,
                    content: "[Tool Output]\nstatus: success\naction: run: ./evaluate.sh proof-full\n".to_string(),
                },
            ],
            step: 5,
            request_counter: 2,
        };
        write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");
        let capsule = ChallengeCapsule {
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            ..ChallengeCapsule::default()
        };

        let evidence = extract_action_evidence(
            &checkpoint_path,
            Some(&capsule),
            Some("./evaluate.sh proof-full"),
        )
        .expect("extract evidence");

        assert_eq!(evidence.read_count, 1);
        assert_eq!(evidence.write_count, 1);
        assert_eq!(evidence.command_execution_count, 2);
        assert!(evidence.fast_loop_command_seen);
        assert!(evidence.final_evaluate_command_seen);
    }

    #[test]
    fn rust_swe_case_profiles_cover_recovery_gate_cases() {
        let expected = [
            (
                "06-rust-swebench-bincode-serde-decoder-memory",
                "cargo test --quiet --features serde --test issues issue_474",
            ),
            (
                "07-rust-swebench-chrono-epoch-truncation",
                "cargo test --quiet --lib round::tests::",
            ),
            (
                "08-rust-swebench-axum-fallback-merge",
                "cargo test --quiet -p axum --lib --features headers routing::tests::",
            ),
            (
                "09-rust-swebench-cargo-dist-create-release",
                "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact",
            ),
            (
                "10-rust-swebench-cc-rs-compile-intermediates",
                "cargo test --quiet compile_intermediates",
            ),
        ];

        for (case_id, fast_loop) in expected {
            let profile = rust_swe_case_profile(case_id).expect("profile");
            assert_eq!(profile.final_eval_command, "./evaluate.sh proof-full");
            assert!(
                profile
                    .fast_loop_commands
                    .iter()
                    .any(|command| *command == fast_loop),
                "missing fast loop for {case_id}"
            );
            assert!(
                !profile.likely_owner_files.is_empty(),
                "missing owners for {case_id}"
            );
        }
    }

    #[test]
    fn read_checkpoint_validation_state_parses_repair_phase_fields() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let checkpoint_path = temp_dir.path().join("checkpoint.json");
        let checkpoint = serde_json::json!({
            "snapshot": {
                "benchmark_case_ledger": {
                    "validation_status": "failed: fast-loop",
                    "last_validation_failure": "test `round::tests::test_duration_round_close_to_min_max` failed | at src/round.rs:800",
                    "validation_details": {
                        "failing_test_names": ["round::tests::test_duration_round_close_to_min_max"],
                        "primary_failure_test_name": "round::tests::test_duration_round_close_to_min_max",
                        "primary_failure_path": "src/round.rs",
                        "primary_failure_line": 800,
                        "assertion_excerpt": "assertion `left == right` failed",
                        "repair_required": true,
                        "repair_phase_terminal": "needs_patch",
                        "failure_anchor_reread_attempted": true,
                        "failure_anchor_reread_honored": true,
                        "implementation_reread_allowed": true,
                        "implementation_reread_attempted": true,
                        "implementation_reread_honored": true,
                        "repair_phase_invalid_action_count": 1,
                        "post_fast_loop_patch_attempted": true,
                        "post_fast_loop_validation_rerun_attempted": false,
                        "patch_packet_injected": true,
                        "patch_packet_honored_range": "188-254",
                        "recommended_rerun_command": "cargo test --quiet --lib round::tests::test_duration_round_close_to_min_max",
                        "fast_loop_rerun_match_kind": "subset_fast_loop",
                        "failed_edit_records": [{
                            "action_kind": "replace_block",
                            "path": "src/round.rs",
                            "search_hash": "abc",
                            "replace_hash": "def",
                            "failure_reason": "Search block is ambiguous; found 2 matches at lines 151, 188",
                            "matching_line_numbers": [151, 188],
                            "attempts": 1
                        }]
                    }
                }
            }
        });
        write_json(&checkpoint_path, &checkpoint).expect("write checkpoint");

        let state = read_checkpoint_validation_state(&checkpoint_path).expect("validation state");

        assert_eq!(
            state.primary_failure_test_name.as_deref(),
            Some("round::tests::test_duration_round_close_to_min_max")
        );
        assert_eq!(state.repair_phase_terminal.as_deref(), Some("needs_patch"));
        assert!(state.failure_anchor_reread_attempted);
        assert!(state.failure_anchor_reread_honored);
        assert!(state.implementation_reread_allowed);
        assert!(state.implementation_reread_attempted);
        assert!(state.implementation_reread_honored);
        assert_eq!(state.repair_phase_invalid_action_count, 1);
        assert!(state.patch_packet_injected);
        assert_eq!(state.patch_packet_honored_range.as_deref(), Some("188-254"));
        assert_eq!(
            state.recommended_rerun_command.as_deref(),
            Some("cargo test --quiet --lib round::tests::test_duration_round_close_to_min_max")
        );
        assert_eq!(
            state.fast_loop_rerun_match_kind.as_deref(),
            Some("subset_fast_loop")
        );
        assert_eq!(state.failed_edit_records.len(), 1);
        assert_eq!(
            state.failed_edit_records[0].matching_line_numbers,
            vec![151, 188]
        );
    }

    #[test]
    fn judge_output_summary_truncates_large_logs() {
        let large = (0..80)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let summary = summarize_judge_output(&large);
        assert!(summary.contains("truncated 80 lines"));
        assert!(summary.contains("line 0"));
        assert!(summary.contains("line 79"));
    }

    #[test]
    fn run_shell_command_with_env_applies_cargo_target_dir_override() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let target_dir = temp_dir.path().join("eval-target");
        let outcome = run_shell_command_with_env(
            "evaluation",
            "printf '%s' \"$CARGO_TARGET_DIR\"",
            &temp_dir.path().join("evaluate.sh"),
            temp_dir.path(),
            &[("CARGO_TARGET_DIR", target_dir.as_os_str())],
        )
        .expect("shell command");

        assert!(outcome.passed);
        assert_eq!(outcome.stdout, target_dir.display().to_string());
    }

    #[test]
    fn workspace_challenge_command_wrappers_point_to_case_root_scripts() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sandbox_root = temp_dir.path().join("sandbox");
        let workspace_dir = sandbox_root.join("workspace").join("proof-full");
        fs::create_dir_all(&workspace_dir).expect("workspace");

        write_workspace_challenge_command_wrappers(&workspace_dir).expect("write wrappers");
        let evaluate_wrapper =
            fs::read_to_string(workspace_dir.join("evaluate.sh")).expect("read evaluate wrapper");
        let reset_wrapper =
            fs::read_to_string(workspace_dir.join("reset.sh")).expect("read reset wrapper");
        assert!(evaluate_wrapper.contains("cd \"$(dirname \"$0\")/../..\""));
        assert!(evaluate_wrapper.contains("exec ./evaluate.sh"));
        assert!(reset_wrapper.contains("cd \"$(dirname \"$0\")/../..\""));
        assert!(reset_wrapper.contains("exec ./reset.sh"));
    }

    #[test]
    fn challenge_evaluation_target_dir_is_attempt_scoped() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let metadata = ChallengeMetadata {
            case_root: temp_dir.path().join("case"),
            sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
            workspace_dir: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full"),
            condition: "proof-full".to_string(),
            objective_file: temp_dir.path().join("run").join("START_HERE.md"),
            success_file: temp_dir.path().join("run").join("SUCCESS.md"),
            reference_file: None,
            reset_command: "./reset.sh proof-full".to_string(),
            evaluate_command: "./evaluate.sh proof-full".to_string(),
            expected_files_touched: Vec::new(),
            allowed_generated_files: Vec::new(),
            primary_metrics: Vec::new(),
            tags: Vec::new(),
            capsule_file: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full")
                .join(".quorp")
                .join("challenge-capsule.json"),
            capsule: ChallengeCapsule::default(),
        };

        let attempt_one = challenge_evaluation_target_dir(&metadata, 1);
        let attempt_two = challenge_evaluation_target_dir(&metadata, 2);
        assert_ne!(attempt_one, attempt_two);
        assert!(attempt_one.ends_with("attempt-001"));
        assert!(attempt_two.ends_with("attempt-002"));
        assert!(
            attempt_one
                .components()
                .any(|component| component.as_os_str() == CHALLENGE_EVALUATION_CARGO_CACHE_DIR)
        );
    }

    #[test]
    fn cargo_dist_snapshot_challenge_uses_workspace_target_for_evaluation() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let mut metadata = ChallengeMetadata {
            case_root: temp_dir.path().join("case"),
            sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
            workspace_dir: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full"),
            condition: "proof-full".to_string(),
            objective_file: temp_dir.path().join("run").join("START_HERE.md"),
            success_file: temp_dir.path().join("run").join("SUCCESS.md"),
            reference_file: None,
            reset_command: "./reset.sh proof-full".to_string(),
            evaluate_command: "./evaluate.sh proof-full".to_string(),
            expected_files_touched: Vec::new(),
            allowed_generated_files: Vec::new(),
            primary_metrics: Vec::new(),
            tags: Vec::new(),
            capsule_file: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full")
                .join(".quorp")
                .join("challenge-capsule.json"),
            capsule: ChallengeCapsule::default(),
        };
        let evaluation_target_dir = temp_dir.path().join("eval-target");

        assert_eq!(
            challenge_evaluation_env(&metadata, &evaluation_target_dir).len(),
            1
        );

        metadata.allowed_generated_files =
            vec!["cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap".to_string()];
        assert!(challenge_evaluation_env(&metadata, &evaluation_target_dir).is_empty());
    }

    #[test]
    fn cc_rs_challenge_sets_sdkroot_for_macos_sdk_free_evaluation() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let metadata = ChallengeMetadata {
            case_root: temp_dir.path().join("05-cc-rs-compile-intermediates"),
            sandbox_root: temp_dir.path().join("run").join(CHALLENGE_SANDBOX_DIR),
            workspace_dir: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full"),
            condition: "proof-full".to_string(),
            objective_file: temp_dir.path().join("run").join("START_HERE.md"),
            success_file: temp_dir.path().join("run").join("SUCCESS.md"),
            reference_file: None,
            reset_command: "./reset.sh proof-full".to_string(),
            evaluate_command: "./evaluate.sh proof-full".to_string(),
            expected_files_touched: vec!["src/lib.rs".to_string()],
            allowed_generated_files: Vec::new(),
            primary_metrics: Vec::new(),
            tags: vec!["cc-rs".to_string()],
            capsule_file: temp_dir
                .path()
                .join("run")
                .join("workspace")
                .join("proof-full")
                .join(".quorp")
                .join("challenge-capsule.json"),
            capsule: ChallengeCapsule::default(),
        };
        let evaluation_target_dir = temp_dir.path().join("eval-target");
        let env = challenge_evaluation_env(&metadata, &evaluation_target_dir);

        assert!(
            env.iter().any(|(name, value)| {
                *name == "SDKROOT" && *value == Path::new("/").as_os_str()
            })
        );
        assert!(env.iter().any(|(name, value)| {
            *name == "CARGO_TARGET_DIR" && *value == evaluation_target_dir.as_os_str()
        }));
    }

    #[test]
    fn challenge_capsule_extracts_chrono_owner_and_fast_loop() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root");
        let case_root =
            repo_root.join("benchmark/challenges/rust-swebench-top5/02-chrono-epoch-truncation");
        let challenge =
            resolve_challenge_case(&case_root.join("START_HERE.md"), Some("proof-full"))
                .expect("resolve challenge")
                .expect("challenge case");
        let capsule = compile_challenge_capsule(&challenge, &case_root).expect("capsule");
        assert_eq!(capsule.case_class, "narrow-owner-first");
        assert!(
            capsule
                .owner_files
                .iter()
                .any(|path| path == "src/round.rs")
        );
        assert!(
            capsule
                .fast_loop_commands
                .iter()
                .any(|command| command.contains("round::tests::"))
        );
    }

    #[test]
    fn challenge_capsule_detects_axum_companion_files() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root");
        let case_root =
            repo_root.join("benchmark/challenges/rust-swebench-top5/03-axum-fallback-merge");
        let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
            .expect("resolve challenge")
            .expect("challenge case");
        let capsule = compile_challenge_capsule(&challenge, &case_root).expect("capsule");
        assert_eq!(capsule.case_class, "breadth-heavy-companion");
        assert!(
            capsule
                .companion_files_required
                .iter()
                .any(|path| path == "axum/CHANGELOG.md")
        );
        assert!(
            capsule
                .strong_hints
                .iter()
                .any(|hint| hint.contains("panic strings"))
        );
    }

    #[test]
    fn prepare_challenge_run_restores_capsule_after_reset() {
        let (_temp_dir, case_root) = create_challenge_case_fixture();
        fs::write(
            case_root.join("reset.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\ncondition=\"$1\"\nrm -rf \"workspace/${condition}/.quorp\"\nmkdir -p \"workspace/${condition}\"\n",
        )
        .expect("reset script");
        fs::write(
            case_root.join("evaluate.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n",
        )
        .expect("evaluate script");

        let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
            .expect("resolve challenge")
            .expect("challenge case");
        let result_dir = tempfile::tempdir().expect("result dir");

        let prepared = prepare_challenge_run(result_dir.path(), &challenge).expect("prepare");

        assert!(prepared.challenge_metadata.capsule_file.exists());
        let capsule_json =
            fs::read_to_string(&prepared.challenge_metadata.capsule_file).expect("read capsule");
        assert!(capsule_json.contains("\"owner_files\""));
    }

    #[test]
    fn prepare_challenge_run_uses_flat_warpos_workspace_root() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let case_root = temp_dir.path().join("06-flat-case");
        fs::create_dir_all(case_root.join("src")).expect("src");
        fs::write(
            case_root.join("START_HERE.md"),
            "# Objective\n\nFix the flat challenge.\n",
        )
        .expect("objective");
        fs::write(case_root.join("SUCCESS.md"), "# Success\n").expect("success");
        fs::write(case_root.join("REFERENCE.md"), "# Reference\n").expect("reference");
        fs::write(case_root.join("LOCAL_REPRO.md"), "# Repro\n").expect("repro");
        fs::write(
            case_root.join("Cargo.toml"),
            "[package]\nname = \"flat_case\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("cargo");
        fs::write(
            case_root.join("src").join("lib.rs"),
            "pub fn sample() -> u32 { 1 }\n",
        )
        .expect("lib");
        fs::write(
            case_root.join(".benchmark-root.json"),
            serde_json::json!({
                "suite": "rust-swebench-top5",
                "issue": "06-flat-case",
                "condition": "proof-full",
            })
            .to_string(),
        )
        .expect("marker");
        fs::write(case_root.join("issue.json"), "{}").expect("issue");
        fs::write(
            case_root.join("benchmark.json"),
            serde_json::json!({
                "id": "06-flat-case",
                "title": "Flat challenge",
                "difficulty": "medium",
                "category": "rust",
                "repo_condition": ["proof-full"],
                "objective_file": "START_HERE.md",
                "success_file": "SUCCESS.md",
                "reset_command": "./reset.sh <condition>",
                "evaluate_command": "./evaluate.sh <condition>",
                "estimated_minutes": 1,
                "expected_files_touched": ["src/lib.rs"],
                "primary_metrics": ["total_tokens"],
                "tags": ["rust", "flat"],
            })
            .to_string(),
        )
        .expect("benchmark");
        fs::write(
            case_root.join("evaluate.sh"),
            "#!/usr/bin/env bash\nset -euo pipefail\nexit 0\n",
        )
        .expect("evaluate");

        let challenge = resolve_challenge_case(&case_root.join("START_HERE.md"), None)
            .expect("resolve challenge")
            .expect("challenge case");
        let result_dir = tempfile::tempdir().expect("result dir");

        let prepared = prepare_challenge_run(result_dir.path(), &challenge).expect("prepare");
        let sandbox_root = result_dir.path().join(CHALLENGE_SANDBOX_DIR);

        assert_eq!(prepared.challenge_metadata.workspace_dir, sandbox_root);
        assert_eq!(
            prepared.challenge_metadata.objective_file,
            sandbox_root.join("START_HERE.md")
        );
        assert_eq!(
            prepared.challenge_metadata.success_file,
            sandbox_root.join("SUCCESS.md")
        );
        assert!(sandbox_root.join("reset.sh").exists());
        assert!(result_dir.path().join(".quorp-flat-baseline").exists());
        assert!(prepared.challenge_metadata.capsule_file.exists());
        assert!(!sandbox_root.join("workspace").join("proof-full").exists());
    }

    #[test]
    fn allowed_generated_files_do_not_count_as_widening() {
        assert!(!detect_widening_against_expected(
            &[
                "cargo-dist/src/config.rs".to_string(),
                "cargo-dist/tests/snapshots/demo.snap".to_string(),
            ],
            &["cargo-dist/src/config.rs".to_string()],
            &["cargo-dist/tests/snapshots/demo.snap".to_string()],
        ));
        assert!(detect_widening_against_expected(
            &[
                "cargo-dist/src/config.rs".to_string(),
                "cargo-dist/tests/snapshots/demo.snap".to_string(),
            ],
            &["cargo-dist/src/config.rs".to_string()],
            &[],
        ));
    }

    #[test]
    fn benchmark_objective_includes_context_files() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("proof-full");
        fs::create_dir_all(&workspace).expect("mkdir");
        fs::write(
            workspace.join(".benchmark-root.json"),
            "{\"benchmark\":\"toy\"}",
        )
        .expect("root");
        fs::write(workspace.join("issue.json"), "{\"issue\":\"ISSUE-00\"}").expect("issue");
        fs::write(workspace.join("START_HERE.md"), "read start here").expect("start");
        fs::write(workspace.join("YOU_ARE_HERE.txt"), "toy workspace").expect("you are here");
        let objective = temp_dir.path().join("README.md");
        fs::write(&objective, "Fix the bug.").expect("objective");
        let resolved = ResolvedBenchmark {
            benchmark_root: temp_dir.path().to_path_buf(),
            issue_id: "ISSUE-00".to_string(),
            benchmark_name: "ISSUE-00".to_string(),
            issue_dir: None,
            workspace_source: workspace.clone(),
            objective_source: objective,
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: collect_context_files(&workspace),
            repair_artifacts: Vec::new(),
        };
        let rendered = build_benchmark_objective(&resolved, &workspace, "safe_local", None)
            .expect("objective");
        assert!(rendered.contains("Fix the bug."));
        assert!(rendered.contains("issue.json"));
        assert!(rendered.contains("START_HERE.md"));
    }

    #[test]
    fn benchmark_objective_includes_helper_briefing_when_present() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("proof-full");
        fs::create_dir_all(&workspace).expect("mkdir");
        let objective = temp_dir.path().join("README.md");
        fs::write(&objective, "Fix the bug.").expect("objective");
        let resolved = ResolvedBenchmark {
            benchmark_root: temp_dir.path().to_path_buf(),
            issue_id: "ISSUE-00".to_string(),
            benchmark_name: "ISSUE-00".to_string(),
            issue_dir: None,
            workspace_source: workspace.clone(),
            objective_source: objective,
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: Vec::new(),
            repair_artifacts: Vec::new(),
        };
        let rendered = build_benchmark_objective(
            &resolved,
            &workspace,
            "safe_local",
            Some("{\"summary\":\"look at pricing\"}"),
        )
        .expect("objective");
        assert!(rendered.contains("## Helper Briefing"));
        assert!(rendered.contains("\"summary\":\"look at pricing\""));
    }

    #[test]
    fn load_benchmark_briefing_prefers_case_specific_json_entry() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let briefing_path = temp_dir.path().join("briefings.json");
        fs::write(
            &briefing_path,
            serde_json::json!({
                "default": "{\"summary\":\"default\"}",
                "ISSUE-42": "{\"summary\":\"case-specific\"}"
            })
            .to_string(),
        )
        .expect("write briefing map");
        let briefing =
            load_benchmark_briefing(Some(&briefing_path), "ISSUE-42").expect("load briefing");
        assert_eq!(briefing.as_deref(), Some("{\"summary\":\"case-specific\"}"));
    }

    #[test]
    fn safe_benchmark_model_defaults_to_safe_local_runtime() {
        let model_id = safe_benchmark_model_id().expect("broker default model");
        assert_eq!(model_id, SAFE_LOCAL_BENCHMARK_MODEL_ID);
    }

    #[test]
    fn native_benchmark_defaults_ignore_ambient_model_env() {
        let _guard = test_env_guard();
        let original_model = std::env::var("QUORP_MODEL").ok();
        let original_provider = std::env::var("QUORP_PROVIDER").ok();
        unsafe {
            std::env::set_var("QUORP_MODEL", "ssd_moe/qwen35-35b-a3b");
            std::env::set_var("QUORP_PROVIDER", "local");
        }

        let resolved =
            resolve_benchmark_model_id(BenchmarkExecutor::Native, None).expect("safe model");

        if let Some(value) = original_model {
            unsafe {
                std::env::set_var("QUORP_MODEL", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_MODEL");
            }
        }
        if let Some(value) = original_provider {
            unsafe {
                std::env::set_var("QUORP_PROVIDER", value);
            }
        } else {
            unsafe {
                std::env::remove_var("QUORP_PROVIDER");
            }
        }

        assert_eq!(resolved, SAFE_LOCAL_BENCHMARK_MODEL_ID);
    }

    #[test]
    fn heavy_local_model_requires_explicit_opt_in() {
        let error = ensure_safe_local_model_selection("qwen35-35b-a3b", false)
            .expect_err("heavy model should be rejected without opt-in");
        assert!(error.to_string().contains("--allow-heavy-local-model"));
        ensure_safe_local_model_selection("qwen35-35b-a3b", true).expect("opt-in should pass");
        ensure_safe_local_model_selection("qwen3-coder-30b-a3b", true)
            .expect("coder heavy model should pass with opt-in");
    }

    #[test]
    fn resolved_default_benchmark_model_is_allowed_without_explicit_opt_in() {
        let model_id = safe_benchmark_model_id().expect("default benchmark model");

        assert!(allow_resolved_benchmark_model_without_opt_in(
            None, &model_id, false
        ));
        assert!(!allow_resolved_benchmark_model_without_opt_in(
            Some("ssd_moe/qwen35-27b"),
            &model_id,
            false
        ));
    }

    #[test]
    fn safe_prompt_is_trimmed_under_cap() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("proof-full");
        fs::create_dir_all(workspace.join(".witness")).expect("mkdir");
        fs::write(workspace.join("AGENTS.md"), "read map first\n".repeat(80)).expect("agents");
        fs::write(
            workspace.join("agent-map.json"),
            serde_json::json!({
                "owners": [{"crate": "toy", "paths": ["crates/toy"], "validation": ["cargo test --quiet"]}]
            })
            .to_string(),
        )
        .expect("agent-map");
        fs::write(
            workspace.join("test-map.json"),
            serde_json::json!({
                "crates": [{"crate": "toy", "tests": ["cargo test -p toy-domain --quiet"]}]
            })
            .to_string(),
        )
        .expect("test-map");
        fs::write(
            workspace.join(".witness").join("witness-graph.json"),
            serde_json::json!({"nodes": [{"id": "toy-domain"}], "edges": []}).to_string(),
        )
        .expect("witness");
        let objective = temp_dir.path().join("README.md");
        fs::write(
            &objective,
            "# ISSUE\n\n".to_string() + &"Long brief line.\n".repeat(200),
        )
        .expect("objective");
        let resolved = ResolvedBenchmark {
            benchmark_root: temp_dir.path().to_path_buf(),
            issue_id: "ISSUE-00".to_string(),
            benchmark_name: "ISSUE-00".to_string(),
            issue_dir: None,
            workspace_source: workspace.clone(),
            objective_source: objective,
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: collect_context_files(&workspace),
            repair_artifacts: Vec::new(),
        };
        let rendered = build_benchmark_objective(&resolved, &workspace, "safe_local", None)
            .expect("objective");
        assert!(estimate_token_count(&rendered) <= SAFE_PROMPT_TOKEN_CAP + 64);
    }

    #[test]
    fn trimmed_prompt_rebases_paths_into_attempt_workspace() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let workspace = temp_dir.path().join("proof-full");
        fs::create_dir_all(workspace.join(".witness")).expect("mkdir");
        fs::write(workspace.join("START_HERE.md"), "start here\n".repeat(120)).expect("start");
        fs::write(workspace.join("AGENTS.md"), "guardrails\n".repeat(120)).expect("agents");
        fs::write(
            workspace.join("agent-map.json"),
            serde_json::json!({
                "owners": [{"crate": "toy", "paths": ["crates/toy"], "validation": ["cargo test -p toy-domain --quiet"]}]
            })
            .to_string(),
        )
        .expect("agent-map");
        fs::write(
            workspace.join("test-map.json"),
            serde_json::json!({
                "crates": [{"crate": "toy", "tests": ["cargo test -p toy-domain --quiet"]}]
            })
            .to_string(),
        )
        .expect("test-map");
        fs::write(
            workspace.join(".witness").join("witness-graph.json"),
            serde_json::json!({"nodes": [{"id": "toy-domain"}], "edges": []}).to_string(),
        )
        .expect("witness");

        let objective = workspace.join("README.md");
        fs::write(&objective, "Long brief line.\n".repeat(200)).expect("objective");
        let repair_artifact = workspace.join("repair-notes.md");
        fs::write(&repair_artifact, "repair").expect("repair notes");

        let resolved = ResolvedBenchmark {
            benchmark_root: temp_dir.path().to_path_buf(),
            issue_id: "ISSUE-00".to_string(),
            benchmark_name: "ISSUE-00".to_string(),
            issue_dir: None,
            workspace_source: workspace.clone(),
            objective_source: objective,
            visible_evaluator: None,
            collector_evaluator: None,
            context_files: collect_context_files(&workspace),
            repair_artifacts: vec![repair_artifact.clone()],
        };

        let rendered = build_benchmark_objective(&resolved, &workspace, "safe_local", None)
            .expect("objective");
        assert!(rendered.contains("README.md"));
        assert!(rendered.contains("START_HERE.md"));
    }

    #[test]
    fn benchmark_completion_policy_keeps_repo_capsule_for_safe_mode() {
        let _guard = test_env_guard();
        clear_benchmark_completion_policy_env_overrides();

        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "safe_local",
            Some("ssd_moe/deepseek-coder-v2-lite-turbo"),
        );
        assert!(policy.include_repo_capsule);
        assert_eq!(policy.first_turn_max_completion_tokens, Some(1536));
        assert_eq!(policy.later_turn_max_completion_tokens, Some(2048));
        assert!(!policy.disable_reasoning);
        assert!(policy.native_tool_calls);
        assert_eq!(
            policy
                .watchdog
                .as_ref()
                .and_then(|watchdog| watchdog.total_timeout_ms),
            Some(360_000)
        );
        assert_eq!(
            benchmark_action_contract_mode(&policy),
            "native_tool_calls_v1"
        );
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::BenchmarkStatePacket)
        );
    }

    #[test]
    fn benchmark_completion_policy_applies_action_contract_overrides() {
        let _guard = test_env_guard();
        clear_benchmark_completion_policy_env_overrides();
        unsafe {
            std::env::set_var("QUORP_BENCH_NATIVE_TOOL_CALLS", "false");
            std::env::set_var("QUORP_BENCH_PROMPT_COMPACTION_POLICY", "last6-ledger768");
        }

        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "heavy_local",
            Some("ssd_moe/qwen3-coder-30b-a3b"),
        );
        clear_benchmark_completion_policy_env_overrides();

        assert!(!policy.native_tool_calls);
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
    }

    #[test]
    fn qwen35_27b_benchmark_defaults_use_strict_json_and_disable_reasoning() {
        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "heavy_local",
            Some("ssd_moe/qwen35-27b"),
        );

        assert!(policy.include_repo_capsule);
        assert!(
            policy
                .watchdog
                .as_ref()
                .and_then(|watchdog| watchdog.total_timeout_ms)
                .is_some()
        );
        assert!(policy.disable_reasoning);
        assert!(!policy.native_tool_calls);
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
    }

    #[test]
    fn qwen36_27b_benchmark_defaults_use_strict_json_and_disable_reasoning() {
        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "heavy_local",
            Some("ssd_moe/qwen36-27b"),
        );

        assert!(policy.include_repo_capsule);
        assert!(policy.disable_reasoning);
        assert!(!policy.native_tool_calls);
        assert_eq!(policy.first_turn_max_completion_tokens, Some(3072));
        assert_eq!(policy.later_turn_max_completion_tokens, Some(1536));
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
    }

    #[test]
    fn qwen3_coder_benchmark_defaults_use_strict_json_and_tighter_caps() {
        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "heavy_local",
            Some("ssd_moe/qwen3-coder-30b-a3b"),
        );

        assert!(policy.include_repo_capsule);
        assert!(policy.disable_reasoning);
        assert!(!policy.native_tool_calls);
        assert_eq!(policy.first_turn_max_completion_tokens, Some(4096));
        assert_eq!(policy.later_turn_max_completion_tokens, Some(3072));
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );
        assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
    }

    #[test]
    fn nvidia_qwen_coder_benchmark_defaults_use_strict_json_and_profile_label() {
        let safety_label = benchmark_safety_mode_label(
            BenchmarkExecutor::Native,
            "nvidia/qwen/qwen3-coder-480b-a35b-instruct",
        );
        let policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            &safety_label,
            Some("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
        );

        assert_eq!(safety_label, "nvidia_qwen_benchmark");
        assert!(policy.include_repo_capsule);
        assert!(policy.disable_reasoning);
        assert!(!policy.native_tool_calls);
        assert_eq!(policy.first_turn_max_completion_tokens, Some(4096));
        assert_eq!(policy.later_turn_max_completion_tokens, Some(3072));
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::BenchmarkStatePacket)
        );
        assert_eq!(
            policy.safety_mode_label.as_deref(),
            Some("nvidia_qwen_benchmark")
        );
        assert_eq!(benchmark_action_contract_mode(&policy), "strict_json_v1");
    }

    #[test]
    fn native_batch_skips_local_prewarm_for_nvidia_qwen() {
        assert!(native_batch_model_uses_remote_provider(
            "nvidia/qwen/qwen3-coder-480b-a35b-instruct"
        ));
        assert!(native_batch_model_uses_remote_provider(
            "qwen/qwen3-coder-480b-a35b-instruct"
        ));
        assert!(!native_batch_model_uses_remote_provider(
            "ssd_moe/qwen3-coder-30b-a3b"
        ));
    }

    #[test]
    fn requested_compaction_override_preserves_existing_default_when_absent() {
        let mut policy = benchmark_completion_policy(
            BenchmarkExecutor::Native,
            "heavy_local",
            Some("ssd_moe/qwen35-27b"),
        );

        apply_requested_prompt_compaction_override(&mut policy, None);
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );

        apply_requested_prompt_compaction_override(&mut policy, Some(PromptCompactionPolicy::Off));
        assert_eq!(
            policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Off)
        );
    }

    #[test]
    fn evaluator_requires_structured_success_flag_when_present() {
        assert!(evaluator_passed(true, "{\"success\": true}"));
        assert!(!evaluator_passed(true, "{\"success\": false}"));
        assert!(!evaluator_passed(false, "{\"success\": true}"));
        assert!(evaluator_passed(true, "plain stdout"));
    }

    #[test]
    fn benchmark_lock_refuses_second_holder() {
        let temp_home = tempfile::tempdir().expect("tempdir");
        let lock_path = benchmark_run_lock_path_for_home(temp_home.path());
        let first_lock = BenchmarkRunLock::acquire_at(lock_path.clone()).expect("first lock");
        let second_error =
            BenchmarkRunLock::acquire_at(lock_path).expect_err("second lock must fail");
        assert!(second_error.to_string().contains("benchmark lock"));
        drop(first_lock);
    }

    #[test]
    fn benchmark_run_completes_with_fake_model_server() {
        let _env_guard = test_env_guard();
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_results = tempfile::tempdir().expect("temp results");
        let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }

        let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

        let result = run_benchmark(BenchmarkRunOptions {
            path: issue_dir,
            executor: BenchmarkExecutor::Native,
            model_id: Some("qwen3-coder-30b-a3b".to_string()),
            base_url_override: Some(base_url),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(1_000),
            result_dir: temp_results.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
            max_attempts: Some(1),
            allow_heavy_local_model: true,
            condition: None,
            keep_sandbox: false,
        });

        if let Some(home) = original_home {
            unsafe {
                std::env::set_var("HOME", home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }

        result.expect("benchmark run should complete");
        server_handle.join().expect("join fake model server");

        let report_path = temp_results.path().join("benchmark-report.json");
        let report: BenchmarkReport =
            serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
                .expect("parse benchmark report");
        assert!(report.success, "expected mocked benchmark to succeed");
        assert_eq!(report.attempts_run, 1);
        assert_eq!(report.provider_kind, "local");
        assert_eq!(report.auth_mode, "local_bearer");
        assert_eq!(report.usage_source, "provider_response");
        assert!(!report.proxy_visible_remote_egress_expected);
        assert_eq!(report.requested_provider.as_deref(), Some("local"));
        assert_eq!(
            report.requested_model.as_deref(),
            Some("qwen3-coder-30b-a3b")
        );
        assert_eq!(
            report.effective_model.as_deref(),
            Some("qwen3-coder-30b-a3b")
        );
        assert!(!report.used_local_fallback);
        assert_eq!(
            report.final_stop_reason,
            Some(quorp_agent_core::StopReason::Success)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.visible_evaluation.as_ref())
                .is_some_and(|outcome| outcome.passed)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.collector_evaluation.as_ref())
                .is_some_and(|outcome| outcome.passed)
        );
        assert!(
            report
                .attempts
                .first()
                .map(|attempt| attempt
                    .changed_files
                    .iter()
                    .any(|path| path == "crates/toy-domain/src/lib.rs"))
                .unwrap_or(false)
        );

        let fixed_file = temp_results
            .path()
            .join("attempt-001")
            .join("workspace")
            .join("crates/toy-domain/src/lib.rs");
        let fixed_content = fs::read_to_string(&fixed_file).expect("read fixed file");
        assert!(fixed_content.contains("scheduled_at_period_end"));
    }

    #[test]
    fn benchmark_run_completes_with_fake_safe_local_model_server() {
        let _env_guard = test_env_guard();
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_results = tempfile::tempdir().expect("temp results");
        let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }

        let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

        let result = run_benchmark(BenchmarkRunOptions {
            path: issue_dir,
            executor: BenchmarkExecutor::Native,
            model_id: Some("ssd_moe/deepseek-coder-v2-lite-turbo".to_string()),
            base_url_override: Some(base_url),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(1_000),
            result_dir: temp_results.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
            max_attempts: Some(1),
            allow_heavy_local_model: false,
            condition: None,
            keep_sandbox: false,
        });

        if let Some(home) = original_home {
            unsafe {
                std::env::set_var("HOME", home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }

        result.expect("safe local benchmark run should complete");
        server_handle.join().expect("join fake model server");

        let report_path = temp_results.path().join("benchmark-report.json");
        let report: BenchmarkReport =
            serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
                .expect("parse benchmark report");
        assert!(report.success, "expected mocked benchmark to succeed");
        assert_eq!(report.provider_kind, "local");
        assert_eq!(
            report.requested_model.as_deref(),
            Some("ssd_moe/deepseek-coder-v2-lite-turbo")
        );
        assert_eq!(
            report.effective_model.as_deref(),
            Some("deepseek-coder-v2-lite-turbo")
        );
    }

    #[test]
    fn benchmark_run_records_effective_prompt_compaction_policy_for_verified_27b() {
        let _env_guard = test_env_guard();
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_results = tempfile::tempdir().expect("temp results");
        let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }

        let (base_url, server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_millis(250),
        );

        run_benchmark(BenchmarkRunOptions {
            path: issue_dir,
            executor: BenchmarkExecutor::Native,
            model_id: Some(SAFE_LOCAL_BENCHMARK_MODEL_ID.to_string()),
            base_url_override: Some(base_url),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(1_000),
            result_dir: temp_results.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
            max_attempts: Some(1),
            allow_heavy_local_model: true,
            condition: None,
            keep_sandbox: false,
        })
        .expect("safe local benchmark run should complete");
        server_handle.join().expect("join fake model server");

        if let Some(home) = original_home {
            unsafe {
                std::env::set_var("HOME", home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }

        let manifest_path = temp_results.path().join("benchmark-manifest.json");
        let manifest: BenchmarkManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
                .expect("parse manifest");
        assert_eq!(
            manifest.compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );

        let request_path = temp_results
            .path()
            .join("attempt-001")
            .join("agent")
            .join("request.json");
        let request: quorp_agent_core::AgentRunRequest =
            serde_json::from_str(&fs::read_to_string(&request_path).expect("read request"))
                .expect("parse request");
        assert_eq!(
            request.completion_policy.prompt_compaction_policy,
            Some(PromptCompactionPolicy::Last6Ledger768)
        );

        let turn_request_path = temp_results
            .path()
            .join("attempt-001")
            .join("agent")
            .join("artifacts")
            .join("model_turns")
            .join("request-0001.json");
        let turn_request: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&turn_request_path).expect("read turn request"),
        )
        .expect("parse turn request");
        assert_eq!(
            turn_request["prompt_compaction_policy"].as_str(),
            Some("last6-ledger768")
        );
    }

    #[test]
    fn benchmark_resume_replays_from_checkpoint_with_fake_model_server() {
        let _env_guard = test_env_guard();
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_results = tempfile::tempdir().expect("temp results");
        let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }

        let (initial_base_url, initial_server_handle) = start_fake_completion_server(
            serde_json::json!({
                "assistant_message": "Applying the smallest possible fix.",
                "actions": [
                    {
                        "ApplyPatch": {
                            "path": "crates/toy-domain/src/lib.rs",
                            "patch": "--- a/crates/toy-domain/src/lib.rs\n+++ b/crates/toy-domain/src/lib.rs\n@@ -1,6 +1,6 @@\n pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n     if delayed_change {\n-        \"immediate\"\n+        \"scheduled_at_period_end\"\n     } else {\n         \"immediate\"\n     }\n"
                        }
                    }
                ],
                "task_updates": [],
                "memory_updates": [],
                "requested_mode_change": null,
                "verifier_plan": null
            })
            .to_string(),
            Duration::from_secs(3),
        );

        run_benchmark(BenchmarkRunOptions {
            path: issue_dir.clone(),
            executor: BenchmarkExecutor::Native,
            model_id: Some("qwen3-coder-30b-a3b".to_string()),
            base_url_override: Some(initial_base_url.clone()),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(1_000),
            result_dir: temp_results.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
            max_attempts: Some(1),
            allow_heavy_local_model: true,
            condition: None,
            keep_sandbox: false,
        })
        .expect("initial benchmark run should complete");
        let request_path = temp_results
            .path()
            .join("attempt-001")
            .join("agent")
            .join("request.json");
        let mut request: quorp_agent_core::AgentRunRequest =
            serde_json::from_str(&fs::read_to_string(&request_path).expect("read request"))
                .expect("parse request");
        request.base_url_override = Some(initial_base_url.clone());
        fs::write(
            &request_path,
            serde_json::to_vec_pretty(&request).expect("serialize request"),
        )
        .expect("write request");

        resume_benchmark(BenchmarkResumeOptions {
            result_dir: temp_results.path().to_path_buf(),
        })
        .expect("resume should complete");
        initial_server_handle
            .join()
            .expect("join initial fake model server");

        if let Some(home) = original_home {
            unsafe {
                std::env::set_var("HOME", home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }

        let report_path = temp_results.path().join("benchmark-report.json");
        let report: BenchmarkReport =
            serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
                .expect("parse benchmark report");
        assert!(
            report.success,
            "expected resumed benchmark to remain successful"
        );
        assert_eq!(report.attempts_run, 1);
        assert_eq!(
            report.final_stop_reason,
            Some(quorp_agent_core::StopReason::Success)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.visible_evaluation.as_ref())
                .is_some_and(|outcome| outcome.passed)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.collector_evaluation.as_ref())
                .is_some_and(|outcome| outcome.passed)
        );
    }

    #[test]
    fn benchmark_run_reports_failure_cleanly_with_bad_model_response() {
        let _env_guard = test_env_guard();
        let temp_home = tempfile::tempdir().expect("temp home");
        let temp_results = tempfile::tempdir().expect("temp results");
        let (_fixture_dir, issue_dir) = create_toy_preview_benchmark_fixture();

        let original_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", temp_home.path());
        }

        let (base_url, server_handle) = start_fake_completion_server(
            "{\"assistant_message\":\"oops\"".to_string(),
            Duration::from_secs(5),
        );

        run_benchmark(BenchmarkRunOptions {
            path: issue_dir,
            executor: BenchmarkExecutor::Native,
            model_id: Some("qwen3-coder-30b-a3b".to_string()),
            base_url_override: Some(base_url),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 8,
            max_seconds: Some(120),
            max_total_tokens: Some(1_000),
            result_dir: temp_results.path().to_path_buf(),
            autonomy_profile: quorp_agent_core::AutonomyProfile::AutonomousHost,
            max_attempts: Some(1),
            allow_heavy_local_model: true,
            condition: None,
            keep_sandbox: false,
        })
        .expect("benchmark run should still complete reporting after failure");
        server_handle.join().expect("join bad fake model server");

        if let Some(home) = original_home {
            unsafe {
                std::env::set_var("HOME", home);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }

        let report_path = temp_results.path().join("benchmark-report.json");
        let report: BenchmarkReport =
            serde_json::from_str(&fs::read_to_string(&report_path).expect("read benchmark report"))
                .expect("parse benchmark report");
        assert!(
            !report.success,
            "expected malformed completion to fail the benchmark"
        );
        assert_eq!(
            report.final_stop_reason,
            Some(quorp_agent_core::StopReason::FatalError)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.agent_error_message.as_ref())
                .is_some_and(|message| message.contains("Structured agent turn was invalid JSON"))
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.visible_evaluation.as_ref())
                .is_some_and(|outcome| !outcome.passed)
        );
        assert!(
            report
                .attempts
                .first()
                .and_then(|attempt| attempt.collector_evaluation.as_ref())
                .is_some_and(|outcome| !outcome.passed)
        );
    }

    #[test]
    fn challenge_judge_native_completes_with_safe_local_model_server() {
        let _env_guard = test_env_guard();
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let sandbox_root = temp_dir.path().join("sandbox");
        let workspace_dir = sandbox_root.join("workspace").join("proof-full");
        let attempt_dir = temp_dir.path().join("attempt-001");
        fs::create_dir_all(&workspace_dir).expect("workspace");
        fs::create_dir_all(&attempt_dir).expect("attempt");
        fs::write(workspace_dir.join("START_HERE.md"), "Fix the issue.").expect("objective");
        fs::write(workspace_dir.join("SUCCESS.md"), "Make the evaluator pass.").expect("success");
        fs::write(workspace_dir.join("REFERENCE.md"), "Upstream provenance.").expect("reference");

        let (base_url, server_handle) = start_fake_completion_server(
            r#"{"passed":true,"summary":"looks good","rationale":"the evaluation passed"}"#
                .to_string(),
            Duration::from_millis(250),
        );

        let manifest = BenchmarkManifest {
            resolved: ResolvedBenchmark {
                benchmark_root: sandbox_root.clone(),
                issue_id: "01-safe-judge".to_string(),
                benchmark_name: "Safe judge benchmark".to_string(),
                issue_dir: None,
                workspace_source: workspace_dir.clone(),
                objective_source: workspace_dir.join("START_HERE.md"),
                visible_evaluator: None,
                collector_evaluator: None,
                context_files: vec![workspace_dir.join("REFERENCE.md")],
                repair_artifacts: Vec::new(),
            },
            executor: BenchmarkExecutor::Native,
            model_id: SAFE_LOCAL_BENCHMARK_MODEL_ID.to_string(),
            safety_mode_label: "safe_local".to_string(),
            scenario_label: None,
            base_url_override: Some(base_url),
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 1,
            max_seconds: Some(30),
            max_total_tokens: None,
            autonomy_profile: "autonomous_host".to_string(),
            max_attempts: 1,
            challenge: Some(ChallengeMetadata {
                case_root: sandbox_root.clone(),
                sandbox_root: sandbox_root.clone(),
                workspace_dir: workspace_dir.clone(),
                condition: "proof-full".to_string(),
                objective_file: workspace_dir.join("START_HERE.md"),
                success_file: workspace_dir.join("SUCCESS.md"),
                reference_file: Some(workspace_dir.join("REFERENCE.md")),
                reset_command: "./reset.sh proof-full".to_string(),
                evaluate_command: "./evaluate.sh proof-full".to_string(),
                expected_files_touched: vec!["src/lib.rs".to_string()],
                allowed_generated_files: Vec::new(),
                primary_metrics: vec!["evaluate_passed".to_string()],
                tags: vec!["rust".to_string()],
                capsule_file: workspace_dir.join(".quorp").join("challenge-capsule.json"),
                capsule: ChallengeCapsule::default(),
            }),
            keep_sandbox: true,
            completion_policy: quorp_agent_core::CompletionPolicy::default(),
        };
        let evaluation = EvaluatorOutcome {
            name: "evaluation".to_string(),
            script: sandbox_root.join("evaluate.sh"),
            command: Some("./evaluate.sh proof-full".to_string()),
            duration_ms: 10,
            exit_code: 0,
            passed: true,
            stdout: "{\"success\":true}".to_string(),
            stderr: String::new(),
        };
        let outcome = quorp_agent_core::AgentRunOutcome {
            stop_reason: quorp_agent_core::StopReason::Success,
            total_steps: 1,
            total_billed_tokens: 12,
            duration_ms: 25,
            transcript: Vec::new(),
            error_message: None,
        };
        let metrics = RequestMetricsSummary {
            max_prompt_token_estimate: Some(256),
            max_completion_token_cap: Some(512),
            watchdog_near_limit: false,
            watchdog_triggered: false,
            first_request_prompt_token_estimate: Some(256),
            first_request_raw_prompt_token_estimate: Some(256),
            first_request_compacted_prompt_token_estimate: None,
            first_request_first_token_latency_ms: Some(10),
            first_model_turn_started: true,
            first_action_emitted: false,
            prompt_token_series_by_turn: Vec::new(),
        };
        let usage = crate::quorp::agent_local::HeadlessUsageSummary {
            model_requests: 1,
            reported_billed_tokens: 320,
            estimated_billed_tokens: 320,
            total_billed_tokens: 320,
            input_tokens: 256,
            output_tokens: 64,
            reasoning_tokens: 0,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
        };
        let changed_files = vec!["src/lib.rs".to_string()];
        let validations: Vec<String> = Vec::new();
        let context = ChallengeJudgeContext {
            manifest: &manifest,
            metadata: manifest.challenge.as_ref().expect("challenge metadata"),
            attempt_number: 1,
            attempt_dir: &attempt_dir,
            outcome: &outcome,
            evaluation: &evaluation,
            changed_files: &changed_files,
            validations: &validations,
            metrics: &metrics,
            usage: &usage,
        };

        let judge = run_challenge_judge(&context);
        server_handle.join().expect("join judge model server");

        assert!(judge.passed, "expected judge request to succeed");
        assert_eq!(judge.summary, "looks good");
        assert_eq!(judge.rationale, "the evaluation passed");
    }

    #[test]
    fn judge_transport_failure_does_not_block_deterministic_success() {
        let transport_failure = ChallengeJudgeOutcome {
            passed: false,
            summary: "judge request failed".to_string(),
            rationale: "first token timeout after 30000ms".to_string(),
            model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
            raw_response: serde_json::json!({}),
            error: None,
        };
        assert!(!judge_blocks_deterministic_success(&transport_failure));

        let semantic_failure = ChallengeJudgeOutcome {
            passed: false,
            summary: "patch changed unrelated files".to_string(),
            rationale: "the diff widened beyond the target".to_string(),
            model_id: "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string(),
            raw_response: serde_json::json!({}),
            error: None,
        };
        assert!(judge_blocks_deterministic_success(&semantic_failure));
    }

    #[test]
    fn transient_challenge_judge_errors_are_retryable() {
        assert!(transient_challenge_judge_error(
            "NVIDIA NIM returned 503 Service Unavailable: ResourceExhausted"
        ));
        assert!(transient_challenge_judge_error(
            "first token timeout after 30000ms"
        ));
        assert!(!transient_challenge_judge_error(
            "judge response could not be parsed"
        ));
    }

    fn start_fake_completion_server(
        turn_content: String,
        idle_shutdown_after: Duration,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake model server");
        listener.set_nonblocking(true).expect("set nonblocking");
        let address = listener.local_addr().expect("local addr");
        let base_url = format!("http://{address}/v1");
        let stream_response_body = serde_json::json!({
            "id": "chatcmpl-fake",
            "choices": [
                {
                    "index": 0,
                    "delta": { "content": turn_content },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 24,
                "total_tokens": 66
            }
        })
        .to_string();
        let json_response_body = serde_json::json!({
            "id": "chatcmpl-fake",
            "choices": [
                {
                    "index": 0,
                    "message": { "content": turn_content },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 24,
                "total_tokens": 66
            }
        })
        .to_string();
        let stream_body = format!("data: {stream_response_body}\n\ndata: [DONE]\n\n");
        let handle = thread::spawn(move || {
            let mut served_requests = 0usize;
            let mut last_request_at = Instant::now();
            let no_request_shutdown_after = Duration::from_secs(60);
            loop {
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                            log::trace!("fake model server set read timeout failed: {error}");
                        }
                        let mut request_bytes = Vec::new();
                        loop {
                            let mut buffer = [0u8; 8192];
                            match stream.read(&mut buffer) {
                                Ok(0) => break,
                                Ok(bytes_read) => {
                                    request_bytes.extend_from_slice(&buffer[..bytes_read]);
                                    if expected_http_request_len(&request_bytes).is_some_and(
                                        |expected_len| request_bytes.len() >= expected_len,
                                    ) {
                                        break;
                                    }
                                }
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    break;
                                }
                                Err(error) => {
                                    log::trace!("fake model server read failed: {error}");
                                    break;
                                }
                            }
                        }
                        let request_text = String::from_utf8_lossy(&request_bytes);
                        let (content_type, body) = if request_text.contains("\"stream\":false") {
                            ("application/json", json_response_body.as_str())
                        } else {
                            ("text/event-stream", stream_body.as_str())
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        if let Err(error) = stream.write_all(response.as_bytes()) {
                            log::trace!("fake model server write failed: {error}");
                        }
                        if let Err(error) = stream.flush() {
                            log::trace!("fake model server flush failed: {error}");
                        }
                        served_requests += 1;
                        last_request_at = Instant::now();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if served_requests > 0 && last_request_at.elapsed() >= idle_shutdown_after {
                            break;
                        }
                        if last_request_at.elapsed() >= no_request_shutdown_after {
                            break;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });
        (base_url, handle)
    }

    fn expected_http_request_len(request_bytes: &[u8]) -> Option<usize> {
        let header_end = request_bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")?;
        let headers = std::str::from_utf8(&request_bytes[..header_end]).ok()?;
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        Some(header_end + 4 + content_length)
    }

    #[test]
    fn docker_resume_normalizes_manifest_paths() {
        let _guard = test_env_guard();
        unsafe {
            std::env::set_var("QUORP_IN_DOCKER", "1");
            std::env::set_var("QUORP_DOCKER_HOST_RESULT_DIR", "/host/results");
            std::env::set_var("QUORP_DOCKER_HOST_WORKSPACE_ROOT", "/host/source");
            std::env::set_var("QUORP_DOCKER_CONTAINER_WORKSPACE_ROOT", "/workspace");
        }

        let mut manifest = BenchmarkManifest {
            resolved: ResolvedBenchmark {
                benchmark_root: PathBuf::from("/host/source/bench"),
                issue_id: "ISSUE-1".to_string(),
                benchmark_name: "sample".to_string(),
                issue_dir: None,
                workspace_source: PathBuf::from("/host/source/workspace"),
                objective_source: PathBuf::from("/host/source/README.md"),
                visible_evaluator: Some(PathBuf::from("/host/source/evaluate_visible.sh")),
                collector_evaluator: None,
                context_files: vec![PathBuf::from("/host/source/AGENTS.md")],
                repair_artifacts: vec![PathBuf::from("/host/results/sandbox/fix.json")],
            },
            executor: BenchmarkExecutor::Native,
            model_id: "model".to_string(),
            safety_mode_label: "safe".to_string(),
            scenario_label: None,
            base_url_override: None,
            briefing_file: None,
            compaction_policy: None,
            seed_transcript: None,
            max_steps: 5,
            max_seconds: Some(60),
            max_total_tokens: None,
            autonomy_profile: "autonomous_host".to_string(),
            max_attempts: 1,
            challenge: Some(ChallengeMetadata {
                case_root: PathBuf::from("/host/cases/01"),
                sandbox_root: PathBuf::from("/host/results/sandbox"),
                workspace_dir: PathBuf::from("/host/results/sandbox/workspace/proof-full"),
                condition: "proof-full".to_string(),
                objective_file: PathBuf::from("/host/results/sandbox/QUORP_CHALLENGE_OBJECTIVE.md"),
                success_file: PathBuf::from("/host/results/sandbox/expected/success.md"),
                reference_file: Some(PathBuf::from(
                    "/host/results/sandbox/workspace/proof-full/REFERENCE.md",
                )),
                reset_command: "./reset.sh proof-full".to_string(),
                evaluate_command: "./evaluate.sh proof-full".to_string(),
                expected_files_touched: Vec::new(),
                allowed_generated_files: Vec::new(),
                primary_metrics: Vec::new(),
                tags: Vec::new(),
                capsule_file: PathBuf::from(
                    "/host/results/sandbox/workspace/proof-full/.quorp/challenge-capsule.json",
                ),
                capsule: ChallengeCapsule::default(),
            }),
            keep_sandbox: true,
            completion_policy: quorp_agent_core::CompletionPolicy::default(),
        };

        normalize_manifest_paths_for_runtime(&mut manifest, Path::new("/quorp-results"));

        assert_eq!(
            manifest.resolved.benchmark_root,
            PathBuf::from("/workspace/bench")
        );
        assert_eq!(
            manifest.resolved.workspace_source,
            PathBuf::from("/workspace/workspace")
        );
        assert_eq!(
            manifest.resolved.objective_source,
            PathBuf::from("/workspace/README.md")
        );
        assert_eq!(
            manifest.resolved.visible_evaluator,
            Some(PathBuf::from("/workspace/evaluate_visible.sh"))
        );
        assert_eq!(
            manifest.resolved.repair_artifacts,
            vec![PathBuf::from("/quorp-results/sandbox/fix.json")]
        );
        let challenge = manifest.challenge.expect("challenge");
        assert_eq!(
            challenge.sandbox_root,
            PathBuf::from("/quorp-results/sandbox")
        );
        assert_eq!(
            challenge.workspace_dir,
            PathBuf::from("/quorp-results/sandbox/workspace/proof-full")
        );
        assert_eq!(
            challenge.objective_file,
            PathBuf::from("/quorp-results/sandbox/QUORP_CHALLENGE_OBJECTIVE.md")
        );

        unsafe {
            std::env::remove_var("QUORP_IN_DOCKER");
            std::env::remove_var("QUORP_DOCKER_HOST_RESULT_DIR");
            std::env::remove_var("QUORP_DOCKER_HOST_WORKSPACE_ROOT");
            std::env::remove_var("QUORP_DOCKER_CONTAINER_WORKSPACE_ROOT");
        }
    }

    #[test]
    fn classify_failure_labels_repair_loop_stalled_from_agent_error() {
        let report: BenchmarkReport = serde_json::from_value(serde_json::json!({
            "benchmark_name": "Example",
            "issue_id": "example",
            "success": false,
            "attempts_run": 1,
            "max_attempts": 1,
            "total_billed_tokens": 0,
            "final_stop_reason": "stalled",
            "changed_files": [],
            "widening_happened": false,
            "attempts": [{
                "attempt": 1,
                "executor": "native",
                "model_id": "ssd_moe/qwen36-27b",
                "safety_mode_label": "safe",
                "scenario_label": null,
                "agent_stop_reason": "stalled",
                "agent_error_message": "Autonomous repair loop stalled because the model kept responding without a concrete repair action.",
                "total_steps": 3,
                "total_billed_tokens": 0,
                "changed_files": [],
                "validations": [],
                "widening_happened": false,
                "attempt_dir": "/tmp/attempt",
                "workspace_dir": "/tmp/workspace",
                "agent_result_dir": "/tmp/agent"
            }]
        }))
        .expect("report");

        assert_eq!(
            classify_primary_failure(&report).as_deref(),
            Some("repair_loop_stalled")
        );
        assert_eq!(
            classify_local_agent_failure(&report, Some("repair_loop_stalled")).as_deref(),
            Some("repair_loop_stalled")
        );
    }
}
