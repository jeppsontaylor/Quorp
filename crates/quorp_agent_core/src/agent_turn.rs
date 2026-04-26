use serde::Serialize;

use crate::agent_context::{AgentConfig, effective_approval_policy};
use crate::agent_protocol::{
    ActionApprovalPolicy, AgentAction, AgentMode, PreviewEditPayload, ReadFileRange, ValidationPlan,
};

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct AgentTurnResponse {
    pub assistant_message: String,
    pub actions: Vec<AgentAction>,
    pub task_updates: Vec<TaskItem>,
    pub memory_updates: Vec<MemoryUpdate>,
    pub requested_mode_change: Option<AgentMode>,
    pub verifier_plan: Option<ValidationPlan>,
    #[serde(skip_serializing, skip_deserializing)]
    pub parse_warnings: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct TaskItem {
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize)]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

fn parse_task_status(raw: &str) -> TaskStatus {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pending" => TaskStatus::Pending,
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        "blocked" => TaskStatus::Blocked,
        _ => TaskStatus::Pending,
    }
}

impl TaskStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct MemoryUpdate {
    pub kind: String,
    pub content: String,
    pub path: Option<String>,
}

pub fn parse_agent_turn_response(text: &str) -> Result<Option<AgentTurnResponse>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let candidate = if trimmed.starts_with('{') {
        trimmed
    } else if let Some(inner) = extract_fenced_json(trimmed) {
        inner
    } else if starts_with_line_oriented_action(trimmed)
        && let Some(turn) = parse_line_oriented_tool_turn(trimmed)
    {
        return Ok(Some(turn));
    } else if let Some(inner) = extract_first_json_object(trimmed) {
        inner
    } else if looks_like_incomplete_structured_turn(trimmed) {
        return Err(
            "Structured agent turn was invalid JSON: EOF while parsing an object".to_string(),
        );
    } else if let Some(turn) = parse_line_oriented_tool_turn(trimmed) {
        return Ok(Some(turn));
    } else {
        return Ok(None);
    };

    let (value, mut warnings) = match serde_json::from_str::<serde_json::Value>(candidate) {
        Ok(value) => (value, Vec::new()),
        Err(error) => {
            if let Some(repaired) = repair_json_like_object(candidate)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&repaired)
            {
                (
                    value,
                    vec![
                        "Repaired JSON-like model object syntax; raw JSON is preferred."
                            .to_string(),
                    ],
                )
            } else if trimmed.starts_with('{') && trailing_character_error(&error) {
                let Some(inner) = extract_first_json_object(trimmed) else {
                    return Err(format!("Structured agent turn was invalid JSON: {error}"));
                };
                let value =
                    serde_json::from_str::<serde_json::Value>(inner).map_err(|inner_error| {
                        format!("Structured agent turn was invalid JSON: {inner_error}")
                    })?;
                let mut warnings = Vec::new();
                if inner.len() < trimmed.len() {
                    let discarded = trimmed[inner.len()..].trim();
                    if !discarded.is_empty() {
                        warnings.push(format!(
                            "Discarded trailing text after structured JSON: {}",
                            truncate_warning_text(discarded, 120)
                        ));
                    }
                }
                (value, warnings)
            } else {
                return Err(format!("Structured agent turn was invalid JSON: {error}"));
            }
        }
    };
    let object = value
        .as_object()
        .ok_or_else(|| "Structured agent turn must be a JSON object".to_string())?;

    let assistant_message =
        parse_optional_string_field(object, "assistant_message", &mut warnings).unwrap_or_default();
    let actions = parse_actions(object, &mut warnings)?;
    let task_updates = parse_task_updates(object.get("task_updates"), &mut warnings);
    let memory_updates = parse_memory_updates(object.get("memory_updates"), &mut warnings);
    let requested_mode_change =
        parse_optional_json_field::<AgentMode>(object, "requested_mode_change", &mut warnings);
    let verifier_plan =
        parse_optional_json_field::<ValidationPlan>(object, "verifier_plan", &mut warnings);

    Ok(Some(AgentTurnResponse {
        assistant_message,
        actions,
        task_updates,
        memory_updates,
        requested_mode_change,
        verifier_plan,
        parse_warnings: warnings,
    }))
}

fn repair_json_like_object(text: &str) -> Option<String> {
    let mut output = String::with_capacity(text.len() + 16);
    let mut index = 0usize;
    let mut previous_significant = None;
    let mut changed = false;
    let mut in_string = false;
    let mut escaped = false;

    while index < text.len() {
        let ch = text.get(index..)?.chars().next()?;
        if in_string {
            output.push(ch);
            index += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                previous_significant = Some('"');
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }

        if is_json_like_key_start(ch)
            && matches!(previous_significant, Some('{') | Some(','))
            && let Some((identifier, after_identifier, after_whitespace)) =
                read_json_like_identifier(text, index)
            && text
                .get(after_whitespace..)
                .and_then(|tail| tail.chars().next())
                == Some(':')
        {
            output.push('"');
            output.push_str(identifier);
            output.push('"');
            output.push_str(text.get(after_identifier..after_whitespace)?);
            index = after_whitespace;
            changed = true;
            continue;
        }

        if previous_significant == Some(':')
            && is_json_like_bare_value_start(ch)
            && let Some((value, after_value)) = read_json_like_bare_value(text, index)
            && !matches!(value, "true" | "false" | "null")
        {
            output.push('"');
            output.push_str(value);
            output.push('"');
            index = after_value;
            changed = true;
            previous_significant = Some('"');
            continue;
        }

        output.push(ch);
        index += ch.len_utf8();
        if !ch.is_whitespace() {
            previous_significant = Some(ch);
        }
    }

    changed.then_some(output)
}

fn is_json_like_key_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_json_like_bare_value_start(ch: char) -> bool {
    ch == '_' || ch == '.' || ch == '/' || ch.is_ascii_alphabetic()
}

fn read_json_like_identifier(text: &str, start: usize) -> Option<(&str, usize, usize)> {
    let mut end = start;
    for (relative_index, ch) in text.get(start..)?.char_indices() {
        if relative_index == 0 {
            if !is_json_like_key_start(ch) {
                return None;
            }
        } else if !(ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()) {
            break;
        }
        end = start + relative_index + ch.len_utf8();
    }
    let mut after_whitespace = end;
    for (relative_index, ch) in text.get(end..)?.char_indices() {
        if !ch.is_whitespace() {
            after_whitespace = end + relative_index;
            break;
        }
        after_whitespace = end + relative_index + ch.len_utf8();
    }
    Some((text.get(start..end)?, end, after_whitespace))
}

fn read_json_like_bare_value(text: &str, start: usize) -> Option<(&str, usize)> {
    let mut end = start;
    for (relative_index, ch) in text.get(start..)?.char_indices() {
        if relative_index == 0 {
            if !is_json_like_bare_value_start(ch) {
                return None;
            }
        } else if ch.is_whitespace() || matches!(ch, ',' | '}' | ']') {
            break;
        }
        end = start + relative_index + ch.len_utf8();
    }
    Some((text.get(start..end)?, end))
}

fn parse_line_oriented_tool_turn(text: &str) -> Option<AgentTurnResponse> {
    if line_oriented_action_may_have_multiline_payload(text)
        && let Some(action) = parse_line_oriented_action(text)
    {
        return Some(line_oriented_turn(vec![action]));
    }

    let mut actions = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        actions.push(parse_line_oriented_action(line)?);
    }
    if actions.is_empty() {
        return None;
    }
    Some(line_oriented_turn(actions))
}

fn line_oriented_turn(actions: Vec<AgentAction>) -> AgentTurnResponse {
    AgentTurnResponse {
        assistant_message: String::new(),
        actions,
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: vec![
            "Parsed line-oriented tool syntax; raw JSON is preferred.".to_string(),
        ],
    }
}

fn starts_with_line_oriented_action(text: &str) -> bool {
    let Some(first_line) = text.lines().next() else {
        return false;
    };
    let first_line = first_line.trim_start_matches(['-', '*']).trim();
    let raw_name = first_line
        .split_once(':')
        .and_then(|(raw_name, _)| canonical_action_tag(raw_name.trim()).map(|_| raw_name.trim()))
        .or_else(|| first_line.split_whitespace().next());
    raw_name
        .and_then(|name| canonical_action_tag(&name.replace(['-', ' '], "_")))
        .is_some()
}

fn line_oriented_action_may_have_multiline_payload(text: &str) -> bool {
    let Some(first_line) = text.lines().next() else {
        return false;
    };
    let first_line = first_line.trim_start_matches(['-', '*']).trim();
    let raw_name = first_line
        .split_once(':')
        .and_then(|(raw_name, _)| canonical_action_tag(raw_name.trim()).map(|_| raw_name.trim()))
        .or_else(|| first_line.split_whitespace().next());
    raw_name
        .map(|name| name.to_ascii_lowercase().replace(['-', ' '], "_"))
        .is_some_and(|name| {
            matches!(
                name.as_str(),
                "applypatch"
                    | "apply_patch"
                    | "patch"
                    | "applypreview"
                    | "apply_preview"
                    | "replace_range"
                    | "replacerange"
                    | "replaceblock"
                    | "replace_block"
                    | "modify_toml"
                    | "modifytoml"
                    | "previewedit"
                    | "preview_edit"
                    | "preview"
            )
        })
}

fn parse_line_oriented_action(line: &str) -> Option<AgentAction> {
    let line = line.trim_start_matches(['-', '*']).trim();
    let (raw_name, rest) = if let Some((raw_name, rest)) = line.split_once(':')
        && canonical_action_tag(raw_name.trim()).is_some()
    {
        (raw_name.trim().to_string(), rest.trim().to_string())
    } else {
        let tokens = shlex::split(line)?;
        let raw_name = tokens.first()?.trim().to_string();
        let rest = line.get(raw_name.len()..)?.trim().to_string();
        (raw_name, rest)
    };
    let action_name = raw_name
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    match action_name.as_str() {
        "readfile" | "read_file" => {
            let (path, range) = parse_line_path_and_range(&rest)?;
            Some(AgentAction::ReadFile { path, range })
        }
        "listdirectory" | "list_directory" | "ls" => Some(AgentAction::ListDirectory {
            path: first_non_assignment_token(&rest).unwrap_or_else(|| ".".to_string()),
        }),
        "searchtext" | "search_text" | "grep" | "rg" => Some(AgentAction::SearchText {
            query: first_non_assignment_token(&rest)?,
            limit: parse_usize_assignment(&rest, "limit").unwrap_or(20),
        }),
        "searchsymbols" | "search_symbols" => Some(AgentAction::SearchSymbols {
            query: first_non_assignment_token(&rest)?,
            limit: parse_usize_assignment(&rest, "limit").unwrap_or(20),
        }),
        "findfiles" | "find_files" | "fd" => Some(AgentAction::FindFiles {
            query: first_non_assignment_token(&rest)?,
            limit: parse_usize_assignment(&rest, "limit").unwrap_or(20),
        }),
        "structuralsearch" | "structural_search" | "ast_grep" | "ast-grep" | "sg" => {
            Some(AgentAction::StructuralSearch {
                pattern: parse_string_assignment(&rest, "pattern")
                    .or_else(|| first_non_assignment_token(&rest))?,
                language: parse_string_assignment(&rest, "language")
                    .or_else(|| parse_string_assignment(&rest, "lang")),
                path: parse_string_assignment(&rest, "path"),
                limit: parse_usize_assignment(&rest, "limit").unwrap_or(20),
            })
        }
        "structuraleditpreview" | "structural_edit_preview" => {
            Some(AgentAction::StructuralEditPreview {
                pattern: parse_string_assignment(&rest, "pattern")?,
                rewrite: parse_string_assignment(&rest, "rewrite")?,
                language: parse_string_assignment(&rest, "language")
                    .or_else(|| parse_string_assignment(&rest, "lang")),
                path: parse_string_assignment(&rest, "path"),
            })
        }
        "cargodiagnostics" | "cargo_diagnostics" => Some(AgentAction::CargoDiagnostics {
            command: parse_string_assignment(&rest, "command"),
            include_clippy: parse_bool_assignment(&rest, "include_clippy").unwrap_or(false),
        }),
        "explainvalidationfailure" | "explain_validation_failure" => {
            if rest.trim().is_empty() {
                return None;
            }
            Some(AgentAction::ExplainValidationFailure {
                command: first_non_assignment_token(&rest).unwrap_or_else(|| "validation".into()),
                output: rest.trim().to_string(),
            })
        }
        "suggesteditanchors" | "suggest_edit_anchors" | "anchors" => {
            let (path, range) = parse_line_path_and_range(&rest)?;
            Some(AgentAction::SuggestEditAnchors {
                path,
                range,
                search_hint: parse_string_assignment(&rest, "search_hint")
                    .or_else(|| parse_string_assignment(&rest, "hint")),
            })
        }
        "previewedit" | "preview_edit" | "preview" => {
            if let (Some(expected_hash), Some(replacement)) = (
                parse_string_assignment(&rest, "expected_hash")
                    .or_else(|| parse_string_assignment(&rest, "content_hash"))
                    .or_else(|| parse_string_assignment(&rest, "hash")),
                parse_string_assignment(&rest, "replacement")
                    .or_else(|| parse_string_assignment(&rest, "replace_with"))
                    .or_else(|| parse_string_assignment(&rest, "new")),
            ) {
                let (path, range) = parse_line_path_and_range(&rest)?;
                return Some(AgentAction::PreviewEdit {
                    path,
                    edit: PreviewEditPayload::ReplaceRange {
                        range: range?,
                        expected_hash,
                        replacement,
                    },
                });
            }
            let (path, patch) = parse_line_path_and_patch(&rest)?;
            Some(AgentAction::PreviewEdit {
                path,
                edit: PreviewEditPayload::ApplyPatch { patch },
            })
        }
        "applypreview" | "apply_preview" => Some(AgentAction::ApplyPreview {
            preview_id: parse_string_assignment(&rest, "preview_id")
                .or_else(|| first_non_assignment_token(&rest))?,
        }),
        "applypatch" | "apply_patch" | "patch" => {
            let (path, patch) = parse_line_path_and_patch(&rest)?;
            Some(AgentAction::ApplyPatch { path, patch })
        }
        "replacerange" | "replace_range" => {
            let (path, range) = parse_line_path_and_range(&rest)?;
            let Some(expected_hash) = parse_string_assignment(&rest, "expected_hash")
                .or_else(|| parse_string_assignment(&rest, "content_hash"))
                .or_else(|| parse_string_assignment(&rest, "hash"))
            else {
                return Some(AgentAction::ReadFile { path, range });
            };
            Some(AgentAction::ReplaceRange {
                path,
                range: range?,
                expected_hash,
                replacement: parse_string_assignment(&rest, "replacement")
                    .or_else(|| parse_string_assignment(&rest, "replace_with"))
                    .or_else(|| parse_string_assignment(&rest, "new"))?,
            })
        }
        "modifytoml" | "modify_toml" => {
            let path = first_non_assignment_token(&rest)?;
            let expected_hash = parse_string_assignment(&rest, "expected_hash")
                .or_else(|| parse_string_assignment(&rest, "content_hash"))
                .or_else(|| parse_string_assignment(&rest, "hash"))
                .unwrap_or_else(|| "not_specified_yet".to_string());
            Some(AgentAction::ModifyToml {
                path,
                expected_hash,
                operations: parse_line_toml_operations(&rest)?,
            })
        }
        "replaceblock" | "replace_block" => {
            let (path, range) = parse_line_path_and_range(&rest)?;
            let search_block = parse_string_assignment(&rest, "search_block")
                .or_else(|| parse_string_assignment(&rest, "search"))
                .or_else(|| parse_string_assignment(&rest, "old"))?;
            let replace_block = parse_string_assignment(&rest, "replace_block")
                .or_else(|| parse_string_assignment(&rest, "replace_with"))
                .or_else(|| parse_string_assignment(&rest, "replacement"))
                .or_else(|| parse_string_assignment(&rest, "replace"))
                .or_else(|| parse_string_assignment(&rest, "new"))?;
            Some(AgentAction::ReplaceBlock {
                path,
                search_block,
                replace_block,
                range,
            })
        }
        "runvalidation" | "run_validation" => Some(AgentAction::RunValidation {
            plan: parse_line_validation_plan(&rest)?,
        }),
        "run" | "runcommand" | "run_command" | "shell" | "bash" => {
            if rest.trim().is_empty() {
                return None;
            }
            Some(AgentAction::RunCommand {
                command: rest.trim().to_string(),
                timeout_ms: parse_usize_assignment(&rest, "timeout_ms")
                    .and_then(|value| u64::try_from(value).ok())
                    .unwrap_or(30_000),
            })
        }
        _ => None,
    }
}

fn parse_line_path_and_range(rest: &str) -> Option<(String, Option<ReadFileRange>)> {
    let range = parse_range_from_text(rest);
    let tokens = shlex::split(rest)?;
    let path = tokens
        .iter()
        .find(|token| {
            !token.contains('=')
                && token.as_str() != "lines"
                && token.as_str() != "line"
                && parse_range_token(token).is_none()
        })?
        .to_string();
    Some((path, range))
}

fn parse_line_path_and_patch(rest: &str) -> Option<(String, String)> {
    let (path, _range) = parse_line_path_and_range(rest)?;
    let patch = parse_string_assignment(rest, "patch")
        .or_else(|| parse_json_string_field_from_text(rest, "patch"))?;
    Some((path, patch))
}

fn parse_json_string_field_from_text(text: &str, key: &str) -> Option<String> {
    let object_text = extract_first_json_object(text)?;
    let value = serde_json::from_str::<serde_json::Value>(object_text).ok()?;
    value.get(key)?.as_str().map(str::to_string)
}

fn parse_line_toml_operations(text: &str) -> Option<Vec<crate::agent_protocol::TomlEditOperation>> {
    let operations = extract_json_array_after_key(text, "operations")
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .or_else(|| {
            let object_text = extract_first_json_object(text)?;
            let value = serde_json::from_str::<serde_json::Value>(object_text).ok()?;
            value.get("operations").cloned()
        })?;
    parse_toml_operations(&operations)
}

fn extract_json_array_after_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let key_index = text.find(key)?;
    let tail = text.get(key_index + key.len()..)?;
    let relative_start = tail.find('[')?;
    let start = key_index + key.len() + relative_start;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (relative_index, ch) in text.get(start..)?.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth = depth.saturating_add(1),
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return text.get(start..=start + relative_index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_line_validation_plan(rest: &str) -> Option<ValidationPlan> {
    let mut plan = ValidationPlan::default();
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("fmt") {
        plan.fmt = true;
    }
    if lower.contains("clippy") {
        plan.clippy = true;
    }
    if lower.contains("workspace_tests") || lower.contains("workspace-tests") {
        plan.workspace_tests = true;
    }
    if let Some(tests) = parse_parenthesized_list(trimmed, "tests") {
        plan.tests.extend(tests);
    } else if trimmed.contains("::") {
        plan.tests.push(trimmed.to_string());
    }
    if plan.is_empty() { None } else { Some(plan) }
}

fn parse_parenthesized_list(text: &str, key: &str) -> Option<Vec<String>> {
    let start = text.find(&format!("{key}("))? + key.len() + 1;
    let tail = text.get(start..)?;
    let end = tail.find(')')?;
    let inner = tail.get(..end)?;
    let values = inner
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn first_non_assignment_token(rest: &str) -> Option<String> {
    shlex::split(rest)?
        .into_iter()
        .find(|token| !token.contains('=') && parse_range_token(token).is_none())
}

fn parse_usize_assignment(rest: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key}=");
    shlex::split(rest)?
        .into_iter()
        .find_map(|token| token.strip_prefix(&prefix)?.parse::<usize>().ok())
}

fn parse_string_assignment(rest: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    shlex::split(rest)?
        .into_iter()
        .find_map(|token| token.strip_prefix(&prefix).map(str::to_string))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool_assignment(rest: &str, key: &str) -> Option<bool> {
    let value = parse_string_assignment(rest, key)?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_range_from_text(text: &str) -> Option<ReadFileRange> {
    for key in ["range=", "line_range="] {
        if let Some(start) = text.find(key) {
            let tail = text.get(start + key.len()..)?;
            if let Some(range) = parse_bracketed_range(tail).or_else(|| parse_range_token(tail)) {
                return Some(range);
            }
        }
    }
    let tokens = shlex::split(text)?;
    for window in tokens.windows(2) {
        if matches!(window.first().map(String::as_str), Some("lines" | "line")) {
            return parse_range_token(window.get(1)?);
        }
    }
    tokens.iter().find_map(|token| parse_range_token(token))
}

fn parse_bracketed_range(text: &str) -> Option<ReadFileRange> {
    let text = text.trim();
    let inner = text.strip_prefix('[')?.split_once(']')?.0;
    parse_range_numbers(inner)
}

fn parse_range_token(token: &str) -> Option<ReadFileRange> {
    let cleaned = token
        .trim()
        .trim_matches(',')
        .trim_matches('"')
        .trim_matches('\'');
    parse_bracketed_range(cleaned)
        .or_else(|| parse_range_numbers(cleaned.strip_prefix("lines=")?))
        .or_else(|| parse_range_numbers(cleaned.strip_prefix("line_range=")?))
        .or_else(|| parse_range_numbers(cleaned.strip_prefix("range=")?))
        .or_else(|| parse_range_numbers(cleaned))
}

fn parse_range_numbers(text: &str) -> Option<ReadFileRange> {
    let values = text
        .split([',', '-', ':'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| value.parse::<usize>().ok())
        .collect::<Vec<_>>();
    let start_line = *values.first()?;
    let end_line = *values.get(1)?;
    ReadFileRange {
        start_line,
        end_line,
    }
    .normalized()
}

fn trailing_character_error(error: &serde_json::Error) -> bool {
    error.to_string().contains("trailing characters")
}

fn extract_fenced_json(text: &str) -> Option<&str> {
    let stripped = text.strip_prefix("```json")?.strip_suffix("```")?;
    Some(stripped.trim())
}

fn extract_first_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;

    for (index, byte) in bytes.iter().enumerate() {
        if start.is_none() {
            if *byte == b'{' {
                start = Some(index);
                depth = 1;
            }
            continue;
        }

        if in_string {
            if escape {
                escape = false;
                continue;
            }
            match *byte {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let start_index = start?;
                    return text.get(start_index..=index);
                }
            }
            _ => {}
        }
    }

    None
}

fn looks_like_incomplete_structured_turn(text: &str) -> bool {
    text.contains("```json")
        || text.contains("\"actions\"")
        || text.contains("\"assistant_message\"")
        || text.contains("\"task_updates\"")
        || text.contains("\"memory_updates\"")
        || text.contains('{')
}

fn truncate_warning_text(text: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated
}

fn parse_optional_string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let value = object.get(key)?;
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(text.clone()),
        _ => {
            warnings.push(format!("Ignored non-string `{key}` field."));
            None
        }
    }
}

fn parse_actions(
    object: &serde_json::Map<String, serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Result<Vec<AgentAction>, String> {
    let Some(value) = object.get("actions") else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    match serde_json::from_value::<Vec<AgentAction>>(value.clone()) {
        Ok(actions) => Ok(actions),
        Err(error) => {
            if let Some(actions) = parse_flat_actions(value) {
                warnings.push(format!(
                    "Normalized flat action schema after strict action parse failed: {error}"
                ));
                return Ok(actions);
            }

            let detail = format!("Structured agent turn `actions` field was invalid: {error}");
            warnings.push(
                "Ignored malformed optional metadata while keeping valid actions.".to_string(),
            );
            Err(detail)
        }
    }
}

fn parse_flat_actions(value: &serde_json::Value) -> Option<Vec<AgentAction>> {
    let items = value.as_array()?;
    let mut actions = Vec::with_capacity(items.len());

    for item in items {
        if let Ok(action) = serde_json::from_value::<AgentAction>(item.clone()) {
            actions.push(action);
            continue;
        }
        if let Some(action) = parse_relaxed_tagged_action(item) {
            actions.push(action);
            continue;
        }
        actions.push(parse_flat_action(item)?);
    }

    Some(actions)
}

fn parse_relaxed_tagged_action(value: &serde_json::Value) -> Option<AgentAction> {
    let object = value.as_object()?;
    let mut matching_tags = object
        .keys()
        .filter_map(|key| canonical_action_tag(key).map(|tag| (key, tag)));
    let (original_key, canonical_tag) = matching_tags.next()?;
    if matching_tags.next().is_some() {
        return None;
    }

    let mut payload = object.get(original_key)?.clone();
    if canonical_tag == "ReplaceBlock"
        && let serde_json::Value::Object(payload_object) = &mut payload
    {
        copy_outer_range_alias(object, payload_object);
    }
    let payload = normalize_tagged_payload(canonical_tag, payload);
    let mut normalized = serde_json::Map::new();
    normalized.insert(canonical_tag.to_string(), payload.clone());
    serde_json::from_value::<AgentAction>(serde_json::Value::Object(normalized))
        .ok()
        .or_else(|| hash_guard_prerequisite_read(canonical_tag, &payload))
}

fn normalize_tagged_payload(tag: &str, payload: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut object) = payload else {
        return payload;
    };

    match tag {
        "ReadFile" => normalize_range_array_field(&mut object),
        "RunCommand" => copy_string_alias(&mut object, "command", &["cmd"]),
        "SuggestEditAnchors" => {
            normalize_range_array_field(&mut object);
            copy_string_alias(&mut object, "search_hint", &["hint", "query"]);
        }
        "PreviewEdit" => {
            copy_string_alias(&mut object, "path", &["file"]);
            normalize_preview_edit_payload(&mut object);
        }
        "ReplaceRange" => {
            normalize_range_array_field(&mut object);
            copy_string_alias(&mut object, "path", &["file"]);
            copy_string_alias(&mut object, "expected_hash", &["content_hash", "hash"]);
            copy_string_alias(
                &mut object,
                "replacement",
                &["replace_with", "replace", "new", "content"],
            );
        }
        "ModifyToml" => {
            copy_string_alias(&mut object, "path", &["file"]);
            copy_string_alias(&mut object, "expected_hash", &["content_hash", "hash"]);
            normalize_toml_operations_field(&mut object);
            if object.get("operations").is_some() && object.get("expected_hash").is_none() {
                object.insert(
                    "expected_hash".to_string(),
                    serde_json::Value::String("not_specified_yet".to_string()),
                );
            }
        }
        "ApplyPreview" => {
            copy_string_alias(&mut object, "preview_id", &["id"]);
        }
        "ExplainValidationFailure" => {
            copy_string_alias(&mut object, "command", &["cmd", "test", "name"]);
            copy_string_alias(&mut object, "output", &["stderr", "stdout", "text"]);
        }
        "SuggestImplementationTargets" => {
            copy_string_alias(&mut object, "command", &["cmd", "test", "name"]);
            copy_string_alias(&mut object, "output", &["stderr", "stdout", "text"]);
            copy_string_alias(
                &mut object,
                "failing_path",
                &["path", "file", "failure_path"],
            );
            if object.get("failing_line").is_none()
                && let Some(value) = object
                    .get("line")
                    .or_else(|| object.get("failure_line"))
                    .cloned()
            {
                object.insert("failing_line".to_string(), value);
            }
        }
        "ReplaceBlock" => {
            normalize_range_array_field(&mut object);
            copy_string_alias(&mut object, "search_block", &["search", "old"]);
            copy_string_alias(
                &mut object,
                "replace_block",
                &["replace_with", "replacement", "replace", "new"],
            );
        }
        _ => {}
    }

    serde_json::Value::Object(object)
}

fn hash_guard_prerequisite_read(tag: &str, payload: &serde_json::Value) -> Option<AgentAction> {
    let object = payload.as_object()?;
    match tag {
        "ModifyToml" => Some(AgentAction::ReadFile {
            path: string_field(object, &["path", "file"])?,
            range: None,
        }),
        "ReplaceRange" => Some(AgentAction::ReadFile {
            path: string_field(object, &["path", "file"])?,
            range: parse_flat_read_file_range(object),
        }),
        "PreviewEdit" => {
            let path = string_field(object, &["path", "file"])?;
            let edit = object.get("edit")?.as_object()?;
            if let Some(payload) = edit
                .get("modify_toml")
                .or_else(|| edit.get("ModifyToml"))
                .and_then(serde_json::Value::as_object)
                && string_field(payload, &["expected_hash", "content_hash", "hash"]).is_none()
            {
                return Some(AgentAction::ReadFile { path, range: None });
            }
            if let Some(payload) = edit
                .get("replace_range")
                .or_else(|| edit.get("ReplaceRange"))
                .and_then(serde_json::Value::as_object)
                && string_field(payload, &["expected_hash", "content_hash", "hash"]).is_none()
            {
                return Some(AgentAction::ReadFile {
                    path,
                    range: parse_flat_read_file_range(payload),
                });
            }
            None
        }
        _ => None,
    }
}

fn normalize_preview_edit_payload(object: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(edit) = object.get("edit").cloned() {
        if let Some(normalized) = normalize_preview_edit_value(edit) {
            object.insert("edit".to_string(), normalized);
        }
        return;
    }
    if let Some(patch) = object.get("patch").cloned()
        && patch.as_str().is_some()
    {
        object.insert(
            "edit".to_string(),
            serde_json::json!({ "apply_patch": { "patch": patch } }),
        );
        return;
    }
    if object.get("operations").is_some() {
        copy_string_alias(object, "expected_hash", &["content_hash", "hash"]);
        if object.get("expected_hash").is_none() {
            object.insert(
                "expected_hash".to_string(),
                serde_json::Value::String("not_specified_yet".to_string()),
            );
        }
        if let Some(expected_hash) = object.get("expected_hash").cloned()
            && let Some(operations) = object.get("operations").cloned()
        {
            object.insert(
                "edit".to_string(),
                serde_json::json!({
                    "modify_toml": {
                        "expected_hash": expected_hash,
                        "operations": operations
                    }
                }),
            );
            return;
        }
    }
    if object.get("range").is_some()
        && (object.get("replacement").is_some()
            || object.get("replace_with").is_some()
            || object.get("replace").is_some()
            || object.get("new").is_some()
            || object.get("content").is_some())
        && (object.get("expected_hash").is_some()
            || object.get("content_hash").is_some()
            || object.get("hash").is_some())
    {
        normalize_range_array_field(object);
        copy_string_alias(object, "expected_hash", &["content_hash", "hash"]);
        copy_string_alias(
            object,
            "replacement",
            &["replace_with", "replace", "new", "content"],
        );
        if let (Some(range), Some(expected_hash), Some(replacement)) = (
            object.get("range").cloned(),
            object.get("expected_hash").cloned(),
            object.get("replacement").cloned(),
        ) {
            object.insert(
                "edit".to_string(),
                serde_json::json!({
                    "replace_range": {
                        "range": range,
                        "expected_hash": expected_hash,
                        "replacement": replacement
                    }
                }),
            );
            return;
        }
    }
    let search_block = object
        .get("search_block")
        .or_else(|| object.get("search"))
        .or_else(|| object.get("old"))
        .cloned();
    let replace_block = object
        .get("replace_block")
        .or_else(|| object.get("replace_with"))
        .or_else(|| object.get("replacement"))
        .or_else(|| object.get("replace"))
        .or_else(|| object.get("new"))
        .cloned();
    if let (Some(search_block), Some(replace_block)) = (search_block, replace_block) {
        let mut payload = serde_json::Map::new();
        payload.insert("search_block".to_string(), search_block);
        payload.insert("replace_block".to_string(), replace_block);
        if let Some(range) = object.get("range").cloned() {
            payload.insert("range".to_string(), range);
        } else if let Some(line_range) = object.get("line_range").cloned() {
            payload.insert("range".to_string(), line_range);
        }
        normalize_range_array_field(&mut payload);
        object.insert(
            "edit".to_string(),
            serde_json::Value::Object(
                [(
                    "replace_block".to_string(),
                    serde_json::Value::Object(payload),
                )]
                .into_iter()
                .collect(),
            ),
        );
    }
}

fn normalize_preview_edit_value(value: serde_json::Value) -> Option<serde_json::Value> {
    if serde_json::from_value::<PreviewEditPayload>(value.clone()).is_ok() {
        return Some(value);
    }
    let serde_json::Value::Object(mut object) = value else {
        return None;
    };
    if let Some(payload) = object
        .remove("apply_patch")
        .or_else(|| object.remove("ApplyPatch"))
    {
        return Some(serde_json::json!({ "apply_patch": payload }));
    }
    if let Some(mut payload) = object
        .remove("replace_block")
        .or_else(|| object.remove("ReplaceBlock"))
    {
        if let serde_json::Value::Object(payload_object) = &mut payload {
            normalize_range_array_field(payload_object);
        }
        return Some(serde_json::json!({ "replace_block": payload }));
    }
    if let Some(mut payload) = object
        .remove("replace_range")
        .or_else(|| object.remove("ReplaceRange"))
    {
        if let serde_json::Value::Object(payload_object) = &mut payload {
            normalize_range_array_field(payload_object);
            copy_string_alias(payload_object, "expected_hash", &["content_hash", "hash"]);
            copy_string_alias(
                payload_object,
                "replacement",
                &["replace_with", "replace", "new", "content"],
            );
        }
        return Some(serde_json::json!({ "replace_range": payload }));
    }
    if let Some(mut payload) = object
        .remove("modify_toml")
        .or_else(|| object.remove("ModifyToml"))
    {
        if let serde_json::Value::Object(payload_object) = &mut payload {
            normalize_toml_operations_field(payload_object);
        }
        return Some(serde_json::json!({ "modify_toml": payload }));
    }
    let edit_kind = object
        .get("kind")
        .or_else(|| object.get("type"))
        .or_else(|| object.get("action"))
        .and_then(serde_json::Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"));
    if edit_kind
        .as_deref()
        .is_some_and(|kind| matches!(kind, "applypatch" | "apply_patch" | "patch"))
        || object.get("patch").is_some()
    {
        let patch = object.get("patch")?.clone();
        return Some(serde_json::json!({ "apply_patch": { "patch": patch } }));
    }
    if edit_kind
        .as_deref()
        .is_some_and(|kind| matches!(kind, "replaceblock" | "replace_block" | "replace"))
        || object.get("search_block").is_some()
        || object.get("search").is_some()
    {
        copy_string_alias(&mut object, "search_block", &["search", "old"]);
        copy_string_alias(
            &mut object,
            "replace_block",
            &["replace_with", "replacement", "replace", "new"],
        );
        normalize_range_array_field(&mut object);
        object.remove("kind");
        object.remove("type");
        object.remove("action");
        return Some(serde_json::json!({ "replace_block": object }));
    }
    if edit_kind
        .as_deref()
        .is_some_and(|kind| matches!(kind, "replacerange" | "replace_range"))
        || object.get("replacement").is_some()
            && object.get("expected_hash").is_some()
            && object.get("range").is_some()
    {
        copy_string_alias(&mut object, "expected_hash", &["content_hash", "hash"]);
        copy_string_alias(
            &mut object,
            "replacement",
            &["replace_with", "replace", "new", "content"],
        );
        normalize_range_array_field(&mut object);
        object.remove("kind");
        object.remove("type");
        object.remove("action");
        return Some(serde_json::json!({ "replace_range": object }));
    }
    if edit_kind
        .as_deref()
        .is_some_and(|kind| matches!(kind, "modifytoml" | "modify_toml" | "toml"))
        || object.get("operations").is_some()
    {
        copy_string_alias(&mut object, "expected_hash", &["content_hash", "hash"]);
        if object.get("expected_hash").is_none() {
            object.insert(
                "expected_hash".to_string(),
                serde_json::Value::String("not_specified_yet".to_string()),
            );
        }
        normalize_toml_operations_field(&mut object);
        object.remove("kind");
        object.remove("type");
        object.remove("action");
        return Some(serde_json::json!({ "modify_toml": object }));
    }
    None
}

fn normalize_range_array_field(object: &mut serde_json::Map<String, serde_json::Value>) {
    if object.get("range").is_none()
        && let Some(line_range) = object.get("line_range").cloned()
    {
        object.insert("range".to_string(), line_range);
    }
    let Some(range) = object.get("range").and_then(serde_json::Value::as_array) else {
        return;
    };
    let start_line = range.first().and_then(serde_json::Value::as_u64);
    let end_line = range.get(1).and_then(serde_json::Value::as_u64);
    if let (Some(start_line), Some(end_line)) = (start_line, end_line) {
        object.insert(
            "range".to_string(),
            serde_json::json!({
                "start_line": start_line,
                "end_line": end_line
            }),
        );
    }
}

fn normalize_toml_operations_field(object: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(operations) = object.get("operations").cloned()
        && let Some(normalized) = normalize_toml_operations_value(operations)
    {
        object.insert("operations".to_string(), normalized);
    }
}

fn copy_outer_range_alias(
    outer: &serde_json::Map<String, serde_json::Value>,
    payload: &mut serde_json::Map<String, serde_json::Value>,
) {
    if payload.get("range").is_none()
        && let Some(range) = outer.get("range").cloned()
    {
        payload.insert("range".to_string(), range);
    }
    if payload.get("line_range").is_none()
        && let Some(line_range) = outer.get("line_range").cloned()
    {
        payload.insert("line_range".to_string(), line_range);
    }
}

fn copy_string_alias(
    object: &mut serde_json::Map<String, serde_json::Value>,
    canonical_key: &str,
    aliases: &[&str],
) {
    if object.get(canonical_key).is_some() {
        return;
    }
    if let Some(value) = aliases
        .iter()
        .filter_map(|alias| object.get(*alias))
        .find(|value| value.as_str().is_some())
        .cloned()
    {
        object.insert(canonical_key.to_string(), value);
    }
}

fn canonical_action_tag(name: &str) -> Option<&'static str> {
    match name {
        "RunCommand" | "run_command" | "runcommand" => Some("RunCommand"),
        "ReadFile" | "read_file" | "readfile" => Some("ReadFile"),
        "ListDirectory" | "list_directory" | "listdirectory" => Some("ListDirectory"),
        "SearchText" | "search_text" | "searchtext" => Some("SearchText"),
        "SearchSymbols" | "search_symbols" | "searchsymbols" => Some("SearchSymbols"),
        "FindFiles" | "find_files" | "findfiles" => Some("FindFiles"),
        "StructuralSearch" | "structural_search" | "structuralsearch" => Some("StructuralSearch"),
        "StructuralEditPreview" | "structural_edit_preview" | "structuraleditpreview" => {
            Some("StructuralEditPreview")
        }
        "CargoDiagnostics" | "cargo_diagnostics" | "cargodiagnostics" => Some("CargoDiagnostics"),
        "GetRepoCapsule" | "get_repo_capsule" | "getrepocapsule" => Some("GetRepoCapsule"),
        "ExplainValidationFailure" | "explain_validation_failure" | "explainvalidationfailure" => {
            Some("ExplainValidationFailure")
        }
        "SuggestImplementationTargets"
        | "suggest_implementation_targets"
        | "suggestimplementationtargets"
        | "implementation_targets"
        | "implementationtargets" => Some("SuggestImplementationTargets"),
        "SuggestEditAnchors" | "suggest_edit_anchors" | "suggesteditanchors" => {
            Some("SuggestEditAnchors")
        }
        "PreviewEdit" | "preview_edit" | "previewedit" => Some("PreviewEdit"),
        "ReplaceRange" | "replace_range" | "replacerange" => Some("ReplaceRange"),
        "ModifyToml" | "modify_toml" | "modifytoml" => Some("ModifyToml"),
        "ApplyPreview" | "apply_preview" | "applypreview" => Some("ApplyPreview"),
        "WriteFile" | "write_file" | "writefile" => Some("WriteFile"),
        "ApplyPatch" | "apply_patch" | "applypatch" => Some("ApplyPatch"),
        "ReplaceBlock" | "replace_block" | "replaceblock" => Some("ReplaceBlock"),
        "SetExecutable" | "set_executable" | "setexecutable" => Some("SetExecutable"),
        "McpCallTool" | "mcp_call_tool" | "mcpcalltool" => Some("McpCallTool"),
        "RunValidation" | "run_validation" | "runvalidation" => Some("RunValidation"),
        _ => None,
    }
}

fn parse_flat_action(value: &serde_json::Value) -> Option<AgentAction> {
    let object = value.as_object()?;
    let action_name = string_field(object, &["action", "type", "tool", "name"])?;
    let normalized_action_name = action_name
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");

    match normalized_action_name.as_str() {
        "run" | "runcommand" | "run_command" | "shell" | "bash" => Some(AgentAction::RunCommand {
            command: string_field(object, &["command", "cmd"])?,
            timeout_ms: u64_field(object, &["timeout_ms", "timeout"]).unwrap_or(30_000),
        }),
        "readfile" | "read_file" => Some(AgentAction::ReadFile {
            path: string_field(object, &["path", "file"])?,
            range: parse_flat_read_file_range(object),
        }),
        "listdirectory" | "list_directory" | "ls" => Some(AgentAction::ListDirectory {
            path: string_field(object, &["path", "directory"]).unwrap_or_else(|| ".".to_string()),
        }),
        "searchtext" | "search_text" | "grep" | "rg" => Some(AgentAction::SearchText {
            query: string_field(object, &["query", "q", "pattern"])?,
            limit: usize_field(object, &["limit"]).unwrap_or(20),
        }),
        "searchsymbols" | "search_symbols" => Some(AgentAction::SearchSymbols {
            query: string_field(object, &["query", "q"])?,
            limit: usize_field(object, &["limit"]).unwrap_or(20),
        }),
        "findfiles" | "find_files" | "fd" => Some(AgentAction::FindFiles {
            query: string_field(object, &["query", "q", "pattern"])?,
            limit: usize_field(object, &["limit"]).unwrap_or(20),
        }),
        "structuralsearch" | "structural_search" | "ast_grep" | "ast-grep" | "sg" => {
            Some(AgentAction::StructuralSearch {
                pattern: string_field(object, &["pattern", "query", "q"])?,
                language: string_field(object, &["language", "lang"]),
                path: string_field(object, &["path", "file", "directory"]),
                limit: usize_field(object, &["limit"]).unwrap_or(20),
            })
        }
        "structuraleditpreview" | "structural_edit_preview" => {
            Some(AgentAction::StructuralEditPreview {
                pattern: string_field(object, &["pattern"])?,
                rewrite: string_field(object, &["rewrite"])?,
                language: string_field(object, &["language", "lang"]),
                path: string_field(object, &["path", "file", "directory"]),
            })
        }
        "cargodiagnostics" | "cargo_diagnostics" => Some(AgentAction::CargoDiagnostics {
            command: string_field(object, &["command", "cmd"]),
            include_clippy: bool_field(object, &["include_clippy", "clippy"]).unwrap_or(false),
        }),
        "getrepocapsule" | "get_repo_capsule" => Some(AgentAction::GetRepoCapsule {
            query: string_field(object, &["query", "q"]),
            limit: usize_field(object, &["limit"]).unwrap_or(20),
        }),
        "explainvalidationfailure" | "explain_validation_failure" => {
            Some(AgentAction::ExplainValidationFailure {
                command: string_field(object, &["command", "cmd"]).unwrap_or_else(|| {
                    string_field(object, &["test", "name"]).unwrap_or_else(|| "validation".into())
                }),
                output: string_field(object, &["output", "stderr", "stdout", "text"])?,
            })
        }
        "suggestimplementationtargets"
        | "suggest_implementation_targets"
        | "implementationtargets"
        | "implementation_targets" => Some(AgentAction::SuggestImplementationTargets {
            command: string_field(object, &["command", "cmd"]).unwrap_or_else(|| {
                string_field(object, &["test", "name"]).unwrap_or_else(|| "validation".into())
            }),
            output: string_field(object, &["output", "stderr", "stdout", "text"])?,
            failing_path: string_field(object, &["failing_path", "path", "file", "failure_path"]),
            failing_line: usize_field(object, &["failing_line", "line", "failure_line"]),
        }),
        "suggesteditanchors" | "suggest_edit_anchors" | "anchors" => {
            Some(AgentAction::SuggestEditAnchors {
                path: string_field(object, &["path", "file"])?,
                range: parse_flat_read_file_range(object),
                search_hint: string_field(object, &["search_hint", "hint", "query"]),
            })
        }
        "previewedit" | "preview_edit" | "preview" => Some(AgentAction::PreviewEdit {
            path: string_field(object, &["path", "file"])?,
            edit: parse_flat_preview_edit_payload(object)?,
        }),
        "replacerange" | "replace_range" => {
            let path = string_field(object, &["path", "file"])?;
            let range = parse_flat_read_file_range(object)?;
            let Some(expected_hash) =
                string_field(object, &["expected_hash", "content_hash", "hash"])
            else {
                return Some(AgentAction::ReadFile {
                    path,
                    range: Some(range),
                });
            };
            Some(AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                replacement: string_field(
                    object,
                    &["replacement", "replace_with", "replace", "new", "content"],
                )?,
            })
        }
        "modifytoml" | "modify_toml" => {
            let path = string_field(object, &["path", "file"])?;
            let expected_hash = string_field(object, &["expected_hash", "content_hash", "hash"])
                .unwrap_or_else(|| "not_specified_yet".to_string());
            Some(AgentAction::ModifyToml {
                path,
                expected_hash,
                operations: parse_toml_operations(object.get("operations")?)?,
            })
        }
        "applypreview" | "apply_preview" => Some(AgentAction::ApplyPreview {
            preview_id: string_field(object, &["preview_id", "id"])?,
        }),
        "writefile" | "write_file" => Some(AgentAction::WriteFile {
            path: string_field(object, &["path", "file"])?,
            content: string_field(object, &["content", "text"])?,
        }),
        "applypatch" | "apply_patch" => Some(AgentAction::ApplyPatch {
            path: string_field(object, &["path", "file"])?,
            patch: string_field(object, &["patch"])?,
        }),
        "replaceblock" | "replace_block" => Some(AgentAction::ReplaceBlock {
            path: string_field(object, &["path", "file"])?,
            search_block: string_field(object, &["search_block", "search", "old"])?,
            replace_block: string_field(
                object,
                &[
                    "replace_block",
                    "replace_with",
                    "replacement",
                    "replace",
                    "new",
                ],
            )?,
            range: parse_flat_read_file_range(object),
        }),
        "setexecutable" | "set_executable" | "chmod_x" => Some(AgentAction::SetExecutable {
            path: string_field(object, &["path", "file"])?,
        }),
        _ => None,
    }
}

fn parse_flat_preview_edit_payload(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<PreviewEditPayload> {
    if let Some(edit) = object.get("edit").cloned()
        && let Some(normalized) = normalize_preview_edit_value(edit)
        && let Ok(payload) = serde_json::from_value::<PreviewEditPayload>(normalized)
    {
        return Some(payload);
    }
    if let Some(patch) = string_field(object, &["patch"]) {
        return Some(PreviewEditPayload::ApplyPatch { patch });
    }
    if string_field(object, &["expected_hash", "content_hash", "hash"]).is_some()
        && string_field(
            object,
            &["replacement", "replace_with", "replace", "new", "content"],
        )
        .is_some()
        && parse_flat_read_file_range(object).is_some()
    {
        return Some(PreviewEditPayload::ReplaceRange {
            range: parse_flat_read_file_range(object)?,
            expected_hash: string_field(object, &["expected_hash", "content_hash", "hash"])?,
            replacement: string_field(
                object,
                &["replacement", "replace_with", "replace", "new", "content"],
            )?,
        });
    }
    if object.get("operations").is_some() {
        return Some(PreviewEditPayload::ModifyToml {
            expected_hash: string_field(object, &["expected_hash", "content_hash", "hash"])
                .unwrap_or_else(|| "not_specified_yet".to_string()),
            operations: parse_toml_operations(object.get("operations")?)?,
        });
    }
    let search_block = string_field(object, &["search_block", "search", "old"])?;
    let replace_block = string_field(
        object,
        &[
            "replace_block",
            "replace_with",
            "replacement",
            "replace",
            "new",
        ],
    )?;
    Some(PreviewEditPayload::ReplaceBlock {
        search_block,
        replace_block,
        range: parse_flat_read_file_range(object),
    })
}

fn parse_toml_operations(
    value: &serde_json::Value,
) -> Option<Vec<crate::agent_protocol::TomlEditOperation>> {
    let normalized = normalize_toml_operations_value(value.clone())?;
    serde_json::from_value::<Vec<crate::agent_protocol::TomlEditOperation>>(normalized).ok()
}

fn normalize_toml_operations_value(value: serde_json::Value) -> Option<serde_json::Value> {
    let serde_json::Value::Array(items) = value else {
        return None;
    };
    let normalized = items
        .into_iter()
        .map(normalize_toml_operation_value)
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::Value::Array(normalized))
}

fn normalize_toml_operation_value(value: serde_json::Value) -> Option<serde_json::Value> {
    if serde_json::from_value::<crate::agent_protocol::TomlEditOperation>(value.clone()).is_ok() {
        return Some(value);
    }
    let serde_json::Value::Object(mut object) = value else {
        return None;
    };
    for (key, op) in [
        ("set_dependency", "set_dependency"),
        ("SetDependency", "set_dependency"),
        ("remove_dependency", "remove_dependency"),
        ("RemoveDependency", "remove_dependency"),
    ] {
        if let Some(payload) = object.remove(key) {
            let serde_json::Value::Object(mut payload) = payload else {
                return None;
            };
            normalize_toml_dependency_map_shorthand(&mut payload, op);
            payload.insert("op".to_string(), serde_json::Value::String(op.to_string()));
            normalize_toml_operation_aliases(&mut payload);
            return Some(serde_json::Value::Object(payload));
        }
    }
    if object.get("op").is_none()
        && let Some(op) = object
            .get("operation")
            .or_else(|| object.get("kind"))
            .or_else(|| object.get("type"))
            .or_else(|| object.get("action"))
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
    {
        object.insert("op".to_string(), serde_json::Value::String(op));
    }
    if object.get("op").is_none() && object.get("table").is_some() && object.get("name").is_some() {
        let inferred = if object
            .get("remove")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            "remove_dependency"
        } else {
            "set_dependency"
        };
        object.insert(
            "op".to_string(),
            serde_json::Value::String(inferred.to_string()),
        );
    }
    normalize_toml_operation_aliases(&mut object);
    Some(serde_json::Value::Object(object))
}

fn normalize_toml_operation_aliases(object: &mut serde_json::Map<String, serde_json::Value>) {
    copy_string_alias(
        object,
        "name",
        &[
            "dependency",
            "dependency_name",
            "crate",
            "crate_name",
            "package_name",
        ],
    );
    if object.get("table").is_none()
        && object
            .get("op")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|op| matches!(op, "set_dependency" | "remove_dependency"))
    {
        object.insert(
            "table".to_string(),
            serde_json::Value::String("dependencies".to_string()),
        );
    }
    if object.get("default_features").is_none()
        && let Some(value) = object.get("default-features").cloned()
    {
        object.insert("default_features".to_string(), value);
    }
    copy_string_alias(object, "version", &["spec"]);
}

fn normalize_toml_dependency_map_shorthand(
    object: &mut serde_json::Map<String, serde_json::Value>,
    op: &str,
) {
    if object.get("name").is_some() || object.len() != 1 {
        return;
    }
    let Some((candidate_name, candidate_value)) = object
        .iter()
        .next()
        .map(|(key, value)| (key.clone(), value.clone()))
    else {
        return;
    };
    if is_toml_operation_field(&candidate_name) {
        return;
    }
    object.insert(
        "name".to_string(),
        serde_json::Value::String(candidate_name),
    );
    if op == "set_dependency" && object.get("version").is_none() {
        if let Some(version) = candidate_value.as_str() {
            object.insert(
                "version".to_string(),
                serde_json::Value::String(version.to_string()),
            );
        } else if let Some(fields) = candidate_value.as_object() {
            for (key, value) in fields {
                object.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
}

fn is_toml_operation_field(field: &str) -> bool {
    matches!(
        field,
        "op" | "operation"
            | "kind"
            | "type"
            | "action"
            | "table"
            | "name"
            | "dependency"
            | "dependency_name"
            | "crate"
            | "crate_name"
            | "package"
            | "package_name"
            | "version"
            | "spec"
            | "features"
            | "default_features"
            | "default-features"
            | "optional"
            | "path"
            | "remove"
    )
}

fn parse_flat_read_file_range(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<ReadFileRange> {
    if let Some(range) = object.get("range")
        && let Ok(parsed) = serde_json::from_value::<ReadFileRange>(range.clone())
    {
        return parsed.normalized();
    }

    if let Some(line_range) = object
        .get("line_range")
        .and_then(serde_json::Value::as_array)
    {
        let start_line = line_range
            .first()
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        let end_line = line_range
            .get(1)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok());
        if let (Some(start_line), Some(end_line)) = (start_line, end_line) {
            return ReadFileRange {
                start_line,
                end_line,
            }
            .normalized();
        }
    }

    let start_line = usize_field(object, &["start_line", "start"]);
    let end_line = usize_field(object, &["end_line", "end"]);
    match (start_line, end_line) {
        (Some(start_line), Some(end_line)) => ReadFileRange {
            start_line,
            end_line,
        }
        .normalized(),
        _ => None,
    }
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .next()
}

fn u64_field(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(serde_json::Value::as_u64)
}

fn usize_field(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<usize> {
    u64_field(object, keys).and_then(|value| usize::try_from(value).ok())
}

fn bool_field(object: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(serde_json::Value::as_bool)
}

fn parse_optional_json_field<T>(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            warnings.push(format!("Ignored malformed `{key}` field: {error}"));
            None
        }
    }
}

fn parse_task_updates(
    value: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Vec<TaskItem> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        warnings.push("Ignored non-array `task_updates` field.".to_string());
        return Vec::new();
    };

    let mut parsed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match parse_task_item(item, warnings) {
            Some(task) => parsed.push(task),
            None => warnings.push(format!("Ignored malformed `task_updates[{index}]` entry.")),
        }
    }
    parsed
}

fn parse_task_item(value: &serde_json::Value, warnings: &mut Vec<String>) -> Option<TaskItem> {
    match value {
        serde_json::Value::String(title) => Some(TaskItem {
            title: title.trim().to_string(),
            status: TaskStatus::Pending,
        }),
        serde_json::Value::Object(object) => {
            let title = ["title", "progress", "summary", "message", "content"]
                .iter()
                .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
                .unwrap_or_default()
                .trim()
                .to_string();

            let status = match object.get("status") {
                Some(serde_json::Value::String(raw_status)) => {
                    let parsed_status = parse_task_status(raw_status);
                    if parsed_status == TaskStatus::Pending
                        && !raw_status.trim().eq_ignore_ascii_case("pending")
                    {
                        warnings.push(format!(
                            "Coerced unsupported task status `{}` to `pending`.",
                            raw_status.trim()
                        ));
                    }
                    parsed_status
                }
                Some(serde_json::Value::Null) | None => TaskStatus::Pending,
                Some(_) => {
                    warnings.push("Coerced non-string task status to `pending`.".to_string());
                    TaskStatus::Pending
                }
            };

            if title.is_empty() {
                warnings.push(
                    "Task update was missing a `title`; using a generic placeholder.".to_string(),
                );
            }

            Some(TaskItem {
                title: if title.is_empty() {
                    "status updated".to_string()
                } else {
                    title
                },
                status,
            })
        }
        _ => None,
    }
}

fn parse_memory_updates(
    value: Option<&serde_json::Value>,
    warnings: &mut Vec<String>,
) -> Vec<MemoryUpdate> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        warnings.push("Ignored non-array `memory_updates` field.".to_string());
        return Vec::new();
    };

    let mut parsed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match parse_memory_update(item, warnings) {
            Some(update) => parsed.push(update),
            None => warnings.push(format!(
                "Ignored malformed `memory_updates[{index}]` entry."
            )),
        }
    }
    parsed
}

fn parse_memory_update(
    value: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Option<MemoryUpdate> {
    match value {
        serde_json::Value::String(content) => Some(MemoryUpdate {
            kind: "note".to_string(),
            content: content.trim().to_string(),
            path: None,
        }),
        serde_json::Value::Object(object) => {
            let kind = object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.get("type").and_then(serde_json::Value::as_str))
                .unwrap_or("note")
                .trim()
                .to_string();
            let content = object
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let path = object
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);

            if content.is_empty() {
                warnings.push("Ignored memory update without textual `content`.".to_string());
                return None;
            }

            Some(MemoryUpdate {
                kind,
                content,
                path,
            })
        }
        _ => None,
    }
}

pub fn render_agent_turn_text(turn: &AgentTurnResponse, config: &AgentConfig) -> String {
    let mut lines = Vec::new();
    let assistant_message = turn.assistant_message.trim();
    if !assistant_message.is_empty() {
        lines.push(assistant_message.to_string());
    }

    if !turn.parse_warnings.is_empty() {
        lines.push("Parsing notes:".to_string());
        lines.extend(
            turn.parse_warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }

    if let Some(mode) = turn.requested_mode_change {
        lines.push(format!("Mode request: switch to {}", mode.label()));
    }

    if !turn.task_updates.is_empty() {
        lines.push("Task updates:".to_string());
        lines.extend(
            turn.task_updates
                .iter()
                .map(|item| format!("- [{}] {}", item.status.label(), item.title)),
        );
    }

    if let Some(plan) = turn.verifier_plan.as_ref() {
        let summary = plan.summary();
        if !summary.is_empty() {
            lines.push(format!("Verifier plan: {summary}"));
        }
    }

    if !turn.actions.is_empty() {
        lines.push("Action receipts:".to_string());
        for action in &turn.actions {
            let approval = match effective_approval_policy(action, config) {
                ActionApprovalPolicy::AutoApproveReadOnly => "auto",
                ActionApprovalPolicy::RequireExplicitConfirmation => "confirm",
            };
            lines.push(format!("- {} [{approval}]", action.summary()));
        }
    }

    if !turn.memory_updates.is_empty() {
        lines.push("Memory updates:".to_string());
        lines.extend(
            turn.memory_updates
                .iter()
                .map(|item| match item.path.as_deref() {
                    Some(path) => {
                        format!("- {} ({}): {}", item.kind.trim(), path, item.content.trim())
                    }
                    None => format!("- {}: {}", item.kind.trim(), item.content.trim()),
                }),
        );
    }

    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_turn_json() -> &'static str {
        r#"{"assistant_message":"ok","actions":[{"ReadFile":{"path":"src/main.rs"}}],"task_updates":[],"memory_updates":[],"requested_mode_change":null,"verifier_plan":null}"#
    }

    #[test]
    fn parses_raw_json_turn() {
        let parsed = parse_agent_turn_response(sample_turn_json())
            .expect("parse")
            .expect("turn");
        assert_eq!(parsed.assistant_message, "ok");
        assert_eq!(parsed.actions.len(), 1);
        assert!(parsed.parse_warnings.is_empty());
    }

    #[test]
    fn parses_flat_read_file_action_schema() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "Reading the suggested failing slice.",
                "actions": [
                    {
                        "action": "read_file",
                        "path": "src/round.rs",
                        "line_range": [769, 802]
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert!(!parsed.parse_warnings.is_empty());
        assert_eq!(
            parsed.actions[0],
            AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 769,
                    end_line: 802,
                }),
            }
        );
    }

    #[test]
    fn parses_snake_case_tagged_actions_inside_actions_array() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "",
                "actions": [
                    {"read_file": {"path": "src/lib.rs", "range": [4, 8]}},
                    {"replace_block": {"path": "src/lib.rs", "search": "old", "replace_with": "new", "line_range": [4, 8]}},
                    {"run_validation": {"plan": {"tests": ["lib::tests::smoke"]}}}
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 3);
        assert!(matches!(
            parsed.actions[0],
            AgentAction::ReadFile {
                range: Some(ReadFileRange {
                    start_line: 4,
                    end_line: 8
                }),
                ..
            }
        ));
        assert!(matches!(
            parsed.actions[1],
            AgentAction::ReplaceBlock {
                range: Some(ReadFileRange {
                    start_line: 4,
                    end_line: 8
                }),
                ..
            }
        ));
        assert!(matches!(
            parsed.actions[2],
            AgentAction::RunValidation { .. }
        ));
        assert!(!parsed.parse_warnings.is_empty());
    }

    #[test]
    fn repairs_json_like_unquoted_keys_for_remote_models() {
        let parsed = parse_agent_turn_response(
            r#"{
                actions: [
                    {
                        ModifyToml: {
                            path: Cargo.toml,
                            expected_hash: d90cc110472497e2,
                            operations: [
                                {
                                    op: set_dependency,
                                    table: dependencies,
                                    name: chrono,
                                    version: "0.4"
                                }
                            ]
                        }
                    }
                ],
                assistant_message: patching
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert!(
            parsed
                .parse_warnings
                .iter()
                .any(|warning| { warning.contains("Repaired JSON-like model object syntax") })
        );
        assert_eq!(parsed.assistant_message, "patching");
        assert!(matches!(
            &parsed.actions[0],
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } if path == "Cargo.toml"
                && expected_hash == "d90cc110472497e2"
                && matches!(
                    &operations[0],
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        version,
                        ..
                    } if table == "dependencies"
                        && name == "chrono"
                        && version.as_deref() == Some("0.4")
                )
        ));
    }

    #[test]
    fn parses_preview_edit_tagged_and_flat_forms() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "",
                "actions": [
                    {
                        "preview_edit": {
                            "path": "src/lib.rs",
                            "edit": {
                                "replace_block": {
                                    "search_block": "old",
                                    "replace_block": "new",
                                    "range": [10, 12]
                                }
                            }
                        }
                    },
                    {
                        "action": "preview_edit",
                        "path": "src/lib.rs",
                        "patch": "@@ -1 +1 @@\n-old\n+new\n"
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 2);
        assert!(matches!(
            parsed.actions[0],
            AgentAction::PreviewEdit {
                edit: PreviewEditPayload::ReplaceBlock {
                    range: Some(ReadFileRange {
                        start_line: 10,
                        end_line: 12
                    }),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            parsed.actions[1],
            AgentAction::PreviewEdit {
                edit: PreviewEditPayload::ApplyPatch { .. },
                ..
            }
        ));
    }

    #[test]
    fn parses_intent_edit_actions_and_preview_payloads() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "",
                "actions": [
                    {
                        "replace_range": {
                            "path": "src/lib.rs",
                            "range": [4, 6],
                            "content_hash": "0123456789abcdef",
                            "replacement": "new lines"
                        }
                    },
                    {
                        "ModifyToml": {
                            "path": "Cargo.toml",
                            "expected_hash": "fedcba9876543210",
                            "operations": [
                                {
                                    "table": "dependencies",
                                    "name": "chrono",
                                    "version": "0.4",
                                    "default-features": false
                                }
                            ]
                        }
                    },
                    {
                        "action": "preview_edit",
                        "path": "src/lib.rs",
                        "range": [10, 11],
                        "expected_hash": "aaaaaaaaaaaaaaaa",
                        "replacement": "line"
                    },
                    {
                        "modify_toml": {
                            "path": "Cargo.toml",
                            "operations": [
                                {
                                    "table": "dependencies",
                                    "name": "uuid",
                                    "version": "1"
                                }
                            ]
                        }
                    },
                    {
                        "apply_preview": {
                            "preview_id": "pv_abc123"
                        }
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert!(matches!(
            &parsed.actions[0],
            AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                ..
            } if path == "src/lib.rs" && range.start_line == 4 && expected_hash == "0123456789abcdef"
        ));
        assert!(matches!(
            &parsed.actions[1],
            AgentAction::ModifyToml {
                path,
                operations,
                ..
            } if path == "Cargo.toml" && operations.len() == 1
        ));
        assert!(matches!(
            &parsed.actions[2],
            AgentAction::PreviewEdit {
                edit: PreviewEditPayload::ReplaceRange { range, .. },
                ..
            } if range.start_line == 10
        ));
        assert!(matches!(
            &parsed.actions[3],
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } if path == "Cargo.toml"
                && expected_hash == "not_specified_yet"
                && operations.len() == 1
        ));
        assert!(matches!(
            &parsed.actions[4],
            AgentAction::ApplyPreview { preview_id } if preview_id == "pv_abc123"
        ));
    }

    #[test]
    fn parses_line_oriented_modify_toml_with_placeholder_hash_when_missing() {
        let missing_hash = parse_agent_turn_response(
            r#"modify_toml Cargo.toml [operations [{"type":"set_dependency","table":"dependencies","name":"chrono","version":"0.4"}]]"#,
        )
        .expect("parse")
        .expect("turn");

        assert!(matches!(
            &missing_hash.actions[0],
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } if path == "Cargo.toml"
                && expected_hash == "not_specified_yet"
                && operations.len() == 1
        ));

        let parsed = parse_agent_turn_response(
            r#"modify_toml Cargo.toml expected_hash=d90cc110472497e2 [operations [{"type":"set_dependency","name":"chrono","version":"0.4"}]]"#,
        )
        .expect("parse")
        .expect("turn");

        match &parsed.actions[0] {
            AgentAction::ModifyToml {
                path,
                expected_hash,
                operations,
            } => {
                assert_eq!(path, "Cargo.toml");
                assert_eq!(expected_hash, "d90cc110472497e2");
                assert!(matches!(
                    &operations[0],
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "chrono"
                ));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn parses_modify_toml_dependency_name_aliases() {
        let parsed = parse_agent_turn_response(
            r#"{
                "actions": [
                    {
                        "modify_toml": {
                            "path": "Cargo.toml",
                            "expected_hash": "0123456789abcdef",
                            "operations": [
                                {"set_dependency": {"dependency_name": "chrono", "version": "0.4"}},
                                {"type": "set_dependency", "dependency": "uuid", "version": "1"},
                                {"set_dependency": {"rand": "0.8"}}
                            ]
                        }
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        match &parsed.actions[0] {
            AgentAction::ModifyToml { operations, .. } => {
                assert!(matches!(
                    &operations[0],
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "chrono"
                ));
                assert!(matches!(
                    &operations[1],
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        ..
                    } if table == "dependencies" && name == "uuid"
                ));
                assert!(matches!(
                    &operations[2],
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table,
                        name,
                        version,
                        ..
                    } if table == "dependencies" && name == "rand" && version.as_deref() == Some("0.8")
                ));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn parses_flat_run_command_action_schema() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "Rerunning the fast loop.",
                "actions": [
                    {
                        "action": "run_command",
                        "command": "cargo test --quiet --lib round::tests::",
                        "timeout_ms": 30000
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::RunCommand {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                timeout_ms: 30000,
            }
        );
    }

    #[test]
    fn parses_tagged_action_with_extra_metadata() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search_block": "old",
                            "replace_block": "new"
                        },
                        "range": [243, 246]
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert!(!parsed.parse_warnings.is_empty());
        assert_eq!(
            parsed.actions[0],
            AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: Some(ReadFileRange {
                    start_line: 243,
                    end_line: 246,
                }),
            }
        );
    }

    #[test]
    fn parses_tagged_action_alias_fields() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search_block": "old",
                            "replace_with": "new"
                        }
                    },
                    {
                        "ReadFile": {
                            "path": "src/round.rs",
                            "range": [160, 250]
                        }
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            }
        );
        assert_eq!(
            parsed.actions[1],
            AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 160,
                    end_line: 250,
                }),
            }
        );
    }

    #[test]
    fn parses_ranged_replace_block_forms() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search": "old",
                            "new": "new",
                            "range": [170, 220]
                        }
                    },
                    {
                        "action": "replace_block",
                        "path": "src/round.rs",
                        "search": "other old",
                        "replace_with": "other new",
                        "line_range": [221, 240]
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: Some(ReadFileRange {
                    start_line: 170,
                    end_line: 220,
                }),
            }
        );
        assert_eq!(
            parsed.actions[1],
            AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "other old".to_string(),
                replace_block: "other new".to_string(),
                range: Some(ReadFileRange {
                    start_line: 221,
                    end_line: 240,
                }),
            }
        );
    }

    #[test]
    fn parses_read_only_repair_assistance_actions() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "Need anchors before patching.",
                "actions": [
                    {
                        "action": "explain_validation_failure",
                        "command": "cargo test round",
                        "output": "thread panicked at src/round.rs:42:5"
                    },
                    {
                        "action": "suggest_edit_anchors",
                        "path": "src/round.rs",
                        "range": [40, 52],
                        "hint": "round_duration"
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::ExplainValidationFailure {
                command: "cargo test round".to_string(),
                output: "thread panicked at src/round.rs:42:5".to_string(),
            }
        );
        assert_eq!(
            parsed.actions[1],
            AgentAction::SuggestEditAnchors {
                path: "src/round.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 40,
                    end_line: 52,
                }),
                search_hint: Some("round_duration".to_string()),
            }
        );
    }

    #[test]
    fn parses_snake_case_implementation_target_action_wrapper() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "Need target ranking.",
                "actions": [
                    {
                        "suggest_implementation_targets": {
                            "cmd": "cargo test issue_474",
                            "stderr": "error[E0432]: unresolved import `serde`",
                            "path": "tests/issues/issue_474.rs",
                            "line": 12
                        }
                    }
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::SuggestImplementationTargets {
                command: "cargo test issue_474".to_string(),
                output: "error[E0432]: unresolved import `serde`".to_string(),
                failing_path: Some("tests/issues/issue_474.rs".to_string()),
                failing_line: Some(12),
            }
        );
    }

    #[test]
    fn parses_line_oriented_tool_actions() {
        let parsed = parse_agent_turn_response(
            "read_file src/round.rs range=[781, 813]\nrun_validation: tests(round::tests::test_duration_round_close_to_epoch)",
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 2);
        assert!(!parsed.parse_warnings.is_empty());
        assert_eq!(
            parsed.actions[0],
            AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 781,
                    end_line: 813,
                }),
            }
        );
        assert_eq!(
            parsed.actions[1],
            AgentAction::RunValidation {
                plan: ValidationPlan {
                    tests: vec!["round::tests::test_duration_round_close_to_epoch".to_string()],
                    ..ValidationPlan::default()
                },
            }
        );
    }

    #[test]
    fn parses_line_oriented_anchor_suggestion() {
        let parsed =
            parse_agent_turn_response("suggest_edit_anchors src/round.rs range=[40, 52] hint=span")
                .expect("parse")
                .expect("turn");

        assert_eq!(
            parsed.actions[0],
            AgentAction::SuggestEditAnchors {
                path: "src/round.rs".to_string(),
                range: Some(ReadFileRange {
                    start_line: 40,
                    end_line: 52,
                }),
                search_hint: Some("span".to_string()),
            }
        );
    }

    #[test]
    fn parses_line_oriented_apply_patch_payloads() {
        let parsed = parse_agent_turn_response(
            "apply_patch Cargo.toml [{\"patch\":\"--- a/Cargo.toml\\n+++ b/Cargo.toml\\n@@ -1 +1 @@\\n-old\\n+new\\n\"}]",
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert!(matches!(
            &parsed.actions[0],
            AgentAction::ApplyPatch { path, patch }
                if path == "Cargo.toml" && patch.contains("@@ -1 +1 @@")
        ));
    }

    #[test]
    fn parses_multiline_line_oriented_preview_edit_payload() {
        let parsed = parse_agent_turn_response(
            "preview_edit Cargo.toml patch=\"--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1 @@\n-old\n+new\n\"",
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert!(matches!(
            &parsed.actions[0],
            AgentAction::PreviewEdit {
                path,
                edit: PreviewEditPayload::ApplyPatch { patch }
            } if path == "Cargo.toml" && patch.contains("+new")
        ));
    }

    #[test]
    fn line_oriented_parser_rejects_mixed_prose() {
        let parsed = parse_agent_turn_response(
            "I will inspect first.\nread_file src/round.rs range=[781, 813]",
        )
        .expect("parse");

        assert!(parsed.is_none());
    }

    #[test]
    fn parses_fenced_json_turn() {
        let wrapped = format!("```json\n{}\n```", sample_turn_json());
        let parsed = parse_agent_turn_response(&wrapped)
            .expect("parse")
            .expect("turn");
        assert_eq!(parsed.assistant_message, "ok");
    }

    #[test]
    fn parses_json_wrapped_in_explanatory_text() {
        let wrapped = format!(
            "I found the next action.\n{}\nThis should be executed next.",
            sample_turn_json()
        );
        let parsed = parse_agent_turn_response(&wrapped)
            .expect("parse")
            .expect("turn");
        assert_eq!(parsed.assistant_message, "ok");
    }

    #[test]
    fn parses_json_with_trailing_prose() {
        let wrapped = format!(
            "{}\n\nI will inspect the workspace next.",
            sample_turn_json()
        );
        let parsed = parse_agent_turn_response(&wrapped)
            .expect("parse")
            .expect("turn");
        assert_eq!(parsed.assistant_message, "ok");
        assert!(!parsed.parse_warnings.is_empty());
    }

    #[test]
    fn parses_json_with_trailing_fenced_text() {
        let wrapped = format!(
            "{}\n```text\nI will inspect the workspace next.\n```",
            sample_turn_json()
        );
        let parsed = parse_agent_turn_response(&wrapped)
            .expect("parse")
            .expect("turn");
        assert_eq!(parsed.assistant_message, "ok");
        assert!(!parsed.parse_warnings.is_empty());
    }

    #[test]
    fn ignores_plain_text_without_json() {
        let parsed = parse_agent_turn_response("plain text only").expect("parse");
        assert!(parsed.is_none());
    }

    #[test]
    fn incomplete_embedded_json_is_recoverable_error() {
        let error = parse_agent_turn_response(
            "I have the fix.\n```json\n{\"assistant_message\":\"patching\",\"actions\":[{\"ReadFile\":{\"path\":\"src/lib.rs\"}}]\n```",
        )
        .expect_err("incomplete embedded JSON should error");
        assert!(error.contains("EOF while parsing"));
    }

    #[test]
    fn parses_deepseek_style_optional_metadata_leniently() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "I will inspect the objective first.",
                "actions": [
                    {
                        "ReadFile": {
                            "path": "README.md"
                        }
                    }
                ],
                "task_updates": [
                    {
                        "status": "Read the objective file to understand the requirements and context for the challenge.",
                        "progress": "File read initiated."
                    }
                ],
                "memory_updates": [
                    {
                        "type": "FileContent",
                        "path": "README.md",
                        "content": "Objective summary."
                    }
                ],
                "requested_mode_change": null,
                "verifier_plan": null,
                "extra_field": {"ignored": true}
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.task_updates.len(), 1);
        assert_eq!(parsed.task_updates[0].title, "File read initiated.");
        assert_eq!(parsed.task_updates[0].status, TaskStatus::Pending);
        assert_eq!(parsed.memory_updates.len(), 1);
        assert_eq!(parsed.memory_updates[0].kind, "FileContent");
        assert_eq!(parsed.memory_updates[0].path.as_deref(), Some("README.md"));
        assert!(!parsed.parse_warnings.is_empty());
    }

    #[test]
    fn malformed_optional_metadata_does_not_drop_valid_actions() {
        let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "valid",
                "actions": [{"ReadFile":{"path":"src/main.rs"}}],
                "task_updates": {"status":"bad"},
                "requested_mode_change": {"bad": true}
            }"#,
        )
        .expect("parse")
        .expect("turn");

        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.assistant_message, "valid");
        assert!(!parsed.parse_warnings.is_empty());
    }
}
