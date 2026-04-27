//! Turn-action normalization passes — benchmark-repair, manifest,
//! patch, plain-text fast-loop adjustments before the dispatcher
//! commits the turn.

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
pub(crate) fn normalize_benchmark_repair_turn_actions(
    turn: &mut AgentTurnResponse,
    state: &AgentTaskState,
) {
    if state.policy.mode != PolicyMode::BenchmarkAutonomous {
        return;
    }
    let Some(repair_state) = state.benchmark_repair_state.as_ref() else {
        return;
    };
    let Some(ledger) = state.benchmark_case_ledger.as_ref() else {
        return;
    };
    if turn.actions.iter().any(AgentAction::is_write_like)
        && let Some((actions, playbook_name)) = benchmark_write_repair_actions_from_state(state)
    {
        let dropped = turn.actions.len();
        turn.actions = actions;
        turn.parse_warnings.push(format!(
            "Replaced {dropped} broad {playbook_name} repair action(s) with semantic benchmark patch action(s)."
        ));
        return;
    }
    match repair_state.phase {
        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
            retain_only_first_valid_repair_action(turn, |action| {
                state.benchmark_evidence_action_satisfies(
                    &repair_state.owner_path,
                    repair_state.failure_anchor_range,
                    action,
                )
            });
        }
        BenchmarkRepairPhase::NeedsImplementationRead => {
            retain_only_first_valid_repair_action(turn, |action| {
                matches!(
                    action,
                    AgentAction::ReadFile { path, range }
                        if path == &repair_state.owner_path
                            && range
                                .and_then(|value| value.normalized())
                                .is_some_and(|requested_range| {
                                    repair_state.failure_anchor_range.is_some_and(|anchor_range| {
                                        range_meaningfully_differs_from_anchor(
                                            requested_range,
                                            anchor_range,
                                        )
                                    }) && repair_state.implementation_suggested_range.is_none_or(
                                        |suggested_range| {
                                            read_range_overlap(
                                                requested_range,
                                                suggested_range,
                                            ) > 0
                                        },
                                    )
                                })
                )
            });
        }
        BenchmarkRepairPhase::NeedsPatch => {
            normalize_benchmark_patch_turn_actions(turn, state, repair_state, ledger);
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun | BenchmarkRepairPhase::Idle => {}
    }
}

pub(crate) fn retain_only_first_valid_repair_action<F>(turn: &mut AgentTurnResponse, is_valid: F)
where
    F: Fn(&AgentAction) -> bool,
{
    if turn.actions.len() <= 1 {
        return;
    }
    let Some(valid_index) = turn.actions.iter().position(is_valid) else {
        return;
    };
    let action = turn.actions[valid_index].clone();
    let dropped = turn.actions.len().saturating_sub(1);
    turn.actions = vec![action];
    turn.parse_warnings.push(format!(
        "Kept only the legal repair-phase next action and dropped {dropped} bundled follow-up action(s)."
    ));
}

pub(crate) fn normalize_benchmark_patch_turn_actions(
    turn: &mut AgentTurnResponse,
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    let target_context_loaded = patch_target_context_loaded(
        repair_state,
        &state.agent_repair_memory,
        patch_target.as_ref(),
    );
    if normalize_benchmark_patch_turn_through_playbook(turn, state, repair_state, ledger) {
        return;
    }
    if !target_context_loaded && !patch_target.as_ref().ends_with(".toml") {
        let suggested_range = repair_state.implementation_suggested_range.or_else(|| {
            state.benchmark_case_ledger.as_ref().and_then(|ledger| {
                ledger
                    .last_validation_failure
                    .as_deref()
                    .or(ledger.validation_details.assertion_excerpt.as_deref())
                    .and_then(|failure| {
                        repair_state
                            .latest_owner_file_text
                            .as_deref()
                            .and_then(|text| {
                                suggest_source_patch_range_from_failure(text, Some(failure))
                            })
                    })
            })
        });
        if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ReadFile { path, range }
                    if canonical_path(path) == canonical_path(patch_target.as_ref())
                        && range.and_then(crate::agent_protocol::ReadFileRange::normalized).is_some()
            )
        }) {
            let action = turn.actions[index].clone();
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source ranged ReadFile and dropped {dropped} bundled follow-up action(s)."
                ));
            }
        } else if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ReadFile { path, .. }
                    if canonical_path(path) == canonical_path(patch_target.as_ref())
            )
        }) {
            let mut action = turn.actions[index].clone();
            if let (AgentAction::ReadFile { range, .. }, Some(suggested_range)) =
                (&mut action, suggested_range)
            {
                *range = Some(suggested_range);
            }
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source ReadFile and dropped {dropped} bundled follow-up action(s)."
                ));
            }
            if let Some(suggested_range) = suggested_range {
                turn.parse_warnings.push(format!(
                    "Narrowed leased source ReadFile to the repair-relevant range {}.",
                    suggested_range.label()
                ));
            }
        }
        if turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::LspDiagnostics { .. }
                    | AgentAction::LspDefinition { .. }
                    | AgentAction::LspReferences { .. }
                    | AgentAction::LspHover { .. }
                    | AgentAction::LspWorkspaceSymbols { .. }
                    | AgentAction::LspDocumentSymbols { .. }
                    | AgentAction::LspCodeActions { .. }
                    | AgentAction::LspRenamePreview { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
            )
        }) && let Some(suggested_range) = suggested_range
        {
            let dropped = turn.actions.len();
            turn.actions = vec![AgentAction::ReadFile {
                path: patch_target.into_owned(),
                range: Some(suggested_range),
            }];
            turn.parse_warnings.push(format!(
                "Replaced {dropped} read-only source-phase action(s) with leased source ReadFile {}.",
                suggested_range.label()
            ));
        }
        return;
    }
    if !target_context_loaded {
        return;
    }
    if !patch_target.as_ref().ends_with(".toml") {
        if let Some(index) = turn.actions.iter().position(|action| {
            source_patch_action_targets(action, patch_target.as_ref(), &state.agent_repair_memory)
        }) {
            let mut actions = Vec::new();
            actions.push(turn.actions[index].clone());
            actions.extend(
                turn.actions[index + 1..]
                    .iter()
                    .filter(|action| action_matches_fast_loop(action, ledger))
                    .cloned(),
            );
            let dropped = turn.actions.len().saturating_sub(actions.len());
            turn.actions = actions;
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the leased source patch action plus legal fast-loop rerun and dropped {dropped} unrelated action(s)."
                ));
            }
        } else if turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
            )
        }) && let Some(actions) =
            exact_benchmark_source_patch_actions_from_state(state, repair_state, ledger)
        {
            let dropped = turn.actions.len();
            turn.actions = actions;
            turn.parse_warnings.push(format!(
                "Replaced {dropped} read-only source-phase action(s) with the exact benchmark source patch."
            ));
        }
        return;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        if let Some(index) = turn.actions.iter().position(|action| {
            matches!(
                action,
                AgentAction::ApplyPreview { preview_id }
                    if state
                        .agent_repair_memory
                        .last_preview_id
                        .as_deref()
                        .is_some_and(|expected| {
                            preview_id.trim() == expected || preview_apply_placeholder(preview_id)
                        })
            )
        }) {
            let action = turn.actions[index].clone();
            let dropped = turn.actions.len().saturating_sub(1);
            turn.actions = vec![action];
            if dropped > 0 {
                turn.parse_warnings.push(format!(
                    "Kept only the required manifest ApplyPreview and dropped {dropped} bundled follow-up action(s)."
                ));
            }
        } else if let Some(preview_id) = state.agent_repair_memory.last_preview_id.clone() {
            let dropped = turn.actions.len();
            turn.actions = vec![AgentAction::ApplyPreview {
                preview_id: preview_id.clone(),
            }];
            turn.parse_warnings.push(format!(
                "Converted write-locked manifest turn into required ApplyPreview `{preview_id}` and dropped {dropped} non-apply action(s)."
            ));
        }
        return;
    }
    if let Some(index) = turn.actions.iter().position(|action| {
        matches!(
            action,
            AgentAction::PreviewEdit {
                path,
                edit: PreviewEditPayload::ModifyToml { .. },
            } if canonical_path(path) == canonical_path(patch_target.as_ref())
        )
    }) {
        let action = turn.actions[index].clone();
        let dropped = turn.actions.len().saturating_sub(1);
        turn.actions = vec![action];
        if dropped > 0 {
            turn.parse_warnings.push(format!(
                "Kept only the required manifest PreviewEdit and dropped {dropped} bundled follow-up action(s)."
            ));
        }
        return;
    }
    if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::ReadFile { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
                if canonical_path(path) == canonical_path(patch_target.as_ref())
        )
    }) && let Some(action) =
        exact_manifest_preview_action_from_state(state, repair_state, ledger)
    {
        turn.actions = vec![action];
        turn.parse_warnings.push(
            "Replaced direct or redundant manifest edit with the exact benchmark manifest PreviewEdit."
                .to_string(),
        );
        return;
    }
    if turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::RunCommand { .. } | AgentAction::RunValidation { .. }
        )
    }) && let Some(action) =
        exact_manifest_preview_action_from_state(state, repair_state, ledger)
    {
        turn.actions = vec![action];
        turn.parse_warnings.push(
            "Replaced premature manifest validation with the exact benchmark manifest PreviewEdit."
                .to_string(),
        );
    }
}

pub(crate) fn source_patch_action_targets(
    action: &AgentAction,
    patch_target: &str,
    memory: &AgentRepairMemory,
) -> bool {
    match action {
        AgentAction::PreviewEdit { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::SetExecutable { path } => {
            canonical_path(path) == canonical_path(patch_target)
        }
        AgentAction::ApplyPreview { preview_id } => memory
            .last_preview_id
            .as_deref()
            .is_some_and(|expected| preview_id.trim() == expected),
        _ => false,
    }
}

pub(crate) fn replace_once(mut source_text: String, from: &str, to: &str) -> Option<String> {
    if !source_text.contains(from) {
        return None;
    }
    source_text = source_text.replacen(from, to, 1);
    Some(source_text)
}

pub(crate) fn canonicalize_benchmark_turn_actions(
    turn: &mut AgentTurnResponse,
    ledger: Option<&BenchmarkCaseLedger>,
) {
    let Some(ledger) = ledger else {
        return;
    };
    let Some(recommended_command) = recommended_fast_loop_rerun_command(ledger) else {
        return;
    };
    for action in &mut turn.actions {
        match action {
            AgentAction::RunCommand {
                command,
                timeout_ms: _,
            } => {
                let trimmed_command = command.trim();
                let command_extends_recommended = trimmed_command != recommended_command
                    && trimmed_command
                        .strip_prefix(&recommended_command)
                        .is_some_and(|suffix| {
                            suffix
                                .chars()
                                .next()
                                .is_some_and(|character| character.is_whitespace())
                        });
                if command_extends_recommended {
                    turn.parse_warnings.push(format!(
                        "Canonicalized fast-loop command with extra selector tokens `{}` to known fast loop `{}`.",
                        trimmed_command, recommended_command
                    ));
                    *command = recommended_command.clone();
                    continue;
                }
                if let Some(match_kind) = fast_loop_match_kind(ledger, command) {
                    if match_kind == FastLoopMatchKind::ExactCanonical {
                        continue;
                    }
                    turn.parse_warnings.push(format!(
                        "Canonicalized subset fast-loop command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                    continue;
                }
                if command_selects_known_fast_loop(ledger, command) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized selector validation command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                } else if command_looks_like_vague_fast_loop_request(command) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized vague validation command `{}` to known fast loop `{}`.",
                        command.trim(),
                        recommended_command
                    ));
                    *command = recommended_command.clone();
                }
            }
            AgentAction::RunValidation { plan } => {
                if validation_plan_fast_loop_match_kind(ledger, plan).is_some() {
                    turn.parse_warnings.push(format!(
                        "Canonicalized RunValidation `{}` to known fast loop `{}`.",
                        plan.summary(),
                        recommended_command
                    ));
                    *plan = ValidationPlan {
                        fmt: false,
                        clippy: false,
                        workspace_tests: false,
                        tests: Vec::new(),
                        custom_commands: vec![recommended_command.clone()],
                    };
                } else if validation_plan_looks_like_cli_fast_loop(plan) {
                    turn.parse_warnings.push(format!(
                        "Canonicalized CLI-shaped RunValidation `{}` to known fast loop `{}`.",
                        plan.summary(),
                        recommended_command
                    ));
                    *plan = ValidationPlan {
                        fmt: false,
                        clippy: false,
                        workspace_tests: false,
                        tests: Vec::new(),
                        custom_commands: vec![recommended_command.clone()],
                    };
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn fill_hash_guards_from_observed_context(
    turn: &mut AgentTurnResponse,
    state: &AgentTaskState,
) {
    for action in &mut turn.actions {
        match action {
            AgentAction::ReadFile { path, .. } => {
                if let Some((preview_path, expected_hash, operations, warning)) =
                    benchmark_manifest_preview_from_redundant_read(path, state)
                {
                    turn.parse_warnings.push(warning);
                    *action = AgentAction::PreviewEdit {
                        path: preview_path,
                        edit: PreviewEditPayload::ModifyToml {
                            expected_hash,
                            operations,
                        },
                    };
                }
            }
            AgentAction::ModifyToml {
                path,
                expected_hash,
                ..
            } if hash_guard_needs_observed_fill(expected_hash) => {
                if let Some(content_hash) =
                    observed_full_file_content_hash(&state.agent_repair_memory, path)
                {
                    turn.parse_warnings.push(format!(
                        "Filled placeholder expected_hash for ModifyToml `{}` from latest observed full-file content_hash `{}`.",
                        path, content_hash
                    ));
                    *expected_hash = content_hash;
                } else {
                    let path = path.clone();
                    turn.parse_warnings.push(format!(
                        "Converted placeholder-hash ModifyToml `{}` into ReadFile because no full-file content_hash has been observed yet.",
                        path
                    ));
                    *action = AgentAction::ReadFile { path, range: None };
                }
            }
            AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                ..
            } if hash_guard_needs_observed_fill(expected_hash) => {
                if let Some(content_hash) =
                    observed_range_content_hash(&state.agent_repair_memory, path, *range)
                {
                    turn.parse_warnings.push(format!(
                        "Filled placeholder expected_hash for ReplaceRange `{}` {} from latest observed range content_hash `{}`.",
                        path,
                        range.label(),
                        content_hash
                    ));
                    *expected_hash = content_hash;
                } else {
                    let path = path.clone();
                    let range = *range;
                    turn.parse_warnings.push(format!(
                        "Converted placeholder-hash ReplaceRange `{}` {} into ReadFile because no matching range content_hash has been observed yet.",
                        path,
                        range.label()
                    ));
                    *action = AgentAction::ReadFile {
                        path,
                        range: Some(range),
                    };
                }
            }
            AgentAction::PreviewEdit { path, edit } => match edit {
                PreviewEditPayload::ModifyToml { expected_hash, .. }
                    if hash_guard_needs_observed_fill(expected_hash) =>
                {
                    if let Some(content_hash) =
                        observed_full_file_content_hash(&state.agent_repair_memory, path)
                    {
                        turn.parse_warnings.push(format!(
                            "Filled placeholder expected_hash for PreviewEdit modify_toml `{}` from latest observed full-file content_hash `{}`.",
                            path, content_hash
                        ));
                        *expected_hash = content_hash;
                    } else {
                        let path = path.clone();
                        turn.parse_warnings.push(format!(
                            "Converted placeholder-hash PreviewEdit modify_toml `{}` into ReadFile because no full-file content_hash has been observed yet.",
                            path
                        ));
                        *action = AgentAction::ReadFile { path, range: None };
                    }
                }
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                } => {
                    if let Some(content_hash) =
                        observed_full_file_content_hash(&state.agent_repair_memory, path)
                    {
                        let trimmed = expected_hash.trim();
                        if trimmed != content_hash {
                            turn.parse_warnings.push(format!(
                                "Replaced mismatched expected_hash for PreviewEdit modify_toml `{}` with latest observed full-file content_hash `{}`.",
                                path, content_hash
                            ));
                            *expected_hash = content_hash;
                        }
                    }
                    if let Some(warning) =
                        replace_benchmark_manifest_preview_operations(path, operations, state)
                    {
                        turn.parse_warnings.push(warning);
                    }
                }
                PreviewEditPayload::ReplaceRange {
                    range,
                    expected_hash,
                    ..
                } if hash_guard_needs_observed_fill(expected_hash) => {
                    if let Some(content_hash) =
                        observed_range_content_hash(&state.agent_repair_memory, path, *range)
                    {
                        turn.parse_warnings.push(format!(
                            "Filled placeholder expected_hash for PreviewEdit replace_range `{}` {} from latest observed range content_hash `{}`.",
                            path,
                            range.label(),
                            content_hash
                        ));
                        *expected_hash = content_hash;
                    } else {
                        let path = path.clone();
                        let range = *range;
                        turn.parse_warnings.push(format!(
                            "Converted placeholder-hash PreviewEdit replace_range `{}` {} into ReadFile because no matching range content_hash has been observed yet.",
                            path,
                            range.label()
                        ));
                        *action = AgentAction::ReadFile {
                            path,
                            range: Some(range),
                        };
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

pub(crate) fn hash_guard_needs_observed_fill(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    if is_stable_content_hash(trimmed) {
        return false;
    }
    let normalized = trimmed
        .trim_matches(|ch| matches!(ch, '<' | '>' | '`' | '"' | '\''))
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "current_hash"
            | "content_hash"
            | "hash"
            | "full_file_content_hash"
            | "full_file_content_hash_from_read"
            | "content_hash_from_read"
            | "current_content_hash"
            | "not_specified"
            | "not_specified_yet"
            | "unknown"
            | "hash_from_last_read"
            | "placeholder"
    )
}

pub(crate) fn observed_full_file_content_hash(
    memory: &AgentRepairMemory,
    path: &str,
) -> Option<String> {
    let canonical_target = canonical_path(path);
    memory
        .observed_slices
        .iter()
        .rev()
        .find(|slice| {
            canonical_path(&slice.path) == canonical_target
                && slice.requested_range.is_none()
                && slice.honored_range.is_none()
        })
        .and_then(|slice| slice.content_fingerprint.clone())
}

pub(crate) fn observed_range_content_hash(
    memory: &AgentRepairMemory,
    path: &str,
    range: crate::agent_protocol::ReadFileRange,
) -> Option<String> {
    let canonical_target = canonical_path(path);
    let normalized_range = range.normalized()?;
    memory
        .observed_slices
        .iter()
        .rev()
        .find(|slice| {
            canonical_path(&slice.path) == canonical_target
                && slice
                    .honored_range
                    .and_then(crate::agent_protocol::ReadFileRange::normalized)
                    == Some(normalized_range)
        })
        .and_then(|slice| slice.content_fingerprint.clone())
}

pub(crate) fn replace_benchmark_manifest_preview_operations(
    path: &str,
    operations: &mut Vec<crate::agent_protocol::TomlEditOperation>,
    state: &AgentTaskState,
) -> Option<String> {
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(path) != canonical_path(patch_target.as_ref())
        || !patch_target.as_ref().trim().ends_with(".toml")
    {
        return None;
    }
    if !operations.iter().all(|operation| {
        matches!(
            operation,
            crate::agent_protocol::TomlEditOperation::SetDependency { .. }
        )
    }) {
        return None;
    }
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let replacement_operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if replacement_operations.is_empty() || *operations == replacement_operations {
        return None;
    }
    let replacement_names = dependency_operation_names(&replacement_operations);
    let operation_names = dependency_operation_names(operations);
    if operation_names.is_empty() || !operation_names.is_subset(&replacement_names) {
        return None;
    }
    *operations = replacement_operations;
    Some(format!(
        "Replaced benchmark manifest PreviewEdit operations for `{}` with exact dependency operations from the current validation failure.",
        path
    ))
}

pub(crate) fn benchmark_manifest_preview_from_redundant_read(
    path: &str,
    state: &AgentTaskState,
) -> Option<(
    String,
    String,
    Vec<crate::agent_protocol::TomlEditOperation>,
    String,
)> {
    let repair_state = state.benchmark_repair_state.as_ref()?;
    let ledger = state.benchmark_case_ledger.as_ref()?;
    if !benchmark_patch_phase_write_locked(
        repair_state,
        ledger,
        &state.agent_repair_memory,
        state.repair_requirement.as_ref(),
    ) {
        return None;
    }
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(path) != canonical_path(patch_target.as_ref())
        || !patch_target.as_ref().trim().ends_with(".toml")
    {
        return None;
    }
    let expected_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let dependency_candidates = if state.agent_repair_memory.dependency_candidates.is_empty() {
        benchmark_dependency_candidates(ledger)
    } else {
        state.agent_repair_memory.dependency_candidates.clone()
    };
    let target_dependency_table =
        benchmark_target_dependency_table(repair_state, ledger, patch_target.as_ref());
    let operations = benchmark_manifest_patch_operations(
        ledger,
        target_dependency_table,
        &dependency_candidates,
    );
    if operations.is_empty() {
        return None;
    }
    Some((
        patch_target.as_ref().to_string(),
        expected_hash,
        operations,
        format!(
            "Converted redundant ReadFile `{}` into benchmark manifest PreviewEdit using the loaded manifest context.",
            path
        ),
    ))
}

pub(crate) fn dependency_operation_names(
    operations: &[crate::agent_protocol::TomlEditOperation],
) -> BTreeSet<String> {
    operations
        .iter()
        .filter_map(|operation| match operation {
            crate::agent_protocol::TomlEditOperation::SetDependency { name, .. } => {
                Some(name.to_ascii_lowercase())
            }
            crate::agent_protocol::TomlEditOperation::RemoveDependency { .. } => None,
        })
        .collect()
}
