//! Judge / attempt / report / classify / objective / extract machinery
//! for the benchmark runner.
//!
//! Carved out of `crates/quorp/src/quorp/benchmark.rs` to keep that
//! file under the 2,000-LOC hard cap. Everything here is reachable from
//! the entry points that stayed in the parent (run_benchmark,
//! resume_benchmark, run_benchmark_batch, run_benchmark_from_manifest,
//! run_challenge_benchmark, finalize_challenge_attempt, etc.).
//!
//! Visibility convention:
//!
//! * Functions and types the parent file calls into → `pub(crate)`.
//! * Calls back into the parent's private helpers → `super::name(...)`.
//! * Constants and private structs defined in the parent → `super::NAME`.

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
pub(crate) fn run_challenge_judge(context: &ChallengeJudgeContext<'_>) -> ChallengeJudgeOutcome {
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

pub(crate) fn request_challenge_judge_completion(
    context: &ChallengeJudgeContext<'_>,
    judge_prompt: &str,
) -> Result<(String, serde_json::Value), String> {
    let runtime = tokio::runtime::Runtime::new();
    match runtime {
        Ok(runtime) => runtime.block_on(async {
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
            request_single_completion_details(&request)
                .await
                .map(|completion| (completion.content, completion.raw_response))
        }),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn transient_challenge_judge_error(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("first token timeout")
        || normalized.contains("timeout")
        || normalized.contains("503")
        || normalized.contains("service unavailable")
        || normalized.contains("resourceexhausted")
        || normalized.contains("workers are busy")
}

pub(crate) fn build_challenge_judge_prompt(context: &ChallengeJudgeContext<'_>) -> String {
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

pub(crate) fn parse_challenge_judge_response(content: &str) -> Result<ChallengeJudgeOutcome, String> {
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

pub(crate) fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if start > end {
        return None;
    }
    Some(&text[start..=end])
}

pub(crate) fn read_headless_usage_summary(
    path: &Path,
) -> anyhow::Result<crate::quorp::agent_runner::HeadlessUsageSummary> {
    if !path.exists() {
        return Ok(crate::quorp::agent_runner::HeadlessUsageSummary::default());
    }
    let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let usage = summary
        .get("usage")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::from_value(usage).unwrap_or_default())
}

pub(crate) fn read_headless_routing_summary(path: &Path) -> anyhow::Result<RoutingSummary> {
    if !path.exists() {
        return Ok(RoutingSummary::default());
    }
    let summary: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let routing = summary
        .get("routing")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(serde_json::from_value(routing).unwrap_or_default())
}

pub(crate) fn load_seed_context(path: Option<&Path>) -> anyhow::Result<Vec<TranscriptMessage>> {
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

pub(crate) fn run_attempt_executor(
    manifest: &BenchmarkManifest,
    workspace: &Path,
    objective_file: PathBuf,
    remaining_budget: Option<u64>,
    result_dir: PathBuf,
) -> anyhow::Result<quorp_agent_core::AgentRunOutcome> {
    let seed_context = load_seed_context(manifest.seed_transcript.as_deref())?;
    run_headless_agent(HeadlessRunOptions {
        workspace: workspace.to_path_buf(),
        objective_file,
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
    })
}

pub(crate) fn events_file_has_first_task_model_request(events_path: &Path) -> anyhow::Result<bool> {
    if !events_path.exists() {
        return Ok(false);
    }
    let events = fs::read_to_string(events_path)
        .with_context(|| format!("failed to read {}", events_path.display()))?;
    Ok(events.contains(r#""event":"model_request_started""#))
}

pub(crate) fn attempt_report_for_bootstrap_stall(
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
        agent_final_failure_classification: Some(bootstrap_stall_class),
        planner_model: None,
        executor_model: Some(manifest.model_id.clone()),
        deterministic_evaluation_passed: None,
        judge: None,
        primary_failure: None,
        routing: RoutingSummary::default(),
    }
}

pub(crate) struct BenchmarkBootstrapWatchdog {
    stop_flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BenchmarkBootstrapWatchdog {
    pub(crate) fn spawn(
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

pub(crate) fn detect_widening_against_expected(
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

pub(crate) fn maybe_continue_attempts(
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
            ANSI_GREEN,
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

pub(crate) fn finalize_attempt(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
    attempt_dir: &Path,
    outcome: quorp_agent_core::AgentRunOutcome,
) -> anyhow::Result<AttemptReport> {
    let resolved = &manifest.resolved;
    let workspace_dir = attempt_dir.join("workspace");
    let agent_result_dir = attempt_dir.join("agent");
    let visible_evaluation = match resolved.visible_evaluator.as_ref() {
        Some(script) => {
            log_phase(
                "visible",
                ANSI_BLUE,
                format!("running visible evaluator {}", script.display()),
            );
            Some(run_visible_evaluator(script, &workspace_dir)?)
        }
        None => None,
    };
    let collector_evaluation = match resolved.collector_evaluator.as_ref() {
        Some(script) => {
            log_phase(
                "collector",
                ANSI_BLUE,
                format!("running collector evaluator {}", script.display()),
            );
            Some(run_collector_evaluator(
                script,
                &workspace_dir,
                attempt_dir,
            )?)
        }
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
        .agent_repair_scorecard
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
        parser_recovery_count: validation_state
            .agent_repair_scorecard
            .parser_recovery_count,
        repair_invalid_action_streak_max: validation_state
            .agent_repair_scorecard
            .repair_invalid_action_streak_max,
        repair_submode_entered: validation_state
            .agent_repair_scorecard
            .repair_submode_entered,
        repair_submode_turns: validation_state.agent_repair_scorecard.repair_submode_turns,
        repair_write_locked: validation_state.agent_repair_scorecard.repair_write_locked,
        write_phase_action_refusal_count: validation_state
            .agent_repair_scorecard
            .write_phase_action_refusal_count,
        patch_scaffold_offered: validation_state
            .agent_repair_scorecard
            .patch_scaffold_offered,
        patch_scaffold_honored: validation_state
            .agent_repair_scorecard
            .patch_scaffold_honored,
        preview_apply_locked: validation_state.agent_repair_scorecard.preview_apply_locked,
        preview_apply_action_refusal_count: validation_state
            .agent_repair_scorecard
            .preview_apply_action_refusal_count,
        write_phase_write_emitted: validation_state
            .agent_repair_scorecard
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
            .agent_repair_scorecard
            .rolled_back_write_count,
        rolled_back_non_support_edit_count: validation_state
            .agent_repair_scorecard
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
        agent_repair_memory: validation_state.agent_repair_memory,
        preview_edit_count: validation_state.agent_repair_scorecard.preview_edit_count,
        preview_edit_success_count: validation_state
            .agent_repair_scorecard
            .preview_edit_success_count,
        preview_created_count: validation_state
            .agent_repair_scorecard
            .preview_created_count,
        replace_range_count: validation_state.agent_repair_scorecard.replace_range_count,
        replace_range_hash_mismatch_count: validation_state
            .agent_repair_scorecard
            .replace_range_hash_mismatch_count,
        modify_toml_count: validation_state.agent_repair_scorecard.modify_toml_count,
        apply_preview_count: validation_state.agent_repair_scorecard.apply_preview_count,
        apply_preview_hash_mismatch_count: validation_state
            .agent_repair_scorecard
            .apply_preview_hash_mismatch_count,
        syntax_preview_count: validation_state.agent_repair_scorecard.syntax_preview_count,
        syntax_preview_failure_count: validation_state
            .agent_repair_scorecard
            .syntax_preview_failure_count,
        target_redirect_count: validation_state
            .agent_repair_scorecard
            .target_redirect_count,
        evidence_file_fixation_count: validation_state
            .agent_repair_scorecard
            .evidence_file_fixation_count,
        agent_repair_scorecard: validation_state.agent_repair_scorecard,
        agent_final_failure_classification: None,
        planner_model: None,
        executor_model: Some(manifest.model_id.clone()),
        deterministic_evaluation_passed: None,
        judge: None,
        primary_failure: None,
        routing,
    })
}

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

pub(crate) fn sha256_file_if_exists(path: &Path) -> anyhow::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            Ok(Some(format!("{:x}", hasher.finalize())))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub(crate) fn classify_primary_failure(report: &BenchmarkReport) -> Option<String> {
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

pub(crate) fn classify_agent_failure(
    report: &BenchmarkReport,
    primary_failure: Option<&str>,
) -> Option<String> {
    if report.success {
        return Some("success".to_string());
    }
    if report.pre_model_bootstrap_stalled {
        return Some("infra_runtime".to_string());
    }
    let scorecard = &report.agent_repair_scorecard;
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

pub(crate) fn attempt_passed(attempt: &AttemptReport) -> bool {
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

pub(crate) fn deterministic_evaluation_passed(attempt: &AttemptReport) -> bool {
    (attempt.visible_evaluation.is_some()
        || attempt.collector_evaluation.is_some()
        || attempt.evaluation.is_some())
        && evaluations_all_passed(attempt)
}

pub(crate) fn evaluations_all_passed(attempt: &AttemptReport) -> bool {
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

pub(crate) fn judge_blocks_deterministic_success(judge: &ChallengeJudgeOutcome) -> bool {
    if judge.passed {
        return false;
    }
    !matches!(
        judge.summary.as_str(),
        "judge request failed" | "judge runtime could not start"
    )
}

pub(crate) fn count_evaluation_commands(attempt: &AttemptReport) -> usize {
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

pub(crate) fn count_mistakes_corrected(attempts: &[AttemptReport]) -> usize {
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

pub(crate) fn git_numstat(workspace_dir: &Path) -> anyhow::Result<(u64, u64)> {
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

pub(crate) fn prepare_attempt_workspace(
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

pub(crate) fn synthesize_objective(
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

pub(crate) fn build_benchmark_objective(
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
        "## Required Operating Rules\n- Start from the owning crate or nearest nearest owner.\n- Validate locally first and widen only when forced by the dependency graph or public contract.\n- Continue after the first visible green run when collector validation still fails.\n- Include files changed, validation commands, widening, and attempt count in the final report.".to_string(),
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

pub(crate) fn load_benchmark_briefing(
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

pub(crate) fn select_benchmark_briefing_text(value: &serde_json::Value, issue_id: &str) -> Option<String> {
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

pub(crate) fn indent_block(text: &str) -> String {
    if text.trim().is_empty() {
        return "<empty>".to_string();
    }
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn summarize_judge_output(text: &str) -> String {
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

pub(crate) fn summarize_context_file(path: &Path) -> anyhow::Result<String> {
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

pub(crate) fn summarize_plaintext_lines(content: &str, limit: usize) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect::<Vec<_>>()
        .join(" | ")
}

pub(crate) fn summarize_agent_map(content: &str) -> anyhow::Result<String> {
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

pub(crate) fn summarize_test_map(content: &str) -> anyhow::Result<String> {
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

pub(crate) fn summarize_witness_graph(content: &str) -> anyhow::Result<String> {
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

pub(crate) fn summarize_challenge_capsule(content: &str) -> anyhow::Result<String> {
    let capsule: ChallengeCapsule = serde_json::from_str(content)?;
    Ok(format!(
        "class={} owners=[{}] fast_loop=[{}] companion=[{}]",
        capsule.case_class,
        capsule.owner_files.join(", "),
        capsule.fast_loop_commands.join(" | "),
        capsule.companion_files_required.join(", ")
    ))
}

pub(crate) fn trim_prompt_to_safe_cap(
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

pub(crate) fn write_benchmark_agent_config(workspace_dir: &Path) -> anyhow::Result<()> {
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
  "Stay on owner files and named tests until the fast loop says the current guess is wrong.",
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

pub(crate) fn git_changed_files(workspace_dir: &Path) -> anyhow::Result<Vec<String>> {
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

pub(crate) fn challenge_ignored_changed_files(
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

pub(crate) fn filter_ignored_changed_files(
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

pub(crate) fn count_non_support_changed_files(
    changed_files: &[String],
    ignored_changed_files: &[String],
) -> usize {
    filter_ignored_changed_files(changed_files, ignored_changed_files)
        .into_iter()
        .filter(|path| is_reportable_changed_file(path))
        .filter(|path| !is_support_or_generated_changed_file(path))
        .count()
}

pub(crate) fn is_reportable_changed_file(path: &str) -> bool {
    let normalized = path.trim();
    !normalized.is_empty()
        && !normalized.starts_with("target/")
        && !normalized.starts_with(".warpos-capture-probe/")
        && !normalized.starts_with(".quorp/")
}

pub(crate) fn is_support_or_generated_changed_file(path: &str) -> bool {
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

pub(crate) fn read_checkpoint_validation_state(
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
    let agent_repair_memory = checkpoint
        .get("snapshot")
        .and_then(|value| value.get("agent_repair_memory"))
        .and_then(|value| {
            serde_json::from_value::<quorp_agent_core::AgentRepairMemory>(value.clone()).ok()
        })
        .unwrap_or_default();
    let agent_repair_scorecard = agent_repair_memory.scorecard.clone();
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
        .or_else(|| agent_repair_memory.diagnostic_class.clone());
    let implementation_target_lease = validation_details
        .and_then(|value| value.get("implementation_target_lease"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| agent_repair_memory.implementation_target_lease.clone());
    let dependency_candidates = validation_details
        .and_then(|value| value.get("dependency_candidates"))
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
        .unwrap_or_else(|| agent_repair_memory.dependency_candidates.clone());
    let target_dependency_table = validation_details
        .and_then(|value| value.get("target_dependency_table"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| agent_repair_memory.target_dependency_table.clone());
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
        agent_repair_memory,
        agent_repair_scorecard,
    })
}

pub(crate) fn extract_validation_summaries(events_path: &Path) -> anyhow::Result<Vec<String>> {
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

pub(crate) fn extract_request_metrics(events_path: &Path) -> anyhow::Result<RequestMetricsSummary> {
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

pub(crate) fn extract_read_range_observations(
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

pub(crate) fn extract_action_evidence(
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

pub(crate) fn extract_tool_output_action(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("action:").map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn is_read_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("read_file")
        || normalized.starts_with("list_directory")
        || normalized.starts_with("search_text")
        || normalized.starts_with("search_symbols")
        || normalized.starts_with("get_repo_capsule")
}

pub(crate) fn is_write_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("write_file")
        || normalized.starts_with("apply_patch")
        || normalized.starts_with("replace_block")
        || normalized.starts_with("modify_toml")
        || normalized.starts_with("set_executable")
        || normalized.starts_with("apply_preview")
}

pub(crate) fn is_command_action(action: &str) -> bool {
    let normalized = action.to_ascii_lowercase();
    normalized.starts_with("run:")
        || normalized.starts_with("run ")
        || normalized.starts_with("run_validation")
        || normalized.contains("cargo test")
        || normalized.contains("./evaluate.sh")
}

pub(crate) fn command_matches(actual: &str, expected: &str) -> bool {
    let actual = normalize_command_for_match(actual);
    let expected = normalize_command_for_match(expected);
    !expected.is_empty() && actual.contains(&expected)
}

pub(crate) fn normalize_command_for_match(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches("action:")
        .trim_start_matches("run:")
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn extract_control_loop_summary(events_path: &Path) -> anyhow::Result<ControlLoopSummary> {
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

pub(crate) fn detect_widening(changed_files: &[String]) -> bool {
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

pub(crate) fn resolve_benchmark_model_id(
    _executor: BenchmarkExecutor,
    requested_model: Option<String>,
) -> anyhow::Result<String> {
    if let Some(model_id) = requested_model.filter(|value| {
        value.trim() == crate::quorp::provider_config::NVIDIA_QWEN_MODEL
            || value.trim()
                == format!("nvidia/{}", crate::quorp::provider_config::NVIDIA_QWEN_MODEL)
    }) {
        return Ok(model_id);
    }
    Ok(crate::quorp::provider_config::NVIDIA_QWEN_MODEL.to_string())
}

pub(crate) fn base_url_override_for_executor(
    executor: BenchmarkExecutor,
    base_url_override: Option<String>,
) -> Option<String> {
    match executor {
        BenchmarkExecutor::Native => base_url_override,
    }
}

pub(crate) fn benchmark_provider_summary(
    executor: BenchmarkExecutor,
    model_id: &str,
    base_url_override: Option<&str>,
) -> BenchmarkProviderSummary {
    let _ = executor;

    let provider = crate::quorp::tui::model_registry::chat_model_provider(
        model_id,
        crate::quorp::executor::interactive_provider_from_env(),
    );
    match provider {
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
    }
}

pub(crate) fn benchmark_safety_mode_label(executor: BenchmarkExecutor, model_id: &str) -> String {
    match executor {
        BenchmarkExecutor::Native if is_nvidia_qwen_coder_model_id(model_id) => {
            "nvidia_qwen_benchmark".to_string()
        }
        BenchmarkExecutor::Native => "remote_api".to_string(),
    }
}

pub(crate) fn is_nvidia_qwen_coder_model_id(model_id: &str) -> bool {
    let normalized = model_id.to_ascii_lowercase();
    normalized == "nvidia/qwen/qwen3-coder-480b-a35b-instruct"
        || normalized == "qwen/qwen3-coder-480b-a35b-instruct"
}

pub(crate) fn benchmark_completion_policy(
    executor: BenchmarkExecutor,
    _safety_mode_label: &str,
    model_id: Option<&str>,
) -> quorp_agent_core::CompletionPolicy {
    let _ = executor;
    let mut completion_policy = quorp_agent_core::CompletionPolicy {
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
        safety_mode_label: Some("remote_api".to_string()),
        prompt_compaction_policy: Some(PromptCompactionPolicy::BenchmarkStatePacket),
    };
    apply_model_specific_benchmark_policy_defaults(model_id, &mut completion_policy);
    apply_benchmark_completion_policy_env_overrides(&mut completion_policy);
    completion_policy
}

pub(crate) fn apply_model_specific_benchmark_policy_defaults(
    model_id: Option<&str>,
    completion_policy: &mut quorp_agent_core::CompletionPolicy,
) {
    let Some(model_id) = model_id else {
        return;
    };
    if is_nvidia_qwen_coder_model_id(model_id) {
        completion_policy.include_repo_capsule = true;
        completion_policy.disable_reasoning = true;
        completion_policy.native_tool_calls = false;
        completion_policy.first_turn_max_completion_tokens = Some(4096);
        completion_policy.later_turn_max_completion_tokens = Some(4096);
        completion_policy.prompt_compaction_policy =
            Some(PromptCompactionPolicy::BenchmarkStatePacket);
        completion_policy.watchdog = Some(quorp_agent_core::CompletionWatchdogConfig {
            first_token_timeout_ms: Some(120_000),
            idle_timeout_ms: Some(30_000),
            total_timeout_ms: Some(360_000),
        });
        completion_policy.safety_mode_label = Some("nvidia_qwen_benchmark".to_string());
    }
}

pub(crate) fn apply_benchmark_completion_policy_env_overrides(
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

pub(crate) fn env_override_u32(key: &str) -> Option<u32> {
    let raw = env::var(key).ok()?;
    let parsed = raw.trim().parse::<u32>().ok()?;
    Some(parsed)
}

pub(crate) fn env_override_bool(key: &str) -> Option<bool> {
    let raw = env::var(key).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub(crate) fn env_override_prompt_compaction_policy(key: &str) -> Option<PromptCompactionPolicy> {
    let raw = env::var(key).ok()?;
    PromptCompactionPolicy::parse(raw.trim())
}

pub(crate) fn benchmark_action_contract_mode(
    completion_policy: &quorp_agent_core::CompletionPolicy,
) -> &'static str {
    if completion_policy.native_tool_calls {
        "native_tool_calls_v1"
    } else {
        "strict_json_v1"
    }
}

pub(crate) fn benchmark_attempt_lineage(
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

pub(crate) fn estimate_token_count(text: &str) -> u64 {
    let char_count = text.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

pub(crate) fn default_safe_mode_label() -> String {
    "remote_api".to_string()
}

pub(crate) fn discover_completed_attempts(result_dir: &Path) -> anyhow::Result<usize> {
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

pub(crate) fn load_existing_attempts(result_dir: &Path) -> anyhow::Result<Vec<AttemptReport>> {
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

pub(crate) fn parse_autonomy_profile(value: &str) -> anyhow::Result<quorp_agent_core::AutonomyProfile> {
    match value.trim() {
        "interactive" => Ok(quorp_agent_core::AutonomyProfile::Interactive),
        "autonomous_host" => Ok(quorp_agent_core::AutonomyProfile::AutonomousHost),
        "autonomous_sandboxed" => Ok(quorp_agent_core::AutonomyProfile::AutonomousSandboxed),
        other => Err(anyhow::anyhow!("unknown autonomy profile `{other}`")),
    }
}

pub(crate) fn attempt_dir(result_dir: &Path, attempt: usize) -> PathBuf {
    result_dir.join(format!("attempt-{attempt:03}"))
}

pub(crate) fn benchmark_bootstrap_progress_path(result_dir: &Path) -> PathBuf {
    result_dir.join(BENCHMARK_BOOTSTRAP_PROGRESS_FILE)
}

pub(crate) fn attempt_bootstrap_progress_path(attempt_dir: &Path) -> PathBuf {
    attempt_dir.join(BENCHMARK_BOOTSTRAP_PROGRESS_FILE)
}

pub(crate) fn epoch_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn read_bootstrap_progress(path: &Path) -> anyhow::Result<Option<BenchmarkBootstrapProgress>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let progress = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(progress))
}

pub(crate) fn write_bootstrap_progress_files(
    root_progress_path: &Path,
    attempt_progress_path: &Path,
    progress: &BenchmarkBootstrapProgress,
) -> anyhow::Result<()> {
    write_json(root_progress_path, progress)?;
    write_json(attempt_progress_path, progress)?;
    Ok(())
}

impl BenchmarkBootstrapTracker {
    pub(crate) fn new(result_dir: &Path, attempt_dir: &Path, attempt: usize) -> anyhow::Result<Self> {
        let tracker = Self {
            root_progress_path: benchmark_bootstrap_progress_path(result_dir),
            attempt_progress_path: attempt_bootstrap_progress_path(attempt_dir),
            attempt,
            started_at: Instant::now(),
        };
        tracker.update(BOOTSTRAP_PHASE_BENCHMARK_STARTED, None)?;
        Ok(tracker)
    }

    pub(crate) fn update(&self, phase: &str, detail: Option<String>) -> anyhow::Result<()> {
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

    pub(crate) fn mark_first_task_model_request(&self) -> anyhow::Result<()> {
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

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub(crate) fn log_phase(label: &str, color: &str, message: String) {
    eprintln!("{ANSI_BOLD}{color}[{label}]{ANSI_RESET} {message}");
}

impl BenchmarkRunLock {
    pub(crate) fn acquire() -> anyhow::Result<Self> {
        Self::acquire_at(benchmark_run_lock_path()?)
    }

    pub(crate) fn acquire_at(path: PathBuf) -> anyhow::Result<Self> {
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
                    "another headless benchmark run already holds the benchmark lock at {}: {}",
                    path.display(),
                    detail
                );
            }
            Err(error) => Err(error.into()),
        }
    }
}

pub(crate) fn lock_is_stale(path: &Path) -> bool {
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

pub(crate) fn benchmark_run_lock_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set for benchmark lockfile")?;
    Ok(benchmark_run_lock_path_for_home(Path::new(&home)))
}

pub(crate) fn benchmark_run_lock_path_for_home(home: &Path) -> PathBuf {
    home.join(".config")
        .join("quorp")
        .join("locks")
        .join("benchmark-run.lock")
}
