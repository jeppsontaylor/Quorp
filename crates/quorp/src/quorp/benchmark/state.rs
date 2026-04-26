//! Benchmark classifiers, summarizers, objective synthesis, agent
//! config writer, and changed-files helpers.
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

