//! Benchmark report writers — write_report, write_synthetic_failure_report,
//! write_benchmark_proof_receipt.
//!
//! Carved out of `engine.rs` so each child of `benchmark` stays under
//! the 2,000-LOC hard cap.

#![allow(
    clippy::collapsible_match,
    clippy::disallowed_methods,
    clippy::manual_contains,
    clippy::too_many_arguments,
    dead_code,
    unused_imports
)]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;
use crate::quorp::agent_runner::{HeadlessRunOptions, resume_headless_agent, run_headless_agent};
use crate::quorp::tui::chat_service::{
    ChatServiceMessage, ChatServiceRole, StreamRequest, request_single_completion_details,
};
use quorp_agent_core::{PromptCompactionPolicy, TranscriptMessage, TranscriptRole};
use quorp_benchmark::{
    AttemptReport, BatchCaseReport, BenchmarkReport, ChallengeCapsule, ChallengeJudgeOutcome,
    ChallengeManifest, ChallengeMetadata, EvaluatorOutcome, PromptTokenTurnSample,
    ReadRangeObservation, ResolvedBenchmark, ResolvedChallengeCase, RoutingSummary,
    challenge_evaluation_env, challenge_evaluation_target_dir, copy_dir_all, ensure_git_baseline,
    prepare_challenge_run as prepare_benchmark_challenge_run, rebase_attempt_path,
    render_batch_report, render_report_markdown, render_run_summary,
    reset_challenge_workspace_for_attempt as reset_benchmark_challenge_workspace_for_attempt,
    resolve_benchmark, resolve_challenge_case, run_collector_evaluator, run_shell_command_with_env,
    run_visible_evaluator, substitute_condition, summarize_batch_report, summarize_markdown_brief,
    summarize_run_report, summarize_workspace_root,
};
use quorp_core::{ProofReceipt, RawArtifact, ValidationRecord};
pub(crate) fn write_report(
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
                        .agent_repair_scorecard
                        .preview_edit_count
                        .saturating_add(attempt.agent_repair_scorecard.replace_range_count)
                        .saturating_add(attempt.agent_repair_scorecard.modify_toml_count)
                        .saturating_add(attempt.agent_repair_scorecard.apply_preview_count)
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
    let mut routing_summary = RoutingSummary::default();
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
        routing_summary.used_fallback |= attempt.routing.used_fallback;
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
        used_fallback: routing_summary.used_fallback,
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
        full_validation_before_fast_loop: last_attempt
            .is_some_and(|attempt| attempt.full_validation_before_fast_loop),
        prose_only_recovery_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.prose_only_recovery_count)
            .unwrap_or(0),
        bare_replace_block_retry_count: last_attempt
            .map(|attempt| {
                attempt
                    .agent_repair_scorecard
                    .bare_replace_block_retry_count
            })
            .unwrap_or(0),
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
        agent_repair_memory: last_attempt
            .map(|attempt| attempt.agent_repair_memory.clone())
            .unwrap_or_default(),
        agent_repair_scorecard: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.clone())
            .unwrap_or_default(),
        preview_edit_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.preview_edit_count)
            .unwrap_or(0),
        preview_edit_success_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.preview_edit_success_count)
            .unwrap_or(0),
        preview_created_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.preview_created_count)
            .unwrap_or(0),
        replace_range_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.replace_range_count)
            .unwrap_or(0),
        replace_range_hash_mismatch_count: last_attempt
            .map(|attempt| {
                attempt
                    .agent_repair_scorecard
                    .replace_range_hash_mismatch_count
            })
            .unwrap_or(0),
        modify_toml_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.modify_toml_count)
            .unwrap_or(0),
        apply_preview_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.apply_preview_count)
            .unwrap_or(0),
        apply_preview_hash_mismatch_count: last_attempt
            .map(|attempt| {
                attempt
                    .agent_repair_scorecard
                    .apply_preview_hash_mismatch_count
            })
            .unwrap_or(0),
        syntax_preview_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.syntax_preview_count)
            .unwrap_or(0),
        syntax_preview_failure_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.syntax_preview_failure_count)
            .unwrap_or(0),
        target_redirect_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.target_redirect_count)
            .unwrap_or(0),
        evidence_file_fixation_count: last_attempt
            .map(|attempt| attempt.agent_repair_scorecard.evidence_file_fixation_count)
            .unwrap_or(0),
        agent_final_failure_classification: None,
        planner_model,
        executor_model,
        deterministic_evaluation_passed,
        judge,
        primary_failure: None,
    };
    let primary_failure = classify_primary_failure(&report);
    let agent_final_failure_classification =
        classify_agent_failure(&report, primary_failure.as_deref());
    let last_failure_class = agent_final_failure_classification
        .clone()
        .or_else(|| primary_failure.clone());
    let report = BenchmarkReport {
        primary_failure,
        last_failure_class,
        agent_final_failure_classification,
        ..report
    };
    write_json(&result_dir.join("benchmark-report.json"), &report)?;
    fs::write(
        result_dir.join("benchmark-report.md"),
        render_report_markdown(&report),
    )?;
    write_benchmark_proof_receipt(result_dir, &report)?;
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

pub(crate) fn write_synthetic_failure_report(
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
        used_fallback: false,
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
        full_validation_before_fast_loop: false,
        prose_only_recovery_count: 0,
        bare_replace_block_retry_count: 0,
        patch_packet_injected: false,
        patch_packet_honored_range: None,
        recommended_rerun_command: None,
        fast_loop_rerun_match_kind: None,
        failed_edit_records: Vec::new(),
        agent_repair_memory: quorp_agent_core::AgentRepairMemory::default(),
        agent_repair_scorecard: quorp_agent_core::AgentRepairScorecard::default(),
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
        agent_final_failure_classification: Some(
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
    write_benchmark_proof_receipt(result_dir, &report)?;
    Ok(())
}

pub(crate) fn write_benchmark_proof_receipt(
    result_dir: &Path,
    report: &BenchmarkReport,
) -> anyhow::Result<()> {
    let mut receipt = ProofReceipt::new(format!("{}:{}", report.benchmark_name, report.issue_id));
    receipt.sandbox_path = report.sandbox_root.clone();
    receipt.changed_files = report.changed_files.iter().map(PathBuf::from).collect();
    receipt.evaluator_result = Some(if report.success {
        "success".to_string()
    } else {
        format!("failed exit_code={}", report.exit_code)
    });
    receipt.provider = report
        .effective_provider
        .clone()
        .or(Some(report.provider_kind.clone()));
    receipt.model = report
        .effective_model
        .clone()
        .or(Some(report.model_id.clone()));
    receipt.usage.insert(
        "total_billed_tokens".to_string(),
        report.total_billed_tokens,
    );
    receipt
        .usage
        .insert("prompt_tokens".to_string(), report.prompt_tokens);
    receipt
        .usage
        .insert("completion_tokens".to_string(), report.completion_tokens);
    receipt
        .usage
        .insert("reasoning_tokens".to_string(), report.reasoning_tokens);
    receipt.usage.insert(
        "cache_read_input_tokens".to_string(),
        report.cache_read_input_tokens,
    );
    receipt.usage.insert(
        "cache_write_input_tokens".to_string(),
        report.cache_write_input_tokens,
    );
    for attempt in &report.attempts {
        let attempt_events = attempt.agent_result_dir.join("events.jsonl");
        let attempt_events_hash = sha256_file_if_exists(&attempt_events)?;
        for command in &attempt.validations {
            receipt.validation.push(ValidationRecord {
                command: command.clone(),
                cwd: attempt.workspace_dir.clone(),
                exit_code: if report.success { 0 } else { report.exit_code },
                raw_log_path: attempt_events.exists().then(|| attempt_events.clone()),
                raw_log_sha256: attempt_events_hash.clone(),
            });
        }
        for evaluation in [
            attempt.visible_evaluation.as_ref(),
            attempt.collector_evaluation.as_ref(),
            attempt.evaluation.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            receipt.validation.push(ValidationRecord {
                command: evaluation
                    .command
                    .clone()
                    .unwrap_or_else(|| evaluation.name.clone()),
                cwd: attempt.workspace_dir.clone(),
                exit_code: evaluation.exit_code,
                raw_log_path: None,
                raw_log_sha256: None,
            });
        }
    }
    for (name, path) in [
        (
            "benchmark_report_json",
            result_dir.join("benchmark-report.json"),
        ),
        (
            "benchmark_report_markdown",
            result_dir.join("benchmark-report.md"),
        ),
        ("event_log", result_dir.join("events.jsonl")),
    ] {
        if path.exists() {
            receipt.raw_artifacts.insert(
                name.to_string(),
                RawArtifact {
                    sha256: sha256_file_if_exists(&path)?,
                    path,
                },
            );
        }
    }
    if !report.success {
        receipt.residual_risks.push(
            "benchmark did not pass; inspect benchmark-report.json and events.jsonl".to_string(),
        );
    }
    write_json(&result_dir.join("proof-receipt.json"), &receipt)
}
