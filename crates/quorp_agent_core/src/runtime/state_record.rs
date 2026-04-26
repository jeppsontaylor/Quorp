//! `AgentTaskState`: constructors, snapshot/restore, runtime summary,
//! and the recorder methods (`record_*`, `note_*`, observation slice
//! tracking, repair-requirement helpers).
//!
//! Carved out of runtime.rs's single inherent impl block so each child
//! file stays under the 2,000-LOC hard cap.

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
    pub(crate) fn new(request: &AgentRunRequest, config: AgentConfig) -> Self {
        let mut acceptance_criteria = vec![
            format!("Complete the requested goal: {}", request.goal.trim()),
            "Do not stop after edits unless validation is green.".to_string(),
        ];
        if let Some(evaluate_command) = request
            .run_metadata
            .get("evaluate_command")
            .and_then(serde_json::Value::as_str)
            .filter(|command| !command.trim().is_empty())
        {
            acceptance_criteria.push(format!(
                "Keep going until the visible evaluator succeeds: {evaluate_command}"
            ));
        }
        Self {
            goal: request.goal.clone(),
            current_mode: request.agent_mode,
            autonomy_profile: request.autonomy_profile,
            policy: config.policy.clone(),
            acceptance_criteria,
            working_set: BTreeSet::new(),
            workspace_root: request.project_root.display().to_string(),
            workspace_root_entries: metadata_string_list(
                &request.run_metadata,
                "editable_workspace_entries",
            )
            .or_else(|| metadata_string_list(&request.run_metadata, "workspace_root_entries"))
            .unwrap_or_default(),
            last_tool_summary: None,
            last_failing_verifier: None,
            last_safe_checkpoint: None,
            last_parse_error: None,
            last_failed_tool_error: None,
            stall_count: 0,
            redundant_inspection_turns: 0,
            recoverable_inspection_failures: 0,
            parser_recovery_failures: 0,
            parser_recovery_validation_fingerprint: None,
            parser_recovery_same_validation_streak: 0,
            has_mutating_change: false,
            verified_green: false,
            validation_queue: VecDeque::new(),
            config,
            total_billed_tokens: 0,
            repair_recovery_turns_remaining: 0,
            benchmark_transcript_compression: metadata_bool(
                &request.run_metadata,
                "benchmark_transcript_compression",
            )
            .unwrap_or_else(|| {
                metadata_bool(&request.run_metadata, "benchmark_mode").unwrap_or(false)
            }),
            benchmark_case_ledger: benchmark_case_ledger_from_metadata(&request.run_metadata),
            repair_requirement: None,
            last_successful_write_action: None,
            benchmark_repair_state: None,
            failed_edit_records: Vec::new(),
            agent_repair_memory: AgentRepairMemory::default(),
        }
    }

    pub(crate) fn snapshot(&self) -> AgentTaskStateSnapshot {
        AgentTaskStateSnapshot {
            current_mode: self.current_mode,
            acceptance_criteria: self.acceptance_criteria.clone(),
            working_set: self.working_set.clone(),
            last_tool_summary: self.last_tool_summary.clone(),
            last_failing_verifier: self.last_failing_verifier.clone(),
            last_safe_checkpoint: self.last_safe_checkpoint.clone(),
            last_parse_error: self.last_parse_error.clone(),
            stall_count: self.stall_count,
            redundant_inspection_turns: self.redundant_inspection_turns,
            recoverable_inspection_failures: self.recoverable_inspection_failures,
            parser_recovery_failures: self.parser_recovery_failures,
            parser_recovery_validation_fingerprint: self
                .parser_recovery_validation_fingerprint
                .clone(),
            parser_recovery_same_validation_streak: self.parser_recovery_same_validation_streak,
            has_mutating_change: self.has_mutating_change,
            verified_green: self.verified_green,
            validation_queue: self.validation_queue.clone(),
            total_billed_tokens: self.total_billed_tokens,
            last_failed_tool_error: self.last_failed_tool_error.clone(),
            repair_recovery_turns_remaining: self.repair_recovery_turns_remaining,
            benchmark_case_ledger: self.benchmark_case_ledger.clone(),
            repair_requirement: self.repair_requirement.clone(),
            last_successful_write_action: self.last_successful_write_action.clone(),
            benchmark_repair_state: self.benchmark_repair_state.clone(),
            failed_edit_records: self.failed_edit_records.clone(),
            agent_repair_memory: self.agent_repair_memory.clone(),
        }
    }

    pub(crate) fn restore(&mut self, snapshot: AgentTaskStateSnapshot) {
        self.current_mode = snapshot.current_mode;
        self.acceptance_criteria = snapshot.acceptance_criteria;
        self.working_set = snapshot.working_set;
        self.last_tool_summary = snapshot.last_tool_summary;
        self.last_failing_verifier = snapshot.last_failing_verifier;
        self.last_safe_checkpoint = snapshot.last_safe_checkpoint;
        self.last_parse_error = snapshot.last_parse_error;
        self.stall_count = snapshot.stall_count;
        self.redundant_inspection_turns = snapshot.redundant_inspection_turns;
        self.recoverable_inspection_failures = snapshot.recoverable_inspection_failures;
        self.parser_recovery_failures = snapshot.parser_recovery_failures;
        self.parser_recovery_validation_fingerprint =
            snapshot.parser_recovery_validation_fingerprint;
        self.parser_recovery_same_validation_streak =
            snapshot.parser_recovery_same_validation_streak;
        self.has_mutating_change = snapshot.has_mutating_change;
        self.verified_green = snapshot.verified_green;
        self.validation_queue = snapshot.validation_queue;
        self.total_billed_tokens = snapshot.total_billed_tokens;
        self.last_failed_tool_error = snapshot.last_failed_tool_error;
        self.repair_recovery_turns_remaining = snapshot.repair_recovery_turns_remaining;
        self.benchmark_case_ledger = snapshot.benchmark_case_ledger;
        self.repair_requirement = snapshot.repair_requirement;
        self.last_successful_write_action = snapshot.last_successful_write_action;
        self.benchmark_repair_state = snapshot.benchmark_repair_state;
        self.failed_edit_records = snapshot.failed_edit_records;
        self.agent_repair_memory = snapshot.agent_repair_memory;
    }

    pub(crate) fn runtime_summary(&self) -> String {
        let mut lines = vec![
            "[Runtime State]".to_string(),
            format!("Goal: {}", self.goal),
            format!("Mode: {}", self.current_mode.label()),
            format!("Autonomy profile: {}", self.autonomy_profile.label()),
            format!("Policy mode: {}", self.policy.mode.label()),
            format!("Workspace root: {}", self.workspace_root),
            format!(
                "Verification: {}",
                if self.verified_green {
                    "green"
                } else if self.has_mutating_change {
                    "pending"
                } else {
                    "not required yet"
                }
            ),
            format!("Stall count: {}", self.stall_count),
            format!(
                "Parser recovery failures: {}",
                self.parser_recovery_failures
            ),
            format!("Total billed tokens: {}", self.total_billed_tokens),
        ];
        if !self.acceptance_criteria.is_empty() {
            lines.push(format!(
                "Acceptance criteria: {}",
                self.acceptance_criteria.join(" | ")
            ));
        }
        if !self.working_set.is_empty() {
            let rendered = self
                .working_set
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Working set: {rendered}"));
        }
        if !self.workspace_root_entries.is_empty() {
            lines.push(format!(
                "Workspace entries: {}",
                self.workspace_root_entries
                    .iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(summary) = self.last_tool_summary.as_ref() {
            lines.push(format!("Last tool result: {summary}"));
        }
        if let Some(error) = self.last_failed_tool_error.as_ref() {
            lines.push(format!("Last failed tool error: {error}"));
        }
        if !self.failed_edit_records.is_empty() {
            lines.push(format!(
                "Failed edit memory: {}",
                render_failed_edit_memory(&self.failed_edit_records)
            ));
        }
        if self.benchmark_case_ledger.is_some() && !self.agent_repair_memory.is_empty() {
            lines.push(format!(
                "Agent memory: {}",
                render_agent_repair_memory(&self.agent_repair_memory)
            ));
        }
        if self.repair_recovery_turns_remaining > 0 {
            lines.push(format!(
                "Repair recovery turns remaining: {}",
                self.repair_recovery_turns_remaining
            ));
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref()
            && repair_state.phase != BenchmarkRepairPhase::Idle
        {
            lines.push(format!(
                "Benchmark repair phase: {}",
                repair_state.phase.label()
            ));
            lines.push(format!(
                "Repair next step: {}",
                benchmark_repair_phase_instruction(repair_state.phase)
            ));
            if let Some(range) = benchmark_repair_phase_suggested_range(repair_state) {
                lines.push(format!("Repair suggested range: {}", range.label()));
            }
        }
        if let Some(requirement) = self.repair_requirement.as_ref() {
            lines.push(format!(
                "Repair target: {} ({})",
                requirement.path,
                if requirement.exact_reread_completed {
                    "exact reread captured"
                } else {
                    "exact reread required before the next write"
                }
            ));
            if let Some(range) = requirement.suggested_range {
                lines.push(format!("Suggested reread range: {}", range.label()));
            }
        }
        if let Some(error) = self.last_parse_error.as_ref() {
            lines.push(format!("Last parse error: {error}"));
        }
        if let Some(verifier) = self.last_failing_verifier.as_ref() {
            lines.push(format!("Last failing verifier: {verifier}"));
        }
        if let Some(checkpoint) = self.last_safe_checkpoint.as_ref() {
            lines.push(format!("Last safe checkpoint: {checkpoint}"));
        }
        if !self.validation_queue.is_empty() {
            let queued = self
                .validation_queue
                .iter()
                .map(ValidationPlan::summary)
                .collect::<Vec<_>>()
                .join(" -> ");
            lines.push(format!("Queued validation: {queued}"));
        }
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            if ledger.validation_details.repair_required {
                lines.push("[Repair Phase] Stay on the owner file until the fast loop is green again. Do not widen scope, do not keep rereading without a concrete range, and do not stop after explanation-only turns.".to_string());
            }
            lines.push("[Benchmark Ledger]".to_string());
            if !ledger.case_class.is_empty() {
                lines.push(format!("Case class: {}", ledger.case_class));
            }
            if !ledger.owner_files.is_empty() {
                lines.push(format!(
                    "Owner files: {}",
                    render_short_list(&ledger.owner_files, 4)
                ));
            }
            if !ledger.expected_touch_targets.is_empty() {
                lines.push(format!(
                    "Expected touch targets: {}",
                    render_short_list(&ledger.expected_touch_targets, 5)
                ));
            }
            if !ledger.fast_loop_commands.is_empty() {
                lines.push(format!(
                    "Fast loop: {}",
                    render_short_list(&ledger.fast_loop_commands, 2)
                ));
            }
            if !ledger.named_tests.is_empty() {
                lines.push(format!(
                    "Named tests: {}",
                    render_short_list(&ledger.named_tests, 4)
                ));
            }
            if !ledger.companion_files_required.is_empty() {
                lines.push(format!(
                    "Companion files: {}",
                    render_short_list(&ledger.companion_files_required, 4)
                ));
            }
            if let Some(hypothesis) = ledger.current_hypothesis.as_ref() {
                lines.push(format!(
                    "Current hypothesis: {}",
                    truncate_visible_text(hypothesis, 180)
                ));
            }
            if let Some(status) = ledger.validation_status.as_ref() {
                lines.push(format!(
                    "Validation status: {}",
                    truncate_visible_text(status, 180)
                ));
            }
            if let Some(failure) = ledger.last_validation_failure.as_ref() {
                lines.push(format!(
                    "Last validation failure: {}",
                    truncate_visible_text(failure, 180)
                ));
            }
            if !ledger.validation_details.failing_test_names.is_empty() {
                lines.push(format!(
                    "Failing tests: {}",
                    render_short_list(&ledger.validation_details.failing_test_names, 4)
                ));
            }
            if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
                let line = ledger
                    .validation_details
                    .primary_failure_line
                    .map(|value| format!(":{value}"))
                    .unwrap_or_default();
                lines.push(format!("Primary failure location: {path}{line}"));
            }
            if let Some(test_name) = ledger.validation_details.primary_failure_test_name.as_ref() {
                lines.push(format!("Primary failure test: {test_name}"));
            }
            if let Some(assertion_excerpt) = ledger.validation_details.assertion_excerpt.as_ref() {
                lines.push(format!(
                    "Assertion excerpt: {}",
                    truncate_visible_text(assertion_excerpt, 180)
                ));
            }
            if ledger.validation_details.repair_required {
                lines.push("Repair required: true".to_string());
            }
            if let Some(phase) = ledger.validation_details.repair_phase_terminal.as_ref() {
                lines.push(format!("Repair phase terminal: {phase}"));
            }
            lines.push(format!(
                "Failure-anchor reread: attempted={} honored={}",
                ledger.validation_details.failure_anchor_reread_attempted,
                ledger.validation_details.failure_anchor_reread_honored
            ));
            lines.push(format!(
                "Implementation reread: allowed={} attempted={} honored={}",
                ledger.validation_details.implementation_reread_allowed,
                ledger.validation_details.implementation_reread_attempted,
                ledger.validation_details.implementation_reread_honored
            ));
            lines.push(format!(
                "Repair phase invalid actions: {}",
                ledger.validation_details.repair_phase_invalid_action_count
            ));
            if ledger.validation_details.patch_packet_injected {
                lines.push("Patch packet injected: true".to_string());
            }
            if let Some(range) = ledger
                .validation_details
                .patch_packet_honored_range
                .as_ref()
            {
                lines.push(format!("Patch packet range: {range}"));
            }
            if let Some(command) = ledger.validation_details.recommended_rerun_command.as_ref() {
                lines.push(format!("Recommended rerun command: {command}"));
            }
            if let Some(match_kind) = ledger
                .validation_details
                .fast_loop_rerun_match_kind
                .as_ref()
            {
                lines.push(format!("Fast-loop rerun match kind: {match_kind}"));
            }
            if !ledger.validation_details.failed_edit_records.is_empty() {
                lines.push(format!(
                    "Failed edit memory: {}",
                    render_failed_edit_memory(&ledger.validation_details.failed_edit_records)
                ));
            }
            lines.push(format!(
                "Post-fast-loop patch attempted: {}",
                ledger.validation_details.post_fast_loop_patch_attempted
            ));
            lines.push(format!(
                "Post-fast-loop validation rerun attempted: {}",
                ledger
                    .validation_details
                    .post_fast_loop_validation_rerun_attempted
            ));
        }
        lines.join("\n")
    }

    pub(crate) fn note_benchmark_hypothesis(
        &mut self,
        assistant_message: &str,
        task_updates: &[crate::agent_turn::TaskItem],
    ) {
        let Some(ledger) = self.benchmark_case_ledger.as_mut() else {
            return;
        };
        let preferred_task = task_updates
            .iter()
            .find(|item| !item.title.trim().is_empty())
            .map(|item| item.title.trim().to_string());
        let candidate = preferred_task
            .or_else(|| {
                let text = assistant_message.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
            .map(|text| truncate_visible_text(&text, 180));
        if let Some(candidate) = candidate {
            ledger.current_hypothesis = Some(candidate);
        }
    }

    pub(crate) fn sync_benchmark_repair_state_to_ledger(&mut self) {
        self.prime_benchmark_patch_target_requirement();
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            self.agent_repair_memory.diagnostic_class =
                ledger.validation_details.diagnostic_class.clone();
            self.agent_repair_memory.dependency_candidates =
                benchmark_dependency_candidates(ledger);
            self.agent_repair_memory.ranked_implementation_targets =
                ranked_implementation_targets_for_ledger(ledger);
            self.agent_repair_memory.implementation_target_lease = target_lease_for_ledger(ledger);
        } else {
            self.agent_repair_memory.dependency_candidates.clear();
        }
        self.agent_repair_memory.current_required_action =
            repair_requirement_action_label(self.repair_requirement.as_ref()).or_else(|| {
                benchmark_required_action_label(
                    self.benchmark_repair_state.as_ref(),
                    self.benchmark_case_ledger.as_ref(),
                    &self.agent_repair_memory,
                )
            });
        self.agent_repair_memory.scorecard.preview_apply_locked =
            preview_apply_locked(&self.agent_repair_memory);
        let Some(ledger) = self.benchmark_case_ledger.as_mut() else {
            return;
        };
        ledger.validation_details.failed_edit_records = self.failed_edit_records.clone();
        ledger.validation_details.implementation_target_lease = target_lease_for_ledger(ledger);
        if let Some(repair_state) = self.benchmark_repair_state.as_ref() {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory)
                    .into_owned();
            self.agent_repair_memory.repair_phase =
                Some(repair_state.phase.state_label().to_string());
            self.agent_repair_memory.scorecard.repair_submode_entered = true;
            self.agent_repair_memory.target_dependency_table =
                benchmark_target_dependency_table(repair_state, ledger, &patch_target)
                    .map(str::to_string);
            self.agent_repair_memory.scorecard.repair_write_locked =
                benchmark_patch_phase_write_locked(
                    repair_state,
                    ledger,
                    &self.agent_repair_memory,
                    self.repair_requirement.as_ref(),
                );
            self.agent_repair_memory.scorecard.patch_scaffold_offered = repair_state.phase
                == BenchmarkRepairPhase::NeedsPatch
                && patch_target.ends_with(".toml");
            self.agent_repair_memory.context_sufficient = matches!(
                repair_state.phase,
                BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun
            );
            ledger.validation_details.primary_failure_test_name =
                repair_state.primary_failure_test_name.clone();
            ledger.validation_details.repair_phase_terminal =
                Some(repair_state.phase.label().to_string());
            ledger.validation_details.failure_anchor_reread_attempted =
                repair_state.failure_anchor_reread_attempted;
            ledger.validation_details.failure_anchor_reread_honored =
                repair_state.failure_anchor_reread_honored;
            ledger.validation_details.implementation_reread_allowed =
                repair_state.implementation_reread_allowed;
            ledger.validation_details.implementation_reread_attempted =
                repair_state.implementation_reread_attempted;
            ledger.validation_details.implementation_reread_honored =
                repair_state.implementation_reread_honored;
            ledger.validation_details.repair_phase_invalid_action_count =
                repair_state.invalid_action_count;
            if matches!(
                repair_state.phase,
                BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun
            ) {
                ledger.validation_details.patch_packet_injected = true;
                ledger.validation_details.patch_packet_honored_range = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .map(|range| range.label());
                ledger.validation_details.recommended_rerun_command =
                    recommended_fast_loop_rerun_command(ledger);
            }
        } else {
            self.agent_repair_memory.repair_phase =
                Some(BenchmarkRepairPhase::Idle.state_label().to_string());
            self.agent_repair_memory.context_sufficient = false;
            self.agent_repair_memory.target_dependency_table = None;
            self.agent_repair_memory.scorecard.repair_write_locked = false;
            self.agent_repair_memory.scorecard.patch_scaffold_offered = false;
            self.agent_repair_memory.scorecard.preview_apply_locked = false;
            ledger.validation_details.repair_phase_terminal =
                Some(BenchmarkRepairPhase::Idle.label().to_string());
            ledger.validation_details.failure_anchor_reread_attempted = false;
            ledger.validation_details.failure_anchor_reread_honored = false;
            ledger.validation_details.implementation_reread_allowed = false;
            ledger.validation_details.implementation_reread_attempted = false;
            ledger.validation_details.implementation_reread_honored = false;
            ledger.validation_details.repair_phase_invalid_action_count = 0;
        }
    }

    pub(crate) fn record_invalid_turn(&mut self, step: usize, error_class: &str, summary: &str) {
        self.agent_repair_memory.scorecard.parser_recovery_count = self
            .agent_repair_memory
            .scorecard
            .parser_recovery_count
            .saturating_add(1);
        push_capped(
            &mut self.agent_repair_memory.invalid_turns,
            AgentRepairInvalidTurn {
                step,
                error_class: error_class.to_string(),
                summary: truncate_visible_text(summary, 180),
            },
            12,
        );
    }

    pub(crate) fn benchmark_repair_submode_active(&self) -> bool {
        self.benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required)
            && self
                .benchmark_repair_state
                .as_ref()
                .is_some_and(|repair_state| repair_state.phase != BenchmarkRepairPhase::Idle)
    }

    pub(crate) fn note_repair_submode_turn(&mut self) {
        if self.benchmark_repair_submode_active() {
            self.agent_repair_memory.scorecard.repair_submode_entered = true;
            self.agent_repair_memory.scorecard.repair_submode_turns = self
                .agent_repair_memory
                .scorecard
                .repair_submode_turns
                .saturating_add(1);
        }
    }

    pub(crate) fn reset_parser_recovery_tracking(&mut self) {
        self.parser_recovery_validation_fingerprint = None;
        self.parser_recovery_same_validation_streak = 0;
    }

    pub(crate) fn benchmark_validation_fingerprint(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if !ledger.validation_details.repair_required {
            return None;
        }
        let repair_phase = self
            .benchmark_repair_state
            .as_ref()
            .map(|repair_state| repair_state.phase.label())
            .unwrap_or("idle");
        let target_lease = target_lease_for_ledger(ledger).unwrap_or_default();
        let requirement = self
            .repair_requirement
            .as_ref()
            .map(|requirement| {
                let range = requirement
                    .suggested_range
                    .map(|value| value.label())
                    .unwrap_or_else(|| "full-file".to_string());
                format!(
                    "{}:{}:{}",
                    requirement.path, range, requirement.exact_reread_completed
                )
            })
            .unwrap_or_default();
        Some(short_text_fingerprint(&format!(
            "{}|{}|{}|{}|{}|{}|{}",
            ledger
                .last_validation_failure
                .as_deref()
                .unwrap_or_default(),
            ledger
                .validation_details
                .diagnostic_class
                .as_deref()
                .unwrap_or_default(),
            repair_phase,
            target_lease,
            ledger.validation_details.post_fast_loop_patch_attempted,
            ledger
                .validation_details
                .post_fast_loop_validation_rerun_attempted,
            requirement
        )))
    }

    pub(crate) fn note_parser_recovery_failure(
        &mut self,
        step: usize,
        error_class: &str,
        summary: &str,
    ) -> bool {
        self.parser_recovery_failures = self.parser_recovery_failures.saturating_add(1);
        self.last_parse_error = Some(summary.to_string());
        self.record_invalid_turn(step, error_class, summary);
        let Some(fingerprint) = self.benchmark_validation_fingerprint() else {
            self.reset_parser_recovery_tracking();
            return false;
        };
        if self.parser_recovery_validation_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            self.parser_recovery_same_validation_streak = self
                .parser_recovery_same_validation_streak
                .saturating_add(1);
        } else {
            self.parser_recovery_validation_fingerprint = Some(fingerprint);
            self.parser_recovery_same_validation_streak = 1;
        }
        self.benchmark_repair_submode_active() && self.parser_recovery_same_validation_streak >= 2
    }

    pub(crate) fn repair_requirement_prefers_full_file(requirement: &RepairRequirement) -> bool {
        requirement.path.trim().ends_with(".toml")
    }

    pub(crate) fn repair_requirement_read_is_valid(
        requirement: &RepairRequirement,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
    ) -> bool {
        if path != requirement.path {
            return false;
        }
        if Self::repair_requirement_prefers_full_file(requirement) {
            return range.and_then(|value| value.normalized()).is_none();
        }
        range.and_then(|value| value.normalized()).is_some()
    }

    pub(crate) fn repair_requirement_prompt(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            format!(
                "Issue exactly one `ReadFile` for `{}` before any next write.",
                requirement.path
            )
        } else {
            format!(
                "Issue exactly one focused `ReadFile` for `{}` before any next write.",
                requirement.path
            )
        }
    }

    pub(crate) fn repair_requirement_correction(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            "Correction: emit exactly one full-file `ReadFile` now. Do not patch, rerun tests, search, or widen scope first."
                .to_string()
        } else {
            "Correction: emit exactly one `ReadFile` with a concrete line range now. Do not patch, rerun tests, search, or widen scope first."
                .to_string()
        }
    }

    pub(crate) fn repair_requirement_next_step(requirement: &RepairRequirement) -> String {
        if Self::repair_requirement_prefers_full_file(requirement) {
            "Next step: issue a fresh full-file `ReadFile` for the same path. Then patch or run the smallest relevant validation. The next write will be refused until that reread succeeds. Do not patch from memory and do not widen scope yet."
                .to_string()
        } else {
            "Next step: issue a fresh `ReadFile` for the same path with a focused line range. Then patch or run the smallest relevant validation. The next write will be refused until that anchored reread succeeds. Do not patch from memory and do not widen scope yet."
                .to_string()
        }
    }

    pub(crate) fn prime_benchmark_patch_target_requirement(&mut self) {
        if self.repair_requirement.is_some() {
            return;
        }
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return;
        };
        let Some(repair_state) = self.benchmark_repair_state.as_ref() else {
            return;
        };
        if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
            return;
        }
        let patch_target =
            benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
        if patch_target_context_loaded(
            repair_state,
            &self.agent_repair_memory,
            patch_target.as_ref(),
        ) {
            return;
        }
        if patch_target.as_ref().ends_with(".toml") {
            if ledger.validation_details.diagnostic_class.as_deref()
                != Some("manifest_dependency_error")
            {
                return;
            }
            self.repair_requirement = Some(RepairRequirement {
                path: patch_target.into_owned(),
                failure_reason: "manifest_dependency_error".to_string(),
                previous_search_block: None,
                suggested_range: None,
                exact_reread_completed: false,
            });
            return;
        }
        let suggested_range = repair_state.implementation_suggested_range.or_else(|| {
            load_workspace_file_text(&self.workspace_root, patch_target.as_ref()).and_then(
                |owner_text| {
                    suggest_source_patch_range_from_failure(
                        &owner_text,
                        ledger
                            .last_validation_failure
                            .as_deref()
                            .or(ledger.validation_details.assertion_excerpt.as_deref()),
                    )
                },
            )
        });
        let Some(suggested_range) = suggested_range else {
            return;
        };
        self.repair_requirement = Some(RepairRequirement {
            path: patch_target.into_owned(),
            failure_reason: ledger
                .validation_details
                .diagnostic_class
                .clone()
                .unwrap_or_else(|| "source_patch_context".to_string()),
            previous_search_block: None,
            suggested_range: Some(suggested_range),
            exact_reread_completed: false,
        });
    }

    pub(crate) fn record_line_oriented_parse(&mut self) {
        self.agent_repair_memory.scorecard.line_oriented_parse_count = self
            .agent_repair_memory
            .scorecard
            .line_oriented_parse_count
            .saturating_add(1);
    }

    pub(crate) fn record_canonical_action(&mut self, step: usize, action: &AgentAction) {
        push_capped(
            &mut self.agent_repair_memory.canonical_action_history,
            canonical_action_record(step, action, self.benchmark_case_ledger.as_ref()),
            32,
        );
    }

    pub(crate) fn record_rejected_actions(
        &mut self,
        phase: BenchmarkRepairPhase,
        actions: &[AgentAction],
        reason: &str,
    ) {
        push_capped(
            &mut self.agent_repair_memory.rejected_actions,
            AgentRepairRejectedAction {
                phase: phase.label().to_string(),
                actions: actions.iter().map(AgentAction::summary).collect(),
                reason: truncate_visible_text(reason, 220),
            },
            12,
        );
        if actions
            .iter()
            .any(|action| action_is_validation_like(action, self.benchmark_case_ledger.as_ref()))
        {
            self.agent_repair_memory
                .scorecard
                .rejected_validation_alias_count = self
                .agent_repair_memory
                .scorecard
                .rejected_validation_alias_count
                .saturating_add(1);
        }
        if reason.contains("test file") || reason.contains("test-file") {
            self.agent_repair_memory.scorecard.test_edit_rejection_count = self
                .agent_repair_memory
                .scorecard
                .test_edit_rejection_count
                .saturating_add(1);
        }
        if reason.contains("target lease") || reason.contains("evidence file") {
            self.agent_repair_memory.scorecard.target_redirect_count = self
                .agent_repair_memory
                .scorecard
                .target_redirect_count
                .saturating_add(1);
        }
        if reason.contains("evidence file")
            || reason.contains("test file")
            || reason.contains("test-file")
        {
            self.agent_repair_memory
                .scorecard
                .evidence_file_fixation_count = self
                .agent_repair_memory
                .scorecard
                .evidence_file_fixation_count
                .saturating_add(1);
        }
    }

    pub(crate) fn record_validation_failure_memory(&mut self, command: String, summary: &str) {
        push_capped(
            &mut self.agent_repair_memory.validation_failures,
            AgentRepairValidationFailure {
                command,
                summary: truncate_visible_text(summary, 260),
            },
            6,
        );
    }

    pub(crate) fn record_observed_slice(
        &mut self,
        path: &str,
        requested_range: Option<crate::agent_protocol::ReadFileRange>,
        honored_range: Option<crate::agent_protocol::ReadFileRange>,
        purpose: Option<String>,
        content: &str,
        content_hash: Option<&str>,
    ) {
        if let Some(honored_range) = honored_range {
            let repeated = self
                .agent_repair_memory
                .observed_slices
                .iter()
                .filter(|slice| slice.path == path)
                .filter_map(|slice| slice.honored_range)
                .any(|previous_range| ranges_substantially_overlap(previous_range, honored_range));
            if repeated {
                self.agent_repair_memory.scorecard.redundant_read_count = self
                    .agent_repair_memory
                    .scorecard
                    .redundant_read_count
                    .saturating_add(1);
            }
        }
        push_capped(
            &mut self.agent_repair_memory.observed_slices,
            AgentRepairObservedSlice {
                path: path.to_string(),
                requested_range,
                honored_range,
                purpose,
                content_fingerprint: content_hash
                    .map(str::trim)
                    .filter(|value| is_stable_content_hash(value))
                    .map(str::to_string)
                    .or_else(|| (!content.trim().is_empty()).then(|| stable_content_hash(content))),
            },
            12,
        );
        if let Some(requirement) = self.repair_requirement.as_mut()
            && requirement.path == path
        {
            let reread_satisfies_requirement = match requirement.suggested_range {
                Some(suggested_range) => honored_range
                    .is_some_and(|range| ranges_substantially_overlap(range, suggested_range)),
                None => honored_range.is_none(),
            };
            if reread_satisfies_requirement {
                requirement.exact_reread_completed = true;
            }
        }
    }

    pub(crate) fn record_first_valid_write_step(&mut self, step: usize) {
        if self
            .agent_repair_memory
            .scorecard
            .first_valid_write_step
            .is_none()
        {
            self.agent_repair_memory.scorecard.first_valid_write_step = Some(step);
        }
    }

    pub(crate) fn record_benchmark_write_kind(&mut self, action: &AgentAction) {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return;
        };
        if !ledger.validation_details.repair_required {
            return;
        }
        if self.benchmark_support_write_target_path(action).is_some() {
            self.agent_repair_memory.scorecard.support_write_count = self
                .agent_repair_memory
                .scorecard
                .support_write_count
                .saturating_add(1);
        } else {
            self.agent_repair_memory.scorecard.source_write_count = self
                .agent_repair_memory
                .scorecard
                .source_write_count
                .saturating_add(1);
        }
    }

    pub(crate) fn benchmark_support_write_target_path(&self, action: &AgentAction) -> Option<String> {
        let target_path = match action {
            AgentAction::PreviewEdit { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::SetExecutable { path } => Some(path.clone()),
            AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::ReplaceRange { path, .. } => Some(path.clone()),
            AgentAction::ApplyPreview { .. } => self
                .agent_repair_memory
                .last_preview_result
                .as_deref()
                .and_then(|output| extract_labeled_line(output, "path:"))
                .or_else(|| {
                    (self.agent_repair_memory.preview_origin.as_deref()
                        == Some("write_locked_manifest"))
                    .then(|| "Cargo.toml".to_string())
                }),
            _ => None,
        }?;
        let path = target_path.trim();
        (path.ends_with(".toml") || is_obvious_test_file(path)).then(|| path.to_string())
    }

    pub(crate) fn should_preserve_support_write_for_validation(&self, action: &AgentAction) -> bool {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        if !action_is_validation_like(action, Some(ledger)) {
            return false;
        }
        let Some(last_write) = self.last_successful_write_action.as_ref() else {
            return false;
        };
        self.benchmark_support_write_target_path(last_write)
            .is_some()
    }

    pub(crate) fn record_controller_injected_read(&mut self) {
        self.agent_repair_memory
            .scorecard
            .controller_injected_read_count = self
            .agent_repair_memory
            .scorecard
            .controller_injected_read_count
            .saturating_add(1);
    }

    pub(crate) fn record_suggested_edit_anchor(
        &mut self,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
        search_hint: Option<&str>,
    ) {
        self.agent_repair_memory.scorecard.anchor_suggestion_count = self
            .agent_repair_memory
            .scorecard
            .anchor_suggestion_count
            .saturating_add(1);
        push_capped(
            &mut self.agent_repair_memory.suggested_edit_anchors,
            AgentRepairSuggestedEditAnchor {
                path: path.to_string(),
                range,
                search_hint: search_hint
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            },
            8,
        );
    }

    pub(crate) fn record_preview_edit(&mut self, action: &AgentAction, output_text: &str) {
        self.agent_repair_memory.scorecard.preview_edit_count = self
            .agent_repair_memory
            .scorecard
            .preview_edit_count
            .saturating_add(1);
        let preview_id = extract_preview_id(output_text);
        if output_text.contains("would_apply: true") || output_text.contains("would_apply=true") {
            self.agent_repair_memory
                .scorecard
                .preview_edit_success_count = self
                .agent_repair_memory
                .scorecard
                .preview_edit_success_count
                .saturating_add(1);
        }
        if preview_id.is_some() {
            self.agent_repair_memory.scorecard.preview_created_count = self
                .agent_repair_memory
                .scorecard
                .preview_created_count
                .saturating_add(1);
            self.agent_repair_memory.last_preview_id = preview_id;
            self.agent_repair_memory.last_preview_path = action_target_path(action);
            self.agent_repair_memory.preview_origin =
                self.current_preview_origin().map(str::to_string);
        }
        if output_text.contains("syntax_preflight:") {
            self.agent_repair_memory.scorecard.syntax_preview_count = self
                .agent_repair_memory
                .scorecard
                .syntax_preview_count
                .saturating_add(1);
            if output_text.contains("syntax_preflight: failed") {
                self.agent_repair_memory
                    .scorecard
                    .syntax_preview_failure_count = self
                    .agent_repair_memory
                    .scorecard
                    .syntax_preview_failure_count
                    .saturating_add(1);
            }
        }
        self.agent_repair_memory.last_preview_result =
            Some(truncate_visible_text(output_text, 260));
    }

    pub(crate) fn current_preview_origin(&self) -> Option<&'static str> {
        let repair_state = self.benchmark_repair_state.as_ref()?;
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let patch_target =
            benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
        let write_locked = benchmark_patch_phase_write_locked(
            repair_state,
            ledger,
            &self.agent_repair_memory,
            self.repair_requirement.as_ref(),
        );
        if write_locked && patch_target.as_ref().ends_with(".toml") {
            Some("write_locked_manifest")
        } else {
            None
        }
    }

    pub(crate) fn record_redundant_inspection_turn(&mut self) {
        self.agent_repair_memory.scorecard.redundant_read_count = self
            .agent_repair_memory
            .scorecard
            .redundant_read_count
            .saturating_add(1);
    }

    pub(crate) fn record_failed_edit(
        &mut self,
        action: &AgentAction,
        failure_reason: &str,
    ) -> Option<FailedEditRecord> {
        let mut record = failed_edit_record_from_action(action, failure_reason)?;
        if let Some(existing) = self
            .failed_edit_records
            .iter_mut()
            .find(|existing| failed_edit_signature_matches(existing, &record))
        {
            existing.attempts = existing.attempts.saturating_add(1);
            existing.failure_reason = record.failure_reason;
            existing.matching_line_numbers = record.matching_line_numbers;
            self.agent_repair_memory
                .scorecard
                .repeated_failed_edit_count = self
                .agent_repair_memory
                .scorecard
                .repeated_failed_edit_count
                .saturating_add(1);
            record = existing.clone();
        } else {
            record.attempts = 1;
            self.failed_edit_records.push(record.clone());
            const MAX_FAILED_EDIT_RECORDS: usize = 8;
            if self.failed_edit_records.len() > MAX_FAILED_EDIT_RECORDS {
                let overflow = self
                    .failed_edit_records
                    .len()
                    .saturating_sub(MAX_FAILED_EDIT_RECORDS);
                self.failed_edit_records.drain(0..overflow);
            }
        }
        self.sync_benchmark_repair_state_to_ledger();
        Some(record)
    }

    pub(crate) fn record_rolled_back_write_validation_failure(&mut self, failure_reason: &str) {
        if !failure_reason.contains("safely rolled back") {
            return;
        }
        let Some(action) = self.last_successful_write_action.clone() else {
            return;
        };
        let Some(requirement) = repair_requirement_from_action(&action, failure_reason) else {
            return;
        };
        self.last_failed_tool_error = Some(failure_reason.to_string());
        self.agent_repair_memory.last_rollback_diagnostic =
            Some(truncate_visible_text(failure_reason, 260));
        self.agent_repair_memory.post_patch_diagnostic_class =
            classify_benchmark_diagnostic(failure_reason);
        self.agent_repair_memory.post_patch_diagnostic_excerpt =
            extract_assertion_excerpt(failure_reason)
                .or_else(|| Some(truncate_visible_text(failure_reason, 220)));
        self.agent_repair_memory.scorecard.rolled_back_write_count = self
            .agent_repair_memory
            .scorecard
            .rolled_back_write_count
            .saturating_add(1);
        if action_target_path(&action)
            .as_deref()
            .is_some_and(|path| !is_support_or_generated_runtime_path(path))
        {
            self.agent_repair_memory
                .scorecard
                .rolled_back_non_support_edit_count = self
                .agent_repair_memory
                .scorecard
                .rolled_back_non_support_edit_count
                .saturating_add(1);
        }
        if let AgentAction::ModifyToml { operations, .. } = &action {
            self.agent_repair_memory.last_manifest_patch_operations = operations.clone();
        }
        self.repair_requirement = Some(requirement);
        self.repair_recovery_turns_remaining = 1;
        self.stall_count = 0;
        self.redundant_inspection_turns = 0;
        if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsPatch;
            repair_state.invalid_action_count = 0;
        }
        self.sync_benchmark_repair_state_to_ledger();
    }

}
