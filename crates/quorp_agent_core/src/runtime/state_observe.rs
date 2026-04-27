//! `AgentTaskState`: action observation, mode setting, validation
//! queue management, outcome processing.

#![allow(dead_code, unused_imports)]

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::future::BoxFuture;
use serde::Serialize;

use super::*;
use crate::agent_context::{
    AgentConfig, AutonomyProfile, PolicyMode, PolicySettings, load_agent_config,
    validation_commands_for_plan,
};
use crate::agent_protocol::{
    ActionOutcome, AgentAction, AgentMode, PreviewEditPayload, ValidationPlan, stable_content_hash,
};
use crate::agent_turn::{AgentTurnResponse, parse_agent_turn_response};

impl AgentTaskState {
    pub(crate) fn observe_outcome(&mut self, outcome: &ActionOutcome) -> String {
        let status = match outcome {
            ActionOutcome::Success { .. } => "success",
            ActionOutcome::Failure { .. } => "failure",
        };
        let action_summary = outcome.action().summary();
        let output_text = outcome.output_text().trim();

        self.last_tool_summary = Some(format!("{action_summary} [{status}]"));
        if matches!(outcome, ActionOutcome::Success { .. }) {
            self.reset_parser_recovery_tracking();
            self.stall_count = 0;
            self.redundant_inspection_turns = 0;
            self.recoverable_inspection_failures = 0;
            self.last_failed_tool_error = None;
            self.repair_recovery_turns_remaining = 0;
            if let AgentAction::ReadFile { path, .. } = outcome.action() {
                let observation = parse_read_file_observation(output_text);
                let honored_range = observation.as_ref().and_then(|value| value.honored_range);
                let requested_range = match outcome.action() {
                    AgentAction::ReadFile { range, .. } => *range,
                    _ => None,
                };
                let read_purpose = self
                    .benchmark_repair_state
                    .as_ref()
                    .filter(|repair_state| repair_state.owner_path == *path)
                    .map(|repair_state| repair_state.phase.label().to_string());
                self.record_observed_slice(
                    path,
                    observation
                        .as_ref()
                        .and_then(|value| value.requested_range)
                        .or(requested_range),
                    honored_range,
                    read_purpose,
                    observation
                        .as_ref()
                        .map(|value| value.content.as_str())
                        .unwrap_or(output_text),
                    observation
                        .as_ref()
                        .and_then(|value| value.content_hash.as_deref()),
                );
                let mut missing_anchor_reread = false;
                if let Some(requirement) = self.repair_requirement.as_mut()
                    && requirement.path == *path
                {
                    requirement.exact_reread_completed =
                        reread_satisfies_requirement(requirement, requested_range, honored_range);
                    missing_anchor_reread = !requirement.exact_reread_completed;
                }
                if missing_anchor_reread {
                    self.last_tool_summary = Some(
                        if self
                            .repair_requirement
                            .as_ref()
                            .is_some_and(Self::repair_requirement_prefers_full_file)
                        {
                            format!(
                                "repair reread for `{}` succeeded, but a full-file read is still required before the next write",
                                path
                            )
                        } else {
                            format!(
                                "repair reread for `{}` succeeded, but an honored focused line range is still required before the next write",
                                path
                            )
                        },
                    );
                }
                let workspace_root = self.workspace_root.clone();
                let active_target_lease = self
                    .agent_repair_memory
                    .implementation_target_lease
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        self.benchmark_case_ledger
                            .as_ref()
                            .and_then(target_lease_for_ledger)
                    });
                if let Some(repair_state) = self.benchmark_repair_state.as_mut()
                    && (repair_state.owner_path == *path
                        || active_target_lease
                            .as_ref()
                            .is_some_and(|target| canonical_path(target) == canonical_path(path)))
                {
                    let read_matches_target_lease = active_target_lease
                        .as_ref()
                        .is_some_and(|target| canonical_path(target) == canonical_path(path));
                    let workspace_owner_text = load_workspace_file_text(&workspace_root, path);
                    if let Some(observation) = observation.as_ref() {
                        let observed_content = observation.content.trim();
                        if !observed_content.is_empty() {
                            let observed_line_count = observed_content.lines().count();
                            let current_line_count = repair_state
                                .latest_owner_file_text
                                .as_deref()
                                .map(str::lines)
                                .map(Iterator::count)
                                .unwrap_or(0);
                            if observed_line_count >= current_line_count {
                                repair_state.latest_owner_file_text =
                                    Some(observation.content.clone());
                            }
                        }
                    }
                    match repair_state.phase {
                        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                            repair_state.failure_anchor_reread_attempted = true;
                            if let Some(honored_range) = honored_range
                                && repair_state
                                    .failure_anchor_range
                                    .is_some_and(|anchor_range| {
                                        read_range_overlap(honored_range, anchor_range) > 0
                                    })
                            {
                                let content = observation
                                    .as_ref()
                                    .map(|value| value.content.as_str())
                                    .unwrap_or_default();
                                let test_only = slice_is_test_only(
                                    content,
                                    repair_state.primary_failure_test_name.as_deref(),
                                );
                                repair_state.failure_anchor_reread_honored = true;
                                repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                    path: path.clone(),
                                    requested_range,
                                    honored_range: Some(honored_range),
                                    kind: OwnerSliceKind::FailureAnchor,
                                    test_only,
                                    slice_content: Some(content.to_string()),
                                });
                                if test_only {
                                    if let Some(target) = active_target_lease.as_ref()
                                        && canonical_path(target) != canonical_path(path)
                                    {
                                        repair_state.owner_path = target.clone();
                                        repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                        repair_state.implementation_reread_allowed = true;
                                        repair_state.implementation_suggested_range = None;
                                    } else {
                                        repair_state.phase =
                                            BenchmarkRepairPhase::NeedsImplementationRead;
                                        repair_state.implementation_reread_allowed = true;
                                        repair_state.implementation_suggested_range =
                                            suggest_implementation_range_from_owner_text(
                                                workspace_owner_text
                                                    .as_deref()
                                                    .or(repair_state
                                                        .latest_owner_file_text
                                                        .as_deref())
                                                    .unwrap_or(content),
                                                repair_state.primary_failure_test_name.as_deref(),
                                            );
                                        if let Some(owner_text) = workspace_owner_text.as_ref() {
                                            repair_state.latest_owner_file_text =
                                                Some(owner_text.clone());
                                        }
                                    }
                                } else {
                                    repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                }
                                repair_state.invalid_action_count = 0;
                            }
                        }
                        BenchmarkRepairPhase::NeedsImplementationRead => {
                            repair_state.implementation_reread_attempted = true;
                            if let Some(honored_range) = honored_range
                                && repair_state
                                    .failure_anchor_range
                                    .is_some_and(|anchor_range| {
                                        range_meaningfully_differs_from_anchor(
                                            honored_range,
                                            anchor_range,
                                        )
                                    })
                            {
                                repair_state.implementation_reread_honored = true;
                                repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                    path: path.clone(),
                                    requested_range,
                                    honored_range: Some(honored_range),
                                    kind: OwnerSliceKind::ImplementationAnchor,
                                    test_only: false,
                                    slice_content: observation
                                        .as_ref()
                                        .map(|value| value.content.clone()),
                                });
                                repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
                                repair_state.invalid_action_count = 0;
                            }
                        }
                        BenchmarkRepairPhase::NeedsPatch
                            if read_matches_target_lease
                                && observation
                                    .as_ref()
                                    .is_some_and(|value| !value.content.trim().is_empty()) =>
                        {
                            repair_state.last_owner_slice = Some(OwnerSliceRecord {
                                path: path.clone(),
                                requested_range,
                                honored_range,
                                kind: OwnerSliceKind::ImplementationAnchor,
                                test_only: false,
                                slice_content: observation
                                    .as_ref()
                                    .map(|value| value.content.clone()),
                            });
                            repair_state.invalid_action_count = 0;
                        }
                        BenchmarkRepairPhase::NeedsPatch
                        | BenchmarkRepairPhase::NeedsFastLoopRerun
                        | BenchmarkRepairPhase::Idle => {}
                    }
                    self.sync_benchmark_repair_state_to_ledger();
                }
            }
            if outcome.action().is_write_like() {
                self.record_benchmark_write_kind(outcome.action());
                if let Some(ledger) = self.benchmark_case_ledger.as_mut()
                    && ledger.validation_details.repair_required
                {
                    ledger.validation_details.post_fast_loop_patch_attempted = true;
                }
                if let AgentAction::ModifyToml { operations, .. } = outcome.action() {
                    self.agent_repair_memory.last_manifest_patch_operations = operations.clone();
                }
                self.agent_repair_memory.post_patch_diagnostic_class = None;
                self.agent_repair_memory.post_patch_diagnostic_excerpt = None;
                if let Some(repair_state) = self.benchmark_repair_state.as_mut()
                    && repair_state.phase == BenchmarkRepairPhase::NeedsPatch
                {
                    repair_state.phase = BenchmarkRepairPhase::NeedsFastLoopRerun;
                    repair_state.invalid_action_count = 0;
                    self.sync_benchmark_repair_state_to_ledger();
                }
                self.last_successful_write_action = Some(outcome.action().clone());
                self.repair_requirement = None;
            }
            if let AgentAction::SuggestEditAnchors {
                path,
                range,
                search_hint,
            } = outcome.action()
            {
                self.record_suggested_edit_anchor(path, *range, search_hint.as_deref());
            }
            if matches!(outcome.action(), AgentAction::PreviewEdit { .. }) {
                self.record_preview_edit(outcome.action(), output_text);
            }
        } else {
            self.last_failed_tool_error = Some(output_text.to_string());
            if outcome.action().is_write_like() {
                self.stall_count = 0;
                self.redundant_inspection_turns = 0;
                self.repair_recovery_turns_remaining = 1;
                self.repair_requirement =
                    repair_requirement_from_action(outcome.action(), output_text);
            }
        }

        match outcome.action() {
            AgentAction::ReplaceRange { .. } => {
                self.agent_repair_memory.scorecard.replace_range_count = self
                    .agent_repair_memory
                    .scorecard
                    .replace_range_count
                    .saturating_add(1);
                if output_text.contains("hash mismatch") {
                    self.agent_repair_memory
                        .scorecard
                        .replace_range_hash_mismatch_count = self
                        .agent_repair_memory
                        .scorecard
                        .replace_range_hash_mismatch_count
                        .saturating_add(1);
                }
            }
            AgentAction::ModifyToml { .. } => {
                self.agent_repair_memory.scorecard.modify_toml_count = self
                    .agent_repair_memory
                    .scorecard
                    .modify_toml_count
                    .saturating_add(1);
            }
            AgentAction::ApplyPreview { .. } => {
                self.agent_repair_memory.scorecard.apply_preview_count = self
                    .agent_repair_memory
                    .scorecard
                    .apply_preview_count
                    .saturating_add(1);
                if output_text.contains("hash mismatch")
                    || output_text.contains("preview_apply_mismatch")
                {
                    self.agent_repair_memory
                        .scorecard
                        .apply_preview_hash_mismatch_count = self
                        .agent_repair_memory
                        .scorecard
                        .apply_preview_hash_mismatch_count
                        .saturating_add(1);
                }
            }
            _ => {}
        }

        match outcome.action() {
            AgentAction::RunValidation { plan } => match outcome {
                ActionOutcome::Success { .. } => {
                    if self.validation_queue.is_empty() {
                        self.verified_green = true;
                    }
                    self.last_failing_verifier = None;
                    self.last_safe_checkpoint = Some(plan.summary());
                    if let Some(ledger) = self.benchmark_case_ledger.as_mut() {
                        if let Some(match_kind) = validation_plan_fast_loop_match_kind(ledger, plan)
                        {
                            self.validation_queue.clear();
                            self.verified_green = true;
                            ledger.validation_status = Some("green: fast-loop".to_string());
                            ledger.last_validation_failure = None;
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted = ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted
                                || ledger.validation_details.post_fast_loop_patch_attempted;
                            ledger.validation_details.repair_required = false;
                            self.benchmark_repair_state = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        } else {
                            ledger.validation_status = Some(format!("green: {}", plan.summary()));
                            ledger.last_validation_failure = None;
                        }
                    }
                }
                ActionOutcome::Failure { .. } => {
                    self.verified_green = false;
                    self.last_failing_verifier = Some(plan.summary());
                    self.validation_queue.clear();
                    self.record_validation_failure_memory(plan.summary(), output_text);
                    self.record_rolled_back_write_validation_failure(output_text);
                    if let Some(ledger) = self.benchmark_case_ledger.as_mut() {
                        if let Some(match_kind) = validation_plan_fast_loop_match_kind(ledger, plan)
                        {
                            record_fast_loop_validation_failure(ledger, output_text);
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            self.benchmark_repair_state =
                                benchmark_repair_state_from_ledger(ledger);
                            self.repair_requirement = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        } else {
                            ledger.validation_status = Some(format!("failed: {}", plan.summary()));
                            ledger.last_validation_failure =
                                Some(truncate_visible_text(output_text, 180));
                        }
                    }
                }
            },
            AgentAction::RunCommand { command, .. } => {
                if matches!(outcome, ActionOutcome::Failure { .. }) {
                    self.record_validation_failure_memory(command.clone(), output_text);
                }
                if let Some(ledger) = self.benchmark_case_ledger.as_mut()
                    && let Some(match_kind) = fast_loop_match_kind(ledger, command)
                {
                    match outcome {
                        ActionOutcome::Failure { .. } => {
                            record_fast_loop_validation_failure(ledger, output_text);
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            self.benchmark_repair_state =
                                benchmark_repair_state_from_ledger(ledger);
                            self.repair_requirement = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        }
                        ActionOutcome::Success { .. } => {
                            self.verified_green = true;
                            self.last_failing_verifier = None;
                            ledger.validation_status = Some("green: fast-loop".to_string());
                            ledger.last_validation_failure = None;
                            ledger.validation_details.fast_loop_rerun_match_kind =
                                Some(match_kind.label().to_string());
                            ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted = ledger
                                .validation_details
                                .post_fast_loop_validation_rerun_attempted
                                || ledger.validation_details.post_fast_loop_patch_attempted;
                            ledger.validation_details.repair_required = false;
                            self.benchmark_repair_state = None;
                            self.sync_benchmark_repair_state_to_ledger();
                        }
                    }
                }
            }
            action if action.is_write_like() => {
                if matches!(outcome, ActionOutcome::Success { .. }) {
                    self.has_mutating_change = true;
                    self.verified_green = false;
                }
            }
            _ => {}
        }

        summarize_tool_observation_for_transcript(
            outcome.action(),
            status,
            output_text,
            self.benchmark_transcript_compression,
            self.repair_requirement.as_ref(),
            self.benchmark_case_ledger.as_ref(),
        )
    }

    pub(crate) fn can_finish_without_more_actions(&self) -> bool {
        self.verified_green
    }
}
