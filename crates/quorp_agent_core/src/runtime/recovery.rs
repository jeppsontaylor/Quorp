//! Path-failure detection, benchmark-case helpers, canonical-action
//! machinery, and BenchmarkRepairState/RepairRequirement type
//! definitions.
//!
//! Carved out of runtime.rs so each child of `quorp_agent_core::runtime`
//! stays under the 2,000-LOC hard cap.

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
pub(crate) fn is_obvious_test_file(path: &str) -> bool {
    let normalized = canonical_path(path);
    normalized.contains("/tests/")
        || normalized.starts_with("tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".test.tsx")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".spec.tsx")
}

pub(crate) fn is_support_or_generated_runtime_path(path: &str) -> bool {
    let normalized = canonical_path(path);
    if normalized.is_empty() {
        return true;
    }
    if normalized.starts_with("target/")
        || normalized.starts_with(".git/")
        || normalized.starts_with(".quorp/")
        || normalized.starts_with(".warpos-capture-probe/")
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "start_here.md"
            | "success.md"
            | "reference.md"
            | "repro_note.md"
            | "runner_feedback.md"
            | "context_warning.md"
            | "benchmark.json"
            | "issue.json"
            | "evaluation.json"
            | "hidden-evaluation.json"
            | "visible-evaluation.json"
            | "collector-evaluation.json"
            | "benchmark-report.json"
            | "benchmark-report.md"
    )
}

pub(crate) fn metadata_string_list(metadata: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
}

pub(crate) fn metadata_bool(metadata: &serde_json::Value, key: &str) -> Option<bool> {
    metadata.get(key).and_then(serde_json::Value::as_bool)
}

pub(crate) fn default_verifier_drain_budget() -> usize {
    4
}

pub(crate) fn default_parser_recovery_budget() -> usize {
    2
}

pub(crate) fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn benchmark_case_ledger_from_metadata(
    metadata: &serde_json::Value,
) -> Option<BenchmarkCaseLedger> {
    let case_class = metadata_string(metadata, "benchmark_case_class").unwrap_or_default();
    let owner_files = metadata_string_list(metadata, "benchmark_owner_files").unwrap_or_default();
    let fast_loop_commands =
        metadata_string_list(metadata, "benchmark_fast_loop_commands").unwrap_or_default();
    let expected_touch_targets =
        metadata_string_list(metadata, "benchmark_expected_touch_targets").unwrap_or_default();
    let companion_files_required =
        metadata_string_list(metadata, "benchmark_companion_files_required").unwrap_or_default();
    let named_tests = metadata_string_list(metadata, "benchmark_named_tests").unwrap_or_default();
    if case_class.is_empty()
        && owner_files.is_empty()
        && fast_loop_commands.is_empty()
        && expected_touch_targets.is_empty()
        && companion_files_required.is_empty()
        && named_tests.is_empty()
    {
        return None;
    }
    Some(BenchmarkCaseLedger {
        case_class,
        owner_files,
        fast_loop_commands,
        expected_touch_targets,
        companion_files_required,
        named_tests,
        current_hypothesis: None,
        validation_status: None,
        last_validation_failure: None,
        validation_details: BenchmarkValidationDetails::default(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathResolutionFailure {
    pub(crate) request_path: String,
    pub(crate) suggested_path: Option<String>,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoverableInspectionFailure {
    pub(crate) action_summary: String,
    pub(crate) error: String,
    pub(crate) path_failure: Option<PathResolutionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkCaseLedger {
    pub(crate) case_class: String,
    pub(crate) owner_files: Vec<String>,
    pub(crate) fast_loop_commands: Vec<String>,
    pub(crate) expected_touch_targets: Vec<String>,
    pub(crate) companion_files_required: Vec<String>,
    pub(crate) named_tests: Vec<String>,
    pub(crate) current_hypothesis: Option<String>,
    pub(crate) validation_status: Option<String>,
    pub(crate) last_validation_failure: Option<String>,
    #[serde(default)]
    pub(crate) validation_details: BenchmarkValidationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkValidationDetails {
    #[serde(default)]
    pub(crate) failing_test_names: Vec<String>,
    #[serde(default)]
    pub(crate) primary_failure_test_name: Option<String>,
    #[serde(default)]
    pub(crate) primary_failure_path: Option<String>,
    #[serde(default)]
    pub(crate) primary_failure_line: Option<usize>,
    #[serde(default)]
    pub(crate) assertion_excerpt: Option<String>,
    #[serde(default)]
    pub(crate) diagnostic_class: Option<String>,
    #[serde(default)]
    pub(crate) implementation_target_lease: Option<String>,
    #[serde(default)]
    pub(crate) repair_required: bool,
    #[serde(default)]
    pub(crate) repair_phase_terminal: Option<String>,
    #[serde(default)]
    pub(crate) failure_anchor_reread_attempted: bool,
    #[serde(default)]
    pub(crate) failure_anchor_reread_honored: bool,
    #[serde(default)]
    pub(crate) implementation_reread_allowed: bool,
    #[serde(default)]
    pub(crate) implementation_reread_attempted: bool,
    #[serde(default)]
    pub(crate) implementation_reread_honored: bool,
    #[serde(default)]
    pub(crate) repair_phase_invalid_action_count: usize,
    #[serde(default)]
    pub(crate) post_fast_loop_patch_attempted: bool,
    #[serde(default)]
    pub(crate) post_fast_loop_validation_rerun_attempted: bool,
    #[serde(default)]
    pub(crate) patch_packet_injected: bool,
    #[serde(default)]
    pub(crate) patch_packet_honored_range: Option<String>,
    #[serde(default)]
    pub(crate) recommended_rerun_command: Option<String>,
    #[serde(default)]
    pub(crate) fast_loop_rerun_match_kind: Option<String>,
    #[serde(default)]
    pub(crate) failed_edit_records: Vec<FailedEditRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FastLoopMatchKind {
    ExactCanonical,
    SubsetFastLoop,
}

impl FastLoopMatchKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ExactCanonical => "exact_fast_loop",
            Self::SubsetFastLoop => "subset_fast_loop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRepairPhase {
    #[default]
    Idle,
    NeedsFailureAnchorRead,
    NeedsImplementationRead,
    NeedsPatch,
    NeedsFastLoopRerun,
}

impl BenchmarkRepairPhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::NeedsFailureAnchorRead => "needs_failure_anchor_read",
            Self::NeedsImplementationRead => "needs_implementation_read",
            Self::NeedsPatch => "needs_patch",
            Self::NeedsFastLoopRerun => "needs_fast_loop_rerun",
        }
    }

    pub(crate) fn state_label(self) -> &'static str {
        match self {
            Self::Idle => "needs_evidence",
            Self::NeedsFailureAnchorRead => "needs_focused_read",
            Self::NeedsImplementationRead => "known_failure",
            Self::NeedsPatch => "context_sufficient",
            Self::NeedsFastLoopRerun => "needs_validation",
        }
    }
}

pub(crate) fn canonical_action_record(
    step: usize,
    action: &AgentAction,
    ledger: Option<&BenchmarkCaseLedger>,
) -> AgentRepairCanonicalAction {
    let kind = match action {
        AgentAction::RunCommand { .. } => {
            if action_is_validation_like(action, ledger) {
                "RunValidation"
            } else {
                "RunCommand"
            }
        }
        AgentAction::ReadFile { .. } => "ReadFile",
        AgentAction::ListDirectory { .. } => "ListDirectory",
        AgentAction::SearchText { .. } => "SearchText",
        AgentAction::SearchSymbols { .. } => "SearchSymbols",
        AgentAction::FindFiles { .. } => "FindFiles",
        AgentAction::StructuralSearch { .. } => "StructuralSearch",
        AgentAction::StructuralEditPreview { .. } => "StructuralEditPreview",
        AgentAction::CargoDiagnostics { .. } => "CargoDiagnostics",
        AgentAction::GetRepoCapsule { .. } => "GetRepoCapsule",
        AgentAction::ExplainValidationFailure { .. } => "ExplainValidationFailure",
        AgentAction::SuggestImplementationTargets { .. } => "SuggestImplementationTargets",
        AgentAction::SuggestEditAnchors { .. } => "SuggestEditAnchors",
        AgentAction::PreviewEdit { .. } => "PreviewEdit",
        AgentAction::ReplaceRange { .. } => "ReplaceRange",
        AgentAction::ModifyToml { .. } => "ModifyToml",
        AgentAction::ApplyPreview { .. } => "ApplyPreview",
        AgentAction::WriteFile { .. } => "WriteFile",
        AgentAction::ApplyPatch { .. } => "ApplyPatch",
        AgentAction::ReplaceBlock { .. } => "ReplaceBlock",
        AgentAction::SetExecutable { .. } => "SetExecutable",
        AgentAction::McpCallTool { .. } => "McpCallTool",
        AgentAction::RunValidation { .. } => "RunValidation",
    }
    .to_string();
    AgentRepairCanonicalAction {
        step,
        kind,
        signature: canonical_action_signature(action, ledger),
        target_path: canonical_action_target_path(action),
        validation_like: action_is_validation_like(action, ledger),
    }
}

pub(crate) fn canonical_action_signature(
    action: &AgentAction,
    ledger: Option<&BenchmarkCaseLedger>,
) -> String {
    match action {
        AgentAction::RunCommand { command, .. } if action_is_validation_like(action, ledger) => {
            format!("validate:{}", canonical_shell(command))
        }
        AgentAction::RunCommand { command, .. } => format!("run:{}", canonical_shell(command)),
        AgentAction::ReadFile { path, range } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "all".to_string());
            format!("read:{}:{range}", canonical_path(path))
        }
        AgentAction::ListDirectory { path } => format!("ls:{}", canonical_path(path)),
        AgentAction::SearchText { query, .. } => {
            format!(
                "search_text:{}",
                query.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        }
        AgentAction::SearchSymbols { query, .. } => format!("search_symbols:{}", query.trim()),
        AgentAction::FindFiles { query, .. } => format!("find_files:{}", query.trim()),
        AgentAction::StructuralSearch {
            pattern,
            language,
            path,
            ..
        } => format!(
            "structural_search:{}:{}:{}",
            language.as_deref().unwrap_or("rust"),
            path.as_deref().unwrap_or("."),
            short_text_fingerprint(pattern)
        ),
        AgentAction::StructuralEditPreview {
            pattern,
            rewrite,
            language,
            path,
        } => format!(
            "structural_preview:{}:{}:{}:{}",
            language.as_deref().unwrap_or("rust"),
            path.as_deref().unwrap_or("."),
            short_text_fingerprint(pattern),
            short_text_fingerprint(rewrite)
        ),
        AgentAction::CargoDiagnostics {
            command,
            include_clippy,
        } => format!(
            "cargo_diagnostics:{}:{}",
            command.as_deref().unwrap_or("default"),
            include_clippy
        ),
        AgentAction::GetRepoCapsule { query, .. } => {
            format!("capsule:{}", query.as_deref().unwrap_or_default().trim())
        }
        AgentAction::ExplainValidationFailure { command, output } => {
            format!(
                "explain_validation:{}:{}",
                canonical_shell(command),
                short_text_fingerprint(output)
            )
        }
        AgentAction::SuggestImplementationTargets {
            command,
            output,
            failing_path,
            failing_line,
        } => {
            let location = failing_path.as_deref().unwrap_or("").trim();
            let line = failing_line
                .map(|value| value.to_string())
                .unwrap_or_default();
            format!(
                "target_suggestions:{}:{}:{}:{}",
                canonical_shell(command),
                short_text_fingerprint(output),
                canonical_path(location),
                line
            )
        }
        AgentAction::SuggestEditAnchors {
            path,
            range,
            search_hint,
        } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "all".to_string());
            format!(
                "anchors:{}:{range}:{}",
                canonical_path(path),
                search_hint.as_deref().unwrap_or_default().trim()
            )
        }
        AgentAction::PreviewEdit { path, edit } => {
            format!(
                "preview:{}:{}",
                canonical_path(path),
                short_text_fingerprint(&format!("{edit:?}"))
            )
        }
        AgentAction::ReplaceRange {
            path,
            range,
            expected_hash,
            replacement,
        } => {
            format!(
                "replace_range:{}:{}:{}:{}",
                canonical_path(path),
                range.label(),
                expected_hash.trim(),
                short_text_fingerprint(replacement)
            )
        }
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => {
            format!(
                "modify_toml:{}:{}:{}",
                canonical_path(path),
                expected_hash.trim(),
                short_text_fingerprint(&format!("{operations:?}"))
            )
        }
        AgentAction::ApplyPreview { preview_id } => {
            format!("apply_preview:{}", preview_id.trim())
        }
        AgentAction::WriteFile { path, content } => {
            format!(
                "write:{}:{}",
                canonical_path(path),
                short_text_fingerprint(content)
            )
        }
        AgentAction::ApplyPatch { path, patch } => {
            format!(
                "patch:{}:{}",
                canonical_path(path),
                short_text_fingerprint(patch)
            )
        }
        AgentAction::ReplaceBlock {
            path,
            search_block,
            replace_block,
            range,
        } => {
            let range = range
                .and_then(crate::agent_protocol::ReadFileRange::normalized)
                .map(|range| range.label())
                .unwrap_or_else(|| "bare".to_string());
            format!(
                "replace:{}:{range}:{}:{}",
                canonical_path(path),
                short_text_fingerprint(search_block),
                short_text_fingerprint(replace_block)
            )
        }
        AgentAction::SetExecutable { path } => format!("chmod:{}", canonical_path(path)),
        AgentAction::McpCallTool {
            server_name,
            tool_name,
            arguments,
        } => format!(
            "mcp:{server_name}:{tool_name}:{}",
            short_text_fingerprint(&arguments.to_string())
        ),
        AgentAction::RunValidation { plan } => format!("validate:{}", plan.summary()),
    }
}

pub(crate) fn canonical_shell(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn command_looks_like_vague_fast_loop_request(command: &str) -> bool {
    let normalized = command
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized == "fast loop"
        || normalized == "the fast loop"
        || normalized == "run fast loop"
        || normalized == "run the fast loop"
        || normalized.contains("fast-loop")
        || normalized.contains("fast loop")
}

pub(crate) fn canonical_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn canonical_action_target_path(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::ReadFile { path, .. }
        | AgentAction::ListDirectory { path }
        | AgentAction::SuggestEditAnchors { path, .. }
        | AgentAction::PreviewEdit { path, .. }
        | AgentAction::ReplaceRange { path, .. }
        | AgentAction::ModifyToml { path, .. }
        | AgentAction::WriteFile { path, .. }
        | AgentAction::ApplyPatch { path, .. }
        | AgentAction::ReplaceBlock { path, .. }
        | AgentAction::SetExecutable { path } => Some(canonical_path(path)),
        _ => None,
    }
}

pub(crate) fn action_is_validation_like(action: &AgentAction, ledger: Option<&BenchmarkCaseLedger>) -> bool {
    match action {
        AgentAction::RunValidation { .. } => true,
        AgentAction::RunCommand { command, .. } => {
            ledger.is_some_and(|ledger| fast_loop_match_kind(ledger, command).is_some())
                || command.contains("cargo test")
                || command.contains("pytest")
                || command.contains("npm test")
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerSliceKind {
    FailureAnchor,
    ImplementationAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct OwnerSliceRecord {
    pub(crate) path: String,
    pub(crate) requested_range: Option<crate::agent_protocol::ReadFileRange>,
    pub(crate) honored_range: Option<crate::agent_protocol::ReadFileRange>,
    pub(crate) kind: OwnerSliceKind,
    pub(crate) test_only: bool,
    pub(crate) slice_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
pub struct BenchmarkRepairState {
    #[serde(default)]
    pub(crate) phase: BenchmarkRepairPhase,
    #[serde(default)]
    pub(crate) owner_path: String,
    #[serde(default)]
    pub(crate) primary_failure_test_name: Option<String>,
    #[serde(default)]
    pub(crate) failure_anchor_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    pub(crate) implementation_suggested_range: Option<crate::agent_protocol::ReadFileRange>,
    #[serde(default)]
    pub(crate) last_owner_slice: Option<OwnerSliceRecord>,
    #[serde(default)]
    pub(crate) latest_owner_file_text: Option<String>,
    #[serde(default)]
    pub(crate) failure_anchor_reread_attempted: bool,
    #[serde(default)]
    pub(crate) failure_anchor_reread_honored: bool,
    #[serde(default)]
    pub(crate) implementation_reread_allowed: bool,
    #[serde(default)]
    pub(crate) implementation_reread_attempted: bool,
    #[serde(default)]
    pub(crate) implementation_reread_honored: bool,
    #[serde(default)]
    pub(crate) invalid_action_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RepairRequirement {
    pub(crate) path: String,
    pub(crate) failure_reason: String,
    pub(crate) previous_search_block: Option<String>,
    pub(crate) suggested_range: Option<crate::agent_protocol::ReadFileRange>,
    pub(crate) exact_reread_completed: bool,
}

pub(crate) fn reread_satisfies_requirement(
    requirement: &RepairRequirement,
    requested_range: Option<crate::agent_protocol::ReadFileRange>,
    honored_range: Option<crate::agent_protocol::ReadFileRange>,
) -> bool {
    if AgentTaskState::repair_requirement_prefers_full_file(requirement) {
        return requested_range
            .and_then(|value| value.normalized())
            .is_none()
            && honored_range.and_then(|value| value.normalized()).is_none();
    }
    let Some(honored_range) = honored_range.and_then(|value| value.normalized()) else {
        return false;
    };
    match requirement
        .suggested_range
        .and_then(|value| value.normalized())
    {
        Some(suggested_range) => {
            honored_range.start_line <= suggested_range.end_line
                && suggested_range.start_line <= honored_range.end_line
        }
        None => true,
    }
}

pub(crate) fn read_range_span(range: crate::agent_protocol::ReadFileRange) -> usize {
    range
        .end_line
        .saturating_sub(range.start_line)
        .saturating_add(1)
}

pub(crate) fn read_range_overlap(
    left: crate::agent_protocol::ReadFileRange,
    right: crate::agent_protocol::ReadFileRange,
) -> usize {
    let start = left.start_line.max(right.start_line);
    let end = left.end_line.min(right.end_line);
    if start > end {
        0
    } else {
        end.saturating_sub(start).saturating_add(1)
    }
}

pub(crate) fn range_meaningfully_differs_from_anchor(
    requested_range: crate::agent_protocol::ReadFileRange,
    anchor_range: crate::agent_protocol::ReadFileRange,
) -> bool {
    if read_range_span(requested_range) > 128 {
        return false;
    }
    let overlap = read_range_overlap(requested_range, anchor_range);
    let shorter_span = read_range_span(requested_range).min(read_range_span(anchor_range));
    overlap.saturating_mul(2) < shorter_span
}

pub(crate) fn ranges_substantially_overlap(
    left: crate::agent_protocol::ReadFileRange,
    right: crate::agent_protocol::ReadFileRange,
) -> bool {
    let overlap = read_range_overlap(left, right);
    let shorter_span = read_range_span(left).min(read_range_span(right));
    shorter_span > 0 && overlap.saturating_mul(5) >= shorter_span.saturating_mul(4)
}

pub(crate) fn push_capped<T>(items: &mut Vec<T>, item: T, cap: usize) {
    items.push(item);
    if items.len() > cap {
        let overflow = items.len().saturating_sub(cap);
        items.drain(0..overflow);
    }
}

pub(crate) fn ranked_implementation_targets_for_ledger(
    ledger: &BenchmarkCaseLedger,
) -> Vec<AgentRepairImplementationTarget> {
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let diagnostic_class = ledger.validation_details.diagnostic_class.as_deref();
    let source_diagnostic = matches!(
        diagnostic_class,
        Some("rust_compile_error" | "test_failure")
    );
    if matches!(
        diagnostic_class,
        Some("manifest_dependency_error" | "manifest_feature_error")
    ) {
        for path in &ledger.expected_touch_targets {
            if path.ends_with("Cargo.toml") && seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: diagnostic_class
                        .unwrap_or("manifest_dependency_error")
                        .to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if source_diagnostic {
        push_ranked_owner_targets(ledger, &mut targets, &mut seen);
    }
    for path in &ledger.expected_touch_targets {
        if source_diagnostic
            && (path.ends_with("Cargo.toml") || benchmark_support_surface_path(path))
        {
            continue;
        }
        if seen.insert(canonical_path(path)) {
            targets.push(AgentRepairImplementationTarget {
                path: path.clone(),
                reason: "expected_touch_target".to_string(),
                rank: targets.len() + 1,
            });
        }
    }
    if !source_diagnostic {
        push_ranked_owner_targets(ledger, &mut targets, &mut seen);
    }
    if source_diagnostic {
        for path in ledger
            .expected_touch_targets
            .iter()
            .filter(|path| benchmark_support_surface_path(path))
        {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "support_surface".to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if source_diagnostic {
        for path in ledger
            .expected_touch_targets
            .iter()
            .filter(|path| path.ends_with("Cargo.toml"))
        {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "manifest_support".to_string(),
                    rank: targets.len() + 1,
                });
            }
        }
    }
    if let Some(path) = ledger.validation_details.primary_failure_path.as_ref()
        && seen.insert(canonical_path(path))
    {
        let reason = if is_obvious_test_file(path) {
            "test_evidence_only"
        } else {
            "diagnostic_anchor"
        };
        targets.push(AgentRepairImplementationTarget {
            path: path.clone(),
            reason: reason.to_string(),
            rank: targets.len() + 1,
        });
    }
    targets
}

pub(crate) fn push_ranked_owner_targets(
    ledger: &BenchmarkCaseLedger,
    targets: &mut Vec<AgentRepairImplementationTarget>,
    seen: &mut BTreeSet<String>,
) {
    for path in &ledger.owner_files {
        if is_obvious_test_file(path) {
            if seen.insert(canonical_path(path)) {
                targets.push(AgentRepairImplementationTarget {
                    path: path.clone(),
                    reason: "test_evidence_only".to_string(),
                    rank: targets.len() + 1,
                });
            }
            continue;
        }
        if seen.insert(canonical_path(path)) {
            targets.push(AgentRepairImplementationTarget {
                path: path.clone(),
                reason: "owner_file".to_string(),
                rank: targets.len() + 1,
            });
        }
    }
}

pub(crate) fn benchmark_support_surface_path(path: &str) -> bool {
    let canonical = canonical_path(path);
    canonical.ends_with(".md") || canonical.contains("changelog")
}

pub(crate) fn target_lease_for_ledger(ledger: &BenchmarkCaseLedger) -> Option<String> {
    ranked_implementation_targets_for_ledger(ledger)
        .into_iter()
        .find(|target| target.reason != "test_evidence_only")
        .map(|target| target.path)
}

pub(crate) fn benchmark_repair_target_path<'a>(
    repair_state: &'a BenchmarkRepairState,
    ledger: &'a BenchmarkCaseLedger,
) -> &'a str {
    if repair_state.owner_path.trim().is_empty() {
        ledger
            .validation_details
            .primary_failure_path
            .as_deref()
            .or_else(|| ledger.owner_files.first().map(String::as_str))
            .unwrap_or("[owner file]")
    } else {
        repair_state.owner_path.as_str()
    }
}

pub(crate) fn benchmark_target_lease_path<'a>(
    ledger: &'a BenchmarkCaseLedger,
    memory: &'a AgentRepairMemory,
) -> Option<Cow<'a, str>> {
    memory
        .implementation_target_lease
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(Cow::Borrowed)
        .or_else(|| {
            ledger
                .validation_details
                .implementation_target_lease
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(Cow::Borrowed)
        })
        .or_else(|| target_lease_for_ledger(ledger).map(Cow::Owned))
}

pub(crate) fn benchmark_patch_target_path<'a>(
    repair_state: &'a BenchmarkRepairState,
    ledger: &'a BenchmarkCaseLedger,
    memory: &'a AgentRepairMemory,
) -> Cow<'a, str> {
    benchmark_target_lease_path(ledger, memory)
        .unwrap_or_else(|| Cow::Borrowed(benchmark_repair_target_path(repair_state, ledger)))
}

pub(crate) fn benchmark_dependency_candidates(ledger: &BenchmarkCaseLedger) -> Vec<String> {
    let mut names = BTreeSet::new();
    if let Some(assertion_excerpt) = ledger.validation_details.assertion_excerpt.as_deref() {
        for name in extract_unresolved_import_names(assertion_excerpt) {
            names.insert(name);
        }
        for name in extract_manifest_feature_dependency_names(assertion_excerpt) {
            names.insert(name);
        }
    }
    if let Some(last_validation_failure) = ledger.last_validation_failure.as_deref() {
        for name in extract_unresolved_import_names(last_validation_failure) {
            names.insert(name);
        }
        for name in extract_manifest_feature_dependency_names(last_validation_failure) {
            names.insert(name);
        }
    }
    names.into_iter().collect()
}

pub(crate) fn benchmark_is_case_06_manifest_repair(ledger: &BenchmarkCaseLedger) -> bool {
    ledger
        .expected_touch_targets
        .iter()
        .any(|path| canonical_path(path) == "src/features/serde/de_owned.rs")
        && ledger
            .expected_touch_targets
            .iter()
            .any(|path| canonical_path(path).eq_ignore_ascii_case("Cargo.toml"))
        && ledger
            .owner_files
            .iter()
            .any(|path| canonical_path(path) == "tests/issues/issue_474.rs")
}

pub(crate) fn benchmark_manifest_dependency_versions(
    ledger: &BenchmarkCaseLedger,
) -> Option<Vec<(&'static str, &'static str)>> {
    if !benchmark_is_case_06_manifest_repair(ledger) {
        return None;
    }
    Some(vec![("chrono", "0.4"), ("uuid", "0.8")])
}

pub(crate) fn benchmark_manifest_patch_operations(
    ledger: &BenchmarkCaseLedger,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
) -> Vec<crate::agent_protocol::TomlEditOperation> {
    let Some(version_map) = benchmark_manifest_dependency_versions(ledger) else {
        return dependency_candidates
            .iter()
            .map(
                |name| crate::agent_protocol::TomlEditOperation::SetDependency {
                    table: target_dependency_table
                        .unwrap_or("dependencies")
                        .to_string(),
                    name: name.clone(),
                    version: Some("<version>".to_string()),
                    features: Vec::new(),
                    default_features: None,
                    optional: None,
                    package: None,
                    path: None,
                },
            )
            .collect();
    };
    let candidate_set = dependency_candidates
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    version_map
        .into_iter()
        .filter(|(name, _)| {
            candidate_set.is_empty() || candidate_set.contains(&name.to_ascii_lowercase())
        })
        .map(
            |(name, version)| crate::agent_protocol::TomlEditOperation::SetDependency {
                table: target_dependency_table
                    .unwrap_or("dependencies")
                    .to_string(),
                name: name.to_string(),
                version: Some(version.to_string()),
                features: vec!["serde".to_string()],
                default_features: None,
                optional: None,
                package: None,
                path: None,
            },
        )
        .collect()
}

pub(crate) fn benchmark_target_dependency_table(
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
    patch_target: &str,
) -> Option<&'static str> {
    if !patch_target.trim().ends_with(".toml") {
        return None;
    }
    let test_scoped = repair_state
        .last_owner_slice
        .as_ref()
        .is_some_and(|slice| slice.test_only)
        || is_obvious_test_file(&repair_state.owner_path)
        || ledger
            .validation_details
            .primary_failure_path
            .as_deref()
            .is_some_and(is_obvious_test_file);
    Some(if test_scoped {
        "dev-dependencies"
    } else {
        "dependencies"
    })
}

pub(crate) fn benchmark_patch_phase_write_locked(
    repair_state: &BenchmarkRepairState,
    ledger: &BenchmarkCaseLedger,
    memory: &AgentRepairMemory,
    requirement: Option<&RepairRequirement>,
) -> bool {
    if repair_state.phase != BenchmarkRepairPhase::NeedsPatch {
        return false;
    }
    if requirement.is_some_and(|requirement| !requirement.exact_reread_completed) {
        return false;
    }
    let patch_target = benchmark_patch_target_path(repair_state, ledger, memory);
    patch_target_context_loaded(repair_state, memory, patch_target.as_ref())
}

pub(crate) fn benchmark_write_phase_refusal(action: &AgentAction, patch_target: &str) -> bool {
    matches!(
        action,
        AgentAction::ReadFile { .. }
            | AgentAction::ListDirectory { .. }
            | AgentAction::SearchText { .. }
            | AgentAction::SearchSymbols { .. }
            | AgentAction::GetRepoCapsule { .. }
            | AgentAction::ExplainValidationFailure { .. }
            | AgentAction::SuggestImplementationTargets { .. }
            | AgentAction::SuggestEditAnchors { .. }
    ) || matches!(
        action,
        AgentAction::PreviewEdit { path, .. }
            | AgentAction::ReplaceRange { path, .. }
            | AgentAction::ModifyToml { path, .. }
            | AgentAction::WriteFile { path, .. }
            | AgentAction::ApplyPatch { path, .. }
            | AgentAction::ReplaceBlock { path, .. }
            | AgentAction::SetExecutable { path }
            if canonical_path(path) != canonical_path(patch_target)
    )
}

pub(crate) fn patch_target_context_loaded(
    repair_state: &BenchmarkRepairState,
    memory: &AgentRepairMemory,
    patch_target: &str,
) -> bool {
    let patch_target = canonical_path(patch_target);
    if repair_state.last_owner_slice.as_ref().is_some_and(|slice| {
        canonical_path(&slice.path) == patch_target
            && !slice.test_only
            && owner_slice_materially_loads_patch_target(slice, &patch_target)
    }) {
        return true;
    }
    if !patch_target.ends_with(".toml") {
        return false;
    }
    memory
        .observed_slices
        .iter()
        .any(|slice| canonical_path(&slice.path) == patch_target)
}

pub(crate) fn owner_slice_materially_loads_patch_target(slice: &OwnerSliceRecord, patch_target: &str) -> bool {
    if patch_target.ends_with(".toml") {
        return true;
    }
    if slice.honored_range.is_some() {
        return true;
    }
    slice.slice_content.as_deref().is_some_and(|content| {
        let trimmed = content.trim_start();
        !trimmed.starts_with("[excerpt lines")
            && !trimmed.contains("... [middle lines omitted] ...")
            && !trimmed.contains("... [truncated]")
    })
}

pub(crate) fn benchmark_required_action_label(
    repair_state: Option<&BenchmarkRepairState>,
    ledger: Option<&BenchmarkCaseLedger>,
    memory: &AgentRepairMemory,
) -> Option<String> {
    let repair_state = repair_state?;
    match repair_state.phase {
        BenchmarkRepairPhase::NeedsFailureAnchorRead => {
            let range = benchmark_repair_phase_suggested_range(repair_state)?;
            Some(format!(
                "read_file {} lines {}",
                repair_state.owner_path,
                range.label()
            ))
        }
        BenchmarkRepairPhase::NeedsImplementationRead => {
            let target = ledger
                .map(|ledger| benchmark_patch_target_path(repair_state, ledger, memory))
                .unwrap_or_else(|| Cow::Borrowed(repair_state.owner_path.as_str()));
            if let Some(range) = benchmark_repair_phase_suggested_range(repair_state) {
                Some(format!("read_file {} lines {}", target, range.label()))
            } else {
                Some(format!("read_file {}", target))
            }
        }
        BenchmarkRepairPhase::NeedsPatch => {
            let target = ledger
                .map(|ledger| benchmark_patch_target_path(repair_state, ledger, memory))
                .unwrap_or_else(|| Cow::Borrowed(repair_state.owner_path.as_str()));
            let target_table = ledger.and_then(|ledger| {
                benchmark_target_dependency_table(repair_state, ledger, target.as_ref())
            });
            if preview_apply_locked(memory) {
                return Some(format!(
                    "apply_preview {}",
                    memory
                        .last_preview_id
                        .as_deref()
                        .unwrap_or("preview_id_from_last_preview")
                ));
            }
            if patch_phase_scaffold_available(memory)
                && !patch_target_context_loaded(repair_state, memory, target.as_ref())
            {
                Some(format!("patch_scaffold {}", target))
            } else if target.as_ref().ends_with(".toml") {
                let dependency_candidates = ledger
                    .map(benchmark_dependency_candidates)
                    .unwrap_or_default();
                let manifest_operations = ledger
                    .map(|ledger| {
                        benchmark_manifest_patch_operations(
                            ledger,
                            target_table,
                            &dependency_candidates,
                        )
                    })
                    .unwrap_or_default();
                let operations = render_toml_edit_operations_brief(&manifest_operations);
                if operations.is_empty() {
                    Some(format!(
                        "preview_edit modify_toml {} [{}]",
                        target,
                        target_table.unwrap_or("dependencies")
                    ))
                } else {
                    Some(format!(
                        "preview_edit modify_toml {} [{}] {}",
                        target,
                        target_table.unwrap_or("dependencies"),
                        operations
                    ))
                }
            } else {
                Some(format!("write_patch {}", target))
            }
        }
        BenchmarkRepairPhase::NeedsFastLoopRerun => ledger
            .and_then(recommended_fast_loop_rerun_command)
            .map(|command| format!("run_fast_loop {command}")),
        BenchmarkRepairPhase::Idle => None,
    }
}

pub(crate) fn repair_requirement_action_label(requirement: Option<&RepairRequirement>) -> Option<String> {
    let requirement = requirement?;
    if requirement.exact_reread_completed {
        return None;
    }
    if AgentTaskState::repair_requirement_prefers_full_file(requirement) {
        Some(format!("read_file {}", requirement.path))
    } else {
        requirement
            .suggested_range
            .map(|range| format!("read_file {} lines {}", requirement.path, range.label()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchOutcome {
    Success,
    RecoverableInspectionFailure(RecoverableInspectionFailure),
    Failure,
}

pub async fn run_agent_task(
    request: &AgentRunRequest,
    completion_client: &dyn CompletionClient,
    tool_executor: &dyn ToolExecutor,
    event_sink: &dyn RuntimeEventSink,
    resume_checkpoint: Option<AgentCheckpoint>,
) -> AgentRunOutcome {
    let started_at = Instant::now();
    let config = load_agent_config(request.project_root.as_path());
    let mut state = AgentTaskState::new(request, config);
    let mut transcript = request.initial_context.clone();
    let mut current_iteration = 0usize;
    let mut request_counter = 1u64;
    let mut verifier_drain_used = 0usize;
    let mut verifier_drain_started = false;

    if let Some(mut checkpoint) = resume_checkpoint {
        state.restore(checkpoint.snapshot);
        transcript = std::mem::take(&mut checkpoint.transcript);
        current_iteration = checkpoint.step;
        request_counter = checkpoint.request_counter;
    } else {
        event_sink.emit(RuntimeEvent::RunStarted {
            goal: request.goal.clone(),
            model_id: request.model_id.clone(),
        });
    }

    loop {
        if let Some(flag) = request.cancellation_flag.as_ref()
            && flag.load(Ordering::Relaxed)
        {
            return finish_run(
                event_sink,
                StopReason::Cancelled,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                None,
            );
        }
        if let Some(max_seconds) = request.max_seconds
            && started_at.elapsed().as_secs() >= max_seconds
        {
            return finish_run(
                event_sink,
                StopReason::TimeBudgetExhausted,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                None,
            );
        }
        if let Some(action) = state.next_validation_action() {
            let draining_after_model_budget = current_iteration >= request.max_iterations;
            if draining_after_model_budget {
                if verifier_drain_used >= request.verifier_drain_budget {
                    let queued_validations = state.queued_validation_summaries();
                    event_sink.emit(RuntimeEvent::PendingValidationBlocked {
                        step: current_iteration,
                        queued_validations,
                        drain_budget: request.verifier_drain_budget,
                    });
                    return finish_run(
                        event_sink,
                        StopReason::PendingValidation,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some(
                            "Queued validation remained pending after the verifier drain budget was exhausted."
                                .to_string(),
                        ),
                    );
                }
                if !verifier_drain_started {
                    event_sink.emit(RuntimeEvent::VerifierDrainStarted {
                        step: current_iteration,
                        plans: state.queued_validation_summaries(),
                        budget: request.verifier_drain_budget,
                    });
                    verifier_drain_started = true;
                }
                verifier_drain_used += 1;
            }
            event_sink.emit(RuntimeEvent::PhaseChanged {
                phase: "verifying",
                detail: Some(action.summary()),
            });
            match dispatch_action(
                current_iteration + 1,
                &mut state,
                action,
                request,
                tool_executor,
                event_sink,
                &mut transcript,
            )
            .await
            {
                Ok(_) => {
                    current_iteration += 1;
                    event_sink.emit(RuntimeEvent::TurnCompleted {
                        transcript: transcript.clone(),
                    });
                    event_sink.emit(RuntimeEvent::CheckpointSaved {
                        checkpoint: AgentCheckpoint {
                            snapshot: state.snapshot(),
                            transcript: transcript.clone(),
                            step: current_iteration,
                            request_counter,
                        },
                    });
                    if verifier_drain_started && state.validation_queue.is_empty() {
                        event_sink.emit(RuntimeEvent::VerifierDrainFinished {
                            step: current_iteration,
                            remaining: 0,
                            verified_green: state.verified_green,
                        });
                    }
                    if state.verified_green && state.validation_queue.is_empty() {
                        event_sink.emit(RuntimeEvent::StatusUpdate {
                            status: AgentRuntimeStatus::Success,
                        });
                        event_sink.emit(RuntimeEvent::PhaseChanged {
                            phase: "success",
                            detail: None,
                        });
                        return finish_run(
                            event_sink,
                            StopReason::Success,
                            current_iteration,
                            state.total_billed_tokens,
                            started_at,
                            transcript,
                            None,
                        );
                    }
                    continue;
                }
                Err(error) => {
                    return fail_and_finish(
                        event_sink,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        error,
                        StopReason::FatalError,
                    );
                }
            }
        }

        if current_iteration >= request.max_iterations {
            return finish_run(
                event_sink,
                StopReason::MaxIterations,
                current_iteration,
                state.total_billed_tokens,
                started_at,
                transcript,
                Some("Max iterations reached before the agent could finish safely.".to_string()),
            );
        }

        event_sink.emit(RuntimeEvent::StatusUpdate {
            status: AgentRuntimeStatus::Thinking,
        });
        event_sink.emit(RuntimeEvent::PhaseChanged {
            phase: "thinking",
            detail: None,
        });
        let mut request_messages = transcript.clone();
        request_messages.push(TranscriptMessage {
            role: TranscriptRole::User,
            content: state.runtime_summary(),
        });
        let completion_request = CompletionRequest {
            request_id: request_counter,
            session_id: request.session_id,
            model_id: request.model_id.clone(),
            agent_mode: state.current_mode,
            latest_input: request.goal.clone(),
            messages: request_messages,
            project_root: request.project_root.clone(),
            base_url_override: request.base_url_override.clone(),
            max_completion_tokens: max_completion_tokens_for_turn(
                &request.completion_policy,
                current_iteration,
                &request.model_id,
                &state,
            ),
            include_repo_capsule: request.completion_policy.include_repo_capsule,
            disable_reasoning: request.completion_policy.disable_reasoning,
            native_tool_calls: request.completion_policy.native_tool_calls,
            watchdog: request.completion_policy.watchdog.clone(),
            safety_mode_label: request.completion_policy.safety_mode_label.clone(),
            prompt_compaction_policy: prompt_compaction_policy_for_turn(
                &request.completion_policy,
                &request.model_id,
                &state,
            ),
            capture_scope: metadata_string(&request.run_metadata, "warpos_capture_scope"),
            capture_call_class: metadata_string(&request.run_metadata, "warpos_capture_call_class"),
        };
        event_sink.emit(RuntimeEvent::ModelRequestStarted {
            step: current_iteration + 1,
            request_id: request_counter,
            message_count: completion_request.messages.len(),
            prompt_token_estimate: estimate_message_tokens(&completion_request.messages),
            completion_token_cap: completion_request.max_completion_tokens,
            safety_mode: completion_request.safety_mode_label.clone(),
        });
        let completion = match completion_client
            .request_completion(&completion_request)
            .await
        {
            Ok(completion) => completion,
            Err(error) => {
                let stop_reason = classify_completion_error_stop_reason(&error);
                return fail_and_finish(
                    event_sink,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    error,
                    stop_reason,
                );
            }
        };
        if let Some(usage) = completion.usage.as_ref() {
            state.total_billed_tokens = state
                .total_billed_tokens
                .saturating_add(usage.total_billed_tokens);
        }
        let output_truncated = completion_response_was_truncated(&completion);
        event_sink.emit(RuntimeEvent::ModelRequestFinished {
            step: current_iteration + 1,
            request_id: request_counter,
            usage: completion.usage,
            watchdog: completion.watchdog,
        });
        request_counter += 1;
        let budget_exhausted_after_turn = request
            .max_total_tokens
            .is_some_and(|max_total_tokens| state.total_billed_tokens >= max_total_tokens);

        match handle_model_turn(
            current_iteration + 1,
            ModelTurnInput {
                content: &completion.content,
                native_turn: completion.native_turn.as_ref(),
                native_turn_error: completion.native_turn_error.as_deref(),
                output_truncated,
            },
            &mut state,
            request,
            tool_executor,
            event_sink,
            &mut transcript,
        )
        .await
        {
            Ok(ControlFlow::Continue) => {
                current_iteration += 1;
                event_sink.emit(RuntimeEvent::TurnCompleted {
                    transcript: transcript.clone(),
                });
                event_sink.emit(RuntimeEvent::CheckpointSaved {
                    checkpoint: AgentCheckpoint {
                        snapshot: state.snapshot(),
                        transcript: transcript.clone(),
                        step: current_iteration,
                        request_counter,
                    },
                });
                if budget_exhausted_after_turn {
                    return finish_run(
                        event_sink,
                        StopReason::BudgetExhausted,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some("The configured token budget was exhausted after completing the current turn.".to_string()),
                    );
                }
            }
            Ok(ControlFlow::ContinueNoBudget) => {
                event_sink.emit(RuntimeEvent::TurnCompleted {
                    transcript: transcript.clone(),
                });
                event_sink.emit(RuntimeEvent::CheckpointSaved {
                    checkpoint: AgentCheckpoint {
                        snapshot: state.snapshot(),
                        transcript: transcript.clone(),
                        step: current_iteration,
                        request_counter,
                    },
                });
                if budget_exhausted_after_turn {
                    return finish_run(
                        event_sink,
                        StopReason::BudgetExhausted,
                        current_iteration,
                        state.total_billed_tokens,
                        started_at,
                        transcript,
                        Some("The configured token budget was exhausted after completing the current turn.".to_string()),
                    );
                }
            }
            Ok(ControlFlow::BreakSuccess) => {
                event_sink.emit(RuntimeEvent::StatusUpdate {
                    status: AgentRuntimeStatus::Success,
                });
                return finish_run(
                    event_sink,
                    StopReason::Success,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    None,
                );
            }
            Ok(ControlFlow::BreakCancelled) => {
                return finish_run(
                    event_sink,
                    StopReason::Cancelled,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    None,
                );
            }
            Err(error) => {
                return fail_and_finish(
                    event_sink,
                    current_iteration,
                    state.total_billed_tokens,
                    started_at,
                    transcript,
                    error,
                    StopReason::FatalError,
                );
            }
        }
    }
}

