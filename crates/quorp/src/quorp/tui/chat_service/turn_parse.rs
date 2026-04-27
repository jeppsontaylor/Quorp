use super::*;

pub(crate) fn estimate_token_count(value: &impl serde::Serialize) -> u64 {
    let text = serde_json::to_string(value).unwrap_or_default();
    let char_count = text.chars().count() as u64;
    char_count.div_ceil(4).max(1)
}

#[derive(Debug, Default, Clone)]
pub(crate) struct NativeToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

pub(crate) fn merge_native_tool_call_chunk(
    payload: &serde_json::Value,
    builders: &mut BTreeMap<usize, NativeToolCallBuilder>,
) {
    let Some(choice) = payload
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return;
    };
    let tool_calls = choice
        .get("delta")
        .and_then(|delta| delta.get("tool_calls"))
        .or_else(|| {
            choice
                .get("message")
                .and_then(|message| message.get("tool_calls"))
        })
        .and_then(serde_json::Value::as_array);
    let Some(tool_calls) = tool_calls else {
        return;
    };
    for (ordinal, tool_call) in tool_calls.iter().enumerate() {
        let index = tool_call
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(ordinal);
        let builder = builders.entry(index).or_default();
        if let Some(id) = tool_call.get("id").and_then(serde_json::Value::as_str) {
            builder.id = Some(id.to_string());
        }
        if let Some(name) = tool_call_name(tool_call) {
            builder.name = Some(name.to_string());
        }
        if let Some(arguments) = tool_call_arguments_as_string(tool_call) {
            builder.arguments.push_str(arguments);
        }
    }
}

pub(crate) fn finalized_native_tool_calls(
    builders: &BTreeMap<usize, NativeToolCallBuilder>,
) -> Vec<serde_json::Value> {
    builders
        .values()
        .map(|builder| {
            json!({
                "id": builder.id,
                "type": "function",
                "function": {
                    "name": builder.name,
                    "arguments": builder.arguments,
                }
            })
        })
        .collect()
}

pub(crate) fn native_turn_from_tool_calls(
    assistant_message: &str,
    tool_calls: &[serde_json::Value],
) -> Result<Option<AgentTurnResponse>, String> {
    if tool_calls.is_empty() {
        return Ok(None);
    }
    let mut actions = Vec::new();
    let mut parse_warnings = Vec::new();
    for tool_call in tool_calls {
        match parse_actions_from_tool_call(tool_call) {
            Ok(parsed_actions) => actions.extend(parsed_actions),
            Err(error) => parse_warnings.push(error),
        }
    }
    if actions.is_empty() {
        return Err(parse_warnings.join(" | "));
    }
    Ok(Some(AgentTurnResponse {
        assistant_message: assistant_message.trim().to_string(),
        actions,
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings,
    }))
}

pub(crate) fn native_turn_from_content_fallback(
    content: &str,
) -> Result<Option<AgentTurnResponse>, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    for (assistant_message, candidate) in native_turn_json_candidates(content) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) else {
            continue;
        };
        if let Some(turn) = native_turn_from_json_value(&value, &assistant_message)? {
            return Ok(Some(turn));
        }
    }
    if let Some(turn) = native_turn_from_pseudo_tool_lines(content)? {
        return Ok(Some(turn));
    }
    Ok(None)
}

pub(crate) fn native_turn_from_json_value(
    value: &serde_json::Value,
    assistant_message_fallback: &str,
) -> Result<Option<AgentTurnResponse>, String> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let assistant_message = object
        .get("assistant_message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(assistant_message_fallback);
    if let Some(tool_calls) = object
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        return native_turn_from_tool_calls(assistant_message, tool_calls);
    }
    if tool_call_name(value).is_some() {
        let single_tool_call = vec![value.clone()];
        return native_turn_from_tool_calls(assistant_message, &single_tool_call);
    }
    Ok(None)
}

pub(crate) fn native_turn_json_candidates(content: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let trimmed = content.trim();
    if !trimmed.is_empty() {
        candidates.push((String::new(), trimmed.to_string()));
    }

    let mut search_offset = 0usize;
    while let Some(fence_start) = content[search_offset..].find("```") {
        let fence_start = search_offset + fence_start;
        let language_start = fence_start + 3;
        let Some(first_newline_rel) = content[language_start..].find('\n') else {
            break;
        };
        let body_start = language_start + first_newline_rel + 1;
        let Some(fence_end_rel) = content[body_start..].find("```") else {
            break;
        };
        let fence_end = body_start + fence_end_rel;
        let body = content[body_start..fence_end].trim();
        if !body.is_empty() {
            let assistant_message = content[..fence_start].trim().to_string();
            candidates.push((assistant_message, body.to_string()));
        }
        search_offset = fence_end + 3;
    }

    if let Some(object_text) = first_balanced_json_object(trimmed)
        && object_text != trimmed
    {
        let assistant_message = trimmed
            .split_once(object_text)
            .map(|(prefix, _)| prefix.trim().to_string())
            .unwrap_or_default();
        candidates.push((assistant_message, object_text.to_string()));
    }

    candidates
}

pub(crate) fn first_balanced_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + index + ch.len_utf8();
                    return Some(&text[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn native_turn_from_pseudo_tool_lines(
    content: &str,
) -> Result<Option<AgentTurnResponse>, String> {
    let mut actions = Vec::new();
    let mut parse_warnings = Vec::new();
    let mut assistant_lines = Vec::new();
    let mut seen_signatures = HashSet::new();
    let mut saw_tool_intent = false;
    let lines = content.lines().map(str::trim).collect::<Vec<_>>();

    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if line.is_empty() || line.starts_with("```") {
            index += 1;
            continue;
        }
        if let Some(tool_call) =
            pseudo_tool_call_from_line_pair(line, lines.get(index + 1).copied())
        {
            saw_tool_intent = true;
            let signature = serde_json::to_string(&tool_call).unwrap_or_else(|_| line.to_string());
            if seen_signatures.insert(signature) {
                match parse_actions_from_tool_call(&tool_call) {
                    Ok(parsed_actions) => {
                        for action in parsed_actions {
                            actions.push(action);
                            if actions.len() >= 4 {
                                break;
                            }
                        }
                        if actions.len() >= 4 {
                            break;
                        }
                    }
                    Err(error) => parse_warnings.push(format!("{line}: {error}")),
                }
            }
            index += 2;
            continue;
        }
        if let Some(tool_call) = pseudo_tool_call_from_line(line) {
            saw_tool_intent = true;
            let signature = serde_json::to_string(&tool_call).unwrap_or_else(|_| line.to_string());
            if !seen_signatures.insert(signature) {
                index += 1;
                continue;
            }
            match parse_actions_from_tool_call(&tool_call) {
                Ok(parsed_actions) => {
                    for action in parsed_actions {
                        actions.push(action);
                        if actions.len() >= 4 {
                            break;
                        }
                    }
                    if actions.len() >= 4 {
                        break;
                    }
                }
                Err(error) => parse_warnings.push(format!("{line}: {error}")),
            }
            index += 1;
            continue;
        }
        if !saw_tool_intent && assistant_lines.len() < 2 {
            assistant_lines.push(line.to_string());
        }
        index += 1;
    }

    if actions.is_empty() {
        return Ok(None);
    }

    Ok(Some(AgentTurnResponse {
        assistant_message: assistant_lines.join(" ").trim().to_string(),
        actions,
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings,
    }))
}

pub(crate) fn pseudo_tool_call_from_line_pair(
    line: &str,
    next_line: Option<&str>,
) -> Option<serde_json::Value> {
    let (raw_tool_name, remainder) = split_pseudo_tool_line(line)?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let mut arguments = arguments_object_from_line(next_line?)?;
    if let serde_json::Value::Object(object) = &mut arguments
        && object.get("path").is_none()
        && let Some(path) = pseudo_tool_path_from_remainder(remainder)
    {
        object.insert(
            "path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
    }
    Some(json!({ "tool_name": tool_name, "tool_args": arguments }))
}

pub(crate) fn arguments_object_from_line(line: &str) -> Option<serde_json::Value> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("arguments")
        .or_else(|| trimmed.strip_prefix("Arguments"))?
        .trim_start_matches(|character: char| character == ':' || character.is_whitespace());
    let object_text = first_balanced_json_object(rest)?;
    serde_json::from_str::<serde_json::Value>(object_text)
        .ok()
        .filter(serde_json::Value::is_object)
}

pub(crate) fn pseudo_tool_path_from_remainder(remainder: &str) -> Option<&str> {
    let trimmed = remainder.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = trimmed
        .split_whitespace()
        .next()?
        .trim_matches(|character| matches!(character, '`' | '"' | '\'' | ',' | ':' | '[' | ']'));
    (!candidate.is_empty()).then_some(candidate)
}

pub(crate) fn pseudo_tool_call_from_line(line: &str) -> Option<serde_json::Value> {
    let (raw_tool_name, remainder) = split_pseudo_tool_line(line)?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let fields = extract_named_fields(
        remainder,
        &[
            "path",
            "query",
            "symbol",
            "limit",
            "command",
            "timeout_ms",
            "line",
            "character",
            "old_name",
            "new_name",
            "server_name",
            "uri",
            "cursor",
            "name",
            "arguments",
            "content",
            "patch",
            "search_block",
            "replace_block",
            "fmt",
            "clippy",
            "workspace_tests",
            "tests",
            "custom_commands",
            "args",
            "process_id",
            "tail_lines",
            "stdin",
            "cwd",
            "host",
            "port",
            "timeout_ms",
            "url",
            "headless",
            "width",
            "height",
            "browser_id",
        ],
    );

    match tool_name {
        "read_file" | "list_directory" | "set_executable" => fields
            .get("path")
            .map(|path| json!({ "tool_name": tool_name, "tool_args": { "path": path } })),
        "process_start" => {
            let command = fields.get("command")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "command".to_string(),
                serde_json::Value::String(command.clone()),
            );
            if let Some(args) = fields.get("args")
                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(args)
            {
                tool_args.insert("args".to_string(), parsed);
            }
            if let Some(cwd) = fields.get("cwd") {
                tool_args.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "process_read" => {
            let process_id = fields.get("process_id")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "process_id".to_string(),
                serde_json::Value::String(process_id.clone()),
            );
            if let Some(tail_lines) = fields
                .get("tail_lines")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("tail_lines".to_string(), serde_json::json!(tail_lines));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "process_write" => {
            let process_id = fields.get("process_id")?;
            let stdin = fields.get("stdin")?;
            Some(json!({
                "tool_name": tool_name,
                "tool_args": {
                    "process_id": process_id,
                    "stdin": stdin
                }
            }))
        }
        "process_stop" => fields.get("process_id").map(|process_id| {
            json!({
                "tool_name": tool_name,
                "tool_args": {
                    "process_id": process_id
                }
            })
        }),
        "process_wait_for_port" => {
            let process_id = fields.get("process_id")?;
            let host = fields.get("host")?;
            let port = fields.get("port")?.parse::<u64>().ok()?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "process_id".to_string(),
                serde_json::Value::String(process_id.clone()),
            );
            tool_args.insert("host".to_string(), serde_json::Value::String(host.clone()));
            tool_args.insert("port".to_string(), serde_json::json!(port));
            if let Some(timeout_ms) = fields
                .get("timeout_ms")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "browser_open" => {
            let url = fields.get("url")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert("url".to_string(), serde_json::Value::String(url.clone()));
            if let Some(headless) = fields
                .get("headless")
                .and_then(|value| value.parse::<bool>().ok())
            {
                tool_args.insert("headless".to_string(), serde_json::json!(headless));
            }
            if let Some(width) = fields.get("width").and_then(|value| value.parse::<u64>().ok()) {
                tool_args.insert("width".to_string(), serde_json::json!(width));
            }
            if let Some(height) = fields.get("height").and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("height".to_string(), serde_json::json!(height));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "browser_screenshot" | "browser_console_logs" | "browser_network_errors"
        | "browser_accessibility_snapshot" | "browser_close" => fields
            .get("browser_id")
            .map(|browser_id| json!({ "tool_name": tool_name, "tool_args": { "browser_id": browser_id } })),
        "search_text" | "search_symbols" => {
            let query = fields.get("query")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "query".to_string(),
                serde_json::Value::String(query.clone()),
            );
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "get_repo_capsule" => {
            let mut tool_args = serde_json::Map::new();
            if let Some(query) = fields.get("query") {
                tool_args.insert(
                    "query".to_string(),
                    serde_json::Value::String(query.clone()),
                );
            }
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "lsp_diagnostics" => fields
            .get("path")
            .map(|path| json!({ "tool_name": tool_name, "tool_args": { "path": path } })),
        "lsp_definition" => {
            let path = fields.get("path")?;
            let symbol = fields.get("symbol")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert("path".to_string(), serde_json::Value::String(path.clone()));
            tool_args.insert(
                "symbol".to_string(),
                serde_json::Value::String(symbol.clone()),
            );
            if let Some(line) = fields
                .get("line")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("line".to_string(), serde_json::json!(line));
            }
            if let Some(character) = fields
                .get("character")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("character".to_string(), serde_json::json!(character));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "lsp_references" => {
            let symbol = fields.get("symbol")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "symbol".to_string(),
                serde_json::Value::String(symbol.clone()),
            );
            if let Some(path) = fields.get("path") {
                tool_args.insert("path".to_string(), serde_json::Value::String(path.clone()));
            }
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "lsp_hover" | "lsp_code_actions" => {
            let path = fields.get("path")?;
            let line = fields.get("line")?.parse::<u64>().ok()?;
            let character = fields.get("character")?.parse::<u64>().ok()?;
            Some(json!({
                "tool_name": tool_name,
                "tool_args": {
                    "path": path,
                    "line": line,
                    "character": character
                }
            }))
        }
        "lsp_workspace_symbols" => {
            let query = fields.get("query")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "query".to_string(),
                serde_json::Value::String(query.clone()),
            );
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "lsp_document_symbols" => fields
            .get("path")
            .map(|path| json!({ "tool_name": tool_name, "tool_args": { "path": path } })),
        "lsp_rename_preview" => {
            let path = fields.get("path")?;
            let old_name = fields.get("old_name")?;
            let new_name = fields.get("new_name")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert("path".to_string(), serde_json::Value::String(path.clone()));
            tool_args.insert(
                "old_name".to_string(),
                serde_json::Value::String(old_name.clone()),
            );
            tool_args.insert(
                "new_name".to_string(),
                serde_json::Value::String(new_name.clone()),
            );
            if let Some(limit) = fields
                .get("limit")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("limit".to_string(), serde_json::json!(limit));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "mcp_list_tools" => fields.get("server_name").map(|server_name| {
            json!({
                "tool_name": tool_name,
                "tool_args": {
                    "server_name": server_name
                }
            })
        }),
        "mcp_list_resources" => {
            let server_name = fields.get("server_name")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "server_name".to_string(),
                serde_json::Value::String(server_name.clone()),
            );
            if let Some(cursor) = fields.get("cursor") {
                tool_args.insert("cursor".to_string(), serde_json::Value::String(cursor.clone()));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "mcp_read_resource" => {
            let server_name = fields.get("server_name")?;
            let uri = fields.get("uri")?;
            Some(json!({
                "tool_name": tool_name,
                "tool_args": {
                    "server_name": server_name,
                    "uri": uri
                }
            }))
        }
        "mcp_list_prompts" => {
            let server_name = fields.get("server_name")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "server_name".to_string(),
                serde_json::Value::String(server_name.clone()),
            );
            if let Some(cursor) = fields.get("cursor") {
                tool_args.insert("cursor".to_string(), serde_json::Value::String(cursor.clone()));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "mcp_get_prompt" => {
            let server_name = fields.get("server_name")?;
            let name = fields.get("name")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "server_name".to_string(),
                serde_json::Value::String(server_name.clone()),
            );
            tool_args.insert("name".to_string(), serde_json::Value::String(name.clone()));
            if let Some(arguments) = fields.get("arguments") {
                let parsed = serde_json::from_str::<serde_json::Value>(arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
                tool_args.insert("arguments".to_string(), parsed);
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        "run_command" => {
            let command = fields.get("command")?;
            let mut tool_args = serde_json::Map::new();
            tool_args.insert(
                "command".to_string(),
                serde_json::Value::String(command.clone()),
            );
            if let Some(timeout_ms) = fields
                .get("timeout_ms")
                .and_then(|value| value.parse::<u64>().ok())
            {
                tool_args.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
            }
            Some(json!({ "tool_name": tool_name, "tool_args": tool_args }))
        }
        _ => None,
    }
}

pub(crate) fn split_pseudo_tool_line(line: &str) -> Option<(&str, &str)> {
    [
        "RunCommand",
        "ReadFile",
        "ListDirectory",
        "SearchText",
        "SearchSymbols",
        "GetRepoCapsule",
        "ExplainValidationFailure",
        "SuggestImplementationTargets",
        "SuggestEditAnchors",
        "PreviewEdit",
        "ReplaceRange",
        "ModifyToml",
        "ApplyPreview",
        "WriteFile",
        "ApplyPatch",
        "ReplaceBlock",
        "SetExecutable",
        "ProcessStart",
        "ProcessRead",
        "ProcessWrite",
        "ProcessStop",
        "ProcessWaitForPort",
        "BrowserOpen",
        "BrowserScreenshot",
        "BrowserConsoleLogs",
        "BrowserNetworkErrors",
        "BrowserAccessibilitySnapshot",
        "BrowserClose",
        "RunValidation",
        "LspDiagnostics",
        "LspDefinition",
        "LspReferences",
        "LspHover",
        "LspWorkspaceSymbols",
        "LspDocumentSymbols",
        "LspCodeActions",
        "LspRenamePreview",
        "McpListTools",
        "McpListResources",
        "McpReadResource",
        "McpListPrompts",
        "McpGetPrompt",
    ]
    .into_iter()
    .find_map(|name| {
        let remainder = line.strip_prefix(name)?;
        let boundary = remainder.chars().next()?;
        if boundary.is_whitespace() || boundary == ':' {
            Some((name, remainder.trim()))
        } else {
            None
        }
    })
}

pub(crate) fn extract_named_fields(input: &str, keys: &[&str]) -> HashMap<String, String> {
    let mut markers = Vec::new();
    for key in keys {
        let needle = format!("{key}:");
        let mut search_start = 0usize;
        while let Some(relative_index) = input[search_start..].find(&needle) {
            let index = search_start + relative_index;
            let boundary_ok = index == 0
                || input[..index].chars().last().is_some_and(|character| {
                    character.is_whitespace() || matches!(character, '(' | '[' | '{' | ',' | ';')
                });
            if boundary_ok {
                markers.push((index, *key));
            }
            search_start = index + needle.len();
        }
    }
    markers.sort_by_key(|(index, _)| *index);
    markers.dedup_by_key(|(index, _)| *index);

    let mut fields = HashMap::new();
    for (ordinal, (start, key)) in markers.iter().enumerate() {
        let value_start = start + key.len() + 1;
        let value_end = markers
            .get(ordinal + 1)
            .map(|(next_start, _)| *next_start)
            .unwrap_or_else(|| input.len());
        let value = input[value_start..value_end]
            .trim()
            .trim_end_matches(',')
            .trim()
            .trim_matches('"')
            .trim_matches('`')
            .trim();
        if !value.is_empty() {
            fields.insert((*key).to_string(), value.to_string());
        }
    }
    fields
}

pub(crate) fn parse_actions_from_tool_call(
    tool_call: &serde_json::Value,
) -> Result<Vec<AgentAction>, String> {
    let raw_tool_name = tool_call_name(tool_call)
        .ok_or_else(|| "native tool call was missing a tool name".to_string())?;
    let tool_name = normalize_tool_name(raw_tool_name);
    let argument_values = tool_call_argument_values(tool_call, tool_name)?;
    let mut actions = Vec::with_capacity(argument_values.len());
    for arguments in &argument_values {
        actions.push(parse_action_from_arguments(tool_name, arguments)?);
    }
    Ok(actions)
}

pub(crate) fn parse_action_from_arguments(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<AgentAction, String> {
    match tool_name {
        "run_command" => Ok(AgentAction::RunCommand {
            command: required_string_argument(arguments, tool_name, "command")?,
            timeout_ms: optional_u64_argument(arguments, "timeout_ms").unwrap_or(30_000),
        }),
        "read_file" => Ok(AgentAction::ReadFile {
            path: required_string_argument(arguments, tool_name, "path")?,
            range: optional_read_file_range_argument(arguments, "range"),
        }),
        "list_directory" => Ok(AgentAction::ListDirectory {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "search_text" => Ok(AgentAction::SearchText {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "search_symbols" => Ok(AgentAction::SearchSymbols {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "find_files" => Ok(AgentAction::FindFiles {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(12),
        }),
        "structural_search" => Ok(AgentAction::StructuralSearch {
            pattern: required_string_argument(arguments, tool_name, "pattern")?,
            language: optional_string_argument(arguments, "language"),
            path: optional_string_argument(arguments, "path"),
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "structural_edit_preview" => Ok(AgentAction::StructuralEditPreview {
            pattern: required_string_argument(arguments, tool_name, "pattern")?,
            rewrite: required_string_argument(arguments, tool_name, "rewrite")?,
            language: optional_string_argument(arguments, "language"),
            path: optional_string_argument(arguments, "path"),
        }),
        "cargo_diagnostics" => Ok(AgentAction::CargoDiagnostics {
            command: optional_string_argument(arguments, "command"),
            include_clippy: optional_bool_argument(arguments, "include_clippy").unwrap_or(false),
        }),
        "lsp_diagnostics" => Ok(AgentAction::LspDiagnostics {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "lsp_definition" => Ok(AgentAction::LspDefinition {
            path: required_string_argument(arguments, tool_name, "path")?,
            symbol: required_string_argument(arguments, tool_name, "symbol")?,
            line: optional_usize_argument(arguments, "line"),
            character: optional_usize_argument(arguments, "character"),
        }),
        "lsp_references" => Ok(AgentAction::LspReferences {
            path: optional_string_argument(arguments, "path"),
            symbol: required_string_argument(arguments, tool_name, "symbol")?,
            line: optional_usize_argument(arguments, "line"),
            character: optional_usize_argument(arguments, "character"),
            limit: optional_usize_argument(arguments, "limit").unwrap_or(32),
        }),
        "lsp_hover" => Ok(AgentAction::LspHover {
            path: required_string_argument(arguments, tool_name, "path")?,
            line: required_usize_argument(arguments, tool_name, "line")?,
            character: required_usize_argument(arguments, tool_name, "character")?,
        }),
        "lsp_workspace_symbols" => Ok(AgentAction::LspWorkspaceSymbols {
            query: required_string_argument(arguments, tool_name, "query")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(32),
        }),
        "lsp_document_symbols" => Ok(AgentAction::LspDocumentSymbols {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "lsp_code_actions" => Ok(AgentAction::LspCodeActions {
            path: required_string_argument(arguments, tool_name, "path")?,
            line: required_usize_argument(arguments, tool_name, "line")?,
            character: required_usize_argument(arguments, tool_name, "character")?,
        }),
        "lsp_rename_preview" => Ok(AgentAction::LspRenamePreview {
            path: required_string_argument(arguments, tool_name, "path")?,
            old_name: required_string_argument(arguments, tool_name, "old_name")?,
            new_name: required_string_argument(arguments, tool_name, "new_name")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(64),
        }),
        "mcp_list_tools" => Ok(AgentAction::McpListTools {
            server_name: required_string_argument(arguments, tool_name, "server_name")?,
        }),
        "mcp_list_resources" => Ok(AgentAction::McpListResources {
            server_name: required_string_argument(arguments, tool_name, "server_name")?,
            cursor: optional_string_argument(arguments, "cursor"),
        }),
        "mcp_read_resource" => Ok(AgentAction::McpReadResource {
            server_name: required_string_argument(arguments, tool_name, "server_name")?,
            uri: required_string_argument(arguments, tool_name, "uri")?,
        }),
        "mcp_list_prompts" => Ok(AgentAction::McpListPrompts {
            server_name: required_string_argument(arguments, tool_name, "server_name")?,
            cursor: optional_string_argument(arguments, "cursor"),
        }),
        "mcp_get_prompt" => Ok(AgentAction::McpGetPrompt {
            server_name: required_string_argument(arguments, tool_name, "server_name")?,
            name: required_string_argument(arguments, tool_name, "name")?,
            arguments: optional_json_argument(arguments, "arguments"),
        }),
        "get_repo_capsule" => Ok(AgentAction::GetRepoCapsule {
            query: optional_string_argument(arguments, "query"),
            limit: optional_usize_argument(arguments, "limit").unwrap_or(8),
        }),
        "explain_validation_failure" => Ok(AgentAction::ExplainValidationFailure {
            command: required_string_argument(arguments, tool_name, "command")?,
            output: required_string_argument(arguments, tool_name, "output")?,
        }),
        "suggest_implementation_targets" => Ok(AgentAction::SuggestImplementationTargets {
            command: required_string_argument(arguments, tool_name, "command")?,
            output: required_string_argument(arguments, tool_name, "output")?,
            failing_path: optional_string_argument(arguments, "failing_path"),
            failing_line: optional_usize_argument(arguments, "failing_line"),
        }),
        "suggest_edit_anchors" => Ok(AgentAction::SuggestEditAnchors {
            path: required_string_argument(arguments, tool_name, "path")?,
            range: optional_read_file_range_argument(arguments, "range"),
            search_hint: optional_string_argument(arguments, "search_hint"),
        }),
        "preview_edit" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            match preview_edit_payload_argument(arguments) {
                Ok(edit) => Ok(AgentAction::PreviewEdit { path, edit }),
                Err(error) => {
                    if let Some(range) = preview_edit_hash_prerequisite_range(arguments) {
                        Ok(AgentAction::ReadFile { path, range })
                    } else {
                        Err(error)
                    }
                }
            }
        }
        "replace_range" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            let range = required_read_file_range_argument(arguments, tool_name, "range")?;
            let Some(expected_hash) = optional_string_argument_alias(
                arguments,
                &["expected_hash", "content_hash", "hash"],
            ) else {
                return Ok(AgentAction::ReadFile {
                    path,
                    range: Some(range),
                });
            };
            Ok(AgentAction::ReplaceRange {
                path,
                range,
                expected_hash,
                replacement: required_string_argument(arguments, tool_name, "replacement")?,
            })
        }
        "modify_toml" => {
            let path = required_string_argument(arguments, tool_name, "path")?;
            let Some(expected_hash) = optional_string_argument_alias(
                arguments,
                &["expected_hash", "content_hash", "hash"],
            ) else {
                return Ok(AgentAction::ReadFile { path, range: None });
            };
            Ok(AgentAction::ModifyToml {
                path,
                expected_hash,
                operations: toml_operations_argument(arguments, tool_name)?,
            })
        }
        "apply_preview" => Ok(AgentAction::ApplyPreview {
            preview_id: required_string_argument(arguments, tool_name, "preview_id")?,
        }),
        "write_file" => Ok(AgentAction::WriteFile {
            path: required_string_argument(arguments, tool_name, "path")?,
            content: required_string_argument(arguments, tool_name, "content")?,
        }),
        "apply_patch" => Ok(AgentAction::ApplyPatch {
            path: required_string_argument(arguments, tool_name, "path")?,
            patch: required_string_argument(arguments, tool_name, "patch")?,
        }),
        "replace_block" => Ok(AgentAction::ReplaceBlock {
            path: required_string_argument(arguments, tool_name, "path")?,
            search_block: required_string_argument(arguments, tool_name, "search_block")?,
            replace_block: required_string_argument(arguments, tool_name, "replace_block")?,
            range: optional_read_file_range_argument(arguments, "range"),
        }),
        "set_executable" => Ok(AgentAction::SetExecutable {
            path: required_string_argument(arguments, tool_name, "path")?,
        }),
        "process_start" => Ok(AgentAction::ProcessStart {
            command: required_string_argument(arguments, tool_name, "command")?,
            args: optional_string_list_argument(arguments, "args"),
            cwd: optional_string_argument(arguments, "cwd"),
        }),
        "process_read" => Ok(AgentAction::ProcessRead {
            process_id: required_string_argument(arguments, tool_name, "process_id")?,
            tail_lines: optional_usize_argument(arguments, "tail_lines").unwrap_or(200),
        }),
        "process_write" => Ok(AgentAction::ProcessWrite {
            process_id: required_string_argument(arguments, tool_name, "process_id")?,
            stdin: required_string_argument(arguments, tool_name, "stdin")?,
        }),
        "process_stop" => Ok(AgentAction::ProcessStop {
            process_id: required_string_argument(arguments, tool_name, "process_id")?,
        }),
        "process_wait_for_port" => Ok(AgentAction::ProcessWaitForPort {
            process_id: required_string_argument(arguments, tool_name, "process_id")?,
            host: required_string_argument(arguments, tool_name, "host")?,
            port: required_u16_argument(arguments, tool_name, "port")?,
            timeout_ms: optional_u64_argument(arguments, "timeout_ms").unwrap_or(60_000),
        }),
        "browser_open" => Ok(AgentAction::BrowserOpen {
            url: required_string_argument(arguments, tool_name, "url")?,
            headless: optional_bool_argument(arguments, "headless").unwrap_or(false),
            width: optional_u32_argument(arguments, "width"),
            height: optional_u32_argument(arguments, "height"),
        }),
        "browser_screenshot" => Ok(AgentAction::BrowserScreenshot {
            browser_id: required_string_argument(arguments, tool_name, "browser_id")?,
        }),
        "browser_console_logs" => Ok(AgentAction::BrowserConsoleLogs {
            browser_id: required_string_argument(arguments, tool_name, "browser_id")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(100),
        }),
        "browser_network_errors" => Ok(AgentAction::BrowserNetworkErrors {
            browser_id: required_string_argument(arguments, tool_name, "browser_id")?,
            limit: optional_usize_argument(arguments, "limit").unwrap_or(100),
        }),
        "browser_accessibility_snapshot" => Ok(AgentAction::BrowserAccessibilitySnapshot {
            browser_id: required_string_argument(arguments, tool_name, "browser_id")?,
        }),
        "browser_close" => Ok(AgentAction::BrowserClose {
            browser_id: required_string_argument(arguments, tool_name, "browser_id")?,
        }),
        "run_validation" => Ok(AgentAction::RunValidation {
            plan: ValidationPlan {
                fmt: optional_bool_argument(arguments, "fmt").unwrap_or(false),
                clippy: optional_bool_argument(arguments, "clippy").unwrap_or(false),
                workspace_tests: optional_bool_argument(arguments, "workspace_tests")
                    .unwrap_or(false),
                tests: optional_string_list_argument(arguments, "tests"),
                custom_commands: optional_string_list_argument(arguments, "custom_commands"),
            },
        }),
        other => Err(format!("unsupported native tool call `{other}`")),
    }
}

pub(crate) fn tool_call_name(tool_call: &serde_json::Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            tool_call
                .get("tool_name")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| tool_call.get("name").and_then(serde_json::Value::as_str))
        .or_else(|| tool_call.get("type").and_then(serde_json::Value::as_str))
}

pub(crate) fn tool_call_argument_values(
    tool_call: &serde_json::Value,
    tool_name: &str,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(raw_arguments) = tool_call_arguments_as_string(tool_call) {
        return parse_tool_argument_values(tool_name, raw_arguments);
    }
    if let Some(arguments) = tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .or_else(|| tool_call.get("tool_args"))
        .or_else(|| tool_call.get("arguments"))
        .cloned()
    {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }
    if let Some(arguments) = tool_call_inline_arguments(tool_call) {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }
    Ok(vec![json!({})])
}

pub(crate) fn tool_call_arguments_as_string(tool_call: &serde_json::Value) -> Option<&str> {
    tool_call
        .get("function")
        .and_then(|function| function.get("arguments"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            tool_call
                .get("arguments")
                .and_then(serde_json::Value::as_str)
        })
}

pub(crate) fn tool_call_inline_arguments(
    tool_call: &serde_json::Value,
) -> Option<serde_json::Value> {
    let object = tool_call.as_object()?;
    let arguments = object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "id" | "index"
                    | "type"
                    | "name"
                    | "function"
                    | "arguments"
                    | "tool_name"
                    | "tool_args"
                    | "tool_call_id"
                    | "tool_call_type"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<String, serde_json::Value>>();
    (!arguments.is_empty()).then_some(serde_json::Value::Object(arguments))
}

pub(crate) fn normalize_tool_name(raw: &str) -> &str {
    match raw {
        "RunCommand" => "run_command",
        "ReadFile" => "read_file",
        "ListDirectory" => "list_directory",
        "SearchText" => "search_text",
        "SearchSymbols" => "search_symbols",
        "FindFiles" => "find_files",
        "StructuralSearch" => "structural_search",
        "StructuralEditPreview" => "structural_edit_preview",
        "CargoDiagnostics" => "cargo_diagnostics",
        "LspDiagnostics" => "lsp_diagnostics",
        "LspDefinition" => "lsp_definition",
        "LspReferences" => "lsp_references",
        "LspHover" => "lsp_hover",
        "LspWorkspaceSymbols" => "lsp_workspace_symbols",
        "LspDocumentSymbols" => "lsp_document_symbols",
        "LspCodeActions" => "lsp_code_actions",
        "LspRenamePreview" => "lsp_rename_preview",
        "McpListTools" => "mcp_list_tools",
        "McpListResources" => "mcp_list_resources",
        "McpReadResource" => "mcp_read_resource",
        "McpListPrompts" => "mcp_list_prompts",
        "McpGetPrompt" => "mcp_get_prompt",
        "GetRepoCapsule" => "get_repo_capsule",
        "ExplainValidationFailure" => "explain_validation_failure",
        "SuggestImplementationTargets" => "suggest_implementation_targets",
        "SuggestEditAnchors" => "suggest_edit_anchors",
        "PreviewEdit" => "preview_edit",
        "ReplaceRange" => "replace_range",
        "ModifyToml" => "modify_toml",
        "ApplyPreview" => "apply_preview",
        "WriteFile" => "write_file",
        "ApplyPatch" => "apply_patch",
        "ReplaceBlock" => "replace_block",
        "SetExecutable" => "set_executable",
        "ProcessStart" => "process_start",
        "ProcessRead" => "process_read",
        "ProcessWrite" => "process_write",
        "ProcessStop" => "process_stop",
        "ProcessWaitForPort" => "process_wait_for_port",
        "BrowserOpen" => "browser_open",
        "BrowserScreenshot" => "browser_screenshot",
        "BrowserConsoleLogs" => "browser_console_logs",
        "BrowserNetworkErrors" => "browser_network_errors",
        "BrowserAccessibilitySnapshot" => "browser_accessibility_snapshot",
        "BrowserClose" => "browser_close",
        "McpCallTool" => "mcp_call_tool",
        "RunValidation" => "run_validation",
        other => other,
    }
}

pub(crate) fn parse_tool_argument_values(
    tool_name: &str,
    raw_arguments: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let trimmed = raw_arguments.trim();
    let normalized = if trimmed.is_empty() { "{}" } else { trimmed };
    if let Ok(arguments) = serde_json::from_str::<serde_json::Value>(normalized) {
        return normalize_tool_argument_values(tool_name, vec![arguments]);
    }

    let mut arguments = Vec::new();
    for value in serde_json::Deserializer::from_str(normalized).into_iter::<serde_json::Value>() {
        let value = value.map_err(|error| {
            format!("native tool `{tool_name}` arguments were invalid JSON: {error}")
        })?;
        arguments.push(value);
    }
    if arguments.is_empty() {
        return Err(format!(
            "native tool `{tool_name}` arguments were invalid JSON: empty payload"
        ));
    }
    normalize_tool_argument_values(tool_name, arguments)
}

pub(crate) fn normalize_tool_argument_values(
    tool_name: &str,
    raw_values: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut normalized = Vec::new();
    for value in raw_values {
        match value {
            serde_json::Value::Object(_) => normalized.push(value),
            serde_json::Value::Array(items) => {
                for item in items {
                    if item.is_object() {
                        normalized.push(item);
                    } else {
                        return Err(format!(
                            "native tool `{tool_name}` arguments must be JSON objects"
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "native tool `{tool_name}` arguments must be JSON objects"
                ));
            }
        }
    }
    if normalized.is_empty() {
        normalized.push(json!({}));
    }
    Ok(normalized)
}

pub(crate) fn required_string_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<String, String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing `{field}`"))
}

pub(crate) fn optional_string_argument(
    arguments: &serde_json::Value,
    field: &str,
) -> Option<String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn optional_string_argument_alias(
    arguments: &serde_json::Value,
    fields: &[&str],
) -> Option<String> {
    fields
        .iter()
        .filter_map(|field| optional_string_argument(arguments, field))
        .next()
}

pub(crate) fn preview_edit_hash_prerequisite_range(
    arguments: &serde_json::Value,
) -> Option<Option<ReadFileRange>> {
    if let Some(edit_object) = arguments.get("edit").and_then(serde_json::Value::as_object) {
        if let Some(modify_toml) = edit_object
            .get("ModifyToml")
            .or_else(|| edit_object.get("modify_toml"))
            .and_then(serde_json::Value::as_object)
            && string_argument_alias(modify_toml, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            return Some(None);
        }
        if let Some(replace_range) = edit_object
            .get("ReplaceRange")
            .or_else(|| edit_object.get("replace_range"))
            .and_then(serde_json::Value::as_object)
            && string_argument_alias(replace_range, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            let value = serde_json::Value::Object(replace_range.clone());
            return Some(optional_read_file_range_argument(&value, "range"));
        }
        if edit_object.get("operations").is_some()
            && string_argument_alias(edit_object, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            return Some(None);
        }
        if edit_object.get("replacement").is_some()
            && edit_object.get("range").is_some()
            && string_argument_alias(edit_object, &["expected_hash", "content_hash", "hash"])
                .is_none()
        {
            let value = serde_json::Value::Object(edit_object.clone());
            return Some(optional_read_file_range_argument(&value, "range"));
        }
    }
    if arguments.get("operations").is_some()
        && optional_string_argument_alias(arguments, &["expected_hash", "content_hash", "hash"])
            .is_none()
    {
        return Some(None);
    }
    if arguments.get("replacement").is_some()
        && arguments.get("range").is_some()
        && optional_string_argument_alias(arguments, &["expected_hash", "content_hash", "hash"])
            .is_none()
    {
        return Some(optional_read_file_range_argument(arguments, "range"));
    }
    None
}

pub(crate) fn optional_read_file_range_argument(
    arguments: &serde_json::Value,
    field: &str,
) -> Option<ReadFileRange> {
    let value = arguments.get(field)?;
    match value {
        serde_json::Value::Array(values) if values.len() == 2 => {
            let start_line = values.first()?.as_u64()? as usize;
            let end_line = values.get(1)?.as_u64()? as usize;
            ReadFileRange {
                start_line,
                end_line,
            }
            .normalized()
        }
        serde_json::Value::Object(map) => {
            let start_line = map.get("start_line")?.as_u64()? as usize;
            let end_line = map.get("end_line")?.as_u64()? as usize;
            ReadFileRange {
                start_line,
                end_line,
            }
            .normalized()
        }
        _ => None,
    }
}

pub(crate) fn required_read_file_range_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<ReadFileRange, String> {
    optional_read_file_range_argument(arguments, field)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing valid `{field}`"))
}

pub(crate) fn toml_operations_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
) -> Result<Vec<quorp_agent_core::agent_protocol::TomlEditOperation>, String> {
    let operations = arguments
        .get("operations")
        .ok_or_else(|| format!("native tool `{tool_name}` was missing `operations`"))?;
    let operations = normalize_toml_operations_value(operations.clone())
        .ok_or_else(|| format!("native tool `{tool_name}` had invalid `operations`"))?;
    serde_json::from_value::<Vec<quorp_agent_core::agent_protocol::TomlEditOperation>>(operations)
        .map_err(|error| format!("native tool `{tool_name}` had invalid `operations`: {error}"))
}

pub(crate) fn normalize_toml_operations_value(
    value: serde_json::Value,
) -> Option<serde_json::Value> {
    let serde_json::Value::Array(items) = value else {
        return None;
    };
    let normalized = items
        .into_iter()
        .map(normalize_toml_operation_value)
        .collect::<Option<Vec<_>>>()?;
    Some(serde_json::Value::Array(normalized))
}

pub(crate) fn normalize_toml_operation_value(
    value: serde_json::Value,
) -> Option<serde_json::Value> {
    if serde_json::from_value::<quorp_agent_core::agent_protocol::TomlEditOperation>(value.clone())
        .is_ok()
    {
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

pub(crate) fn normalize_toml_operation_aliases(
    object: &mut serde_json::Map<String, serde_json::Value>,
) {
    if object.get("name").is_none() {
        for alias in [
            "dependency",
            "dependency_name",
            "crate",
            "crate_name",
            "package_name",
        ] {
            if let Some(value) = object.get(alias).cloned() {
                object.insert("name".to_string(), value);
                break;
            }
        }
    }
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
    if object.get("version").is_none()
        && let Some(value) = object.get("spec").cloned()
    {
        object.insert("version".to_string(), value);
    }
}

pub(crate) fn normalize_toml_dependency_map_shorthand(
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

pub(crate) fn is_toml_operation_field(field: &str) -> bool {
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

pub(crate) fn preview_edit_payload_argument(
    arguments: &serde_json::Value,
) -> Result<PreviewEditPayload, String> {
    if let Some(edit) = arguments.get("edit")
        && let Ok(payload) = serde_json::from_value::<PreviewEditPayload>(edit.clone())
    {
        return Ok(payload);
    }
    if let Some(edit_object) = arguments.get("edit").and_then(serde_json::Value::as_object) {
        if let Some(apply_patch) = edit_object
            .get("ApplyPatch")
            .or_else(|| edit_object.get("apply_patch"))
        {
            let patch = apply_patch
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "native tool `preview_edit` edit.apply_patch was missing `patch`".to_string()
                })?;
            return Ok(PreviewEditPayload::ApplyPatch {
                patch: patch.to_string(),
            });
        }
        if let Some(replace_range) = edit_object
            .get("ReplaceRange")
            .or_else(|| edit_object.get("replace_range"))
        {
            return Ok(PreviewEditPayload::ReplaceRange {
                range: required_read_file_range_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "range",
                )?,
                expected_hash: required_string_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "expected_hash",
                )?,
                replacement: required_string_argument(
                    replace_range,
                    "preview_edit.replace_range",
                    "replacement",
                )?,
            });
        }
        if let Some(modify_toml) = edit_object
            .get("ModifyToml")
            .or_else(|| edit_object.get("modify_toml"))
        {
            return Ok(PreviewEditPayload::ModifyToml {
                expected_hash: required_string_argument(
                    modify_toml,
                    "preview_edit.modify_toml",
                    "expected_hash",
                )?,
                operations: toml_operations_argument(modify_toml, "preview_edit.modify_toml")?,
            });
        }
        if let Some(replace_block) = edit_object
            .get("ReplaceBlock")
            .or_else(|| edit_object.get("replace_block"))
        {
            return Ok(PreviewEditPayload::ReplaceBlock {
                search_block: required_string_argument(
                    replace_block,
                    "preview_edit.replace_block",
                    "search_block",
                )?,
                replace_block: required_string_argument(
                    replace_block,
                    "preview_edit.replace_block",
                    "replace_block",
                )?,
                range: optional_read_file_range_argument(replace_block, "range"),
            });
        }
        let edit_kind = edit_object
            .get("kind")
            .or_else(|| edit_object.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"));
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "applypatch" | "apply_patch" | "patch"))
            || edit_object.get("patch").is_some()
        {
            let patch = edit_object
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "native tool `preview_edit` edit was missing `patch`".to_string())?;
            return Ok(PreviewEditPayload::ApplyPatch {
                patch: patch.to_string(),
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "replaceblock" | "replace_block" | "replace"))
            || edit_object.get("search_block").is_some()
            || edit_object.get("search").is_some()
        {
            let search_block =
                string_argument_alias(edit_object, &["search_block", "search", "old"]).ok_or_else(
                    || "native tool `preview_edit` edit was missing `search_block`".to_string(),
                )?;
            let replace_block = string_argument_alias(
                edit_object,
                &[
                    "replace_block",
                    "replace_with",
                    "replacement",
                    "replace",
                    "new",
                ],
            )
            .ok_or_else(|| {
                "native tool `preview_edit` edit was missing `replace_block`".to_string()
            })?;
            return Ok(PreviewEditPayload::ReplaceBlock {
                search_block,
                replace_block,
                range: {
                    let edit_value = serde_json::Value::Object(edit_object.clone());
                    optional_read_file_range_argument(&edit_value, "range")
                        .or_else(|| optional_read_file_range_argument(&edit_value, "line_range"))
                },
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "replacerange" | "replace_range"))
            || edit_object.get("replacement").is_some()
                && edit_object.get("expected_hash").is_some()
                && edit_object.get("range").is_some()
        {
            let edit_value = serde_json::Value::Object(edit_object.clone());
            return Ok(PreviewEditPayload::ReplaceRange {
                range: required_read_file_range_argument(
                    &edit_value,
                    "preview_edit.replace_range",
                    "range",
                )?,
                expected_hash: string_argument_alias(
                    edit_object,
                    &["expected_hash", "content_hash", "hash"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `expected_hash`".to_string()
                })?,
                replacement: string_argument_alias(
                    edit_object,
                    &["replacement", "replace_with", "replace", "new", "content"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `replacement`".to_string()
                })?,
            });
        }
        if edit_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "modifytoml" | "modify_toml" | "toml"))
            || edit_object.get("operations").is_some() && edit_object.get("expected_hash").is_some()
        {
            let edit_value = serde_json::Value::Object(edit_object.clone());
            return Ok(PreviewEditPayload::ModifyToml {
                expected_hash: string_argument_alias(
                    edit_object,
                    &["expected_hash", "content_hash", "hash"],
                )
                .ok_or_else(|| {
                    "native tool `preview_edit` edit was missing `expected_hash`".to_string()
                })?,
                operations: toml_operations_argument(&edit_value, "preview_edit.modify_toml")?,
            });
        }
    }
    if let Some(patch) = optional_string_argument(arguments, "patch") {
        return Ok(PreviewEditPayload::ApplyPatch { patch });
    }
    if arguments.get("operations").is_some()
        && optional_string_argument(arguments, "expected_hash").is_some()
    {
        return Ok(PreviewEditPayload::ModifyToml {
            expected_hash: required_string_argument(arguments, "preview_edit", "expected_hash")?,
            operations: toml_operations_argument(arguments, "preview_edit")?,
        });
    }
    if optional_read_file_range_argument(arguments, "range").is_some()
        && optional_string_argument(arguments, "expected_hash").is_some()
        && optional_string_argument(arguments, "replacement").is_some()
    {
        return Ok(PreviewEditPayload::ReplaceRange {
            range: required_read_file_range_argument(arguments, "preview_edit", "range")?,
            expected_hash: required_string_argument(arguments, "preview_edit", "expected_hash")?,
            replacement: required_string_argument(arguments, "preview_edit", "replacement")?,
        });
    }
    let object = arguments
        .as_object()
        .ok_or_else(|| "native tool `preview_edit` arguments must be a JSON object".to_string())?;
    let search_block = string_argument_alias(object, &["search_block", "search", "old"])
        .ok_or_else(|| "native tool `preview_edit` was missing `edit`".to_string())?;
    let replace_block = string_argument_alias(
        object,
        &[
            "replace_block",
            "replace_with",
            "replacement",
            "replace",
            "new",
        ],
    )
    .ok_or_else(|| "native tool `preview_edit` was missing `replace_block`".to_string())?;
    Ok(PreviewEditPayload::ReplaceBlock {
        search_block,
        replace_block,
        range: optional_read_file_range_argument(arguments, "range")
            .or_else(|| optional_read_file_range_argument(arguments, "line_range")),
    })
}

pub(crate) fn string_argument_alias(
    object: &serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
) -> Option<String> {
    fields
        .iter()
        .filter_map(|field| object.get(*field))
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .next()
}

pub(crate) fn optional_string_list_argument(
    arguments: &serde_json::Value,
    field: &str,
) -> Vec<String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn optional_u64_argument(arguments: &serde_json::Value, field: &str) -> Option<u64> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            arguments
                .get(field)
                .and_then(serde_json::Value::as_i64)
                .map(|value| value.max(0) as u64)
        })
}

pub(crate) fn optional_usize_argument(arguments: &serde_json::Value, field: &str) -> Option<usize> {
    optional_u64_argument(arguments, field).map(|value| value as usize)
}

pub(crate) fn required_usize_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<usize, String> {
    optional_usize_argument(arguments, field)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing `{field}`"))
}

pub(crate) fn optional_u32_argument(arguments: &serde_json::Value, field: &str) -> Option<u32> {
    optional_u64_argument(arguments, field).map(|value| value as u32)
}

pub(crate) fn required_u16_argument(
    arguments: &serde_json::Value,
    tool_name: &str,
    field: &str,
) -> Result<u16, String> {
    optional_u64_argument(arguments, field)
        .filter(|value| *value > 0 && *value <= u16::MAX as u64)
        .map(|value| value as u16)
        .ok_or_else(|| format!("native tool `{tool_name}` was missing valid `{field}`"))
}

pub(crate) fn optional_bool_argument(arguments: &serde_json::Value, field: &str) -> Option<bool> {
    arguments.get(field).and_then(serde_json::Value::as_bool)
}

pub(crate) fn optional_json_argument(
    arguments: &serde_json::Value,
    field: &str,
) -> Option<serde_json::Value> {
    let value = arguments.get(field)?;
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .or_else(|| Some(serde_json::Value::String(text.clone()))),
        other => Some(other.clone()),
    }
}

pub(crate) fn parse_usage_payload(
    payload: &serde_json::Value,
    latency_ms: u64,
    finish_reason: Option<String>,
    provider_request_id: Option<&str>,
) -> Result<quorp_agent_core::TokenUsage, String> {
    let usage_u64 = |paths: &[&[&str]]| {
        paths.iter().find_map(|path| {
            let mut current = payload;
            for key in *path {
                current = current.get(*key)?;
            }
            current
                .as_u64()
                .or_else(|| current.as_i64().map(|value| value.max(0) as u64))
                .or_else(|| current.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
    };
    let input_tokens = usage_u64(&[&["prompt_tokens"], &["input_tokens"]]).unwrap_or_default();
    let output_tokens =
        usage_u64(&[&["completion_tokens"], &["output_tokens"]]).unwrap_or_default();
    let total_billed_tokens =
        usage_u64(&[&["total_tokens"]]).unwrap_or(input_tokens.saturating_add(output_tokens));
    Ok(quorp_agent_core::TokenUsage {
        input_tokens,
        output_tokens,
        total_billed_tokens,
        reasoning_tokens: usage_u64(&[
            &["reasoning_tokens"],
            &["output_tokens_details", "reasoning_tokens"],
            &["completion_tokens_details", "reasoning_tokens"],
        ]),
        cache_read_input_tokens: usage_u64(&[
            &["cache_read_input_tokens"],
            &["input_tokens_details", "cached_tokens"],
            &["prompt_tokens_details", "cached_tokens"],
        ]),
        cache_write_input_tokens: usage_u64(&[
            &["cache_write_input_tokens"],
            &["input_tokens_details", "cache_write_tokens"],
            &["prompt_tokens_details", "cache_write_tokens"],
        ]),
        provider_request_id: provider_request_id.map(str::to_string).or_else(|| {
            payload
                .get("provider_request_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        latency_ms,
        finish_reason,
        usage_source: quorp_agent_core::UsageSource::Reported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsp_definition_tool_call() {
        let action = parse_action_from_arguments(
            "lsp_definition",
            &serde_json::json!({
                "path": "src/lib.rs",
                "symbol": "Example",
                "line": 12,
                "character": 4
            }),
        )
        .expect("action");
        assert!(matches!(
            action,
            AgentAction::LspDefinition {
                path,
                symbol,
                line: Some(12),
                character: Some(4)
            } if path == "src/lib.rs" && symbol == "Example"
        ));
    }

    #[test]
    fn parses_pseudo_lsp_hover_line() {
        let call = pseudo_tool_call_from_line("LspHover path: src/lib.rs line: 10 character: 3")
            .expect("tool call");
        let actions = parse_actions_from_tool_call(&call).expect("actions");
        assert!(matches!(
            actions.as_slice(),
            [AgentAction::LspHover {
                path,
                line: 10,
                character: 3
            }] if path == "src/lib.rs"
        ));
    }

    #[test]
    fn parses_pseudo_process_start_line() {
        let call = pseudo_tool_call_from_line(
            "ProcessStart command: cargo args: [\"test\", \"--quiet\"] cwd: /tmp/project",
        )
        .expect("tool call");
        let actions = parse_actions_from_tool_call(&call).expect("actions");
        assert!(matches!(
            actions.as_slice(),
            [AgentAction::ProcessStart {
                command,
                args,
                cwd: Some(cwd)
            }] if command == "cargo"
                && matches!(args.as_slice(), [first, second] if first == "test" && second == "--quiet")
                && cwd == "/tmp/project"
        ));
    }

    #[test]
    fn parses_pseudo_browser_open_line() {
        let call = pseudo_tool_call_from_line(
            "BrowserOpen url: https://example.com headless: true width: 1280 height: 720",
        )
        .expect("tool call");
        let actions = parse_actions_from_tool_call(&call).expect("actions");
        assert!(matches!(
            actions.as_slice(),
            [AgentAction::BrowserOpen {
                url,
                headless: true,
                width: Some(1280),
                height: Some(720)
            }] if url == "https://example.com"
        ));
    }
}
