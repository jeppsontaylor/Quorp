//! Benchmark checkpoint / extraction probes, provider+policy
//! resolution, bootstrap progress tracker, and BenchmarkRunLock impl.
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
