//! Implementation-target range suggestion helpers (chrono epoch
//! rounding, axum fallback merge, signature window detection).

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
pub(crate) fn extract_failing_test_names(output_text: &str, named_tests: &[String]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for candidate in named_tests {
        if !candidate.trim().is_empty() && output_text.contains(candidate) {
            names.insert(candidate.trim().to_string());
        }
    }
    for line in output_text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("test ")
            .and_then(|value| value.strip_suffix(" ... FAILED"))
        {
            let value = name.trim();
            if !value.is_empty() {
                names.insert(value.to_string());
            }
        }
        if let Some(name) = trimmed
            .strip_prefix("---- ")
            .and_then(|value| value.strip_suffix(" stdout ----"))
        {
            let value = name.trim();
            if !value.is_empty() {
                names.insert(value.to_string());
            }
        }
    }
    names.into_iter().collect()
}

pub(crate) fn extract_primary_failure_location(
    output_text: &str,
    owner_files: &[String],
    expected_touch_targets: &[String],
) -> (Option<String>, Option<usize>, Option<String>) {
    let candidate_paths = owner_files
        .iter()
        .chain(expected_touch_targets.iter())
        .cloned()
        .collect::<Vec<_>>();
    let real_error_seen = classify_benchmark_diagnostic(output_text)
        .as_deref()
        .is_some_and(|class| class != "test_assertion_failure");
    let mut current_test_name: Option<String> = None;
    for line in output_text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if real_error_seen && (lower.contains("warning:") || lower.contains("unexpected `cfg`")) {
            continue;
        }
        if let Some(test_name) = trimmed
            .strip_prefix("---- ")
            .and_then(|value| value.strip_suffix(" stdout ----"))
        {
            let value = test_name.trim();
            if !value.is_empty() {
                current_test_name = Some(value.to_string());
            }
            continue;
        }
        for path in &candidate_paths {
            if let Some(line_number) = find_line_number_for_path(trimmed, path) {
                return (
                    Some(path.clone()),
                    Some(line_number),
                    current_test_name.clone(),
                );
            }
        }
    }
    for token in output_text.split_whitespace() {
        if let Some((path, line_number)) = parse_path_line_token(token) {
            return (Some(path), Some(line_number), current_test_name.clone());
        }
    }
    (None, None, current_test_name)
}

pub(crate) fn find_line_number_for_path(output_text: &str, path: &str) -> Option<usize> {
    let needle = format!("{path}:");
    output_text.lines().find_map(|line| {
        let index = line.find(&needle)?;
        let remainder = &line[index + needle.len()..];
        remainder
            .split(':')
            .next()
            .and_then(|value| value.parse::<usize>().ok())
    })
}

pub(crate) fn parse_path_line_token(token: &str) -> Option<(String, usize)> {
    let cleaned = token.trim_matches(|character: char| {
        matches!(
            character,
            ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    });
    let path_start = cleaned.find(".rs:")?;
    let path_end = path_start + 3;
    let path = cleaned[..path_end].to_string();
    let remainder = cleaned.get(path_end + 1..)?;
    let line_number = remainder
        .split(':')
        .next()
        .and_then(|value| value.parse::<usize>().ok())?;
    Some((path, line_number))
}

pub(crate) fn extract_assertion_excerpt(output_text: &str) -> Option<String> {
    let lines = output_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let unresolved_imports = extract_unresolved_import_names(output_text);
    if unresolved_imports.len() > 1 {
        return Some(format!(
            "unresolved imports/crates: {}",
            unresolved_imports.join(", ")
        ));
    }
    if classify_benchmark_diagnostic(output_text)
        .as_deref()
        .is_some_and(|class| class != "test_assertion_failure")
    {
        for line in &lines {
            let lower = line.to_ascii_lowercase();
            if lower.contains("warning:") || lower.contains("unexpected `cfg`") {
                continue;
            }
            if lower.starts_with("error")
                || lower.contains("unresolved import")
                || lower.contains("expected one of")
                || lower.contains("mismatched closing delimiter")
                || lower.contains("unclosed delimiter")
            {
                return Some(truncate_visible_text(line, 220));
            }
        }
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("assertion `")
            || lower.contains("panicked at ")
            || lower.starts_with("left:")
            || lower.starts_with("right:")
        {
            return Some(truncate_visible_text(line, 220));
        }
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("warning:") || lower.contains("unexpected `cfg`") {
            continue;
        }
        if lower.contains("assert")
            || lower.contains("panic")
            || lower.contains("expected")
            || lower.contains("left:")
            || lower.contains("right:")
        {
            return Some(truncate_visible_text(line, 220));
        }
    }
    lines
        .into_iter()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("warning:")
                && !lower.contains("unexpected `cfg`")
                && !lower.starts_with("command failed:")
        })
        .map(|line| truncate_visible_text(line, 220))
}

pub(crate) fn extract_unresolved_import_names(output_text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in output_text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(summary_names) = lower
            .contains("unresolved imports/crates:")
            .then(|| unresolved_import_summary_names(line))
        {
            for name in summary_names {
                names.insert(name);
            }
            continue;
        }
        if !(lower.contains("unresolved import")
            || lower.contains("unresolved crate")
            || lower.contains("use of unresolved module or unlinked crate"))
        {
            continue;
        }
        for name in backtick_spans(line) {
            let root = name
                .split("::")
                .next()
                .unwrap_or(name.as_str())
                .trim_matches(|character: char| {
                    character == '{'
                        || character == '}'
                        || character == ','
                        || character.is_whitespace()
                });
            if !root.is_empty() {
                names.insert(root.to_string());
                break;
            }
        }
    }
    names.into_iter().collect()
}

pub(crate) fn extract_manifest_feature_dependency_names(output_text: &str) -> Vec<String> {
    let lower = output_text.to_ascii_lowercase();
    let mut names = BTreeSet::new();
    if lower.contains("uuid") {
        names.insert("uuid".to_string());
    }
    if lower.contains("datetime<utc>")
        || lower.contains("chrono::datetime")
        || lower.contains("chrono")
    {
        names.insert("chrono".to_string());
    }
    names.into_iter().collect()
}

pub(crate) fn unresolved_import_summary_names(line: &str) -> Vec<String> {
    let lower = line.to_ascii_lowercase();
    let marker = "unresolved imports/crates:";
    let Some(marker_index) = lower.find(marker) else {
        return Vec::new();
    };
    let tail = &line[marker_index + marker.len()..];
    let tail = tail.split('|').next().unwrap_or(tail);
    tail.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .trim_matches(|character: char| {
                    character == '`'
                        || character == '{'
                        || character == '}'
                        || character == ','
                        || character.is_whitespace()
                })
                .split("::")
                .next()
                .unwrap_or(value)
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect()
}

pub(crate) fn backtick_spans(line: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut remainder = line;
    while let Some(start) = remainder.find('`') {
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        spans.push(after_start[..end].to_string());
        remainder = &after_start[end + 1..];
    }
    spans
}

pub(crate) fn repair_requirement_from_action(
    action: &AgentAction,
    failure_reason: &str,
) -> Option<RepairRequirement> {
    match action {
        AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::SetExecutable { path } => Some(RepairRequirement {
            path: path.clone(),
            failure_reason: truncate_visible_text(failure_reason, 240),
            previous_search_block: None,
            suggested_range: suggested_reread_range_from_failure(path, failure_reason)
                .or_else(|| fallback_repair_read_range(path)),
            exact_reread_completed: false,
        }),
        AgentAction::ReplaceBlock {
            path, search_block, ..
        } => Some(RepairRequirement {
            path: path.clone(),
            failure_reason: truncate_visible_text(failure_reason, 240),
            previous_search_block: Some(truncate_visible_text(search_block, 1_200)),
            suggested_range: suggested_reread_range_from_failure(path, failure_reason)
                .or_else(|| fallback_repair_read_range(path)),
            exact_reread_completed: false,
        }),
        AgentAction::RunCommand { .. }
        | AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::ApplyPreview { .. }
        | AgentAction::McpCallTool { .. }
        | AgentAction::RunValidation { .. } => None,
    }
}

pub(crate) fn fallback_repair_read_range(path: &str) -> Option<crate::agent_protocol::ReadFileRange> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let end_line = if trimmed.ends_with("Cargo.toml")
        || trimmed.ends_with(".toml")
        || trimmed.ends_with(".json")
    {
        120
    } else {
        160
    };
    Some(crate::agent_protocol::ReadFileRange {
        start_line: 1,
        end_line,
    })
}

pub(crate) fn failed_edit_record_from_action(
    action: &AgentAction,
    failure_reason: &str,
) -> Option<FailedEditRecord> {
    let (action_kind, path, search_hash, replace_hash) = match action {
        AgentAction::ReplaceBlock {
            path,
            search_block,
            replace_block,
            ..
        } => (
            "replace_block",
            path.clone(),
            Some(stable_text_hash(search_block)),
            Some(stable_text_hash(replace_block)),
        ),
        AgentAction::ApplyPatch { path, patch } => (
            "apply_patch",
            path.clone(),
            Some(stable_text_hash(patch)),
            None,
        ),
        AgentAction::WriteFile { path, content } => (
            "write_file",
            path.clone(),
            None,
            Some(stable_text_hash(content)),
        ),
        AgentAction::ReplaceRange {
            path,
            expected_hash,
            replacement,
            ..
        } => (
            "replace_range",
            path.clone(),
            Some(stable_text_hash(expected_hash)),
            Some(stable_text_hash(replacement)),
        ),
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => (
            "modify_toml",
            path.clone(),
            Some(stable_text_hash(expected_hash)),
            Some(stable_text_hash(&format!("{operations:?}"))),
        ),
        AgentAction::ApplyPreview { preview_id } => (
            "apply_preview",
            preview_id.clone(),
            Some(stable_text_hash(preview_id)),
            None,
        ),
        AgentAction::SetExecutable { path } => ("set_executable", path.clone(), None, None),
        AgentAction::RunCommand { .. }
        | AgentAction::ReadFile { .. }
        | AgentAction::ListDirectory { .. }
        | AgentAction::SearchText { .. }
        | AgentAction::SearchSymbols { .. }
        | AgentAction::FindFiles { .. }
        | AgentAction::StructuralSearch { .. }
        | AgentAction::StructuralEditPreview { .. }
        | AgentAction::CargoDiagnostics { .. }
        | AgentAction::GetRepoCapsule { .. }
        | AgentAction::ExplainValidationFailure { .. }
        | AgentAction::SuggestImplementationTargets { .. }
        | AgentAction::SuggestEditAnchors { .. }
        | AgentAction::PreviewEdit { .. }
        | AgentAction::McpCallTool { .. }
        | AgentAction::RunValidation { .. } => return None,
    };
    Some(FailedEditRecord {
        action_kind: action_kind.to_string(),
        path,
        search_hash,
        replace_hash,
        failure_reason: truncate_visible_text(failure_reason, 260),
        matching_line_numbers: extract_matching_line_numbers(failure_reason),
        attempts: 1,
    })
}

pub(crate) fn failed_edit_signature_matches(left: &FailedEditRecord, right: &FailedEditRecord) -> bool {
    left.action_kind == right.action_kind
        && left.path == right.path
        && left.search_hash == right.search_hash
        && left.replace_hash == right.replace_hash
}

pub(crate) fn stable_text_hash(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(crate) fn extract_matching_line_numbers(failure_reason: &str) -> Vec<usize> {
    let lower = failure_reason.to_ascii_lowercase();
    let Some(index) = lower.find("lines ") else {
        return Vec::new();
    };
    let segment = failure_reason[index + "lines ".len()..]
        .split(['.', '\n'])
        .next()
        .unwrap_or_default();
    segment
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|token| token.parse::<usize>().ok())
        .collect()
}

pub(crate) fn failed_edit_is_ambiguous(record: &FailedEditRecord) -> bool {
    record.matching_line_numbers.len() > 1
        || record
            .failure_reason
            .to_ascii_lowercase()
            .contains("ambiguous")
}

pub(crate) fn bare_replace_block_disallowed_for_path(path: &str, records: &[FailedEditRecord]) -> bool {
    records
        .iter()
        .filter(|record| record.action_kind == "replace_block" && record.path == path)
        .filter(|record| failed_edit_is_ambiguous(record))
        .count()
        >= 1
}

pub(crate) fn bare_replace_block_repeats_failed_signature(
    action: &AgentAction,
    records: &[FailedEditRecord],
) -> bool {
    let Some(record) = failed_edit_record_from_action(action, "") else {
        return false;
    };
    records
        .iter()
        .any(|existing| failed_edit_signature_matches(existing, &record))
}

pub(crate) fn render_failed_edit_memory(records: &[FailedEditRecord]) -> String {
    records
        .iter()
        .rev()
        .take(4)
        .map(|record| {
            let lines = if record.matching_line_numbers.is_empty() {
                "lines=unknown".to_string()
            } else {
                format!(
                    "lines={}",
                    record
                        .matching_line_numbers
                        .iter()
                        .take(8)
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            };
            format!(
                "{} {} attempts={} {} reason={}",
                record.action_kind,
                record.path,
                record.attempts,
                lines,
                truncate_visible_text(&record.failure_reason, 120)
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

pub(crate) fn render_agent_repair_memory(memory: &AgentRepairMemory) -> String {
    let mut parts = Vec::new();
    if let Some(action) = memory.current_required_action.as_ref() {
        parts.push(format!("required_next={action}"));
    }
    if let Some(phase) = memory.repair_phase.as_ref() {
        parts.push(format!(
            "phase={phase} context_sufficient={}",
            memory.context_sufficient
        ));
    }
    if let Some(diagnostic_class) = memory.diagnostic_class.as_ref() {
        parts.push(format!("diagnostic_class={diagnostic_class}"));
    }
    if let Some(target) = memory.implementation_target_lease.as_ref() {
        parts.push(format!("target_lease={target}"));
    }
    if let Some(table) = memory.target_dependency_table.as_ref() {
        parts.push(format!("target_table={table}"));
    }
    if !memory.dependency_candidates.is_empty() {
        parts.push(format!(
            "dependency_candidates={}",
            memory.dependency_candidates.join(",")
        ));
    }
    if !memory.last_manifest_patch_operations.is_empty() {
        parts.push(format!(
            "last_manifest_ops={}",
            render_toml_edit_operations_brief(&memory.last_manifest_patch_operations)
        ));
    }
    if let Some(diagnostic_class) = memory.post_patch_diagnostic_class.as_ref() {
        parts.push(format!("post_patch_class={diagnostic_class}"));
    }
    if let Some(excerpt) = memory.post_patch_diagnostic_excerpt.as_ref() {
        parts.push(format!(
            "post_patch_excerpt={}",
            truncate_visible_text(excerpt, 120)
        ));
    }
    if !memory.ranked_implementation_targets.is_empty() {
        parts.push(format!(
            "ranked_targets={}",
            render_ranked_implementation_targets(&memory.ranked_implementation_targets)
        ));
    }
    if !memory.observed_slices.is_empty() {
        let observed = memory
            .observed_slices
            .iter()
            .rev()
            .take(3)
            .map(|slice| {
                let range = slice
                    .honored_range
                    .or(slice.requested_range)
                    .map(|range| range.label())
                    .unwrap_or_else(|| "unranged".to_string());
                let purpose = slice
                    .purpose
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("read");
                format!("{}:{}:{purpose}", slice.path, range)
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("observed_slices={observed}"));
    }
    if !memory.rejected_actions.is_empty() {
        let rejected = memory
            .rejected_actions
            .iter()
            .rev()
            .take(2)
            .map(|record| {
                format!(
                    "{}:{}",
                    record.phase,
                    truncate_visible_text(&record.actions.join("+"), 80)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("rejected={rejected}"));
    }
    if !memory.suggested_edit_anchors.is_empty() {
        let anchors = memory
            .suggested_edit_anchors
            .iter()
            .rev()
            .take(2)
            .map(|anchor| {
                let range = anchor
                    .range
                    .map(|range| range.label())
                    .unwrap_or_else(|| "unranged".to_string());
                format!("{}:{range}", anchor.path)
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("anchor_suggestions={anchors}"));
    }
    if let Some(preview) = memory.last_preview_result.as_ref() {
        parts.push(format!(
            "last_preview={}",
            truncate_visible_text(preview, 120)
        ));
    }
    if let Some(rollback) = memory.last_rollback_diagnostic.as_ref() {
        parts.push(format!(
            "last_rollback={}",
            truncate_visible_text(rollback, 120)
        ));
    }
    let score = &memory.scorecard;
    parts.push(format!(
        "score parser_recovery={} repair_turns={} repair_invalid_max={} write_locked={} write_refusals={} scaffold_offered={} scaffold_honored={} write_emitted={} support_writes={} source_writes={} rolled_back_writes={} rolled_back_non_support={} line_tools={} injected_reads={} redundant_reads={} first_write={} repeated_edits={} validation_rejects={} test_edit_rejects={} target_redirects={} evidence_fixations={} anchors={} previews={}/{} syntax_previews={}/{}",
        score.parser_recovery_count,
        score.repair_submode_turns,
        score.repair_invalid_action_streak_max,
        score.repair_write_locked,
        score.write_phase_action_refusal_count,
        score.patch_scaffold_offered,
        score.patch_scaffold_honored,
        score.write_phase_write_emitted,
        score.support_write_count,
        score.source_write_count,
        score.rolled_back_write_count,
        score.rolled_back_non_support_edit_count,
        score.line_oriented_parse_count,
        score.controller_injected_read_count,
        score.redundant_read_count,
        score
            .first_valid_write_step
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        score.repeated_failed_edit_count,
        score.rejected_validation_alias_count,
        score.test_edit_rejection_count,
        score.target_redirect_count,
        score.evidence_file_fixation_count,
        score.anchor_suggestion_count,
        score.preview_edit_success_count,
        score.preview_edit_count,
        score.syntax_preview_failure_count,
        score.syntax_preview_count
    ));
    parts.join(" | ")
}

pub(crate) fn suggested_reread_range_from_failure(
    path: &str,
    failure_reason: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let needle = format!("{path}:");
    let index = failure_reason.find(&needle)?;
    let remainder = &failure_reason[index + needle.len()..];
    let digits = remainder
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    let line_number = digits.parse::<usize>().ok()?;
    Some(suggested_range_for_line(line_number))
}

pub(crate) fn suggested_range_for_line(line_number: usize) -> crate::agent_protocol::ReadFileRange {
    let start_line = line_number.saturating_sub(8).max(1);
    let end_line = line_number.saturating_add(24);
    crate::agent_protocol::ReadFileRange {
        start_line,
        end_line,
    }
}

pub(crate) fn load_workspace_file_text(workspace_root: &str, path: &str) -> Option<String> {
    let relative_path = path.trim();
    if relative_path.is_empty() {
        return None;
    }
    let candidate_path = PathBuf::from(workspace_root).join(relative_path);
    let canonical_root = PathBuf::from(workspace_root).canonicalize().ok()?;
    let canonical_candidate = candidate_path.canonicalize().ok()?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return None;
    }
    fs::read_to_string(canonical_candidate).ok()
}

pub(crate) fn implementation_name_candidates(primary_failure_test_name: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let leaf = primary_failure_test_name
        .split("::")
        .last()
        .unwrap_or(primary_failure_test_name)
        .trim();
    let leaf = leaf.strip_prefix("test_").unwrap_or(leaf);
    let stop_words = [
        "close", "to", "min", "max", "epoch", "exact", "exactly", "near",
    ];
    let tokens = leaf
        .split('_')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    for window in tokens.windows(2) {
        let first = window[0];
        let second = window[1];
        if stop_words.contains(&first) || stop_words.contains(&second) {
            continue;
        }
        candidates.push(format!("{first}_{second}"));
    }
    for token in tokens {
        if stop_words.contains(&token) {
            continue;
        }
        candidates.push(token.to_string());
    }
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        if seen.insert(candidate.clone()) {
            deduped.push(candidate);
        }
    }
    deduped
}

pub(crate) fn suggest_implementation_range_from_owner_text(
    owner_text: &str,
    primary_failure_test_name: Option<&str>,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let primary_failure_test_name = primary_failure_test_name?;
    if let Some(range) = suggest_axum_fallback_merge_range(owner_text, primary_failure_test_name) {
        return Some(range);
    }
    if let Some(range) = suggest_chrono_epoch_rounding_range(owner_text, primary_failure_test_name)
    {
        return Some(range);
    }
    let candidates = implementation_name_candidates(primary_failure_test_name);
    if candidates.is_empty() {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let mut best_match: Option<(i32, usize)> = None;
    for (index, _) in lines.iter().enumerate() {
        let signature_window = implementation_signature_window(&lines, index);
        if signature_window.is_empty() {
            continue;
        }
        let lower_signature = signature_window.to_ascii_lowercase();
        for candidate in &candidates {
            if !signature_matches_candidate(&lower_signature, candidate) {
                continue;
            }
            let score =
                implementation_signature_score(lines[index], &lower_signature, candidate, index);
            if best_match.as_ref().is_none_or(|(best_score, best_index)| {
                score > *best_score || (score == *best_score && index < *best_index)
            }) {
                best_match = Some((score, index));
            }
        }
    }
    let (_, index) = best_match?;
    let center_line = index.saturating_add(1);
    let start_line = center_line.saturating_sub(18).max(1);
    let end_line = center_line.saturating_add(48);
    Some(crate::agent_protocol::ReadFileRange {
        start_line,
        end_line,
    })
}

pub(crate) fn suggest_chrono_epoch_rounding_range(
    owner_text: &str,
    primary_failure_test_name: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    if !primary_failure_test_name.contains("duration_round")
        && !primary_failure_test_name.contains("duration_trunc")
    {
        return None;
    }
    if !owner_text.contains("DurationExceedsTimestamp") {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let round_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("fn duration_round<"))?;
    let trunc_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("fn duration_trunc<"))?;
    let first_index = round_index.min(trunc_index);
    let second_index = round_index.max(trunc_index);
    Some(crate::agent_protocol::ReadFileRange {
        start_line: first_index.saturating_add(1).saturating_sub(18).max(1),
        end_line: second_index.saturating_add(71),
    })
}

pub(crate) fn suggest_axum_fallback_merge_range(
    owner_text: &str,
    signal_text: &str,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let lower_signal = signal_text.to_ascii_lowercase();
    if !lower_signal.contains("fallback")
        && !lower_signal.contains("merge")
        && !lower_signal.contains("nest")
    {
        return None;
    }
    if !owner_text.contains("pub fn nest<")
        || !owner_text.contains("pub fn merge(")
        || !owner_text.contains("Fallback::Custom")
    {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    let nest_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub fn nest<"))?;
    let merge_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub fn merge("))?;
    let first_index = nest_index.min(merge_index);
    let second_index = nest_index.max(merge_index);
    Some(crate::agent_protocol::ReadFileRange {
        start_line: first_index.saturating_add(1).saturating_sub(8).max(1),
        end_line: second_index.saturating_add(36),
    })
}

pub(crate) fn suggest_source_patch_range_from_failure(
    owner_text: &str,
    failure_text: Option<&str>,
) -> Option<crate::agent_protocol::ReadFileRange> {
    let failure_text = failure_text.unwrap_or_default().to_ascii_lowercase();
    if let Some(range) = suggest_axum_fallback_merge_range(owner_text, &failure_text) {
        return Some(range);
    }
    let mut needles = Vec::new();
    if failure_text.contains("cannotborrowowneddata")
        || owner_text.contains("CannotBorrowOwnedData")
    {
        needles.push("CannotBorrowOwnedData");
    }
    if failure_text.contains("deserialize") {
        needles.push("deserialize");
    }
    if failure_text.contains("visitor") {
        needles.push("visit");
    }
    if failure_text.contains("durationexceedstimestamp")
        || owner_text.contains("DurationExceedsTimestamp")
    {
        needles.push("DurationExceedsTimestamp");
    }
    if needles.is_empty() && owner_text.contains("deserialize_str") {
        needles.push("deserialize_str");
    }
    if needles.is_empty() && owner_text.contains("deserialize_bytes") {
        needles.push("deserialize_bytes");
    }
    if needles.is_empty() {
        return None;
    }
    let lines = owner_text.lines().collect::<Vec<_>>();
    for needle in needles {
        if let Some(index) = lines.iter().position(|line| line.contains(needle)) {
            let center_line = index.saturating_add(1);
            return Some(crate::agent_protocol::ReadFileRange {
                start_line: center_line.saturating_sub(28).max(1),
                end_line: center_line.saturating_add(42),
            });
        }
    }
    None
}

pub(crate) fn benchmark_repair_phase_instruction(phase: BenchmarkRepairPhase) -> &'static str {
    match phase {
        BenchmarkRepairPhase::Idle => "",
        BenchmarkRepairPhase::NeedsFailureAnchorRead => "Read the suggested failing slice now.",
        BenchmarkRepairPhase::NeedsImplementationRead => {
            "You have the failing test slice. Read one implementation slice now."
        }
        BenchmarkRepairPhase::NeedsPatch => {
            "You already have the needed owner-file context. Patch now."
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun => "Patch captured. Rerun the fast loop now.",
    }
}

pub(crate) fn truncate_patch_packet_slice(content: &str) -> String {
    const MAX_LINES: usize = 72;
    const MAX_CHARS: usize = 2400;

    let mut rendered_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;
    for (index, line) in content.lines().enumerate() {
        if index >= MAX_LINES {
            truncated = true;
            break;
        }
        let additional_chars = line.len().saturating_add(1);
        if used_chars.saturating_add(additional_chars) > MAX_CHARS {
            truncated = true;
            break;
        }
        rendered_lines.push(line);
        used_chars = used_chars.saturating_add(additional_chars);
    }
    let mut rendered = rendered_lines.join("\n").trim().to_string();
    if truncated {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("... [truncated]");
    }
    rendered
}

pub(crate) fn extract_exact_range_from_text(
    owner_text: &str,
    range: crate::agent_protocol::ReadFileRange,
) -> Option<String> {
    let normalized = range.normalized()?;
    let lines = owner_text.lines().collect::<Vec<_>>();
    if normalized.end_line > lines.len() {
        return None;
    }
    let start_index = normalized.start_line.saturating_sub(1);
    let end_index = normalized.end_line.min(lines.len());
    if start_index >= end_index {
        return None;
    }
    Some(lines[start_index..end_index].join("\n"))
}

pub(crate) fn owner_slice_packet_content(repair_state: &BenchmarkRepairState) -> Option<String> {
    let last_owner_slice = repair_state.last_owner_slice.as_ref()?;
    if let Some(slice_content) = last_owner_slice.slice_content.as_ref() {
        return Some(slice_content.clone());
    }
    last_owner_slice.honored_range.and_then(|range| {
        repair_state
            .latest_owner_file_text
            .as_deref()
            .and_then(|text| extract_exact_range_from_text(text, range))
    })
}

pub(crate) fn target_slice_content_hash(
    repair_state: &BenchmarkRepairState,
    patch_target: &str,
) -> Option<String> {
    let last_owner_slice = repair_state.last_owner_slice.as_ref()?;
    if last_owner_slice.test_only
        || canonical_path(&last_owner_slice.path) != canonical_path(patch_target)
    {
        return None;
    }
    owner_slice_packet_content(repair_state).map(|content| stable_content_hash(&content))
}

pub(crate) fn target_content_hash_for_patch(
    repair_state: &BenchmarkRepairState,
    memory: &AgentRepairMemory,
    patch_target: &str,
) -> Option<String> {
    observed_full_file_content_hash(memory, patch_target)
        .or_else(|| target_slice_content_hash(repair_state, patch_target))
}

pub(crate) fn benchmark_repair_phase_suggested_range(
    repair_state: &BenchmarkRepairState,
) -> Option<crate::agent_protocol::ReadFileRange> {
    match repair_state.phase {
        BenchmarkRepairPhase::Idle => None,
        BenchmarkRepairPhase::NeedsFailureAnchorRead => repair_state.failure_anchor_range,
        BenchmarkRepairPhase::NeedsImplementationRead => repair_state
            .implementation_suggested_range
            .or(repair_state.failure_anchor_range),
        BenchmarkRepairPhase::NeedsPatch | BenchmarkRepairPhase::NeedsFastLoopRerun => repair_state
            .last_owner_slice
            .as_ref()
            .and_then(|slice| slice.honored_range)
            .or(repair_state.failure_anchor_range),
    }
}

pub(crate) fn benchmark_allowed_implementation_targets(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for path in ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
    {
        let canonical = canonical_path(path);
        if !canonical.trim().is_empty()
            && !is_obvious_test_file(&canonical)
            && seen.insert(canonical.clone())
        {
            targets.push(canonical);
        }
    }
    targets
}

pub(crate) fn benchmark_read_only_test_targets(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for path in ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
        .chain(ledger.companion_files_required.iter())
    {
        let canonical = canonical_path(path);
        if !canonical.trim().is_empty()
            && is_obvious_test_file(&canonical)
            && seen.insert(canonical.clone())
        {
            targets.push(canonical);
        }
    }
    targets
}

pub(crate) fn render_benchmark_target_list(targets: &[String]) -> String {
    if targets.is_empty() {
        return "[none]".to_string();
    }
    targets
        .iter()
        .map(|target| format!("`{target}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn render_ranked_implementation_targets(targets: &[AgentRepairImplementationTarget]) -> String {
    if targets.is_empty() {
        return "[none]".to_string();
    }
    targets
        .iter()
        .take(6)
        .map(|target| format!("#{} `{}` ({})", target.rank, target.path, target.reason))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn recommended_fast_loop_rerun_command(ledger: &BenchmarkCaseLedger) -> Option<String> {
    let canonical = ledger
        .fast_loop_commands
        .iter()
        .find(|command| !command.trim().is_empty())?
        .trim()
        .to_string();
    let failing_tests = fast_loop_selector_pool(ledger);
    if failing_tests.is_empty() {
        return Some(canonical);
    }
    let Some((mut base_tokens, selector_prefix)) = split_fast_loop_candidate(&canonical) else {
        return Some(canonical);
    };
    if base_tokens.is_empty() {
        return Some(canonical);
    }
    if selector_prefix.is_none()
        && base_tokens
            .last()
            .is_some_and(|token| token.as_str() == "--exact")
    {
        return Some(canonical);
    }
    if selector_prefix.is_none() && fast_loop_explicit_selector(&canonical).is_some() {
        return Some(canonical);
    }
    for failing_test in failing_tests {
        if let Some(prefix) = selector_prefix.as_deref() {
            if failing_test.starts_with(prefix) {
                base_tokens.push(failing_test.clone());
                continue;
            }
            base_tokens.push(format!("{prefix}{failing_test}"));
            continue;
        }
        base_tokens.push(failing_test.clone());
    }
    Some(base_tokens.join(" "))
}

pub(crate) fn implementation_signature_window(lines: &[&str], start_index: usize) -> String {
    let mut parts = Vec::new();
    for line in lines.iter().skip(start_index).take(8) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
        if trimmed.ends_with('{') || trimmed.ends_with(';') {
            break;
        }
    }
    parts.join(" ")
}

pub(crate) fn signature_matches_candidate(signature_lower: &str, candidate: &str) -> bool {
    let candidate = candidate.to_ascii_lowercase();
    [
        format!("fn {candidate}("),
        format!("fn {candidate}<"),
        format!("pub fn {candidate}("),
        format!("pub fn {candidate}<"),
        format!("pub(crate) fn {candidate}("),
        format!("pub(crate) fn {candidate}<"),
    ]
    .iter()
    .any(|pattern| signature_lower.contains(pattern))
}

pub(crate) fn implementation_signature_score(
    line: &str,
    signature_lower: &str,
    candidate: &str,
    index: usize,
) -> i32 {
    let trimmed = line.trim();
    let mut score = candidate.len() as i32 * 4;
    if signature_lower.ends_with('{') || signature_lower.contains("{ ") {
        score += 120;
    }
    if signature_lower.ends_with(';') {
        score -= 220;
    }
    if signature_lower.contains("(self") || signature_lower.contains(" self,") {
        score -= 80;
    }
    if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") {
        score += 25;
    }
    if !line.starts_with(char::is_whitespace) {
        score += 50;
    }
    score - (index as i32 / 8)
}

pub(crate) fn slice_is_test_only(content: &str, primary_failure_test_name: Option<&str>) -> bool {
    let lower = content.to_ascii_lowercase();
    let test_markers = ["#[test]", "assert_eq!", "assert!", "fn test_", "mod tests"]
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count();
    let contains_impl_signature = if let Some(test_name) = primary_failure_test_name {
        implementation_name_candidates(test_name)
            .iter()
            .any(|candidate| {
                lower.contains(&format!("fn {candidate}"))
                    || lower.contains(&format!("pub fn {candidate}"))
                    || lower.contains(&format!("pub(crate) fn {candidate}"))
            })
    } else {
        content.lines().any(|line| {
            let trimmed = line.trim_start();
            (trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn "))
                && !trimmed.contains("test_")
        })
    };
    test_markers > 0 && !contains_impl_signature
}

pub(crate) fn benchmark_repair_state_from_ledger(
    ledger: &BenchmarkCaseLedger,
) -> Option<BenchmarkRepairState> {
    let failure_reason = ledger.last_validation_failure.as_ref()?;
    let diagnostic_class = ledger.validation_details.diagnostic_class.as_deref();
    let implementation_lease = target_lease_for_ledger(ledger);
    let primary_failure_path = ledger.validation_details.primary_failure_path.clone();
    let source_lease_should_drive_repair = implementation_lease.as_deref().is_some_and(|path| {
        !is_obvious_test_file(path)
            && !matches!(
                diagnostic_class,
                Some("manifest_dependency_error" | "manifest_feature_error")
            )
            && primary_failure_path
                .as_deref()
                .is_some_and(is_obvious_test_file)
    });
    let owner_path = implementation_lease
        .clone()
        .filter(|_| source_lease_should_drive_repair)
        .or_else(|| primary_failure_path.clone())
        .or_else(|| ledger.owner_files.first().cloned())
        .or_else(|| ledger.expected_touch_targets.first().cloned())?;
    let repair_phase = if source_lease_should_drive_repair {
        BenchmarkRepairPhase::NeedsPatch
    } else {
        BenchmarkRepairPhase::NeedsFailureAnchorRead
    };
    let failure_anchor_range = if source_lease_should_drive_repair {
        None
    } else {
        ledger
            .validation_details
            .primary_failure_line
            .map(suggested_range_for_line)
            .or_else(|| suggested_reread_range_from_failure(&owner_path, failure_reason))
    };
    Some(BenchmarkRepairState {
        phase: repair_phase,
        owner_path,
        primary_failure_test_name: ledger.validation_details.primary_failure_test_name.clone(),
        failure_anchor_range,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    })
}

