//! `AgentTaskState`: benchmark-repair phase messages, parser-recovery
//! message generation, repair-phase correction prompts.

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
    pub(crate) fn benchmark_repair_phase_message(&self) -> Option<String> {
        let ledger = self.benchmark_case_ledger.as_ref()?;
        let repair_state = self.benchmark_repair_state.as_ref()?;
        if repair_state.phase == BenchmarkRepairPhase::Idle
            || !ledger.validation_details.repair_required
        {
            return None;
        }
        let repair_target = benchmark_repair_target_path(repair_state, ledger);
        let suggested_read_range = benchmark_repair_phase_suggested_range(repair_state);
        let suggested_range = suggested_read_range.map(|range| range.label());
        let failing_test = repair_state
            .primary_failure_test_name
            .clone()
            .or_else(|| ledger.validation_details.primary_failure_test_name.clone())
            .or_else(|| {
                ledger
                    .validation_details
                    .failing_test_names
                    .first()
                    .cloned()
            });
        let assertion_excerpt = ledger.validation_details.assertion_excerpt.clone();
        let current_hypothesis = ledger.current_hypothesis.clone();
        let recommended_rerun_command = recommended_fast_loop_rerun_command(ledger);
        let phase_message = match repair_state.phase {
            BenchmarkRepairPhase::Idle => return None,
            phase => benchmark_repair_phase_instruction(phase),
        };
        if repair_state.phase == BenchmarkRepairPhase::NeedsPatch {
            let patch_target =
                benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
            let target_lease = benchmark_target_lease_path(ledger, &self.agent_repair_memory);
            let patch_target_context_loaded = patch_target_context_loaded(
                repair_state,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let honored_range = repair_state
                .last_owner_slice
                .as_ref()
                .and_then(|slice| slice.honored_range)
                .or(repair_state.failure_anchor_range);
            let target_honored_range = repair_state
                .last_owner_slice
                .as_ref()
                .filter(|slice| {
                    canonical_path(&slice.path) == canonical_path(patch_target.as_ref())
                        && !slice.test_only
                })
                .and_then(|slice| slice.honored_range);
            let bare_replace_disallowed = bare_replace_block_disallowed_for_path(
                patch_target.as_ref(),
                &self.failed_edit_records,
            );
            let scaffold_available = patch_phase_scaffold_available(&self.agent_repair_memory);
            let scaffold_required = scaffold_available && !patch_target_context_loaded;
            let write_locked = benchmark_patch_phase_write_locked(
                repair_state,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            );
            let dependency_candidates = benchmark_dependency_candidates(ledger);
            let target_dependency_table =
                benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
            let manifest_operations = benchmark_manifest_patch_operations(
                ledger,
                target_dependency_table,
                &dependency_candidates,
            );
            let target_content_hash = target_content_hash_for_patch(
                repair_state,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let allowed_targets = benchmark_allowed_implementation_targets(ledger);
            let read_only_tests = benchmark_read_only_test_targets(ledger);
            if write_locked && patch_target.ends_with(".toml") {
                let mut lines = vec![
                    "[Patch Packet] Manifest repair mode is active.".to_string(),
                    format!("Patch target: {patch_target}"),
                ];
                if let Some(target_lease) = target_lease.as_deref() {
                    lines.push(format!("Current target lease: {target_lease}"));
                }
                if let Some(diagnostic_class) = self
                    .agent_repair_memory
                    .diagnostic_class
                    .as_deref()
                    .or(ledger.validation_details.diagnostic_class.as_deref())
                {
                    lines.push(format!("Failure class: {diagnostic_class}"));
                }
                if let Some(target_dependency_table) = target_dependency_table {
                    lines.push(format!(
                        "Target dependency table: [{target_dependency_table}]"
                    ));
                }
                if !manifest_operations.is_empty() {
                    lines.push(format!(
                        "Exact dependency ops: {}",
                        render_toml_edit_operations_brief(&manifest_operations)
                    ));
                } else if !dependency_candidates.is_empty() {
                    lines.push(format!(
                        "Dependency candidates: {}",
                        dependency_candidates.join(", ")
                    ));
                }
                if let Some(content_hash) = target_content_hash.as_deref() {
                    lines.push(format!("Observed target content_hash: `{content_hash}`"));
                }
                if !self
                    .agent_repair_memory
                    .last_manifest_patch_operations
                    .is_empty()
                {
                    lines.push(format!(
                        "Previous manifest ops: {}",
                        render_toml_edit_operations_brief(
                            &self.agent_repair_memory.last_manifest_patch_operations
                        )
                    ));
                }
                if let Some(post_patch_diagnostic_class) = self
                    .agent_repair_memory
                    .post_patch_diagnostic_class
                    .as_deref()
                {
                    lines.push(format!(
                        "Post-patch diagnostic class: {post_patch_diagnostic_class}"
                    ));
                }
                if let Some(post_patch_excerpt) = self
                    .agent_repair_memory
                    .post_patch_diagnostic_excerpt
                    .as_deref()
                {
                    lines.push(format!(
                        "Post-patch diagnostic excerpt: {}",
                        truncate_visible_text(post_patch_excerpt, 180)
                    ));
                }
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Exact rerun command: {command}"));
                }
                if preview_apply_locked(&self.agent_repair_memory) {
                    let preview_id = self
                        .agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview");
                    lines.push(format!(
                        "Allowed next action: exactly one `ApplyPreview` with preview_id `{preview_id}`."
                    ));
                    lines.push(
                        "A clean manifest preview already exists. Do not read, search, list, widen scope, or emit a new preview in this turn."
                            .to_string(),
                    );
                } else {
                    lines.push(
                        "Allowed next action: exactly one `PreviewEdit` with `modify_toml` on the leased manifest target."
                            .to_string(),
                    );
                    lines.push(
                        "Cargo.toml is already loaded. Another `ReadFile` on the manifest is invalid in this turn."
                            .to_string(),
                    );
                    lines.push(
                        "Do not search, list, widen scope, or switch to source files before the manifest patch lands."
                            .to_string(),
                    );
                }
                lines.push(
                    "Response shape: return one raw JSON object only. Keep `assistant_message` empty or very short."
                        .to_string(),
                );
                lines.push("Minimal JSON example:".to_string());
                if preview_apply_locked(&self.agent_repair_memory) {
                    lines.push(apply_preview_parser_recovery_example(
                        self.agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview"),
                    ));
                } else {
                    lines.push(manifest_preview_edit_scaffold_example(
                        patch_target.as_ref(),
                        target_content_hash.as_deref(),
                        target_dependency_table,
                        &dependency_candidates,
                        &manifest_operations,
                    ));
                }
                return Some(lines.join("\n"));
            }
            let mut lines = vec![
                "[Patch Packet] Narrow repair mode is active.".to_string(),
                if scaffold_required {
                    format!(
                        "Patch target context is not loaded yet. Use exactly one read-only scaffold action now: `SuggestEditAnchors`, `PreviewEdit`, or `ReadFile` on `{}`. Then write the patch and rerun the fast loop.",
                        patch_target
                    )
                } else if write_locked && patch_target.ends_with(".toml") {
                    if preview_apply_locked(&self.agent_repair_memory) {
                        format!(
                            "Write-locked patch phase: a clean manifest preview already exists for `{}`. Emit one `ApplyPreview` now. Do not read, search, list, or widen scope.",
                            patch_target
                        )
                    } else {
                        format!(
                            "Write-locked patch phase: emit one `PreviewEdit` with `modify_toml` on `{}` now. Do not read, search, list, or widen scope.",
                            patch_target
                        )
                    }
                } else if write_locked {
                    format!(
                        "Write-locked patch phase: emit one write on `{}` now. Fallback: one `PreviewEdit` on the same file, then apply it next turn. Do not read, search, list, or widen scope.",
                        patch_target
                    )
                } else if scaffold_available {
                    format!(
                        "Patch `{}` now. Do not reread evidence files. If anchor confidence is still shaky, you may use exactly one read-only scaffold action first: `PreviewEdit` on the patch target. Rerun the fast loop immediately after the patch.",
                        patch_target
                    )
                } else {
                    format!(
                        "Patch `{}` now. Do not reread or ask for more scaffolding. Rerun the fast loop immediately after the patch.",
                        patch_target
                    )
                },
                format!("Owner path: {repair_target}"),
                format!("Patch target: {patch_target}"),
            ];
            if let Some(target_lease) = target_lease.as_deref() {
                lines.push(format!("Current target lease: {target_lease}"));
            }
            if write_locked {
                lines.push("Repair write locked: true".to_string());
            }
            if let Some(diagnostic_class) = self
                .agent_repair_memory
                .diagnostic_class
                .as_deref()
                .or(ledger.validation_details.diagnostic_class.as_deref())
            {
                lines.push(format!("Diagnostic class: {diagnostic_class}"));
            }
            if !dependency_candidates.is_empty() {
                lines.push(format!(
                    "Missing dependencies: {}",
                    dependency_candidates.join(", ")
                ));
            }
            if let Some(target_dependency_table) = target_dependency_table {
                lines.push(format!(
                    "Target dependency table: [{}]",
                    target_dependency_table
                ));
            }
            if !self
                .agent_repair_memory
                .ranked_implementation_targets
                .is_empty()
            {
                lines.push(format!(
                    "Ranked implementation targets: {}",
                    render_ranked_implementation_targets(
                        &self.agent_repair_memory.ranked_implementation_targets
                    )
                ));
            }
            lines.push(format!(
                "Allowed implementation targets: {}",
                render_benchmark_target_list(&allowed_targets)
            ));
            if !read_only_tests.is_empty() {
                lines.push(format!(
                    "Test files are read-only unless explicitly listed above: {}",
                    render_benchmark_target_list(&read_only_tests)
                ));
            }
            if let Some(required_action) = self.agent_repair_memory.current_required_action.as_ref()
            {
                lines.push(format!("Required next action: {required_action}"));
            }
            if preview_apply_locked(&self.agent_repair_memory) {
                let preview_id = self
                    .agent_repair_memory
                    .last_preview_id
                    .as_deref()
                    .unwrap_or("preview_id_from_last_preview");
                lines.push(format!(
                    "A clean preview exists. Next action must be `ApplyPreview` with preview_id `{preview_id}`."
                ));
            }
            if let Some(range) = honored_range {
                lines.push(format!("Honored implementation range: {}", range.label()));
            }
            if let Some(failing_test) = failing_test {
                lines.push(format!("Primary failure test: {failing_test}"));
            }
            if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
                let line = ledger
                    .validation_details
                    .primary_failure_line
                    .map(|value| format!(":{value}"))
                    .unwrap_or_default();
                lines.push(format!("Primary failure location: {path}{line}"));
            }
            if let Some(assertion_excerpt) = assertion_excerpt {
                lines.push(format!(
                    "Assertion excerpt: {}",
                    truncate_visible_text(&assertion_excerpt, 220)
                ));
            }
            if let Some(current_hypothesis) = current_hypothesis {
                lines.push(format!(
                    "Current hypothesis: {}",
                    truncate_visible_text(&current_hypothesis, 180)
                ));
            }
            if let Some(command) = recommended_rerun_command.as_ref() {
                lines.push(format!("Recommended rerun command: {command}"));
            }
            if !self.failed_edit_records.is_empty() {
                lines.push(format!(
                    "Failed edit memory: {}",
                    render_failed_edit_memory(&self.failed_edit_records)
                ));
            }
            if let Some(preview) = self.agent_repair_memory.last_preview_result.as_ref() {
                lines.push(format!(
                    "Last preview result: {}",
                    truncate_visible_text(preview, 220)
                ));
            }
            if let Some(rollback) = self.agent_repair_memory.last_rollback_diagnostic.as_ref() {
                lines.push(format!(
                    "Last rollback diagnostic: {}",
                    truncate_visible_text(rollback, 220)
                ));
            }
            if !self.agent_repair_memory.is_empty() {
                lines.push(format!(
                    "Agent scorecard: {}",
                    render_agent_repair_memory(&self.agent_repair_memory)
                ));
            }
            if bare_replace_disallowed {
                lines.push(
                    format!(
                        "Allowed actions: `ApplyPatch`, `WriteFile`, or `ReplaceBlock` with an explicit `range` on `{}`. Bare `ReplaceBlock` is paused for this repair episode.",
                        patch_target
                    ),
                );
            } else if patch_target.ends_with(".toml") {
                lines.push(format!(
                    "Allowed actions: `PreviewEdit` with `modify_toml` on `{}` first, then `ApplyPreview`. `ApplyPatch` or `WriteFile` stay disabled while manifest preview/apply mode is active.",
                    patch_target
                ));
            } else {
                lines.push(format!(
                    "Allowed actions: prefer `ReplaceRange` or `PreviewEdit` with `replace_range` on an observed slice of `{}`. `ApplyPatch`, ranged `ReplaceBlock`, and `WriteFile` remain allowed when needed.",
                    patch_target
                ));
            }
            if let Some(content_hash) = target_content_hash.as_ref() {
                lines.push(format!(
                    "Observed target content_hash for `{}`: `{}`",
                    patch_target, content_hash
                ));
            }
            if write_locked {
                if patch_target.ends_with(".toml") {
                    lines.push(
                        "Patch goal: preview the manifest dependency edit, apply that preview, then rerun the exact fast loop."
                            .to_string(),
                    );
                } else {
                    lines.push(format!(
                        "Patch goal: edit `{}` for the current source failure, then rerun the exact fast loop.",
                        patch_target
                    ));
                }
            } else if scaffold_available {
                lines.push(
                    format!(
                        "Optional scaffold: exactly one `PreviewEdit`, `SuggestEditAnchors`, or target `ReadFile` on `{}` before the write. These are read-only and must be followed by a real write turn.",
                        patch_target
                    ),
                );
            }
            lines.push(
                "Patch guidance: do not invent enum variants, methods, or types that are not visible in the read context."
                    .to_string(),
            );
            lines.push(
                "If replacing repeated code, use unique surrounding context, a ranged `ReplaceBlock`, or `ApplyPatch`; do not retry an ambiguous bare `ReplaceBlock`."
                    .to_string(),
            );
            lines.push(
                format!(
                    "Next-step contract: emit exactly one concrete write turn on `{}`, then rerun the fast loop.",
                    patch_target
                ),
            );
            lines.push(
                "Response shape: return one raw JSON object only. Keep `assistant_message` empty or to a few words."
                    .to_string(),
            );
            lines.push("Minimal JSON example:".to_string());
            if scaffold_required {
                lines.push(patch_phase_scaffold_example(patch_target.as_ref()));
            } else {
                lines.push(patch_phase_parser_recovery_example(
                    patch_target.as_ref(),
                    recommended_rerun_command.as_deref(),
                    if patch_target.ends_with(".toml") {
                        if patch_target_context_loaded {
                            honored_range
                        } else {
                            None
                        }
                    } else {
                        target_honored_range
                    },
                    bare_replace_disallowed
                        || !patch_target_context_loaded
                        || (!patch_target.ends_with(".toml") && target_honored_range.is_none()),
                    target_content_hash.as_deref(),
                    target_dependency_table,
                    &dependency_candidates,
                    &manifest_operations,
                ));
            }
            if !patch_target.ends_with(".toml") {
                if let Some(target_honored_range) = target_honored_range {
                    let expected_hash = observed_range_content_hash(
                        &self.agent_repair_memory,
                        patch_target.as_ref(),
                        target_honored_range,
                    )
                    .or_else(|| target_content_hash.clone())
                    .unwrap_or_else(|| "CONTENT_HASH_FROM_READ".to_string());
                    lines.push(format!("Minimal PreviewEdit example: {{\"actions\":[{{\"PreviewEdit\":{{\"path\":\"{}\",\"edit\":{{\"replace_range\":{{\"range\":{{\"start_line\":{},\"end_line\":{}}},\"expected_hash\":\"{}\",\"replacement\":\"FULL_REPLACEMENT_FOR_THOSE_LINES\"}}}}}}}}]}}", patch_target, target_honored_range.start_line, target_honored_range.end_line, expected_hash));
                    lines.push(format!("Minimal ReplaceRange example: {{\"actions\":[{{\"ReplaceRange\":{{\"path\":\"{}\",\"range\":{{\"start_line\":{},\"end_line\":{}}},\"expected_hash\":\"{}\",\"replacement\":\"FULL_REPLACEMENT_FOR_THOSE_LINES\"}}}}]}}", patch_target, target_honored_range.start_line, target_honored_range.end_line, expected_hash));
                } else {
                    lines.push(format!("Minimal ApplyPatch example: {{\"actions\":[{{\"ApplyPatch\":{{\"path\":\"{}\",\"patch\":\"*** Begin Patch\\n*** Update File: {}\\n@@\\n-<old source line>\\n+<new source line>\\n*** End Patch\\n\"}}}}]}}", patch_target, patch_target));
                }
            }
            if let Some(slice_content) = owner_slice_packet_content(repair_state) {
                let rendered_slice = truncate_patch_packet_slice(&slice_content);
                if !rendered_slice.trim().is_empty() {
                    let slice_label = repair_state
                        .last_owner_slice
                        .as_ref()
                        .filter(|slice| {
                            canonical_path(&slice.path) == canonical_path(patch_target.as_ref())
                                && !slice.test_only
                        })
                        .map(|_| "Implementation slice:")
                        .unwrap_or("Last honored evidence slice:");
                    lines.push(slice_label.to_string());
                    lines.push(format!("```rust\n{}\n```", rendered_slice));
                }
            }
            return Some(lines.join("\n"));
        }
        let mut lines = vec![
            "[Repair Phase] The last fast loop failed on a narrow benchmark case.".to_string(),
            phase_message.to_string(),
            format!("Repair target: {repair_target}"),
        ];
        if let Some(required_action) = self.agent_repair_memory.current_required_action.as_ref() {
            lines.push(format!("Required next action: {required_action}"));
        }
        if let Some(range) = suggested_range {
            lines.push(format!("Suggested range: {range}"));
        }
        if let Some(failing_test) = failing_test {
            lines.push(format!("Primary failure test: {failing_test}"));
        }
        if let Some(path) = ledger.validation_details.primary_failure_path.as_ref() {
            let line = ledger
                .validation_details
                .primary_failure_line
                .map(|value| format!(":{value}"))
                .unwrap_or_default();
            lines.push(format!("Primary failure location: {path}{line}"));
        }
        if let Some(assertion_excerpt) = assertion_excerpt {
            lines.push(format!(
                "Assertion excerpt: {}",
                truncate_visible_text(&assertion_excerpt, 220)
            ));
        }
        if let Some(current_hypothesis) = current_hypothesis {
            lines.push(format!(
                "Current hypothesis: {}",
                truncate_visible_text(&current_hypothesis, 180)
            ));
        }
        if let Some(command) = recommended_rerun_command.as_ref() {
            lines.push(format!("Recommended rerun command: {command}"));
            if repair_state.phase == BenchmarkRepairPhase::NeedsFastLoopRerun {
                lines.push(
                    "Response shape: return one raw JSON object only and emit the fast-loop rerun now."
                        .to_string(),
                );
                lines.push("Minimal JSON example:".to_string());
                lines.push(rerun_phase_parser_recovery_example(command));
            }
        }
        if matches!(
            repair_state.phase,
            BenchmarkRepairPhase::NeedsFailureAnchorRead
                | BenchmarkRepairPhase::NeedsImplementationRead
        ) {
            lines.push(
                "Response shape: return one raw JSON object only and emit exactly one ranged `ReadFile` now."
                    .to_string(),
            );
            lines.push("Minimal JSON example:".to_string());
            lines.push(focused_read_parser_recovery_example(
                repair_target,
                suggested_read_range,
            ));
        }
        Some(lines.join("\n"))
    }

    pub(crate) fn parser_recovery_message(&self, output_truncated: bool, error: &str) -> String {
        let generic = parser_recovery_message(output_truncated, error);
        let Some(ledger) = self.benchmark_case_ledger.as_ref() else {
            return generic;
        };
        let Some(repair_state) = self.benchmark_repair_state.as_ref() else {
            return benchmark_general_parser_recovery_message(
                generic,
                ledger,
                self.has_mutating_change,
            );
        };
        if !ledger.validation_details.repair_required {
            return benchmark_general_parser_recovery_message(
                generic,
                ledger,
                self.has_mutating_change,
            );
        }
        let repair_target = benchmark_repair_target_path(repair_state, ledger);
        let recommended_rerun_command = recommended_fast_loop_rerun_command(ledger);
        match repair_state.phase {
            BenchmarkRepairPhase::NeedsPatch => {
                if let Some(requirement) = self.repair_requirement.as_ref()
                    && !requirement.exact_reread_completed
                {
                    let mut lines = vec![
                        generic,
                        "[Parser] A previous owner-file edit failed, so patch phase is paused."
                            .to_string(),
                        Self::repair_requirement_prompt(requirement),
                    ];
                    if let Some(range) = requirement.suggested_range {
                        lines.push(format!("Suggested reread range: {}", range.label()));
                    }
                    lines.push(
                        "Return one raw JSON object only. Do not patch, rerun tests, search, or widen scope in this recovery turn."
                            .to_string(),
                    );
                    return lines.join("\n");
                }
                let patch_target =
                    benchmark_patch_target_path(repair_state, ledger, &self.agent_repair_memory);
                let patch_target_context_loaded = patch_target_context_loaded(
                    repair_state,
                    &self.agent_repair_memory,
                    patch_target.as_ref(),
                );
                let scaffold_available = patch_phase_scaffold_available(&self.agent_repair_memory);
                let scaffold_required = scaffold_available && !patch_target_context_loaded;
                let write_locked = benchmark_patch_phase_write_locked(
                    repair_state,
                    ledger,
                    &self.agent_repair_memory,
                    self.repair_requirement.as_ref(),
                );
                let dependency_candidates = benchmark_dependency_candidates(ledger);
                let target_dependency_table =
                    benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
                let manifest_operations = benchmark_manifest_patch_operations(
                    ledger,
                    target_dependency_table,
                    &dependency_candidates,
                );
                let mut lines = vec![
                    generic.clone(),
                    "[Parser] You are still in patch phase for a narrow benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                        .to_string(),
                    if patch_target_context_loaded || !scaffold_available {
                        "Do not reread evidence files, search, list directories, or widen scope in this recovery turn."
                            .to_string()
                    } else {
                        "The leased patch target has not been loaded yet; use exactly one read-only scaffold action on the patch target or write a concrete patch if you already know the exact edit."
                            .to_string()
                    },
                    format!("Owner path: {repair_target}"),
                    format!("Patch target: {patch_target}"),
                ];
                if let Some(range) = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .or(repair_state.failure_anchor_range)
                {
                    lines.push(format!("Honored implementation range: {}", range.label()));
                }
                let honored_range = repair_state
                    .last_owner_slice
                    .as_ref()
                    .and_then(|slice| slice.honored_range)
                    .or(repair_state.failure_anchor_range);
                let bare_replace_disallowed = bare_replace_block_disallowed_for_path(
                    patch_target.as_ref(),
                    &self.failed_edit_records,
                );
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Recommended rerun command: {command}"));
                }
                if !dependency_candidates.is_empty() {
                    lines.push(format!(
                        "Missing dependencies: {}",
                        dependency_candidates.join(", ")
                    ));
                }
                if let Some(target_dependency_table) = target_dependency_table {
                    lines.push(format!(
                        "Target dependency table: [{}]",
                        target_dependency_table
                    ));
                }
                if !self.failed_edit_records.is_empty() {
                    lines.push(format!(
                        "Failed edit memory: {}",
                        render_failed_edit_memory(&self.failed_edit_records)
                    ));
                }
                if write_locked && patch_target.ends_with(".toml") {
                    if preview_apply_locked(&self.agent_repair_memory) {
                        let preview_id = self
                            .agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview");
                        lines.push(format!(
                            "Allowed action order: exactly one `ApplyPreview` with preview_id `{preview_id}` now. Then rerun the fast loop after the write lands."
                        ));
                    } else {
                        lines.push(format!(
                            "Allowed action order: exactly one `PreviewEdit` with `modify_toml` on `{}` now. Then apply that preview on the next turn and rerun the fast loop after the write lands.",
                            patch_target
                        ));
                    }
                } else if bare_replace_disallowed {
                    lines.push(format!(
                        "Allowed action order: first exactly one write on `{}` (`ApplyPatch`, `WriteFile`, or ranged `ReplaceBlock`), then optionally one immediate fast-loop rerun.",
                        patch_target
                    ));
                } else if patch_target_context_loaded || !scaffold_available {
                    lines.push(format!(
                        "Allowed action order: first exactly one write on `{}` (`ApplyPatch`, `ReplaceBlock`, or `WriteFile`), then optionally one immediate fast-loop rerun.",
                        patch_target
                    ));
                } else {
                    lines.push(format!(
                        "Allowed action order: exactly one `SuggestEditAnchors`, `PreviewEdit`, `ReadFile`, or write action on `{}`. Do not act on the evidence file.",
                        patch_target
                    ));
                }
                let target_content_hash = target_content_hash_for_patch(
                    repair_state,
                    &self.agent_repair_memory,
                    patch_target.as_ref(),
                );
                if write_locked && patch_target.ends_with(".toml") {
                    let mut lines = vec![
                        generic,
                        "[Parser] Manifest patch mode is still active.".to_string(),
                        "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                            .to_string(),
                        "The leased manifest is already loaded. Another `ReadFile` on the manifest will be rejected in this turn."
                            .to_string(),
                        format!("Patch target: {patch_target}"),
                    ];
                    if let Some(target_dependency_table) = target_dependency_table {
                        lines.push(format!(
                            "Target dependency table: [{target_dependency_table}]"
                        ));
                    }
                    if !manifest_operations.is_empty() {
                        lines.push(format!(
                            "Exact dependency ops: {}",
                            render_toml_edit_operations_brief(&manifest_operations)
                        ));
                    }
                    if let Some(content_hash) = target_content_hash.as_deref() {
                        lines.push(format!("Observed target content_hash: `{content_hash}`"));
                    }
                    if let Some(command) = recommended_rerun_command.as_deref() {
                        lines.push(format!("Exact rerun command: {command}"));
                    }
                    if preview_apply_locked(&self.agent_repair_memory) {
                        let preview_id = self
                            .agent_repair_memory
                            .last_preview_id
                            .as_deref()
                            .unwrap_or("preview_id_from_last_preview");
                        lines.push(format!(
                            "Allowed action order: exactly one `ApplyPreview` with preview_id `{preview_id}`, then optional exact fast-loop rerun."
                        ));
                    } else {
                        lines.push(
                            "Allowed action order: exactly one `PreviewEdit` with `modify_toml` on the leased manifest target now. Another manifest read is invalid."
                                .to_string(),
                        );
                    }
                    lines.push("Minimal JSON example:".to_string());
                    if preview_apply_locked(&self.agent_repair_memory) {
                        lines.push(apply_preview_parser_recovery_example(
                            self.agent_repair_memory
                                .last_preview_id
                                .as_deref()
                                .unwrap_or("preview_id_from_last_preview"),
                        ));
                    } else {
                        lines.push(manifest_preview_edit_scaffold_example(
                            patch_target.as_ref(),
                            target_content_hash.as_deref(),
                            target_dependency_table,
                            &dependency_candidates,
                            &manifest_operations,
                        ));
                    }
                    return lines.join("\n");
                }
                if let Some(content_hash) = target_content_hash.as_ref() {
                    lines.push(format!(
                        "Observed target content_hash for `{}`: `{}`",
                        patch_target, content_hash
                    ));
                }
                lines.push("Minimal JSON example:".to_string());
                if scaffold_required {
                    lines.push(patch_phase_scaffold_example(patch_target.as_ref()));
                } else {
                    lines.push(patch_phase_parser_recovery_example(
                        patch_target.as_ref(),
                        recommended_rerun_command.as_deref(),
                        if patch_target_context_loaded {
                            honored_range
                        } else {
                            None
                        },
                        bare_replace_disallowed || !patch_target_context_loaded,
                        target_content_hash.as_deref(),
                        target_dependency_table,
                        &dependency_candidates,
                        &manifest_operations,
                    ));
                }
                lines.join("\n")
            }
            BenchmarkRepairPhase::NeedsFastLoopRerun => {
                let mut lines = vec![
                    generic,
                    "[Parser] You are still in fast-loop rerun phase for this benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only. Do not emit prose before or after the JSON object."
                        .to_string(),
                    "Do not patch or reread in this recovery turn. Emit the smallest fast-loop rerun now."
                        .to_string(),
                ];
                if let Some(command) = recommended_rerun_command.as_deref() {
                    lines.push(format!("Recommended rerun command: {command}"));
                    lines.push("Minimal JSON example:".to_string());
                    lines.push(rerun_phase_parser_recovery_example(command));
                }
                lines.join("\n")
            }
            BenchmarkRepairPhase::NeedsFailureAnchorRead
            | BenchmarkRepairPhase::NeedsImplementationRead => {
                let mut lines = vec![
                    generic,
                    "[Parser] You are still in a focused-read phase for this benchmark repair."
                        .to_string(),
                    "Return one raw JSON object only and emit the required focused `ReadFile` now."
                        .to_string(),
                ];
                if let Some(message) = self.benchmark_repair_phase_message() {
                    lines.push(message);
                }
                lines.join("\n")
            }
            _ => generic,
        }
    }

    pub(crate) fn benchmark_repair_phase_correction_message(
        &mut self,
        actions: &[AgentAction],
    ) -> Result<Option<String>, String> {
        let Some(repair_state_snapshot) = self.benchmark_repair_state.clone() else {
            return Ok(None);
        };
        if repair_state_snapshot.phase == BenchmarkRepairPhase::Idle {
            return Ok(None);
        }
        let owner_path = repair_state_snapshot.owner_path.clone();
        let failure_anchor_range = repair_state_snapshot.failure_anchor_range;
        let implementation_suggested_range = repair_state_snapshot.implementation_suggested_range;
        let phase = repair_state_snapshot.phase;
        let patch_target = self
            .benchmark_case_ledger
            .as_ref()
            .map(|ledger| {
                benchmark_patch_target_path(
                    &repair_state_snapshot,
                    ledger,
                    &self.agent_repair_memory,
                )
                .into_owned()
            })
            .unwrap_or_else(|| owner_path.clone());
        let patch_target_context_loaded = patch_target_context_loaded(
            &repair_state_snapshot,
            &self.agent_repair_memory,
            &patch_target,
        );
        let write_locked = self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            benchmark_patch_phase_write_locked(
                &repair_state_snapshot,
                ledger,
                &self.agent_repair_memory,
                self.repair_requirement.as_ref(),
            )
        });
        let attempted_actions = actions
            .iter()
            .map(AgentAction::summary)
            .collect::<Vec<_>>()
            .join(", ");
        let valid = if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            actions.iter().all(|action| {
                matches!(
                    action,
                    AgentAction::ReadFile { path, range }
                        if Self::repair_requirement_read_is_valid(requirement, path, *range)
                )
            })
        } else {
            match phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => actions.iter().all(|action| {
                    self.benchmark_evidence_action_satisfies(
                        &owner_path,
                        failure_anchor_range,
                        action,
                    )
                }),
                BenchmarkRepairPhase::NeedsImplementationRead => actions.iter().all(|action| {
                    matches!(
                        action,
                        AgentAction::ReadFile { path, range }
                            if path == &owner_path
                                && range
                                    .and_then(|value| value.normalized())
                                    .is_some_and(|requested_range| {
                                        failure_anchor_range.is_some_and(|anchor_range| {
                                            range_meaningfully_differs_from_anchor(
                                                requested_range,
                                                anchor_range,
                                            )
                                        }) && implementation_suggested_range.is_none_or(
                                            |suggested_range| {
                                                read_range_overlap(
                                                    requested_range,
                                                    suggested_range,
                                                ) > 0
                                            },
                                        )
                                    })
                    )
                }),
                BenchmarkRepairPhase::NeedsPatch => {
                    self.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
                        patch_phase_actions_are_valid(
                            actions,
                            &patch_target,
                            ledger,
                            &self.failed_edit_records,
                            &self.agent_repair_memory,
                            patch_target_context_loaded,
                        )
                    })
                }
                BenchmarkRepairPhase::NeedsFastLoopRerun => actions.iter().all(|action| {
                    self.benchmark_case_ledger
                        .as_ref()
                        .is_some_and(|ledger| action_matches_fast_loop(action, ledger))
                }),
                BenchmarkRepairPhase::Idle => true,
            }
        };
        if valid {
            if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
                repair_state.invalid_action_count = 0;
            }
            self.sync_benchmark_repair_state_to_ledger();
            return Ok(None);
        }
        if let Some(repair_state) = self.benchmark_repair_state.as_mut() {
            repair_state.invalid_action_count = repair_state.invalid_action_count.saturating_add(1);
            self.agent_repair_memory
                .scorecard
                .repair_invalid_action_streak_max = self
                .agent_repair_memory
                .scorecard
                .repair_invalid_action_streak_max
                .max(repair_state.invalid_action_count);
        }
        if phase == BenchmarkRepairPhase::NeedsPatch
            && write_locked
            && actions
                .iter()
                .any(|action| benchmark_write_phase_refusal(action, &patch_target))
        {
            self.agent_repair_memory
                .scorecard
                .write_phase_action_refusal_count = self
                .agent_repair_memory
                .scorecard
                .write_phase_action_refusal_count
                .saturating_add(1);
            if patch_target.ends_with(".toml") && preview_apply_locked(&self.agent_repair_memory) {
                self.agent_repair_memory
                    .scorecard
                    .preview_apply_action_refusal_count = self
                    .agent_repair_memory
                    .scorecard
                    .preview_apply_action_refusal_count
                    .saturating_add(1);
            }
        }
        self.record_rejected_actions(
            phase,
            actions,
            "action did not satisfy the current benchmark repair phase",
        );
        self.sync_benchmark_repair_state_to_ledger();
        if phase == BenchmarkRepairPhase::NeedsPatch
            && write_locked
            && patch_target.ends_with(".toml")
        {
            let preview_apply_locked = preview_apply_locked(&self.agent_repair_memory);
            let benchmark_ledger = self.benchmark_case_ledger.as_ref();
            let target_dependency_table = benchmark_ledger.and_then(|ledger| {
                benchmark_target_dependency_table(&repair_state_snapshot, ledger, &patch_target)
            });
            let dependency_candidates = benchmark_ledger
                .map(benchmark_dependency_candidates)
                .unwrap_or_default();
            let manifest_operations = benchmark_ledger
                .map(|ledger| {
                    benchmark_manifest_patch_operations(
                        ledger,
                        target_dependency_table,
                        &dependency_candidates,
                    )
                })
                .unwrap_or_default();
            let target_content_hash = target_content_hash_for_patch(
                &repair_state_snapshot,
                &self.agent_repair_memory,
                patch_target.as_ref(),
            );
            let invalid_action_count = self
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.invalid_action_count)
                .unwrap_or(0);
            if invalid_action_count >= 2 {
                if self
                    .agent_repair_memory
                    .scorecard
                    .write_phase_action_refusal_count
                    > 0
                {
                    return Err(format!(
                        "Autonomous write_phase_action_refusal during {} after repeated off-contract read-only repair actions.",
                        phase.label()
                    ));
                }
                return Err(format!(
                    "Autonomous repair loop stalled during {} after repeated invalid repair-phase actions.",
                    phase.label()
                ));
            }
            let mut lines = vec![
                "[Repair Phase] Manifest patch mode rejected the previous plan.".to_string(),
                format!("Rejected turn plan: {attempted_actions}"),
                "Cargo.toml is already loaded. Do not read it again.".to_string(),
                format!("Patch target: {patch_target}"),
            ];
            if preview_apply_locked {
                let preview_id = self
                    .agent_repair_memory
                    .last_preview_id
                    .as_deref()
                    .unwrap_or("preview_id_from_last_preview");
                lines.push(format!(
                    "A clean manifest preview already exists. Return exactly one raw JSON object with exactly one `ApplyPreview` action using preview_id `{preview_id}` now."
                ));
                lines.push(
                    "No `ReadFile`, `ListDirectory`, `SearchText`, new `PreviewEdit`, or source-file reads are allowed in this correction turn."
                        .to_string(),
                );
            } else {
                lines.push(
                    "Return exactly one raw JSON object with exactly one `PreviewEdit` action carrying `modify_toml` now."
                        .to_string(),
                );
                lines.push(
                    "No `ReadFile`, `ListDirectory`, `SearchText`, direct `ModifyToml`, or source-file reads are allowed in this correction turn."
                        .to_string(),
                );
            }
            if let Some(target_dependency_table) = target_dependency_table {
                lines.push(format!(
                    "Target dependency table: [{target_dependency_table}]"
                ));
            }
            if !manifest_operations.is_empty() {
                lines.push(format!(
                    "Exact dependency ops: {}",
                    render_toml_edit_operations_brief(&manifest_operations)
                ));
            }
            if let Some(content_hash) = target_content_hash.as_deref() {
                lines.push(format!("Observed target content_hash: `{content_hash}`"));
            }
            let rerun_command = benchmark_ledger.and_then(recommended_fast_loop_rerun_command);
            if let Some(command) = rerun_command.as_deref() {
                lines.push(format!("Exact rerun command: {command}"));
            }
            lines.push("Minimal JSON example:".to_string());
            if preview_apply_locked {
                lines.push(apply_preview_parser_recovery_example(
                    self.agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview"),
                ));
            } else {
                lines.push(manifest_preview_edit_scaffold_example(
                    patch_target.as_ref(),
                    target_content_hash.as_deref(),
                    target_dependency_table,
                    &dependency_candidates,
                    &manifest_operations,
                ));
            }
            return Ok(Some(lines.join("\n")));
        }
        let mut lines = vec![
            "[Repair Phase] The proposed next action does not satisfy the current repair step."
                .to_string(),
            format!("Rejected turn plan: {attempted_actions}"),
        ];
        if let Some(requirement) = self.repair_requirement.as_ref()
            && !requirement.exact_reread_completed
        {
            lines.push(format!(
                "[Repair Brief]\nThe previous edit failure still requires a fresh `ReadFile` for `{}` before any next write.",
                requirement.path
            ));
            if let Some(range) = requirement.suggested_range {
                lines.push(format!("Suggested reread range: {}", range.label()));
            }
            lines.push(Self::repair_requirement_correction(requirement));
        } else {
            if let Some(message) = self.benchmark_repair_phase_message() {
                lines.push(message);
            }
            let correction = match phase {
                BenchmarkRepairPhase::NeedsFailureAnchorRead => {
                    "Correction: gather the missing failure evidence now. Prefer the suggested owner-file slice, or use ExplainValidationFailure, SuggestEditAnchors, SearchText, or a directly related owner/test read if the failure has no precise file/line anchor."
                }
                BenchmarkRepairPhase::NeedsImplementationRead => {
                    "Correction: read exactly one implementation slice on the same owner file now. Use an explicit range that is materially different from the failing test slice and overlaps the suggested implementation range."
                }
                BenchmarkRepairPhase::NeedsPatch => {
                    if write_locked && patch_target.ends_with(".toml") {
                        if preview_apply_locked(&self.agent_repair_memory) {
                            "Correction: act on the leased patch target now. The manifest preview already exists, so emit exactly one `ApplyPreview` with the preview id from the last clean preview. Do not read, search, list, or widen scope first."
                        } else {
                            "Correction: act on the leased patch target now. Emit exactly one `PreviewEdit` with `modify_toml` on the manifest. Do not read, search, list, or widen scope first."
                        }
                    } else if write_locked {
                        "Correction: act on the leased patch target now. Emit exactly one write-class action on that file, or one `PreviewEdit` on the same file if you need a dry run. Do not read, search, list, or widen scope first."
                    } else {
                        "Correction: act on the leased patch target now. If this is the first patch-phase scaffold, you may emit exactly one PreviewEdit, SuggestEditAnchors, or target ReadFile on the patch target; otherwise write with ApplyPatch, ranged ReplaceBlock, or WriteFile. Do not reread evidence files or widen scope first."
                    }
                }
                BenchmarkRepairPhase::NeedsFastLoopRerun => {
                    "Correction: rerun the smallest fast loop now so the patch can be validated."
                }
                BenchmarkRepairPhase::Idle => "",
            };
            lines.push(correction.to_string());
            if phase == BenchmarkRepairPhase::NeedsPatch
                && bare_replace_block_disallowed_for_path(&patch_target, &self.failed_edit_records)
            {
                lines.push(
                    "Bare `ReplaceBlock` was rejected because an ambiguous patch-target replacement already failed in this repair episode. Use ranged `ReplaceBlock`, `ApplyPatch`, or `WriteFile`."
                        .to_string(),
                );
            }
        }
        let invalid_action_count = self
            .benchmark_repair_state
            .as_ref()
            .map(|repair_state| repair_state.invalid_action_count)
            .unwrap_or(0);
        if invalid_action_count >= 2 {
            if phase == BenchmarkRepairPhase::NeedsPatch
                && write_locked
                && self
                    .agent_repair_memory
                    .scorecard
                    .write_phase_action_refusal_count
                    > 0
            {
                return Err(format!(
                    "Autonomous write_phase_action_refusal during {} after repeated off-contract read-only repair actions.",
                    phase.label()
                ));
            }
            if phase == BenchmarkRepairPhase::NeedsPatch && !patch_target.ends_with(".toml") {
                return Err(format!(
                    "Autonomous source_patch_refusal during {} after repeated invalid source repair actions.",
                    phase.label()
                ));
            }
            return Err(format!(
                "Autonomous repair loop stalled during {} after repeated invalid repair-phase actions.",
                phase.label()
            ));
        }
        Ok(Some(lines.join("\n")))
    }

}
