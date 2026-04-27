//! `AgentTaskState`: action-validation gates and repair-requirement
//! enforcement (narrow repair, target lease, evidence satisfaction,
//! baseline validation, required-read injection logic).

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
    pub(crate) fn benchmark_narrow_repair_restricts_action(
        &self,
        action: &AgentAction,
    ) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if ledger.case_class != "narrow-owner-first" || !ledger.validation_details.repair_required {
            return None;
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref()
            && benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            )
        {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
            match action {
                AgentAction::ReadFile { .. }
                | AgentAction::ListDirectory { .. }
                | AgentAction::SearchText { .. }
                | AgentAction::SearchSymbols { .. }
                | AgentAction::GetRepoCapsule { .. }
                | AgentAction::ExplainValidationFailure { .. }
                | AgentAction::SuggestImplementationTargets { .. }
                | AgentAction::SuggestEditAnchors { .. } => {
                    return Some(format!(
                        "benchmark_autonomous write-locked patch phase requires acting on `{}` now; do not reread, search, list, or widen scope first",
                        patch_target
                    ));
                }
                _ => {}
            }
        }
        if self
            .benchmark_repair_state
            .as_ref()
            .is_some_and(|repair_state| {
                repair_state.phase == BenchmarkRepairPhase::NeedsFailureAnchorRead
                    && self.benchmark_evidence_action_satisfies(
                        &repair_state.owner_path,
                        repair_state.failure_anchor_range,
                        action,
                    )
            })
        {
            return None;
        }
        let owner_scope = |path: &str| {
            ledger.owner_files.iter().any(|candidate| candidate == path)
                || ledger
                    .expected_touch_targets
                    .iter()
                    .any(|candidate| candidate == path)
        };
        match action {
            AgentAction::ListDirectory { .. }
            | AgentAction::SearchText { .. }
            | AgentAction::SearchSymbols { .. }
            | AgentAction::GetRepoCapsule { .. } => Some(
                "benchmark_autonomous narrow repair mode keeps you on the owner file after a failed fast loop; do not widen to broad repo exploration yet"
                    .to_string(),
            ),
            AgentAction::ReadFile { path, .. }
            | AgentAction::SuggestEditAnchors { path, .. }
            | AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } if !owner_scope(path) => Some(format!(
                "benchmark_autonomous narrow repair mode is restricted to owner files and expected touch targets after a failed fast loop; `{path}` is outside that scope"
            )),
            _ => None,
        }
    }

    pub(crate) fn benchmark_target_lease_violation(&self, action: &AgentAction) -> Option<String> {
        let repair_state = self.benchmark_repair_state.as_ref()?;
        if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
            return None;
        }
        let lease = self
            .agent_repair_memory
            .implementation_target_lease
            .as_deref()
            .filter(|value| !value.trim().is_empty())?;
        let target_path = match action {
            AgentAction::SuggestEditAnchors { path, .. }
            | AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } => path,
            _ => return None,
        };
        if canonical_path(target_path) == canonical_path(lease) {
            return None;
        }
        let evidence_label = if is_obvious_test_file(target_path) {
            "test evidence file"
        } else {
            "non-leased evidence file"
        };
        Some(format!(
            "benchmark_autonomous target lease redirect: `{target_path}` is a {evidence_label}; the current target lease is `{lease}`. Use SuggestEditAnchors, PreviewEdit, ApplyPatch, ranged ReplaceBlock, or WriteFile on the leased implementation target only until validation changes the failure."
        ))
    }

    pub(crate) fn benchmark_evidence_action_satisfies(
        &self,
        owner_path: &str,
        failure_anchor_range: Option<crate::agent_protocol::ReadFileRange>,
        action: &AgentAction,
    ) -> bool {
        match action {
            AgentAction::ReadFile { path, range } => {
                if path == owner_path {
                    if let Some(anchor_range) = failure_anchor_range {
                        return range.and_then(|value| value.normalized()).is_some_and(
                            |requested_range| read_range_overlap(requested_range, anchor_range) > 0,
                        );
                    }
                    return true;
                }
                self.benchmark_related_evidence_path(path)
            }
            AgentAction::SearchText { query, .. } | AgentAction::SearchSymbols { query, .. } => {
                !query.trim().is_empty()
            }
            AgentAction::GetRepoCapsule { .. }
            | AgentAction::ExplainValidationFailure { .. }
            | AgentAction::SuggestImplementationTargets { .. } => true,
            AgentAction::SuggestEditAnchors { path, .. } => {
                path == owner_path || self.benchmark_related_evidence_path(path)
            }
            _ => false,
        }
    }

    pub(crate) fn benchmark_related_evidence_path(&self, path: &str) -> bool {
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        ledger.owner_files.iter().any(|candidate| candidate == path)
            || ledger
                .expected_touch_targets
                .iter()
                .any(|candidate| candidate == path)
            || ledger
                .companion_files_required
                .iter()
                .any(|candidate| candidate == path)
            || ledger
                .validation_details
                .primary_failure_path
                .as_ref()
                .is_some_and(|candidate| candidate == path)
            || is_obvious_test_file(path)
    }

    pub(crate) fn benchmark_needs_baseline_validation(&self) -> bool {
        self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            !self.has_mutating_change
                && ledger.last_validation_failure.is_none()
                && ledger.validation_status.is_none()
                && self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .all(|action| !action.validation_like)
                && ledger
                    .fast_loop_commands
                    .iter()
                    .any(|command| !command.trim().is_empty())
        })
    }

    pub(crate) fn benchmark_baseline_validation_message(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let command = ledger
            .fast_loop_commands
            .iter()
            .find(|command| !command.trim().is_empty())?
            .trim();
        Some(
            [
                "[Benchmark State] You have inspected context but have not established a failing validation anchor yet.",
                "Required next action: run the exact baseline fast loop now. Do not keep reading or searching before this validation.",
                &format!("Exact validation command: {command}"),
                "Response shape: return one raw JSON object only.",
                "Minimal JSON example:",
                &rerun_phase_parser_recovery_example(command),
            ]
            .join("\n"),
        )
    }

    pub(crate) fn repeated_validation_repair_message(
        &self,
        action_summary: &str,
        error: &str,
    ) -> String {
        let mut lines = vec![format!(
            "[Repair Phase]\nThe action `{action_summary}` was rejected because validation already exposed the failure and no repair write has been made yet."
        )];
        lines.push(error.to_string());
        if let Some(message) = self.benchmark_repair_phase_message() {
            lines.push(message);
            return lines.join("\n");
        }
        if let Some(ledger) = self.benchmark_case_ledger.as_ref() {
            let owner_path = ledger
                .owner_files
                .iter()
                .chain(ledger.expected_touch_targets.iter())
                .find(|path| !path.trim().is_empty())
                .map(String::as_str)
                .unwrap_or("[owner file]");
            lines.push(format!("Owner path: {owner_path}"));
            if let Some(failure) = ledger.last_validation_failure.as_ref() {
                lines.push(format!(
                    "Last validation failure: {}",
                    truncate_visible_text(failure, 260)
                ));
            }
            lines.push(
                "Required next action: either read one focused owner slice, ask for edit anchors, or patch the owner file. Do not rerun validation again until after a write."
                    .to_string(),
            );
            lines.push("Allowed actions: ReadFile with a concrete range, SuggestEditAnchors, ApplyPatch, ranged ReplaceBlock, or WriteFile.".to_string());
            lines.push("Minimal focused-read JSON example:".to_string());
            lines.push(focused_read_parser_recovery_example(owner_path, None));
        }
        lines.join("\n")
    }

    pub(crate) fn turn_repeats_known_inspection_only(&self, actions: &[AgentAction]) -> bool {
        !actions.is_empty()
            && actions.iter().all(|action| match action {
                AgentAction::ReadFile { path, range } => {
                    self.working_set.contains(path)
                        && !self.allow_benchmark_focused_same_file_reread(path, *range)
                }
                AgentAction::ListDirectory { path } => self.working_set.contains(path),
                AgentAction::SearchText { query, .. } => self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .any(|record| record.signature == format!("search_text:{}", query.trim())),
                AgentAction::SearchSymbols { query, .. } => self
                    .agent_repair_memory
                    .canonical_action_history
                    .iter()
                    .any(|record| record.signature == format!("search_symbols:{}", query.trim())),
                AgentAction::GetRepoCapsule { query, .. } => {
                    let query = query.as_deref().unwrap_or("").trim();
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| record.signature == format!("repo_capsule:{query}"))
                }
                AgentAction::SuggestEditAnchors {
                    path,
                    range,
                    search_hint,
                } => {
                    let range = range
                        .and_then(|value| value.normalized())
                        .map(|value| value.label())
                        .unwrap_or_else(|| "all".to_string());
                    let hint = search_hint.as_deref().unwrap_or("").trim();
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| {
                            record.signature
                                == format!("anchors:{}:{}:{}", canonical_path(path), range, hint)
                        })
                }
                AgentAction::PreviewEdit { path, edit } => {
                    let signature = format!(
                        "preview:{}:{}",
                        canonical_path(path),
                        short_text_fingerprint(&format!("{edit:?}"))
                    );
                    self.agent_repair_memory
                        .canonical_action_history
                        .iter()
                        .any(|record| record.signature == signature)
                }
                _ => false,
            })
    }

    pub(crate) fn repair_requirement_range_guidance(
        &self,
        actions: &[AgentAction],
    ) -> Option<String> {
        let requirement = self.repair_requirement.as_ref()?;
        if requirement.exact_reread_completed {
            return None;
        }
        let all_reads_target_requirement = !actions.is_empty()
            && actions.iter().all(|action| match action {
                AgentAction::ReadFile { path, range } => {
                    path == &requirement.path
                        && range.and_then(|value| value.normalized()).is_none()
                }
                _ => false,
            });
        if !all_reads_target_requirement || Self::repair_requirement_prefers_full_file(requirement)
        {
            return None;
        }
        let suggested = requirement
            .suggested_range
            .map(|range| format!(" Suggested range: {}.", range.label()))
            .unwrap_or_default();
        Some(format!(
            "[Loop guard]\nThe previous failure requires a focused `ReadFile` for `{}` before you can continue. Request a concrete line range instead of rereading the whole file.{}",
            requirement.path, suggested
        ))
    }

    pub(crate) fn repair_requirement_needs_reread(&self) -> bool {
        self.repair_requirement
            .as_ref()
            .is_some_and(|requirement| !requirement.exact_reread_completed)
    }

    pub(crate) fn required_repair_read_action(&self) -> Option<AgentAction> {
        if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            return Some(AgentAction::ReadFile {
                path: requirement.path.clone(),
                range: if Self::repair_requirement_prefers_full_file(requirement) {
                    None
                } else {
                    Some(requirement.suggested_range?)
                },
            });
        }
        let repair_state = self.benchmark_repair_state.as_ref()?;
        let range = match repair_state.phase {
            BenchmarkRepairPhase::NeedsFailureAnchorRead => repair_state.failure_anchor_range?,
            BenchmarkRepairPhase::NeedsImplementationRead => {
                repair_state.implementation_suggested_range?
            }
            BenchmarkRepairPhase::NeedsPatch
            | BenchmarkRepairPhase::NeedsFastLoopRerun
            | BenchmarkRepairPhase::Idle => return None,
        };
        Some(AgentAction::ReadFile {
            path: repair_state.owner_path.clone(),
            range: Some(range),
        })
    }

    pub(crate) fn should_inject_required_read(&self) -> bool {
        self.benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.repair_required)
            && self.required_repair_read_action().is_some()
            && (self.parser_recovery_failures > 0
                || self
                    .benchmark_repair_state
                    .as_ref()
                    .is_some_and(|repair_state| repair_state.invalid_action_count > 0)
                || self.stall_count > 0)
    }

    pub(crate) fn allow_benchmark_focused_same_file_reread(
        &self,
        path: &str,
        range: Option<crate::agent_protocol::ReadFileRange>,
    ) -> bool {
        if self.has_mutating_change {
            return false;
        }
        if self.agent_repair_memory.scorecard.redundant_read_count >= 2 {
            return false;
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_ref() {
            match repair_state.phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                    return range
                        .and_then(|value| value.normalized())
                        .zip(repair_state.failure_anchor_range)
                        .is_some_and(|(requested_range, anchor_range)| {
                            path == repair_state.owner_path
                                && read_range_overlap(requested_range, anchor_range) > 0
                        });
                }
                BenchmarkRepairPhase::NeedsImplementationRead => {
                    return range
                        .and_then(|value| value.normalized())
                        .zip(repair_state.failure_anchor_range)
                        .is_some_and(|(requested_range, anchor_range)| {
                            path == repair_state.owner_path
                                && range_meaningfully_differs_from_anchor(
                                    requested_range,
                                    anchor_range,
                                )
                        });
                }
                BenchmarkRepairPhase::NeedsPatch
                | BenchmarkRepairPhase::NeedsFastLoopRerun
                | BenchmarkRepairPhase::Idle => {}
            }
        }
        if self.redundant_inspection_turns > 0 {
            return false;
        }
        if self
            .repair_requirement
            .as_ref()
            .is_some_and(|requirement| requirement.path == path)
            && let Some(requirement) = self.repair_requirement.as_ref()
        {
            return Self::repair_requirement_read_is_valid(requirement, path, range);
        }
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return false;
        };
        let Some(last_failure) = ledger.last_validation_failure.as_ref() else {
            return false;
        };
        !last_failure.trim().is_empty()
            && (ledger.owner_files.iter().any(|candidate| candidate == path)
                || ledger
                    .expected_touch_targets
                    .iter()
                    .any(|candidate| candidate == path))
            && range.and_then(|value| value.normalized()).is_some()
    }

    pub(crate) fn note_action(&mut self, action: &AgentAction) {
        if let (Some(ledger), Some(repair_state)) = (
            self.benchmark_case_ledger.as_ref(),
            self.benchmark_repair_state.as_ref(),
        ) {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory)
                    .into_owned();
            let write_locked = benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            );
            if write_locked {
                let targets_patch = match action {
                    AgentAction::PreviewEdit { path, .. }
                    | AgentAction::ReplaceRange { path, .. }
                    | AgentAction::ModifyToml { path, .. }
                    | AgentAction::WriteFile { path, .. }
                    | AgentAction::ApplyPatch { path, .. }
                    | AgentAction::ReplaceBlock { path, .. }
                    | AgentAction::SetExecutable { path } => {
                        canonical_path(path) == canonical_path(&patch_target)
                    }
                    AgentAction::ApplyPreview { .. } => true,
                    _ => false,
                };
                if targets_patch {
                    if matches!(action, AgentAction::PreviewEdit { .. }) {
                        self.agent_repair_memory.scorecard.patch_scaffold_honored = true;
                    }
                    if action.is_write_like() || matches!(action, AgentAction::ApplyPreview { .. })
                    {
                        self.agent_repair_memory.scorecard.write_phase_write_emitted = true;
                    }
                }
            }
        }
        match action {
            AgentAction::ReadFile { path, .. }
            | AgentAction::ListDirectory { path }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path } => {
                self.working_set.insert(path.clone());
            }
            AgentAction::RunValidation { .. } => {}
            AgentAction::RunCommand { command, .. } => {
                self.last_tool_summary = Some(format!("scheduled shell command `{command}`"));
            }
            AgentAction::SearchText { query, .. } => {
                self.last_tool_summary = Some(format!("searched repo text for `{query}`"));
            }
            AgentAction::SearchSymbols { query, .. } => {
                self.last_tool_summary = Some(format!("searched repo symbols for `{query}`"));
            }
            AgentAction::FindFiles { query, .. } => {
                self.last_tool_summary = Some(format!("found files for `{query}`"));
            }
            AgentAction::StructuralSearch { pattern, .. } => {
                self.last_tool_summary = Some(format!("structural search for `{pattern}`"));
            }
            AgentAction::StructuralEditPreview { path, .. } => {
                self.last_tool_summary = Some(format!(
                    "previewed structural edit for `{}`",
                    path.as_deref().unwrap_or(".")
                ));
            }
            AgentAction::CargoDiagnostics { command, .. } => {
                self.last_tool_summary = Some(format!(
                    "ran cargo diagnostics `{}`",
                    command
                        .as_deref()
                        .unwrap_or("cargo check --message-format=json")
                ));
            }
            AgentAction::GetRepoCapsule { query, .. } => {
                self.last_tool_summary = Some(match query {
                    Some(query) if !query.trim().is_empty() => {
                        format!("loaded repo capsule for `{query}`")
                    }
                    _ => "loaded repo capsule".to_string(),
                });
            }
            AgentAction::ExplainValidationFailure { command, .. } => {
                self.last_tool_summary =
                    Some(format!("explained validation failure for `{command}`"));
            }
            AgentAction::SuggestImplementationTargets { command, .. } => {
                self.last_tool_summary =
                    Some(format!("ranked implementation targets for `{command}`"));
            }
            AgentAction::SuggestEditAnchors { path, .. } => {
                self.last_tool_summary = Some(format!("suggested edit anchors for `{path}`"));
            }
            AgentAction::PreviewEdit { path, edit } => {
                self.working_set.insert(path.clone());
                self.last_tool_summary =
                    Some(format!("previewed {} edit for `{path}`", edit.kind_label()));
            }
            AgentAction::ApplyPreview { preview_id } => {
                self.last_tool_summary = Some(format!("applied preview `{preview_id}`"));
            }
            AgentAction::McpCallTool {
                server_name,
                tool_name,
                ..
            } => {
                self.last_tool_summary = Some(format!("requested MCP {server_name}/{tool_name}"));
            }
        }
    }

    pub(crate) fn set_mode(&mut self, mode: AgentMode) {
        self.current_mode = mode;
    }

    pub(crate) fn next_validation_action(&mut self) -> Option<AgentAction> {
        self.validation_queue
            .pop_front()
            .map(|plan| AgentAction::RunValidation { plan })
    }

    pub(crate) fn enqueue_post_edit_validation(&mut self, verifier_plan: Option<&ValidationPlan>) {
        self.validation_queue.clear();
        if let Some(plan) = self.benchmark_fast_loop_validation_plan() {
            self.enqueue_validation_plan(plan);
        }
        let fast_plan = ValidationPlan {
            fmt: true,
            clippy: false,
            workspace_tests: false,
            tests: Vec::new(),
            custom_commands: Vec::new(),
        };
        self.enqueue_validation_plan(fast_plan);

        let followup_plan = verifier_plan
            .cloned()
            .filter(|plan| !plan.is_empty())
            .unwrap_or(ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: true,
                tests: Vec::new(),
                custom_commands: Vec::new(),
            });
        self.enqueue_validation_plan(followup_plan);
    }

    pub(crate) fn benchmark_fast_loop_validation_plan(&self) -> Option<ValidationPlan> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        if !ledger.validation_details.repair_required
            || !ledger.validation_details.post_fast_loop_patch_attempted
            || ledger
                .validation_details
                .post_fast_loop_validation_rerun_attempted
        {
            return None;
        }
        let command = ledger.fast_loop_commands.first()?.trim();
        if command.is_empty() {
            return None;
        }
        Some(ValidationPlan {
            fmt: false,
            clippy: false,
            workspace_tests: false,
            tests: Vec::new(),
            custom_commands: vec![command.to_string()],
        })
    }

    pub(crate) fn repair_requires_patch_next(&self) -> bool {
        self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_details.repair_required
                && self
                    .repair_requirement
                    .as_ref()
                    .is_some_and(|requirement| requirement.exact_reread_completed)
                && !ledger.validation_details.post_fast_loop_patch_attempted
        })
    }

    pub(crate) fn repair_rejects_validation_before_first_write(&self) -> bool {
        if self.has_mutating_change
            || self
                .agent_repair_memory
                .scorecard
                .first_valid_write_step
                .is_some()
        {
            return false;
        }
        let known_failure = !self.agent_repair_memory.validation_failures.is_empty()
            || self
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| ledger.last_validation_failure.is_some());
        let patch_not_attempted = self
            .benchmark_case_ledger
            .as_ref()
            .is_none_or(|ledger| !ledger.validation_details.post_fast_loop_patch_attempted);
        known_failure && patch_not_attempted
    }

    pub(crate) fn action_repeats_validation_before_repair_write(
        &self,
        action: &AgentAction,
    ) -> bool {
        if !self.repair_rejects_validation_before_first_write() {
            return false;
        }
        match action {
            AgentAction::RunValidation { .. } => true,
            AgentAction::RunCommand { command, .. } => self
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| fast_loop_match_kind(ledger, command).is_some()),
            _ => false,
        }
    }

    pub(crate) fn enqueue_full_validation(&mut self) {
        let repair_aware_fast_loop = self.benchmark_fast_loop_validation_plan();
        if repair_aware_fast_loop.is_some()
            && self
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| ledger.validation_details.repair_required)
        {
            if let Some(ledger) = self.benchmark_case_ledger.as_mut() {
                ledger.validation_details.full_validation_before_fast_loop = true;
            }
            if let Some(plan) = repair_aware_fast_loop {
                self.enqueue_validation_plan(plan);
            }
        }
        self.enqueue_validation_plan(ValidationPlan {
            fmt: true,
            clippy: true,
            workspace_tests: true,
            tests: Vec::new(),
            custom_commands: Vec::new(),
        });
    }

    pub(crate) fn enqueue_validation_plan(&mut self, plan: ValidationPlan) {
        if plan.is_empty() {
            return;
        }
        if validation_commands_for_plan(&self.config, &plan).is_empty() {
            return;
        }
        self.validation_queue.push_back(plan);
    }

    pub(crate) fn queued_validation_summaries(&self) -> Vec<String> {
        self.validation_queue
            .iter()
            .map(ValidationPlan::summary)
            .collect()
    }
}
