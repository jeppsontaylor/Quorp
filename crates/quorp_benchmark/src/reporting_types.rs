use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::ValueEnum;
use quorp_agent_core::{AgentRepairMemory, AgentRepairScorecard, FailedEditRecord, StopReason};
use serde::{Deserialize, Serialize};

use crate::{ChallengeMetadata, EvaluatorOutcome};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkExecutor {
    #[default]
    Native,
}

impl BenchmarkExecutor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingSummary {
    #[serde(default)]
    pub scenario_label: Option<String>,
    #[serde(default)]
    pub routing_mode: Option<String>,
    #[serde(default)]
    pub requested_provider: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub candidate_models: Vec<String>,
    #[serde(default)]
    pub effective_provider: Option<String>,
    #[serde(default)]
    pub effective_model: Option<String>,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    #[serde(default)]
    pub comparable: Option<bool>,
    #[serde(default)]
    pub provider_base_url: Option<String>,
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(default)]
    pub proxy_visible_remote_egress_expected: bool,
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub routing_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeJudgeOutcome {
    pub passed: bool,
    pub summary: String,
    pub rationale: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub raw_response: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokenTurnSample {
    pub step: usize,
    pub prompt_token_estimate: u64,
    #[serde(default)]
    pub raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub completion_token_cap: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadRangeObservation {
    pub path: String,
    #[serde(default)]
    pub requested_range: Option<String>,
    #[serde(default)]
    pub honored_range: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptReport {
    pub attempt: usize,
    #[serde(default)]
    pub executor: BenchmarkExecutor,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub safety_mode_label: String,
    #[serde(default)]
    pub scenario_label: Option<String>,
    pub agent_stop_reason: StopReason,
    pub agent_error_message: Option<String>,
    pub total_steps: usize,
    #[serde(default)]
    pub duration_ms: u64,
    pub total_billed_tokens: u64,
    #[serde(default)]
    pub max_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub max_completion_token_cap: Option<u32>,
    #[serde(default)]
    pub watchdog_near_limit: bool,
    #[serde(default)]
    pub watchdog_triggered: bool,
    pub visible_evaluation: Option<EvaluatorOutcome>,
    pub collector_evaluation: Option<EvaluatorOutcome>,
    pub evaluation: Option<EvaluatorOutcome>,
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub ignored_changed_files: Vec<String>,
    pub validations: Vec<String>,
    pub widening_happened: bool,
    pub attempt_dir: PathBuf,
    pub workspace_dir: PathBuf,
    pub agent_result_dir: PathBuf,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub model_requests: usize,
    #[serde(default)]
    pub first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    pub first_model_turn_started: bool,
    #[serde(default)]
    pub first_action_emitted: bool,
    #[serde(default)]
    pub prompt_token_series_by_turn: Vec<PromptTokenTurnSample>,
    #[serde(default)]
    pub read_range_observations: Vec<ReadRangeObservation>,
    #[serde(default)]
    pub read_count: usize,
    #[serde(default)]
    pub write_count: usize,
    #[serde(default)]
    pub command_execution_count: usize,
    #[serde(default)]
    pub parser_recovery_count: usize,
    #[serde(default)]
    pub repair_invalid_action_streak_max: usize,
    #[serde(default)]
    pub repair_submode_entered: bool,
    #[serde(default)]
    pub repair_submode_turns: usize,
    #[serde(default)]
    pub repair_write_locked: bool,
    #[serde(default)]
    pub write_phase_action_refusal_count: usize,
    #[serde(default)]
    pub patch_scaffold_offered: bool,
    #[serde(default)]
    pub patch_scaffold_honored: bool,
    #[serde(default)]
    pub preview_apply_locked: bool,
    #[serde(default)]
    pub preview_apply_action_refusal_count: usize,
    #[serde(default)]
    pub write_phase_write_emitted: bool,
    #[serde(default)]
    pub bootstrap_phase: Option<String>,
    #[serde(default)]
    pub bootstrap_phase_detail: Option<String>,
    #[serde(default)]
    pub first_task_model_request_seen: bool,
    #[serde(default)]
    pub bootstrap_elapsed_ms_before_first_task_request: Option<u64>,
    #[serde(default)]
    pub pre_model_bootstrap_stalled: bool,
    #[serde(default)]
    pub bootstrap_stall_class: Option<String>,
    #[serde(default)]
    pub rolled_back_write_count: usize,
    #[serde(default)]
    pub rolled_back_non_support_edit_count: usize,
    #[serde(default)]
    pub soft_budget_inefficient: bool,
    #[serde(default)]
    pub fast_loop_command_seen: bool,
    #[serde(default)]
    pub agent_final_evaluate_command_seen: bool,
    #[serde(default)]
    pub final_evaluate_command_seen: bool,
    #[serde(default)]
    pub host_evaluation_commands_run: usize,
    #[serde(default)]
    pub non_support_edit_count: usize,
    #[serde(default)]
    pub repo_capsule_injected: bool,
    #[serde(default)]
    pub reasoning_enabled: bool,
    #[serde(default)]
    pub path_resolution_failures: usize,
    #[serde(default)]
    pub recovery_turns: usize,
    #[serde(default)]
    pub action_contract_mode: String,
    #[serde(default)]
    pub action_contract_selected: String,
    #[serde(default)]
    pub action_contract_fallback_reason: Option<String>,
    #[serde(default)]
    pub attempt_lineage: Vec<String>,
    #[serde(default)]
    pub effective_prompt_compaction_policy: Option<String>,
    #[serde(default)]
    pub fast_loop_validation_status: Option<String>,
    #[serde(default)]
    pub last_validation_failure: Option<String>,
    #[serde(default)]
    pub failing_test_names: Vec<String>,
    #[serde(default)]
    pub primary_failure_test_name: Option<String>,
    #[serde(default)]
    pub primary_failure_path: Option<String>,
    #[serde(default)]
    pub primary_failure_line: Option<usize>,
    #[serde(default)]
    pub assertion_excerpt: Option<String>,
    #[serde(default)]
    pub diagnostic_class: Option<String>,
    #[serde(default)]
    pub implementation_target_lease: Option<String>,
    #[serde(default)]
    pub dependency_candidates: Vec<String>,
    #[serde(default)]
    pub target_dependency_table: Option<String>,
    #[serde(default)]
    pub repair_required: bool,
    #[serde(default)]
    pub repair_phase_terminal: Option<String>,
    #[serde(default)]
    pub failure_anchor_reread_attempted: bool,
    #[serde(default)]
    pub failure_anchor_reread_honored: bool,
    #[serde(default)]
    pub implementation_reread_allowed: bool,
    #[serde(default)]
    pub implementation_reread_attempted: bool,
    #[serde(default)]
    pub implementation_reread_honored: bool,
    #[serde(default)]
    pub repair_phase_invalid_action_count: usize,
    #[serde(default)]
    pub post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    pub post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    pub full_validation_before_fast_loop: bool,
    #[serde(default)]
    pub prose_only_recovery_count: usize,
    #[serde(default)]
    pub bare_replace_block_retry_count: usize,
    #[serde(default)]
    pub patch_packet_injected: bool,
    #[serde(default)]
    pub patch_packet_honored_range: Option<String>,
    #[serde(default)]
    pub recommended_rerun_command: Option<String>,
    #[serde(default)]
    pub fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    pub failed_edit_records: Vec<FailedEditRecord>,
    #[serde(default)]
    pub agent_repair_memory: AgentRepairMemory,
    #[serde(default)]
    pub agent_repair_scorecard: AgentRepairScorecard,
    #[serde(default)]
    pub preview_edit_count: usize,
    #[serde(default)]
    pub preview_edit_success_count: usize,
    #[serde(default)]
    pub preview_created_count: usize,
    #[serde(default)]
    pub replace_range_count: usize,
    #[serde(default)]
    pub replace_range_hash_mismatch_count: usize,
    #[serde(default)]
    pub modify_toml_count: usize,
    #[serde(default)]
    pub apply_preview_count: usize,
    #[serde(default)]
    pub apply_preview_hash_mismatch_count: usize,
    #[serde(default)]
    pub syntax_preview_count: usize,
    #[serde(default)]
    pub syntax_preview_failure_count: usize,
    #[serde(default)]
    pub target_redirect_count: usize,
    #[serde(default)]
    pub evidence_file_fixation_count: usize,
    #[serde(default)]
    pub agent_final_failure_classification: Option<String>,
    #[serde(default)]
    pub planner_model: Option<String>,
    #[serde(default)]
    pub executor_model: Option<String>,
    #[serde(default)]
    pub deterministic_evaluation_passed: Option<bool>,
    #[serde(default)]
    pub judge: Option<ChallengeJudgeOutcome>,
    #[serde(default)]
    pub primary_failure: Option<String>,
    #[serde(default)]
    pub routing: RoutingSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub benchmark_name: String,
    pub issue_id: String,
    #[serde(default)]
    pub executor: BenchmarkExecutor,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub safety_mode_label: String,
    #[serde(default)]
    pub scenario_label: Option<String>,
    #[serde(default)]
    pub provider_kind: String,
    #[serde(default)]
    pub provider_base_url: Option<String>,
    #[serde(default)]
    pub auth_mode: String,
    #[serde(default)]
    pub usage_source: String,
    #[serde(default)]
    pub proxy_visible_remote_egress_expected: bool,
    #[serde(default)]
    pub routing_mode: Option<String>,
    #[serde(default)]
    pub requested_provider: Option<String>,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub candidate_models: Vec<String>,
    #[serde(default)]
    pub effective_provider: Option<String>,
    #[serde(default)]
    pub effective_model: Option<String>,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    #[serde(default)]
    pub comparable_run: Option<bool>,
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub routing_status: Option<String>,
    pub success: bool,
    pub attempts_run: usize,
    pub max_attempts: usize,
    pub total_billed_tokens: u64,
    #[serde(default)]
    pub wall_clock_ms: u64,
    pub max_total_tokens: Option<u64>,
    #[serde(default)]
    pub max_prompt_token_estimate_seen: Option<u64>,
    #[serde(default)]
    pub max_completion_token_cap_seen: Option<u32>,
    #[serde(default)]
    pub watchdog_near_limit: bool,
    #[serde(default)]
    pub watchdog_triggered: bool,
    pub final_stop_reason: Option<StopReason>,
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub ignored_changed_files: Vec<String>,
    pub widening_happened: bool,
    pub attempts: Vec<AttemptReport>,
    #[serde(default)]
    pub reset_outcome: Option<EvaluatorOutcome>,
    #[serde(default)]
    pub challenge: Option<ChallengeMetadata>,
    #[serde(default)]
    pub run_dir: PathBuf,
    #[serde(default)]
    pub sandbox_root: Option<PathBuf>,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub lines_added: u64,
    #[serde(default)]
    pub lines_removed: u64,
    #[serde(default)]
    pub mistakes_corrected: usize,
    #[serde(default)]
    pub validation_commands_run: usize,
    #[serde(default)]
    pub evaluation_commands_run: usize,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub run_error: Option<String>,
    #[serde(default)]
    pub setup_failure_class: Option<String>,
    #[serde(default)]
    pub total_requests: usize,
    #[serde(default)]
    pub task_model_call_count: usize,
    #[serde(default)]
    pub tool_call_count: usize,
    #[serde(default)]
    pub edit_count: usize,
    #[serde(default)]
    pub read_count: usize,
    #[serde(default)]
    pub write_count: usize,
    #[serde(default)]
    pub command_execution_count: usize,
    #[serde(default)]
    pub parser_recovery_count: usize,
    #[serde(default)]
    pub repair_invalid_action_streak_max: usize,
    #[serde(default)]
    pub repair_submode_entered: bool,
    #[serde(default)]
    pub repair_submode_turns: usize,
    #[serde(default)]
    pub repair_write_locked: bool,
    #[serde(default)]
    pub write_phase_action_refusal_count: usize,
    #[serde(default)]
    pub patch_scaffold_offered: bool,
    #[serde(default)]
    pub patch_scaffold_honored: bool,
    #[serde(default)]
    pub preview_apply_locked: bool,
    #[serde(default)]
    pub preview_apply_action_refusal_count: usize,
    #[serde(default)]
    pub write_phase_write_emitted: bool,
    #[serde(default)]
    pub bootstrap_phase: Option<String>,
    #[serde(default)]
    pub bootstrap_phase_detail: Option<String>,
    #[serde(default)]
    pub first_task_model_request_seen: bool,
    #[serde(default)]
    pub bootstrap_elapsed_ms_before_first_task_request: Option<u64>,
    #[serde(default)]
    pub pre_model_bootstrap_stalled: bool,
    #[serde(default)]
    pub bootstrap_stall_class: Option<String>,
    #[serde(default)]
    pub rolled_back_write_count: usize,
    #[serde(default)]
    pub rolled_back_non_support_edit_count: usize,
    #[serde(default)]
    pub soft_budget_inefficient: bool,
    #[serde(default)]
    pub fast_loop_command_seen: bool,
    #[serde(default)]
    pub agent_final_evaluate_command_seen: bool,
    #[serde(default)]
    pub final_evaluate_command_seen: bool,
    #[serde(default)]
    pub host_evaluation_commands_run: usize,
    #[serde(default)]
    pub non_support_edit_count: usize,
    #[serde(default)]
    pub last_failure_class: Option<String>,
    #[serde(default)]
    pub evaluation_command_seen: bool,
    #[serde(default)]
    pub text_only_action_failure: bool,
    #[serde(default)]
    pub first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_compacted_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    pub first_model_turn_started: bool,
    #[serde(default)]
    pub first_action_emitted: bool,
    #[serde(default)]
    pub prompt_token_series_by_turn: Vec<PromptTokenTurnSample>,
    #[serde(default)]
    pub read_range_observations: Vec<ReadRangeObservation>,
    #[serde(default)]
    pub repo_capsule_injected: bool,
    #[serde(default)]
    pub reasoning_enabled: bool,
    #[serde(default)]
    pub path_resolution_failures: usize,
    #[serde(default)]
    pub recovery_turns: usize,
    #[serde(default)]
    pub action_contract_mode: String,
    #[serde(default)]
    pub action_contract_selected: String,
    #[serde(default)]
    pub action_contract_fallback_reason: Option<String>,
    #[serde(default)]
    pub attempt_lineage: Vec<String>,
    #[serde(default)]
    pub effective_prompt_compaction_policy: Option<String>,
    #[serde(default)]
    pub fast_loop_validation_status: Option<String>,
    #[serde(default)]
    pub last_validation_failure: Option<String>,
    #[serde(default)]
    pub failing_test_names: Vec<String>,
    #[serde(default)]
    pub primary_failure_test_name: Option<String>,
    #[serde(default)]
    pub primary_failure_path: Option<String>,
    #[serde(default)]
    pub primary_failure_line: Option<usize>,
    #[serde(default)]
    pub assertion_excerpt: Option<String>,
    #[serde(default)]
    pub diagnostic_class: Option<String>,
    #[serde(default)]
    pub implementation_target_lease: Option<String>,
    #[serde(default)]
    pub dependency_candidates: Vec<String>,
    #[serde(default)]
    pub target_dependency_table: Option<String>,
    #[serde(default)]
    pub repair_required: bool,
    #[serde(default)]
    pub repair_phase_terminal: Option<String>,
    #[serde(default)]
    pub failure_anchor_reread_attempted: bool,
    #[serde(default)]
    pub failure_anchor_reread_honored: bool,
    #[serde(default)]
    pub implementation_reread_allowed: bool,
    #[serde(default)]
    pub implementation_reread_attempted: bool,
    #[serde(default)]
    pub implementation_reread_honored: bool,
    #[serde(default)]
    pub repair_phase_invalid_action_count: usize,
    #[serde(default)]
    pub post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    pub post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    pub full_validation_before_fast_loop: bool,
    #[serde(default)]
    pub prose_only_recovery_count: usize,
    #[serde(default)]
    pub bare_replace_block_retry_count: usize,
    #[serde(default)]
    pub patch_packet_injected: bool,
    #[serde(default)]
    pub patch_packet_honored_range: Option<String>,
    #[serde(default)]
    pub recommended_rerun_command: Option<String>,
    #[serde(default)]
    pub fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    pub failed_edit_records: Vec<FailedEditRecord>,
    #[serde(default)]
    pub agent_repair_memory: AgentRepairMemory,
    #[serde(default)]
    pub agent_repair_scorecard: AgentRepairScorecard,
    #[serde(default)]
    pub preview_edit_count: usize,
    #[serde(default)]
    pub preview_edit_success_count: usize,
    #[serde(default)]
    pub preview_created_count: usize,
    #[serde(default)]
    pub replace_range_count: usize,
    #[serde(default)]
    pub replace_range_hash_mismatch_count: usize,
    #[serde(default)]
    pub modify_toml_count: usize,
    #[serde(default)]
    pub apply_preview_count: usize,
    #[serde(default)]
    pub apply_preview_hash_mismatch_count: usize,
    #[serde(default)]
    pub syntax_preview_count: usize,
    #[serde(default)]
    pub syntax_preview_failure_count: usize,
    #[serde(default)]
    pub target_redirect_count: usize,
    #[serde(default)]
    pub evidence_file_fixation_count: usize,
    #[serde(default)]
    pub agent_final_failure_classification: Option<String>,
    #[serde(default)]
    pub planner_model: Option<String>,
    #[serde(default)]
    pub executor_model: Option<String>,
    #[serde(default)]
    pub deterministic_evaluation_passed: Option<bool>,
    #[serde(default)]
    pub judge: Option<ChallengeJudgeOutcome>,
    #[serde(default)]
    pub primary_failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCaseReport {
    pub case_id: String,
    pub case_root: PathBuf,
    pub objective_path: PathBuf,
    pub result_dir: PathBuf,
    pub log_file: PathBuf,
    #[serde(default)]
    pub executor: BenchmarkExecutor,
    pub success: bool,
    pub exit_code: i32,
    pub wall_clock_ms: u64,
    pub total_requests: usize,
    pub total_billed_tokens: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub mistakes_corrected: usize,
    pub judge_passed: Option<bool>,
    pub deterministic_evaluation_passed: Option<bool>,
    pub first_request_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_raw_prompt_token_estimate: Option<u64>,
    #[serde(default)]
    pub first_request_compacted_prompt_token_estimate: Option<u64>,
    pub first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    pub first_model_turn_started: bool,
    #[serde(default)]
    pub first_action_emitted: bool,
    pub final_stop_reason: Option<StopReason>,
    pub primary_failure: Option<String>,
    #[serde(default)]
    pub agent_final_failure_classification: Option<String>,
    #[serde(default)]
    pub adaptive_action_mode_retry: bool,
    pub report_path: PathBuf,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReport {
    pub cases_root: PathBuf,
    pub result_dir: PathBuf,
    pub cases: Vec<BatchCaseReport>,
    pub total_requests: usize,
    pub total_billed_tokens: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub mistakes_corrected: usize,
    pub successful_cases: usize,
    pub failed_cases: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummaryCase {
    pub case_id: String,
    pub success: bool,
    pub primary_failure: Option<String>,
    pub agent_final_failure_classification: Option<String>,
    pub final_stop_reason: Option<StopReason>,
    pub first_valid_write_step: Option<usize>,
    pub parser_recovery_count: usize,
    pub redundant_read_count: usize,
    pub rejected_validation_alias_count: usize,
    pub target_redirect_count: usize,
    pub syntax_preview_failure_count: usize,
    pub adaptive_action_mode_retry: bool,
    pub report_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub cases_root: PathBuf,
    pub result_dir: PathBuf,
    pub cases_run: usize,
    pub successful_cases: usize,
    pub failed_cases: usize,
    pub total_requests: usize,
    pub total_billed_tokens: u64,
    pub cases: Vec<RunSummaryCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkScoreReport {
    pub suite: String,
    pub generated_at_unix_seconds: u64,
    pub output_dir: PathBuf,
    pub run_dirs: Vec<PathBuf>,
    pub total_cases: usize,
    pub solved_cases: usize,
    pub valid_write_cases: usize,
    pub post_write_validation_cases: usize,
    pub diagnostic_classified_cases: usize,
    pub tooling_healthy_cases: usize,
    #[serde(default)]
    pub success_rate_ppm: u32,
    #[serde(default)]
    pub secure_success_cases: usize,
    #[serde(default)]
    pub secure_success_rate_ppm: u32,
    pub total_requests: usize,
    pub total_billed_tokens: u64,
    #[serde(default)]
    pub secure_etts_tokens: u64,
    #[serde(default)]
    pub total_wall_clock_ms: u64,
    #[serde(default)]
    pub median_wall_clock_ms: u64,
    #[serde(default)]
    pub total_patch_lines_changed: u64,
    #[serde(default)]
    pub total_retries: usize,
    #[serde(default)]
    pub proof_lane_counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub slow_first_token_cases: usize,
    #[serde(default)]
    pub watchdog_near_limit_cases: usize,
    #[serde(default)]
    pub patch_quality_risk_cases: usize,
    pub common_blocker: Option<String>,
    pub blocker_counts: BTreeMap<String, usize>,
    pub regressions: Vec<String>,
    pub cases: Vec<BenchmarkScoreCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkScoreCase {
    pub case_id: String,
    pub success: bool,
    #[serde(default)]
    pub secure_success: bool,
    pub progress_score: u8,
    pub progress_phase: String,
    pub failure_classification: String,
    pub primary_failure: Option<String>,
    pub model_id: Option<String>,
    pub executor: Option<String>,
    pub provider_base_url: Option<String>,
    pub action_contract_selected: Option<String>,
    pub result_dir: PathBuf,
    pub report_path: PathBuf,
    pub first_model_turn_started: bool,
    pub first_action_emitted: bool,
    pub diagnostic_class: Option<String>,
    pub implementation_target_lease: Option<String>,
    pub first_valid_write_step: Option<usize>,
    pub post_write_validation: bool,
    pub parser_recovery_count: usize,
    pub redundant_read_count: usize,
    pub rejected_validation_alias_count: usize,
    pub target_redirect_count: usize,
    pub syntax_preview_failure_count: usize,
    pub preview_created_count: usize,
    pub modify_toml_count: usize,
    pub replace_range_count: usize,
    pub apply_preview_count: usize,
    pub wall_clock_ms: u64,
    #[serde(default)]
    pub secure_etts_tokens: u64,
    #[serde(default)]
    pub memory_peak_mb: Option<u64>,
    #[serde(default)]
    pub patch_lines_changed: u64,
    #[serde(default)]
    pub retry_count: usize,
    #[serde(default)]
    pub proof_lanes: Vec<String>,
    #[serde(default)]
    pub first_request_first_token_latency_ms: Option<u64>,
    #[serde(default)]
    pub watchdog_near_limit: bool,
    #[serde(default)]
    pub patch_quality_risk: Option<String>,
    pub total_requests: usize,
    pub total_billed_tokens: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub general_tooling_gap: Option<String>,
}
