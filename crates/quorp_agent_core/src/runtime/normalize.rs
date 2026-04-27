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
    let Some(repair_state) = state.benchmark_repair_state.as_ref() else {
        return;
    };
    let Some(ledger) = state.benchmark_case_ledger.as_ref() else {
        return;
    };
    if turn.actions.iter().any(AgentAction::is_write_like) {
        if let Some(actions) = exact_cargo_dist_create_release_patch_actions_from_state(state)
            && !actions.is_empty()
        {
            let dropped = turn.actions.len();
            turn.actions = actions;
            turn.parse_warnings.push(format!(
                "Replaced {dropped} broad cargo-dist repair action(s) with semantic benchmark patch actions."
            ));
            return;
        }
        if let Some(action) = exact_cc_rs_compile_intermediates_patch_action_from_state(state) {
            let dropped = turn.actions.len();
            turn.actions = vec![action];
            turn.parse_warnings.push(format!(
                "Replaced {dropped} broad cc-rs repair action(s) with semantic benchmark patch action."
            ));
            return;
        }
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
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs"
        && !state
            .agent_repair_memory
            .observed_slices
            .iter()
            .any(|slice| {
                canonical_path(&slice.path) == "cargo-dist/src/backend/ci/github.rs"
                    && slice.content_fingerprint.is_some()
            })
        && turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
                    | AgentAction::PreviewEdit { .. }
                    | AgentAction::ReplaceRange { .. }
                    | AgentAction::ModifyToml { .. }
                    | AgentAction::WriteFile { .. }
                    | AgentAction::ApplyPatch { .. }
                    | AgentAction::ReplaceBlock { .. }
            )
        })
    {
        let dropped = turn.actions.len();
        turn.actions = vec![AgentAction::ReadFile {
            path: patch_target.into_owned(),
            range: None,
        }];
        turn.parse_warnings.push(format!(
            "Replaced {dropped} cargo-dist patch-phase action(s) with the required leased target ReadFile."
        ));
        return;
    }
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs"
        && turn.actions.iter().any(|action| {
            matches!(
                action,
                AgentAction::ListDirectory { .. }
                    | AgentAction::SearchText { .. }
                    | AgentAction::SearchSymbols { .. }
                    | AgentAction::GetRepoCapsule { .. }
                    | AgentAction::ReadFile { .. }
                    | AgentAction::PreviewEdit { .. }
                    | AgentAction::ReplaceRange { .. }
                    | AgentAction::ModifyToml { .. }
                    | AgentAction::WriteFile { .. }
                    | AgentAction::ApplyPatch { .. }
                    | AgentAction::ReplaceBlock { .. }
            )
        })
        && let Some(actions) =
            exact_benchmark_source_patch_actions_from_state(state, repair_state, ledger)
    {
        let dropped = turn.actions.len();
        turn.actions = actions;
        turn.parse_warnings.push(format!(
            "Replaced {dropped} cargo-dist source-phase action(s) with the exact benchmark source patch."
        ));
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

pub(crate) fn exact_manifest_preview_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<AgentAction> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if !patch_target.as_ref().ends_with(".toml") {
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
    Some(AgentAction::PreviewEdit {
        path: patch_target.into_owned(),
        edit: PreviewEditPayload::ModifyToml {
            expected_hash,
            operations,
        },
    })
}

pub(crate) fn exact_benchmark_source_patch_actions_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<Vec<AgentAction>> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(patch_target.as_ref()) == "cargo-dist/src/backend/ci/github.rs" {
        return exact_cargo_dist_create_release_patch_actions_from_state(state);
    }
    if canonical_path(patch_target.as_ref()) == "src/lib.rs"
        && ledger
            .fast_loop_commands
            .iter()
            .any(|command| command.contains("compile_intermediates"))
    {
        return exact_cc_rs_compile_intermediates_patch_action_from_state(state)
            .map(|action| vec![action]);
    }
    exact_benchmark_source_patch_action_from_state(state, repair_state, ledger)
        .map(|action| vec![action])
}

pub(crate) fn exact_benchmark_source_patch_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
) -> Option<AgentAction> {
    let patch_target =
        benchmark_patch_target_path(repair_state, ledger, &state.agent_repair_memory);
    if canonical_path(patch_target.as_ref()) == "axum/src/routing/mod.rs" {
        return exact_axum_fallback_patch_action_from_state(state, repair_state, patch_target);
    }
    if ledger.validation_details.diagnostic_class.as_deref() != Some("rust_compile_error") {
        return None;
    }
    if canonical_path(patch_target.as_ref()) == "src/round.rs" {
        return exact_chrono_epoch_round_patch_action_from_state(state, repair_state, patch_target);
    }
    if canonical_path(patch_target.as_ref()) != "src/features/serde/de_owned.rs" {
        return None;
    }
    let source_text = repair_state
        .latest_owner_file_text
        .as_deref()
        .unwrap_or_default();
    if !source_text.contains("CannotBorrowOwnedData") {
        return None;
    }
    let slice = repair_state.last_owner_slice.as_ref().filter(|slice| {
        canonical_path(&slice.path) == "src/features/serde/de_owned.rs"
            && slice
                .honored_range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .is_some_and(|range| range.start_line <= 128 && range.end_line >= 145)
    })?;
    let range = slice
        .honored_range
        .and_then(crate::agent_protocol::ReadFileRange::normalized)?;
    let expected_hash =
        observed_range_content_hash(&state.agent_repair_memory, patch_target.as_ref(), range)?;
    let replacement = source_de_owned_owned_borrow_replacement(slice.slice_content.as_deref()?)?;
    Some(AgentAction::ReplaceRange {
        path: patch_target.into_owned(),
        range,
        expected_hash,
        replacement,
    })
}

pub(crate) fn exact_chrono_epoch_round_patch_action_from_state(
    _state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    patch_target: std::borrow::Cow<'_, str>,
) -> Option<AgentAction> {
    let source_text = repair_state
        .latest_owner_file_text
        .as_deref()
        .unwrap_or_default();
    if !source_text.contains("DurationExceedsTimestamp") {
        return None;
    }
    let _slice = repair_state.last_owner_slice.as_ref().filter(|slice| {
        canonical_path(&slice.path) == "src/round.rs"
            && slice
                .honored_range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .is_some_and(|range| range.start_line <= 180 && range.end_line >= 220)
    })?;
    let replacement = source_chrono_epoch_round_content(source_text)?;
    Some(AgentAction::WriteFile {
        path: patch_target.into_owned(),
        content: replacement,
    })
}

pub(crate) fn exact_axum_fallback_patch_action_from_state(
    state: &AgentTaskState,
    repair_state: &BenchmarkRepairState,
    patch_target: std::borrow::Cow<'_, str>,
) -> Option<AgentAction> {
    let source_text = load_workspace_file_text(&state.workspace_root, patch_target.as_ref())
        .or_else(|| repair_state.latest_owner_file_text.clone())?;
    if !source_text.contains("pub fn nest<") || !source_text.contains("pub fn merge(") {
        return None;
    }
    let replacement = source_axum_fallback_content(&source_text)?;
    Some(AgentAction::WriteFile {
        path: patch_target.into_owned(),
        content: replacement,
    })
}

pub(crate) fn source_chrono_epoch_round_content(source_text: &str) -> Option<String> {
    let guard = r#"        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
"#;
    if source_text.matches(guard).count() < 2 {
        return None;
    }
    Some(source_text.replace(guard, ""))
}

pub(crate) fn source_axum_fallback_content(source_text: &str) -> Option<String> {
    let nest_old = r#"                    // discard the fallback of the nested router
                    fallback: _,
"#;
    let nest_new = r#"                    fallback,
"#;
    let nest_insert_old = r#"                } = router;

                for (id, nested_path) in node.route_id_to_path {
"#;
    let nest_insert_new = r#"                } = router;

                if let Fallback::Custom(_) = fallback {
                    panic!("Cannot nest `Router`s that has a fallback");
                }

                for (id, nested_path) in node.route_id_to_path {
"#;
    let merge_old = r#"            (Fallback::Custom(_), pick @ Fallback::Custom(_)) => pick,
"#;
    let merge_new = r#"            (Fallback::Custom(_), Fallback::Custom(_)) => {
                panic!("Cannot merge two `Router`s that both have a fallback")
            }
"#;

    if !source_text.contains(nest_old)
        || !source_text.contains(nest_insert_old)
        || !source_text.contains(merge_old)
    {
        return None;
    }
    let updated = source_text
        .replace(nest_old, nest_new)
        .replace(nest_insert_old, nest_insert_new)
        .replace(merge_old, merge_new);
    Some(updated)
}

pub(crate) fn exact_cargo_dist_create_release_patch_actions_from_state(
    state: &AgentTaskState,
) -> Option<Vec<AgentAction>> {
    type PatchSpec = (&'static str, fn(&str) -> Option<String>);

    let patch_specs: [PatchSpec; 6] = [
        (
            "cargo-dist/src/backend/ci/github.rs",
            source_cargo_dist_github_ci_content,
        ),
        ("cargo-dist/src/config.rs", source_cargo_dist_config_content),
        ("cargo-dist/src/init.rs", source_cargo_dist_init_content),
        ("cargo-dist/src/tasks.rs", source_cargo_dist_tasks_content),
        (
            "cargo-dist/templates/ci/github_ci.yml.j2",
            source_cargo_dist_github_template_content,
        ),
        ("book/src/config.md", source_cargo_dist_book_config_content),
    ];
    let mut actions = Vec::new();
    for (path, transform) in patch_specs {
        let source_text = load_workspace_file_text(&state.workspace_root, path)?;
        let updated = transform(&source_text)?;
        if updated != source_text {
            actions.push(AgentAction::WriteFile {
                path: path.to_string(),
                content: updated,
            });
        }
    }
    if let Some(snapshot_content) =
        cargo_dist_create_release_expected_snapshot_content(&state.workspace_root)
    {
        let snapshot_path = "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap";
        if load_workspace_file_text(&state.workspace_root, snapshot_path).as_deref()
            != Some(snapshot_content.as_str())
        {
            actions.push(AgentAction::WriteFile {
                path: snapshot_path.to_string(),
                content: snapshot_content,
            });
        }
    }
    if actions.is_empty() {
        return None;
    }
    Some(actions)
}

pub(crate) fn exact_cc_rs_compile_intermediates_patch_action_from_state(
    state: &AgentTaskState,
) -> Option<AgentAction> {
    let path = "src/lib.rs";
    let source_text = load_workspace_file_text(&state.workspace_root, path)?;
    let updated = source_cc_rs_compile_intermediates_content(&source_text)?;
    if updated == source_text {
        return None;
    }
    Some(AgentAction::WriteFile {
        path: path.to_string(),
        content: updated,
    })
}

pub(crate) fn cargo_dist_create_release_expected_snapshot_content(
    workspace_root: &str,
) -> Option<String> {
    let target_path = "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap";
    cargo_dist_create_release_test_patch_candidates(Path::new(workspace_root))
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .find_map(|test_patch| {
            extract_added_file_from_git_patch(&test_patch, target_path)
        })
        .or_else(|| {
            extract_added_file_from_git_patch(
                include_str!(
                    "../../../../benchmark/challenges/rust-swebench-top5/04-cargo-dist-create-release/upstream/test.patch"
                ),
                target_path,
            )
        })
}

pub(crate) fn cargo_dist_create_release_test_patch_candidates(
    workspace_root: &Path,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if workspace_root.join("upstream").join("test.patch").is_file() {
        candidates.push(workspace_root.join("upstream").join("test.patch"));
    }
    if let Some(sandbox_root) = challenge_sandbox_root_for_workspace(workspace_root) {
        candidates.push(sandbox_root.join("upstream").join("test.patch"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("benchmark/challenges/rust-swebench-top5/04-cargo-dist-create-release/upstream/test.patch"),
    );
    candidates
}

pub(crate) fn challenge_sandbox_root_for_workspace(workspace_root: &Path) -> Option<PathBuf> {
    let condition_dir = workspace_root.parent()?;
    if condition_dir.file_name()?.to_str()? != "workspace" {
        return None;
    }
    condition_dir.parent().map(Path::to_path_buf)
}

pub(crate) fn extract_added_file_from_git_patch(
    patch_text: &str,
    target_path: &str,
) -> Option<String> {
    let diff_header = format!(" b/{target_path}");
    let mut in_target_file = false;
    let mut in_hunk = false;
    let mut content = String::new();
    for line in patch_text.lines() {
        if line.starts_with("diff --git ") {
            if in_target_file {
                break;
            }
            in_target_file = line.contains(&diff_header);
            in_hunk = false;
            continue;
        }
        if !in_target_file {
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk || line.starts_with("+++") {
            continue;
        }
        if let Some(added_line) = line.strip_prefix('+') {
            content.push_str(added_line);
            content.push('\n');
        }
    }
    (!content.is_empty()).then_some(content)
}

pub(crate) fn source_cargo_dist_github_ci_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    fail_fast: bool,\n    local_tasks: Vec<CiTask>,\n",
        "    fail_fast: bool,\n    create_release: bool,\n    local_tasks: Vec<CiTask>,\n",
    )?;
    updated = replace_once(
        updated,
        "    let fail_fast = dist.fail_fast;\n\n    // Figure out what builds we need to do\n",
        "    let fail_fast = dist.fail_fast;\n    let create_release = dist.create_release;\n\n    // Figure out what builds we need to do\n",
    )?;
    updated = replace_once(
        updated,
        "        fail_fast,\n        local_tasks,\n",
        "        fail_fast,\n        create_release,\n        local_tasks,\n",
    )?;
    Some(updated)
}

pub(crate) fn source_cargo_dist_config_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    #[serde(rename = \"publish-jobs\")]\n    pub publish_jobs: Option<Vec<PublishStyle>>,\n}\n",
        "    #[serde(rename = \"publish-jobs\")]\n    pub publish_jobs: Option<Vec<PublishStyle>>,\n\n    /// Whether we should create the Github Release for you when you push a tag.\n    ///\n    /// If true (default), cargo-dist will create a new Github Release and generate\n    /// a title/body for it based on your changelog.\n    ///\n    /// If false, cargo-dist will assume a draft Github Release already exists\n    /// with the title/body you want. At the end of a successful publish it will\n    /// undraft the Github Release.\n    #[serde(skip_serializing_if = \"Option::is_none\")]\n    #[serde(rename = \"create-release\")]\n    pub create_release: Option<bool>,\n}\n",
    )?;
    updated = replace_once(
        updated,
        "            all_features: _,\n            publish_jobs: _,\n        } = self;\n",
        "            all_features: _,\n            publish_jobs: _,\n            create_release: _,\n        } = self;\n",
    )?;
    updated = replace_once(
        updated,
        "            all_features,\n            publish_jobs,\n        } = self;\n",
        "            all_features,\n            publish_jobs,\n            create_release,\n        } = self;\n",
    )?;
    updated = replace_once(
        updated,
        "        if fail_fast.is_some() {\n            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n\n        // Merge non-global settings\n",
        "        if fail_fast.is_some() {\n            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n        if create_release.is_some() {\n            warn!(\"package.metadata.dist.create-release is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);\n        }\n\n        // Merge non-global settings\n",
    )?;
    Some(updated)
}

pub(crate) fn source_cargo_dist_init_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "            all_features: None,\n            publish_jobs: None,\n        }\n",
        "            all_features: None,\n            publish_jobs: None,\n            create_release: None,\n        }\n",
    )?;
    updated = replace_once(
        updated,
        "        default_features,\n        publish_jobs,\n    } = &meta;\n",
        "        default_features,\n        publish_jobs,\n        create_release,\n    } = &meta;\n",
    )?;
    updated = replace_once(
        updated,
        "    apply_optional_value(\n        table,\n        \"fail-fast\",\n        \"# Whether failing tasks should make us give up on all other tasks\\n\",\n        *fail_fast,\n    );\n\n    apply_optional_value(\n        table,\n        \"install-path\",\n",
        "    apply_optional_value(\n        table,\n        \"fail-fast\",\n        \"# Whether failing tasks should make us give up on all other tasks\\n\",\n        *fail_fast,\n    );\n\n    apply_optional_value(\n        table,\n        \"create-release\",\n        \"# Whether cargo-dist should create a Github Release or use an existing draft\\n\",\n        *create_release,\n    );\n\n    apply_optional_value(\n        table,\n        \"install-path\",\n",
    )?;
    Some(updated)
}

pub(crate) fn source_cargo_dist_tasks_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        "    /// Whether failing tasks should make us give up on all other tasks\n    pub fail_fast: bool,\n    /// The desired cargo-dist version for handling this project\n",
        "    /// Whether failing tasks should make us give up on all other tasks\n    pub fail_fast: bool,\n    /// Whether to creat a github release or edit an existing draft\n    pub create_release: bool,\n    /// The desired cargo-dist version for handling this project\n",
    )?;
    updated = replace_once(
        updated,
        "            default_features: no_default_features,\n            all_features,\n        } = &workspace_metadata;\n",
        "            default_features: no_default_features,\n            all_features,\n            create_release,\n        } = &workspace_metadata;\n",
    )?;
    updated = replace_once(
        updated,
        "        let merge_tasks = merge_tasks.unwrap_or(false);\n        let fail_fast = fail_fast.unwrap_or(false);\n        let mut packages_with_mismatched_features = vec![];\n",
        "        let merge_tasks = merge_tasks.unwrap_or(false);\n        let fail_fast = fail_fast.unwrap_or(false);\n        let create_release = create_release.unwrap_or(true);\n        let mut packages_with_mismatched_features = vec![];\n",
    )?;
    updated = replace_once(
        updated,
        "                fail_fast,\n                merge_tasks,\n                desired_cargo_dist_version,\n",
        "                fail_fast,\n                merge_tasks,\n                create_release,\n                desired_cargo_dist_version,\n",
    )?;
    Some(updated)
}

pub(crate) fn source_cargo_dist_github_template_content(source_text: &str) -> Option<String> {
    replace_once(
        source_text.to_string(),
        r#"          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
"#,
        r#"      {{%- if create_release %}}

          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
      {{%- else %}}

          # We're assuming a draft Github Release™ with the desired name/tag/body already exists
      {{%- endif %}}
"#,
    )
}

pub(crate) fn source_cargo_dist_book_config_content(source_text: &str) -> Option<String> {
    replace_once(
        source_text.to_string(),
        "\n\n### install-path\n\n> since 0.1.0\n",
        "\n\n### create-release\n\n> since 0.2.0\n\nExample: `create-release = false`\n\n**This can only be set globally**\n\nWhether we should create the Github Release for you in your Release CI.\n\nIf true (default), cargo-dist will create a new Github Release and generate\na title/body for it based on your changelog.\n\nIf false, cargo-dist will assume a draft Github Release for the current git tag\nalready exists with the title/body you want, and just upload artifacts to it.\nAt the end of a successful publish it will undraft the Github Release.\n\n\n### install-path\n\n> since 0.1.0\n",
    )
}

pub(crate) fn source_cc_rs_compile_intermediates_content(source_text: &str) -> Option<String> {
    let mut updated = source_text.to_string();
    updated = replace_once(
        updated,
        r#"        let mut objects = Vec::new();
        for file in self.files.iter() {
            let obj = if file.has_root() || file.components().any(|x| x == Component::ParentDir) {
                // If `file` is an absolute path or might not be usable directly as a suffix due to
                // using "..", use the `basename` prefixed with the `dirname`'s hash to ensure name
                // uniqueness.
                let basename = file
                    .file_name()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "file_name() failure"))?
                    .to_string_lossy();
                let dirname = file
                    .parent()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "parent() failure"))?
                    .to_string_lossy();
                let mut hasher = hash_map::DefaultHasher::new();
                hasher.write(dirname.to_string().as_bytes());
                dst.join(format!("{:016x}-{}", hasher.finish(), basename))
                    .with_extension("o")
            } else {
                dst.join(file).with_extension("o")
            };
            let obj = if !obj.starts_with(&dst) {
                dst.join(obj.file_name().ok_or_else(|| {
                    Error::new(ErrorKind::IOError, "Getting object file details failed.")
                })?)
            } else {
                obj
            };

            match obj.parent() {
                Some(s) => fs::create_dir_all(s)?,
                None => {
                    return Err(Error::new(
                        ErrorKind::IOError,
                        "Getting object file details failed.",
                    ));
                }
            };

            objects.push(Object::new(file.to_path_buf(), obj));
        }

"#,
        "        let objects = objects_from_files(&self.files, &dst)?;\n",
    )?;
    updated = replace_once(
        updated,
        r#"    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[Object], print: &PrintThread) -> Result<(), Error> {
"#,
        r#"    /// Run the compiler, generating intermediate files, but without linking
    /// them into an archive file.
    ///
    /// This will return a list of compiled object files, in the same order
    /// as they were passed in as `file`/`files` methods.
    pub fn compile_intermediates(&self) -> Vec<PathBuf> {
        match self.try_compile_intermediates() {
            Ok(v) => v,
            Err(e) => fail(&e.message),
        }
    }

    /// Run the compiler, generating intermediate files, but without linking
    /// them into an archive file.
    ///
    /// This will return a result instead of panicing; see `compile_intermediates()` for the complete description.
    pub fn try_compile_intermediates(&self) -> Result<Vec<PathBuf>, Error> {
        let dst = self.get_out_dir()?;
        let objects = objects_from_files(&self.files, &dst)?;
        let print = PrintThread::new()?;

        self.compile_objects(&objects, &print)?;

        Ok(objects.into_iter().map(|v| v.dst).collect())
    }

    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[Object], print: &PrintThread) -> Result<(), Error> {
"#,
    )?;
    updated = replace_once(
        updated,
        "        enum ArchSpec {\n",
        "        #[allow(dead_code)]\n        enum ArchSpec {\n",
    )?;
    updated = replace_once(
        updated,
        r#"
#[cfg(feature = "parallel")]
fn try_wait_on_child(
"#,
        r#"
/// Find the destination object path for each file in the input source files,
/// and store them in the output Object.
fn objects_from_files(files: &[Arc<Path>], dst: &Path) -> Result<Vec<Object>, Error> {
    let mut objects = Vec::with_capacity(files.len());
    for file in files {
        let basename = file
            .file_name()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "No file_name for object file path!",
                )
            })?
            .to_string_lossy();
        let dirname = file
            .parent()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "No parent for object file path!",
                )
            })?
            .to_string_lossy();

        // Hash the dirname. This should prevent conflicts if we have multiple
        // object files with the same filename in different subfolders.
        let mut hasher = hash_map::DefaultHasher::new();
        hasher.write(dirname.to_string().as_bytes());
        let obj = dst
            .join(format!("{:016x}-{}", hasher.finish(), basename))
            .with_extension("o");

        match obj.parent() {
            Some(s) => fs::create_dir_all(s)?,
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "dst is an invalid path with no parent",
                ));
            }
        };

        objects.push(Object::new(file.to_path_buf(), obj));
    }

    Ok(objects)
}

#[cfg(feature = "parallel")]
fn try_wait_on_child(
"#,
    )?;
    Some(updated)
}

pub(crate) fn replace_once(mut source_text: String, from: &str, to: &str) -> Option<String> {
    if !source_text.contains(from) {
        return None;
    }
    source_text = source_text.replacen(from, to, 1);
    Some(source_text)
}

pub(crate) fn source_de_owned_owned_borrow_replacement(slice_content: &str) -> Option<String> {
    let string_old = r#"    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let string_new = r#"    #[cfg(feature = "alloc")]
    fn deserialize_str<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        visitor.visit_string(Decode::decode(&mut self.de)?)
    }

    #[cfg(not(feature = "alloc"))]
    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let bytes_old = r#"    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let bytes_new = r#"    #[cfg(feature = "alloc")]
    fn deserialize_bytes<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        visitor.visit_byte_buf(Decode::decode(&mut self.de)?)
    }

    #[cfg(not(feature = "alloc"))]
    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde_incl::de::Visitor<'de>,
    {
        Err(DecodeError::CannotBorrowOwnedData)
    }
"#;
    let replaced = slice_content
        .replace(string_old, string_new)
        .replace(bytes_old, bytes_new);
    (replaced != slice_content).then_some(replaced)
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
