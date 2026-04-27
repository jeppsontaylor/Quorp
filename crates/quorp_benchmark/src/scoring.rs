use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::{
    BatchCaseReport, BatchReport, BenchmarkReport, BenchmarkScoreCase, BenchmarkScoreReport,
    read_case_report_scorecard,
};

const SLOW_FIRST_TOKEN_LATENCY_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct BenchmarkScoreOptions {
    pub run_dirs: Vec<PathBuf>,
    pub suite: String,
    pub reports_root: PathBuf,
    pub output_root: Option<PathBuf>,
    pub fail_on_regression: bool,
}

#[derive(Debug, Clone)]
pub struct BenchmarkScoreArtifacts {
    pub output_dir: PathBuf,
    pub markdown: String,
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
    let success_rate_ppm = rate_ppm(solved_cases, total_cases);
    let secure_success_cases = cases.iter().filter(|case| case.secure_success).count();
    let secure_success_rate_ppm = rate_ppm(secure_success_cases, total_cases);
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
    let secure_etts_tokens = cases.iter().map(|case| case.secure_etts_tokens).sum();
    let total_wall_clock_ms = cases.iter().map(|case| case.wall_clock_ms).sum();
    let median_wall_clock_ms = median(cases.iter().map(|case| case.wall_clock_ms).collect());
    let total_patch_lines_changed = cases.iter().map(|case| case.patch_lines_changed).sum();
    let total_retries = cases.iter().map(|case| case.retry_count).sum();
    let proof_lane_counts = count_proof_lanes(&cases);
    let slow_first_token_cases = cases
        .iter()
        .filter(|case| {
            case.first_request_first_token_latency_ms
                .is_some_and(|latency| latency >= SLOW_FIRST_TOKEN_LATENCY_MS)
        })
        .count();
    let watchdog_near_limit_cases = cases.iter().filter(|case| case.watchdog_near_limit).count();
    let patch_quality_risk_cases = cases
        .iter()
        .filter(|case| case.patch_quality_risk.is_some())
        .count();
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
        success_rate_ppm,
        secure_success_cases,
        secure_success_rate_ppm,
        total_requests,
        total_billed_tokens,
        secure_etts_tokens,
        total_wall_clock_ms,
        median_wall_clock_ms,
        total_patch_lines_changed,
        total_retries,
        proof_lane_counts,
        slow_first_token_cases,
        watchdog_near_limit_cases,
        patch_quality_risk_cases,
        common_blocker,
        blocker_counts,
        regressions: Vec::new(),
        cases,
    };
    score.regressions = detect_score_regressions(previous_score.as_ref(), &score);
    let markdown = render_scoreboard(&score);

    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    fs::create_dir_all(&output_root)
        .with_context(|| format!("failed to create {}", output_root.display()))?;
    write_json(&output_dir.join("scoreboard.json"), &score)?;
    fs::write(output_dir.join("scoreboard.md"), &markdown)?;
    write_json(&output_root.join("latest.json"), &score)?;
    fs::write(output_root.join("latest.md"), &markdown)?;

    if options.fail_on_regression && !score.regressions.is_empty() {
        anyhow::bail!(
            "benchmark score regressed: {}",
            score.regressions.join("; ")
        );
    }

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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
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
        agent_final_failure_classification: report.agent_final_failure_classification.clone(),
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
        .map(|report| report.agent_repair_scorecard.clone())
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
    let secure_success = case_secure_success(case, report);
    let retry_count = report
        .map(|report| report.attempts_run.saturating_sub(1))
        .unwrap_or_default()
        .max(usize::from(case.adaptive_action_mode_retry));
    let proof_lanes = proof_lanes_for_case(case, report);
    let patch_lines_changed = case.lines_added.saturating_add(case.lines_removed);
    let patch_quality_risk = patch_quality_risk_for_case(report);
    BenchmarkScoreCase {
        case_id: case.case_id.clone(),
        success: case.success,
        secure_success,
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
        secure_etts_tokens: if secure_success {
            case.total_billed_tokens
        } else {
            0
        },
        memory_peak_mb: None,
        patch_lines_changed,
        retry_count,
        proof_lanes,
        first_request_first_token_latency_ms: case.first_request_first_token_latency_ms,
        watchdog_near_limit: report.is_some_and(|report| report.watchdog_near_limit),
        patch_quality_risk,
        total_requests: case.total_requests,
        total_billed_tokens: case.total_billed_tokens,
        lines_added: case.lines_added,
        lines_removed: case.lines_removed,
        general_tooling_gap,
    }
}

fn patch_quality_risk_for_case(report: Option<&BenchmarkReport>) -> Option<String> {
    let report = report?;
    if !report.success || report.write_count == 0 {
        return None;
    }
    let structured_write_count = report
        .preview_edit_count
        .saturating_add(report.replace_range_count)
        .saturating_add(report.modify_toml_count)
        .saturating_add(report.apply_preview_count);
    if structured_write_count == 0 && report.changed_files.len() > 1 {
        return Some("multi_file_write_without_structured_edit".to_string());
    }
    if structured_write_count == 0 && report.lines_added.saturating_add(report.lines_removed) >= 120
    {
        return Some("large_write_without_structured_edit".to_string());
    }
    None
}

fn progress_score_for_case(
    case: &BatchCaseReport,
    report: Option<&BenchmarkReport>,
    scorecard: &quorp_agent_core::AgentRepairScorecard,
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
        .agent_final_failure_classification
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
    scorecard: &quorp_agent_core::AgentRepairScorecard,
) -> String {
    if case.success {
        return "success".to_string();
    }
    let raw = case
        .agent_final_failure_classification
        .as_deref()
        .or_else(|| report.and_then(|report| report.agent_final_failure_classification.as_deref()))
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

fn rate_ppm(numerator: usize, denominator: usize) -> u32 {
    if denominator == 0 {
        return 0;
    }
    let scaled = numerator.saturating_mul(1_000_000) / denominator;
    scaled.try_into().unwrap_or(u32::MAX)
}

fn median(mut values: Vec<u64>) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

fn count_proof_lanes(cases: &[BenchmarkScoreCase]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for proof_lane in cases.iter().flat_map(|case| case.proof_lanes.iter()) {
        *counts.entry(proof_lane.clone()).or_insert(0) += 1;
    }
    counts
}

fn case_secure_success(case: &BatchCaseReport, report: Option<&BenchmarkReport>) -> bool {
    if !case.success {
        return false;
    }
    let deterministic_passed = case
        .deterministic_evaluation_passed
        .or_else(|| report.and_then(|report| report.deterministic_evaluation_passed));
    deterministic_passed.unwrap_or(true)
}

fn proof_lanes_for_case(case: &BatchCaseReport, report: Option<&BenchmarkReport>) -> Vec<String> {
    let mut lanes = Vec::new();
    if report.is_some_and(|report| {
        report.fast_loop_command_seen || report.post_fast_loop_validation_rerun_attempted
    }) {
        lanes.push("fast".to_string());
    }
    if report.is_some_and(|report| report.validation_commands_run > 1) {
        lanes.push("medium".to_string());
    }
    if report.is_some_and(|report| {
        report.evaluation_commands_run > 0
            || report.final_evaluate_command_seen
            || report.agent_final_evaluate_command_seen
    }) {
        lanes.push("evaluation".to_string());
    }
    if case.deterministic_evaluation_passed.is_some()
        || report.is_some_and(|report| report.deterministic_evaluation_passed.is_some())
    {
        lanes.push("deterministic".to_string());
    }
    if case.judge_passed.is_some() || report.is_some_and(|report| report.judge.is_some()) {
        lanes.push("judge".to_string());
    }
    lanes.sort();
    lanes.dedup();
    lanes
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
    if current.secure_success_cases < previous.secure_success_cases {
        regressions.push(format!(
            "secure success cases decreased from {} to {}",
            previous.secure_success_cases, current.secure_success_cases
        ));
    }
    if current.success_rate_ppm < previous.success_rate_ppm {
        regressions.push(format!(
            "success rate decreased from {:.2}% to {:.2}%",
            f64::from(previous.success_rate_ppm) / 10_000.0,
            f64::from(current.success_rate_ppm) / 10_000.0
        ));
    }
    if current.secure_success_rate_ppm < previous.secure_success_rate_ppm {
        regressions.push(format!(
            "secure success rate decreased from {:.2}% to {:.2}%",
            f64::from(previous.secure_success_rate_ppm) / 10_000.0,
            f64::from(current.secure_success_rate_ppm) / 10_000.0
        ));
    }
    let comparable_cost_surface = current.total_cases == previous.total_cases
        && current.solved_cases == previous.solved_cases
        && current.secure_success_cases == previous.secure_success_cases;
    if comparable_cost_surface {
        if current.total_billed_tokens > previous.total_billed_tokens {
            regressions.push(format!(
                "total billed tokens increased from {} to {} at equal solved coverage",
                previous.total_billed_tokens, current.total_billed_tokens
            ));
        }
        if current.secure_etts_tokens > previous.secure_etts_tokens {
            regressions.push(format!(
                "SecureETTS tokens increased from {} to {} at equal secure coverage",
                previous.secure_etts_tokens, current.secure_etts_tokens
            ));
        }
        if current.median_wall_clock_ms > previous.median_wall_clock_ms {
            regressions.push(format!(
                "median wall time increased from {}ms to {}ms at equal solved coverage",
                previous.median_wall_clock_ms, current.median_wall_clock_ms
            ));
        }
        if current.total_retries > previous.total_retries {
            regressions.push(format!(
                "total retries increased from {} to {} at equal solved coverage",
                previous.total_retries, current.total_retries
            ));
        }
        if current.total_patch_lines_changed > previous.total_patch_lines_changed {
            regressions.push(format!(
                "total patch size increased from {} to {} changed lines at equal solved coverage",
                previous.total_patch_lines_changed, current.total_patch_lines_changed
            ));
        }
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

fn read_benchmark_report(path: &Path) -> anyhow::Result<BenchmarkReport> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
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
            "- Success rate: `{:.2}%`",
            f64::from(report.success_rate_ppm) / 10_000.0
        ),
        format!(
            "- Secure success: `{}/{}` (`{:.2}%`)",
            report.secure_success_cases,
            report.total_cases,
            f64::from(report.secure_success_rate_ppm) / 10_000.0
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
        format!("- SecureETTS tokens: `{}`", report.secure_etts_tokens),
        format!("- Total wall time ms: `{}`", report.total_wall_clock_ms),
        format!("- Median wall time ms: `{}`", report.median_wall_clock_ms),
        format!(
            "- Total patch size: `{} changed lines`",
            report.total_patch_lines_changed
        ),
        format!("- Total retries: `{}`", report.total_retries),
        format!(
            "- Slow first-token cases: `{}` (>= {}ms)",
            report.slow_first_token_cases, SLOW_FIRST_TOKEN_LATENCY_MS
        ),
        format!(
            "- Watchdog near-limit cases: `{}`",
            report.watchdog_near_limit_cases
        ),
        format!(
            "- Patch quality risk cases: `{}`",
            report.patch_quality_risk_cases
        ),
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
    lines.push("## Proof Lanes".to_string());
    if report.proof_lane_counts.is_empty() {
        lines.push("- none".to_string());
    } else {
        for (lane, count) in &report.proof_lane_counts {
            lines.push(format!("- `{lane}`: `{count}`"));
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
            "- `{}` phase=`{}` progress={} success={} secure={} class=`{}` model={} first_write={} post_write_validation={} requests={} tokens={} secure_etts={} wall_ms={} first_token_ms={} near_limit={} changed=+{}/-{} retries={} lanes={} patch_risk={} memory_peak_mb={} gap={} report={}",
            case.case_id,
            case.progress_phase,
            case.progress_score,
            case.success,
            case.secure_success,
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
            case.secure_etts_tokens,
            case.wall_clock_ms,
            case.first_request_first_token_latency_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            case.watchdog_near_limit,
            case.lines_added,
            case.lines_removed,
            case.retry_count,
            if case.proof_lanes.is_empty() {
                "none".to_string()
            } else {
                case.proof_lanes.join(",")
            },
            case.patch_quality_risk
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            case.memory_peak_mb
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
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

fn write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    let serialized = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    fs::write(path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_phase_nine_score_regressions() {
        let previous = score_report_for_test(1, 1, 100, 100, 10, 0, 1);
        let current = score_report_for_test(1, 0, 100, 100, 10, 0, 1);

        let regressions = detect_score_regressions(Some(&previous), &current);

        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("secure success cases decreased"))
        );
        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("secure success rate decreased"))
        );
    }

    #[test]
    fn detects_phase_nine_cost_regressions_at_equal_coverage() {
        let previous = score_report_for_test(1, 1, 100, 100, 10, 0, 1);
        let current = score_report_for_test(1, 1, 150, 125, 20, 1, 2);

        let regressions = detect_score_regressions(Some(&previous), &current);

        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("total billed tokens increased"))
        );
        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("SecureETTS tokens increased"))
        );
        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("median wall time increased"))
        );
        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("total retries increased"))
        );
        assert!(
            regressions
                .iter()
                .any(|regression| regression.contains("total patch size increased"))
        );
    }

    #[test]
    fn fail_on_regression_writes_scoreboard_then_returns_error() {
        let test_root = unique_temp_dir("quorp-score-gate");
        let run_dir = test_root.join("run");
        let output_root = test_root.join("scoreboards");
        fs::create_dir_all(&run_dir).expect("run dir");
        fs::create_dir_all(&output_root).expect("output root");
        write_json(
            &output_root.join("latest.json"),
            &score_report_for_test(1, 1, 100, 100, 10, 0, 1),
        )
        .expect("previous score");
        write_json(
            &run_dir.join("benchmark-report.json"),
            &serde_json::json!({
                "benchmark_name": "Regression Fixture",
                "issue_id": "case-a",
                "success": false,
                "attempts_run": 1,
                "max_attempts": 1,
                "total_billed_tokens": 150,
                "max_total_tokens": null,
                "final_stop_reason": "fatal_error",
                "changed_files": [],
                "widening_happened": false,
                "attempts": [],
                "run_dir": run_dir,
                "wall_clock_ms": 20,
                "total_requests": 1,
                "first_model_turn_started": true,
                "first_action_emitted": true,
                "agent_final_failure_classification": "model_edit_strategy"
            }),
        )
        .expect("benchmark report");

        let error = score_benchmark_reports(BenchmarkScoreOptions {
            run_dirs: vec![run_dir],
            suite: String::new(),
            reports_root: test_root.join("reports"),
            output_root: Some(output_root.clone()),
            fail_on_regression: true,
        })
        .expect_err("regression gate should fail");

        assert!(error.to_string().contains("benchmark score regressed"));
        assert!(output_root.join("latest.json").exists());
        assert!(output_root.join("latest.md").exists());
        fs::remove_dir_all(test_root).expect("cleanup");
    }

    fn score_report_for_test(
        solved_cases: usize,
        secure_success_cases: usize,
        total_billed_tokens: u64,
        secure_etts_tokens: u64,
        median_wall_clock_ms: u64,
        total_retries: usize,
        total_patch_lines_changed: u64,
    ) -> BenchmarkScoreReport {
        let total_cases = 1;
        BenchmarkScoreReport {
            suite: "test".to_string(),
            generated_at_unix_seconds: 1,
            output_dir: PathBuf::from("/tmp/quorp-score-test"),
            run_dirs: vec![PathBuf::from("/tmp/quorp-score-test/run")],
            total_cases,
            solved_cases,
            valid_write_cases: solved_cases,
            post_write_validation_cases: solved_cases,
            diagnostic_classified_cases: total_cases,
            tooling_healthy_cases: total_cases,
            success_rate_ppm: rate_ppm(solved_cases, total_cases),
            secure_success_cases,
            secure_success_rate_ppm: rate_ppm(secure_success_cases, total_cases),
            total_requests: 1,
            total_billed_tokens,
            secure_etts_tokens,
            total_wall_clock_ms: median_wall_clock_ms,
            median_wall_clock_ms,
            total_patch_lines_changed,
            total_retries,
            proof_lane_counts: BTreeMap::new(),
            slow_first_token_cases: 0,
            watchdog_near_limit_cases: 0,
            patch_quality_risk_cases: 0,
            common_blocker: None,
            blocker_counts: BTreeMap::new(),
            regressions: Vec::new(),
            cases: Vec::new(),
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()))
    }
}
