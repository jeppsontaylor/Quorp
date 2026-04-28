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

pub(crate) fn parse_challenge_judge_response(
    content: &str,
) -> Result<ChallengeJudgeOutcome, String> {
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
        full_validation_before_fast_loop: validation_state.full_validation_before_fast_loop,
        prose_only_recovery_count: validation_state.prose_only_recovery_count,
        bare_replace_block_retry_count: validation_state.bare_replace_block_retry_count,
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
