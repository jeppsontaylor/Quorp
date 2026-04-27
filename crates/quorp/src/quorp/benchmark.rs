#![allow(
    clippy::collapsible_match,
    clippy::disallowed_methods,
    clippy::manual_contains,
    clippy::too_many_arguments
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::time::Instant;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::quorp::agent_runner::resume_headless_agent;
use quorp_agent_core::PromptCompactionPolicy;
use quorp_benchmark::{
    AttemptReport, BatchCaseReport, BenchmarkReport, ChallengeManifest, ChallengeMetadata,
    EvaluatorOutcome, PromptTokenTurnSample, ResolvedBenchmark, ResolvedChallengeCase,
    challenge_evaluation_env, challenge_evaluation_target_dir,
    prepare_challenge_run as prepare_benchmark_challenge_run, render_batch_report,
    render_run_summary,
    reset_challenge_workspace_for_attempt as reset_benchmark_challenge_workspace_for_attempt,
    resolve_benchmark, resolve_challenge_case, run_shell_command_with_env, substitute_condition,
    summarize_batch_report, summarize_run_report,
};
pub use quorp_benchmark::{BenchmarkExecutor, BenchmarkScoreOptions, score_benchmark_reports};

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

fn apply_requested_prompt_compaction_override(
    completion_policy: &mut quorp_agent_core::CompletionPolicy,
    requested_policy: Option<PromptCompactionPolicy>,
) {
    if let Some(policy) = requested_policy {
        completion_policy.prompt_compaction_policy = Some(policy);
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
    pub condition: Option<String>,
    pub keep_sandbox: bool,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BenchmarkManifest {
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
pub(crate) struct BenchmarkBootstrapProgress {
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

pub(crate) struct BenchmarkBootstrapTracker {
    root_progress_path: PathBuf,
    attempt_progress_path: PathBuf,
    attempt: usize,
    started_at: Instant,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ActionEvidence {
    read_count: usize,
    write_count: usize,
    command_execution_count: usize,
    fast_loop_command_seen: bool,
    final_evaluate_command_seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct CheckpointValidationState {
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
    full_validation_before_fast_loop: bool,
    #[serde(default)]
    prose_only_recovery_count: usize,
    #[serde(default)]
    bare_replace_block_retry_count: usize,
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
    agent_repair_memory: quorp_agent_core::AgentRepairMemory,
    #[serde(default)]
    agent_repair_scorecard: quorp_agent_core::AgentRepairScorecard,
}

#[derive(Debug, Clone)]
pub(crate) struct BenchmarkProviderSummary {
    provider_kind: String,
    provider_base_url: Option<String>,
    auth_mode: String,
    usage_source: String,
    proxy_visible_remote_egress_expected: bool,
}

struct PreparedBatchRuntime {
    base_url_override: Option<String>,
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

pub(crate) struct ChallengeJudgeContext<'a> {
    manifest: &'a BenchmarkManifest,
    metadata: &'a ChallengeMetadata,
    attempt_number: usize,
    attempt_dir: &'a Path,
    outcome: &'a quorp_agent_core::AgentRunOutcome,
    evaluation: &'a EvaluatorOutcome,
    changed_files: &'a [String],
    validations: &'a [String],
    metrics: &'a RequestMetricsSummary,
    usage: &'a crate::quorp::agent_runner::HeadlessUsageSummary,
}

#[derive(Debug, Clone)]
pub(crate) struct SynthesizedObjective {
    path: PathBuf,
    prompt_token_estimate: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RequestMetricsSummary {
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
pub(crate) struct ControlLoopSummary {
    path_resolution_failures: usize,
    recovery_turns: usize,
}

#[derive(Debug)]
struct BenchmarkRunLock {
    path: PathBuf,
    enabled: bool,
}

pub fn run_benchmark(options: BenchmarkRunOptions) -> anyhow::Result<()> {
    let _run_lock = BenchmarkRunLock::acquire()?;
    fs::create_dir_all(&options.result_dir)?;
    if let Some(challenge) = resolve_challenge_case(&options.path, options.condition.as_deref())? {
        return run_challenge_benchmark(&options, challenge);
    }

    let resolved = resolve_benchmark(&options.path)?;
    let model_id = resolve_benchmark_model_id(options.executor, options.model_id.clone())?;
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

fn normalize_manifest_paths_for_runtime(_manifest: &mut BenchmarkManifest, _result_dir: &Path) {}

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
    let prompt = fs::read_to_string(&objective.path)
        .with_context(|| format!("failed to read {}", objective.path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(max_steps.to_le_bytes());
    hasher.update(max_seconds.unwrap_or_default().to_le_bytes());
    hasher.update(max_total_tokens.unwrap_or_default().to_le_bytes());
    Ok(BenchmarkPromptBundle {
        resolved,
        workspace_dir: workspace_dir.to_path_buf(),
        objective_path: objective.path,
        model_id,
        safety_mode_label,
        prompt,
        prompt_fingerprint: format!("{:x}", hasher.finalize()),
        prompt_token_estimate: objective.prompt_token_estimate,
    })
}

pub fn run_benchmark_batch(mut options: BenchmarkBatchRunOptions) -> anyhow::Result<()> {
    fs::create_dir_all(&options.result_dir)?;
    options.result_dir = fs::canonicalize(&options.result_dir)
        .with_context(|| format!("failed to canonicalize {}", options.result_dir.display()))?;
    if let Some(log_dir) = options.log_dir.as_ref() {
        fs::create_dir_all(log_dir)
            .with_context(|| format!("failed to create log dir {}", log_dir.display()))?;
        options.log_dir = Some(
            fs::canonicalize(log_dir)
                .with_context(|| format!("failed to canonicalize {}", log_dir.display()))?,
        );
    }
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
                    .agent_final_failure_classification
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
                agent_final_failure_classification: summary
                    .agent_final_failure_classification
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
                agent_final_failure_classification: Some("launch_failed".to_string()),
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
            .agent_repair_scorecard
            .first_valid_write_step
            .is_none()
        && report
            .agent_final_failure_classification
            .as_deref()
            .is_some_and(|classification| {
                classification == "parser_tool_schema"
                    || classification == "parser_or_action_contract"
            })
        && (report.agent_repair_scorecard.parser_recovery_count > 0
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

    if options.keep_sandbox {
        command.arg("--keep-sandbox");
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
    command.env("QUORP_BENCHMARK_SKIP_LOCK", "1");
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
    if let Some(base_url_override) = options.base_url_override.clone() {
        return Ok(PreparedBatchRuntime {
            base_url_override: Some(base_url_override),
        });
    }
    Ok(PreparedBatchRuntime {
        base_url_override: None,
    })
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

fn prepare_challenge_run(
    result_dir: &Path,
    challenge: &ResolvedChallengeCase,
) -> anyhow::Result<quorp_benchmark::PreparedChallengeRun> {
    prepare_benchmark_challenge_run(
        result_dir,
        challenge,
        CHALLENGE_SANDBOX_DIR,
        CHALLENGE_OBJECTIVE_FILE,
        CHALLENGE_CARGO_CACHE_DIR,
        |message| log_phase("sandbox", ANSI_BLUE, message),
        write_benchmark_agent_config,
    )
}

fn reset_challenge_workspace_for_attempt(
    manifest: &BenchmarkManifest,
    attempt_number: usize,
) -> anyhow::Result<Option<EvaluatorOutcome>> {
    let Some(challenge_metadata) = manifest.challenge.as_ref() else {
        anyhow::bail!("challenge metadata missing from benchmark manifest");
    };
    reset_benchmark_challenge_workspace_for_attempt(
        challenge_metadata,
        attempt_number,
        manifest.executor == BenchmarkExecutor::Native,
        CHALLENGE_CARGO_CACHE_DIR,
        write_benchmark_agent_config,
    )
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
        let evaluation_target_dir = challenge_evaluation_target_dir(
            challenge_metadata,
            attempt_number,
            CHALLENGE_EVALUATION_CARGO_CACHE_DIR,
        );
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
        .agent_repair_scorecard
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
        judge,
        primary_failure: None,
        routing,
    })
}

mod engine;
mod probe;
mod reporting;
mod state;

#[allow(unused_imports)]
pub(crate) use engine::*;
#[allow(unused_imports)]
pub(crate) use probe::*;
#[allow(unused_imports)]
pub(crate) use reporting::*;
#[allow(unused_imports)]
pub(crate) use state::*;

#[cfg(test)]
mod tests;
