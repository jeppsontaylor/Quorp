//! Parser-recovery message renderers, scaffold examples, ControlFlow
//! enum, and ModelTurnInput type. Used by the model turn dispatcher
//! in `runtime/turn.rs`.

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
pub(crate) fn completion_response_was_truncated(completion: &CompletionResponse) -> bool {
    if completion
        .usage
        .as_ref()
        .and_then(|usage| usage.finish_reason.as_deref())
        == Some("length")
    {
        return true;
    }
    completion
        .raw_provider_response
        .as_ref()
        .and_then(|value| value.get("choices"))
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(serde_json::Value::as_str)
        == Some("length")
}

pub(crate) fn is_recoverable_structured_parse_error(error: &str) -> bool {
    error.contains("EOF while parsing")
        || error.contains("Structured agent turn was invalid JSON")
        || error.contains("control character")
        || error.contains("expected `,` or `}`")
        || error.contains("key must be a string")
        || error.contains("expected value")
        || error.contains("trailing characters")
        || error.contains("Structured agent turn `actions` field was invalid")
        || error.contains("unsupported native tool call `")
        || (error.contains("native tool `") && error.contains("was missing `"))
        || (error.contains("native tool `") && error.contains("had invalid `"))
        || (error.contains("native tool `") && error.contains("arguments were invalid JSON"))
        || (error.contains("native tool `") && error.contains("arguments must be JSON objects"))
}

pub(crate) fn structured_parse_error_class(output_truncated: bool, error: &str) -> &'static str {
    if output_truncated {
        "output_truncated"
    } else if error.contains("unsupported native tool call `") {
        "unsupported_native_tool"
    } else if error.contains("native tool `")
        && (error.contains("was missing `")
            || error.contains("had invalid `")
            || error.contains("arguments were invalid JSON")
            || error.contains("arguments must be JSON objects"))
    {
        "malformed_action"
    } else if error.contains("trailing characters") {
        "trailing_characters"
    } else {
        "malformed"
    }
}

pub(crate) fn parser_recovery_message(output_truncated: bool, error: &str) -> String {
    if output_truncated {
        "[Parser]\nParse error class: output_truncated\nThe previous structured JSON was truncated by the model output limit. Return one raw JSON object only, without Markdown fences, explanatory prose, or trailing text. Set `assistant_message` to \"\", omit optional metadata, and prefer `ReplaceRange` or `ReplaceBlock` for a small existing-file edit. Do not emit full-file unified diffs.".to_string()
    } else if error.contains("missing_tool_call") {
        "[Parser]\nParse error class: missing_tool_call\nThe previous structured JSON omitted the required concrete tool action. Return one raw JSON object only and include the required action now. Do not add explanatory prose before or after the JSON object.".to_string()
    } else if error.contains("missing_json_object") {
        "[Parser]\nParse error class: missing_json_object\nThe previous turn used prose instead of a structured JSON object. Return one raw JSON object only, without Markdown fences or explanatory text, and include at least one concrete tool action.".to_string()
    } else if error.contains("unsupported native tool call `") {
        "[Parser]\nParse error class: unsupported_native_tool\nThe previous native tool name is not available in this runtime. Use only the documented tool names: read_file, list_directory, search_text, search_symbols, explain_validation_failure, suggest_edit_anchors, apply_patch, replace_block, write_file, run_command, or run_validation. Return one raw JSON object only and include the concrete supported action now.".to_string()
    } else if error.contains("native tool `")
        && (error.contains("was missing `") || error.contains("had invalid `"))
    {
        "[Parser]\nParse error class: malformed_action\nThe previous native tool call was missing or had invalid required fields. Return one raw JSON object only, include the complete tool payload, and do not add prose before or after the JSON object. For ModifyToml dependency operations, include `op`, `table`, `name`, and either `version` or `path` when setting a dependency.".to_string()
    } else if error.contains("native tool `")
        && (error.contains("arguments were invalid JSON")
            || error.contains("arguments must be JSON objects"))
    {
        "[Parser]\nParse error class: malformed_action\nThe previous native tool call arguments were malformed. Return one raw JSON object only, include a complete JSON object payload for the tool, and do not add prose before or after the JSON object.".to_string()
    } else if error.contains("trailing characters") {
        "[Parser]\nParse error class: trailing_characters\nThe previous structured JSON was valid, but it included trailing text after the first object. Return one raw JSON object only. Do not wrap it in Markdown fences, add explanations, or append any prose after the closing brace.".to_string()
    } else {
        "[Parser]\nParse error class: malformed\nThe previous structured JSON was malformed. Return one raw JSON object only, avoid raw multiline strings or control characters, keep `assistant_message` brief, and include at least one concrete tool action.".to_string()
    }
}

pub(crate) fn benchmark_general_parser_recovery_message(
    generic: String,
    ledger: &BenchmarkCaseLedger,
    has_mutating_change: bool,
) -> String {
    let owner_path = ledger
        .owner_files
        .iter()
        .chain(ledger.expected_touch_targets.iter())
        .find(|path| !path.trim().is_empty())
        .map(String::as_str)
        .unwrap_or(".");
    let mut lines = vec![
        generic,
        "[Parser] This benchmark turn still needs a concrete tool action, not prose."
            .to_string(),
        "Return one raw JSON object only. Do not describe the next step without emitting the tool action."
            .to_string(),
    ];
    if has_mutating_change {
        if let Some(command) = recommended_fast_loop_rerun_command(ledger) {
            lines.push(format!(
                "Preferred next action: run the smallest validation command: {command}"
            ));
            lines.push("Minimal JSON example:".to_string());
            lines.push(rerun_phase_parser_recovery_example(&command));
        }
    } else {
        lines.push(format!(
            "Preferred next action: read the primary owner file `{owner_path}`."
        ));
        lines.push("Minimal JSON example:".to_string());
        lines.push(
            serde_json::json!({
                "assistant_message": format!("Reading {owner_path}."),
                "actions": [{
                    "ReadFile": {
                        "path": owner_path
                    }
                }]
            })
            .to_string(),
        );
    }
    lines.join("\n")
}

pub(crate) fn focused_read_parser_recovery_example(
    path: &str,
    range: Option<crate::agent_protocol::ReadFileRange>,
) -> String {
    let mut read_file = serde_json::json!({
        "path": path
    });
    if let Some(range) = range.and_then(|value| value.normalized())
        && let Some(object) = read_file.as_object_mut()
    {
        object.insert(
            "range".to_string(),
            serde_json::json!({
                "start_line": range.start_line,
                "end_line": range.end_line
            }),
        );
    }
    serde_json::json!({
        "assistant_message": "Reading focused owner slice.",
        "actions": [{
            "ReadFile": read_file
        }]
    })
    .to_string()
}

pub(crate) fn extract_preview_id(output_text: &str) -> Option<String> {
    output_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("preview_id:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn preview_apply_locked(memory: &AgentRepairMemory) -> bool {
    memory.last_preview_id.as_ref().is_some_and(|preview_id| {
        !preview_id.trim().is_empty()
            && memory.preview_origin.as_deref() == Some("write_locked_manifest")
            && memory.scorecard.preview_created_count > memory.scorecard.apply_preview_count
    })
}

pub(crate) fn preview_targets_owner(memory: &AgentRepairMemory, owner_path: &str) -> bool {
    if memory.scorecard.preview_created_count <= memory.scorecard.apply_preview_count {
        return false;
    }
    let owner_path = canonical_path(owner_path);
    memory
        .last_preview_path
        .as_deref()
        .is_some_and(|path| canonical_path(path) == owner_path)
        || memory
            .last_preview_result
            .as_deref()
            .and_then(|output| extract_labeled_line(output, "path:"))
            .is_some_and(|path| canonical_path(&path) == owner_path)
}

pub(crate) fn preview_apply_placeholder(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed == "preview_id_from_last_preview"
}

pub(crate) fn patch_phase_scaffold_example(patch_target: &str) -> String {
    if patch_target.ends_with(".toml") {
        return manifest_preview_edit_scaffold_example(patch_target, None, None, &[], &[]);
    }
    serde_json::json!({
        "assistant_message": "scaffolding patch target",
        "actions": [{
            "SuggestEditAnchors": {
                "path": patch_target,
                "search_hint": "relevant section"
            }
        }]
    })
    .to_string()
}

pub(crate) fn manifest_preview_edit_scaffold_example(
    patch_target: &str,
    observed_content_hash: Option<&str>,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
    manifest_operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    let expected_hash = observed_content_hash.unwrap_or("FULL_FILE_CONTENT_HASH_FROM_READ");
    let operations = if manifest_operations.is_empty() {
        dependency_candidates
            .iter()
            .map(|name| {
                serde_json::json!({
                    "op": "set_dependency",
                    "table": target_dependency_table.unwrap_or("dependencies"),
                    "name": name,
                    "version": "<version>"
                })
            })
            .collect::<Vec<_>>()
    } else {
        manifest_operations
            .iter()
            .map(|operation| serde_json::to_value(operation).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "assistant_message": format!("previewing manifest patch for {patch_target}"),
        "actions": [{
            "PreviewEdit": {
                "path": patch_target,
                "edit": {
                    "modify_toml": {
                        "expected_hash": expected_hash,
                        "operations": if operations.is_empty() {
                            vec![serde_json::json!({
                                "op": "set_dependency",
                                "table": target_dependency_table.unwrap_or("dependencies"),
                                "name": "crate_name",
                                "version": "<version>"
                            })]
                        } else {
                            operations
                        }
                    }
                }
            }
        }]
    })
    .to_string()
}

pub(crate) fn apply_preview_parser_recovery_example(preview_id: &str) -> String {
    serde_json::json!({
        "assistant_message": format!("applying preview {preview_id}"),
        "actions": [{
            "ApplyPreview": {
                "preview_id": preview_id
            }
        }]
    })
    .to_string()
}

pub(crate) fn render_toml_edit_operations_brief(
    operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    operations
        .iter()
        .map(|operation| match operation {
            crate::agent_protocol::TomlEditOperation::SetDependency {
                table,
                name,
                version,
                features,
                ..
            } => {
                let version = version
                    .as_deref()
                    .map(|value| format!(" version={value}"))
                    .unwrap_or_default();
                let features = if features.is_empty() {
                    String::new()
                } else {
                    format!(" features=[{}]", features.join(","))
                };
                format!("set_dependency [{table}] {name}{version}{features}")
            }
            crate::agent_protocol::TomlEditOperation::RemoveDependency { table, name } => {
                format!("remove_dependency [{table}] {name}")
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn patch_phase_parser_recovery_example(
    patch_target: &str,
    recommended_rerun_command: Option<&str>,
    range: Option<crate::agent_protocol::ReadFileRange>,
    require_ranged_replace: bool,
    observed_content_hash: Option<&str>,
    target_dependency_table: Option<&str>,
    dependency_candidates: &[String],
    manifest_operations: &[crate::agent_protocol::TomlEditOperation],
) -> String {
    let first_action = if patch_target.ends_with(".toml") {
        let expected_hash = observed_content_hash.unwrap_or("FULL_FILE_CONTENT_HASH_FROM_READ");
        let operations = if manifest_operations.is_empty() {
            dependency_candidates
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "op": "set_dependency",
                        "table": target_dependency_table.unwrap_or("dependencies"),
                        "name": name,
                        "version": "<version>"
                    })
                })
                .collect::<Vec<_>>()
        } else {
            manifest_operations
                .iter()
                .map(|operation| serde_json::to_value(operation).unwrap_or(serde_json::Value::Null))
                .collect::<Vec<_>>()
        };
        serde_json::json!({
            "ModifyToml": {
                "path": patch_target,
                "expected_hash": expected_hash,
                "operations": if operations.is_empty() {
                    vec![serde_json::json!({
                        "op": "set_dependency",
                        "table": target_dependency_table.unwrap_or("dependencies"),
                        "name": "crate_name",
                        "version": "<version>"
                    })]
                } else {
                    operations
                }
            }
        })
    } else if let Some(range) = range.and_then(|range| range.normalized()) {
        let expected_hash = observed_content_hash.unwrap_or("CONTENT_HASH_FROM_READ");
        serde_json::json!({
            "ReplaceRange": {
                "path": patch_target,
                "range": {
                    "start_line": range.start_line,
                    "end_line": range.end_line
                },
                "expected_hash": expected_hash,
                "replacement": "<full replacement text for that line range>"
            }
        })
    } else if require_ranged_replace && range.is_none() {
        serde_json::json!({
            "ApplyPatch": {
                "path": patch_target,
                "patch": "*** Begin Patch\n*** Update File: <path>\n@@\n-<old line>\n+<new line>\n*** End Patch\n"
            }
        })
    } else {
        let mut replace_block = serde_json::json!({
            "path": patch_target,
            "search_block": "<exact old text from the patch target>",
            "replace_block": "<new text>"
        });
        if let Some(range) = range.and_then(|range| range.normalized())
            && let Some(object) = replace_block.as_object_mut()
        {
            object.insert(
                "range".to_string(),
                serde_json::json!({
                    "start_line": range.start_line,
                    "end_line": range.end_line
                }),
            );
        }
        serde_json::json!({ "ReplaceBlock": replace_block })
    };
    let mut actions = vec![first_action];
    if let Some(command) = recommended_rerun_command {
        actions.push(serde_json::json!({
            "RunCommand": {
                "command": command,
                "timeout_ms": 120000
            }
        }));
    }
    serde_json::json!({
        "assistant_message": "",
        "actions": actions
    })
    .to_string()
}

pub(crate) fn rerun_phase_parser_recovery_example(recommended_rerun_command: &str) -> String {
    serde_json::json!({
        "assistant_message": "rerunning fast loop",
        "actions": [{
            "RunCommand": {
                "command": recommended_rerun_command,
                "timeout_ms": 120000
            }
        }]
    })
    .to_string()
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum ControlFlow {
    Continue,
    ContinueNoBudget,
    BreakSuccess,
    BreakCancelled,
}

pub(crate) struct ModelTurnInput<'a> {
    pub(crate) content: &'a str,
    pub(crate) native_turn: Option<&'a AgentTurnResponse>,
    pub(crate) native_turn_error: Option<&'a str>,
    pub(crate) output_truncated: bool,
}

pub(crate) fn maybe_normalize_write_locked_manifest_turn_content(
    content: &str,
    state: &AgentTaskState,
) -> Option<String> {
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
    if !patch_target.as_ref().ends_with(".toml") {
        return None;
    }
    let apply_locked = preview_apply_locked(&state.agent_repair_memory);
    let preview_id = state.agent_repair_memory.last_preview_id.as_deref();
    let observed_hash =
        observed_full_file_content_hash(&state.agent_repair_memory, patch_target.as_ref())?;
    let trimmed = content.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let actions = value.get_mut("actions")?.as_array_mut()?;
    let mut relevant_action_count = 0usize;
    let mut changed = false;
    for action in actions {
        let Some(action_object) = action.as_object_mut() else {
            continue;
        };
        if let Some(payload) = if action_object.contains_key("ModifyToml") {
            action_object.get_mut("ModifyToml")
        } else {
            action_object.get_mut("modify_toml")
        } {
            let Some(payload_object) = payload.as_object_mut() else {
                continue;
            };
            relevant_action_count = relevant_action_count.saturating_add(1);
            if payload_object.get("path").is_none() && payload_object.get("file").is_none() {
                payload_object.insert(
                    "path".to_string(),
                    serde_json::Value::String(patch_target.as_ref().to_string()),
                );
                changed = true;
            }
            if payload_object.get("expected_hash").is_none()
                && payload_object.get("content_hash").is_none()
                && payload_object.get("hash").is_none()
            {
                payload_object.insert(
                    "expected_hash".to_string(),
                    serde_json::Value::String(observed_hash.clone()),
                );
                changed = true;
            }
            continue;
        }
        let preview_payload = if action_object.contains_key("PreviewEdit") {
            action_object.get_mut("PreviewEdit")
        } else {
            action_object.get_mut("preview_edit")
        };
        if let Some(preview_payload) = preview_payload {
            let Some(preview_object) = preview_payload.as_object_mut() else {
                continue;
            };
            let missing_preview_path =
                preview_object.get("path").is_none() && preview_object.get("file").is_none();
            if missing_preview_path {
                preview_object.insert(
                    "path".to_string(),
                    serde_json::Value::String(patch_target.as_ref().to_string()),
                );
                changed = true;
            }
            let Some(edit_payload) = preview_object
                .get_mut("edit")
                .and_then(|value| value.as_object_mut())
            else {
                continue;
            };
            let modify_toml = if edit_payload.contains_key("modify_toml") {
                edit_payload.get_mut("modify_toml")
            } else {
                edit_payload.get_mut("ModifyToml")
            };
            let Some(modify_toml) = modify_toml else {
                continue;
            };
            let Some(modify_toml_object) = modify_toml.as_object_mut() else {
                continue;
            };
            relevant_action_count = relevant_action_count.saturating_add(1);
            if modify_toml_object.get("expected_hash").is_none()
                && modify_toml_object.get("content_hash").is_none()
                && modify_toml_object.get("hash").is_none()
            {
                modify_toml_object.insert(
                    "expected_hash".to_string(),
                    serde_json::Value::String(observed_hash.clone()),
                );
                changed = true;
            }
            continue;
        }
        let apply_payload = if action_object.contains_key("ApplyPreview") {
            action_object.get_mut("ApplyPreview")
        } else {
            action_object.get_mut("apply_preview")
        };
        let Some(apply_payload) = apply_payload else {
            continue;
        };
        let Some(apply_object) = apply_payload.as_object_mut() else {
            continue;
        };
        relevant_action_count = relevant_action_count.saturating_add(1);
        if apply_locked
            && let Some(preview_id) = preview_id
            && (apply_object.get("preview_id").is_none()
                || apply_object
                    .get("preview_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(preview_apply_placeholder))
        {
            apply_object.insert(
                "preview_id".to_string(),
                serde_json::Value::String(preview_id.to_string()),
            );
            changed = true;
        }
    }
    (changed && relevant_action_count == 1)
        .then(|| serde_json::to_string(&value).ok())
        .flatten()
}

pub(crate) fn maybe_repair_native_manifest_tool_error(
    error: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let normalized_error = error.to_ascii_lowercase();
    if !(normalized_error.contains("modify_toml")
        && normalized_error.contains("operations")
        && (normalized_error.contains("missing field")
            || normalized_error.contains("invalid `operations`")))
    {
        return None;
    }
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
    if !patch_target.as_ref().ends_with(".toml") {
        return None;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        let preview_id = state.agent_repair_memory.last_preview_id.as_ref()?;
        return Some(AgentTurnResponse {
            assistant_message: String::new(),
            actions: vec![AgentAction::ApplyPreview {
                preview_id: preview_id.clone(),
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: vec![format!(
                "Recovered malformed native manifest tool call by applying clean preview `{preview_id}`."
            )],
        });
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
    Some(AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::PreviewEdit {
            path: patch_target.as_ref().to_string(),
            edit: PreviewEditPayload::ModifyToml {
                expected_hash,
                operations,
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered malformed native manifest tool call by constructing the benchmark manifest PreviewEdit from loaded context."
                .to_string(),
        ],
    })
}

pub(crate) fn maybe_repair_manifest_turn_parse_error(
    error: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let normalized_error = error.to_ascii_lowercase();
    if !(normalized_error.contains("previewedit")
        || normalized_error.contains("preview_edit")
        || normalized_error.contains("missing field `edit`")
        || normalized_error.contains("missing field edit"))
    {
        return None;
    }
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
    let action = exact_manifest_preview_action_from_state(state, repair_state, ledger)?;
    Some(AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![action],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered malformed manifest PreviewEdit JSON by constructing the benchmark manifest PreviewEdit from loaded context."
                .to_string(),
        ],
    })
}

pub(crate) fn maybe_repair_plain_text_fast_loop_turn(
    content: &str,
    state: &AgentTaskState,
) -> Option<AgentTurnResponse> {
    let ledger = state.benchmark_case_ledger.as_ref()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') || trimmed.len() > 300 {
        return None;
    }
    if preview_apply_locked(&state.agent_repair_memory) {
        return None;
    }
    if let Some(repair_state) = state.benchmark_repair_state.as_ref()
        && !matches!(repair_state.phase, BenchmarkRepairPhase::NeedsFastLoopRerun)
    {
        return None;
    }
    let normalized = trimmed.to_ascii_lowercase();
    if !(normalized.contains("fast loop")
        && (normalized.contains("run")
            || normalized.contains("running")
            || normalized.contains("rerun")
            || normalized.contains("execute")
            || normalized.contains("executing")))
    {
        return None;
    }
    if normalized.contains("patch") || normalized.contains("edit") {
        return None;
    }
    let command = recommended_fast_loop_rerun_command(ledger)?;
    Some(AgentTurnResponse {
        assistant_message: "Running the benchmark fast loop.".to_string(),
        actions: vec![AgentAction::RunCommand {
            command,
            timeout_ms: 120_000,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Recovered short benchmark prose into the known fast-loop command.".to_string(),
        ],
    })
}
