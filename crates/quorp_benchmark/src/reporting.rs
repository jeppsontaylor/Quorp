use std::fs;
use std::path::{Path, PathBuf};

use crate::{
    BatchCaseReport, BatchReport, BenchmarkReport, RunSummary, RunSummaryCase,
};

pub fn summarize_batch_report(
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

pub fn summarize_run_report(report: &BatchReport) -> RunSummary {
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
                    agent_final_failure_classification: case
                        .agent_final_failure_classification
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

pub fn render_run_summary(summary: &RunSummary) -> String {
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
            "- `{}` success={} primary={} agent={} stop={:?} first_write={} parser_recovery={} redundant_reads={} validation_rejects={} target_redirects={} syntax_preview_failures={} adaptive_retry={} report={}",
            case.case_id,
            case.success,
            case.primary_failure
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            case.agent_final_failure_classification
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

pub fn render_batch_report(report: &BatchReport, elapsed_ms: u64) -> String {
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
            "- `{}` executor={} success={} judge={} deterministic={} wall_clock_ms={} first_prompt_est={} compacted_prompt_est={} first_turn_started={} first_action_emitted={} first_token_ms={} requests={} tokens={} added={} removed={} mistakes={} stop={:?} failure={} agent={} adaptive_retry={} log={} report={}",
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
                .agent_final_failure_classification
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
            case.adaptive_action_mode_retry,
            case.log_file.display(),
            case.report_path.display(),
        ));
    }
    lines.join("\n")
}

pub fn read_case_report_scorecard(
    report_path: &Path,
) -> Option<quorp_agent_core::AgentRepairScorecard> {
    let raw = fs::read_to_string(report_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("agent_repair_scorecard")
        .and_then(|scorecard| serde_json::from_value(scorecard.clone()).ok())
}

pub fn render_report_markdown(report: &BenchmarkReport) -> String {
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
        format!("- Used fallback: `{}`", report.used_fallback),
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
        "- Agent scorecard: parser_recovery=`{}` line_tools=`{}` controller_reads=`{}` redundant_reads=`{}` first_write=`{}` repeated_edits=`{}` validation_rejects=`{}` test_edit_rejects=`{}` target_redirects=`{}` evidence_fixations=`{}` anchors=`{}` syntax_previews=`{}`/`{}` classification=`{}`",
        report.agent_repair_scorecard.parser_recovery_count,
        report.agent_repair_scorecard.line_oriented_parse_count,
        report.agent_repair_scorecard.controller_injected_read_count,
        report.agent_repair_scorecard.redundant_read_count,
        report
            .agent_repair_scorecard
            .first_valid_write_step
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        report.agent_repair_scorecard.repeated_failed_edit_count,
        report.agent_repair_scorecard.rejected_validation_alias_count,
        report.agent_repair_scorecard.test_edit_rejection_count,
        report.agent_repair_scorecard.target_redirect_count,
        report.agent_repair_scorecard.evidence_file_fixation_count,
        report.agent_repair_scorecard.anchor_suggestion_count,
        report.agent_repair_scorecard.syntax_preview_failure_count,
        report.agent_repair_scorecard.syntax_preview_count,
        report
            .agent_final_failure_classification
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
            "  - Agent scorecard: parser_recovery={} line_tools={} controller_reads={} redundant_reads={} first_write={} repeated_edits={} validation_rejects={} test_edit_rejects={} target_redirects={} evidence_fixations={} anchors={} syntax_previews={}/{}",
            attempt.agent_repair_scorecard.parser_recovery_count,
            attempt.agent_repair_scorecard.line_oriented_parse_count,
            attempt.agent_repair_scorecard.controller_injected_read_count,
            attempt.agent_repair_scorecard.redundant_read_count,
            attempt
                .agent_repair_scorecard
                .first_valid_write_step
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            attempt.agent_repair_scorecard.repeated_failed_edit_count,
            attempt.agent_repair_scorecard.rejected_validation_alias_count,
            attempt.agent_repair_scorecard.test_edit_rejection_count,
            attempt.agent_repair_scorecard.target_redirect_count,
            attempt.agent_repair_scorecard.evidence_file_fixation_count,
            attempt.agent_repair_scorecard.anchor_suggestion_count,
            attempt.agent_repair_scorecard.syntax_preview_failure_count,
            attempt.agent_repair_scorecard.syntax_preview_count
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

fn truncate_report_text(value: &str, char_limit: usize) -> String {
    if value.chars().count() <= char_limit {
        return value.to_string();
    }
    let mut truncated = value.chars().take(char_limit).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn render_failed_edit_records_for_report(
    records: &[quorp_agent_core::FailedEditRecord],
) -> String {
    records
        .iter()
        .map(|record| {
            format!(
                "{}:{}:{} lines={:?}",
                record.action_kind,
                record.path,
                record.failure_reason,
                record.matching_line_numbers
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}
