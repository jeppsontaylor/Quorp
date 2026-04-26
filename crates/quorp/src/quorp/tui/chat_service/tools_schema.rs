use super::*;

pub(crate) fn native_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        function_tool(
            "run_command",
            "Run an allowlisted shell command inside the workspace.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1000 }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "read_file",
            "Read a workspace-relative file. Optionally request a focused inclusive line range [start_line, end_line].",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "list_directory",
            "List a workspace-relative directory.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "search_text",
            "Search repo text for a literal query.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "search_symbols",
            "Search indexed symbols before guessing file paths.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "find_files",
            "Find repo files by name or path using the configured fd integration when available.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 64 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "structural_search",
            "Run syntax-aware structural search with ast-grep. Prefer this for Rust constructs when regex would be brittle.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "language": { "type": "string" },
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "structural_edit_preview",
            "Dry-run an ast-grep structural rewrite. This never mutates files; apply the returned preview_id with apply_preview if the preview is correct.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "rewrite": { "type": "string" },
                    "language": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["pattern", "rewrite"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "cargo_diagnostics",
            "Run configured Cargo JSON diagnostics and return compact compiler errors with file/line anchors.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "include_clippy": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        function_tool(
            "get_repo_capsule",
            "Fetch a compact repository capsule for orientation.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 32 }
                },
                "additionalProperties": false
            }),
        ),
        function_tool(
            "explain_validation_failure",
            "Summarize observed validation output into failing tests, excerpts, and file/line anchors. This is read-only and never proposes a gold patch.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "output": { "type": "string" }
                },
                "required": ["command", "output"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "suggest_implementation_targets",
            "Rank likely implementation repair targets from observed validation output and an optional failing location. This is read-only and never proposes code changes.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "output": { "type": "string" },
                    "failing_path": { "type": "string" },
                    "failing_line": { "type": "integer", "minimum": 1 }
                },
                "required": ["command", "output"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "suggest_edit_anchors",
            "Inspect a focused file region and return unique edit anchors plus safe ApplyPatch/ranged ReplaceBlock scaffolds. This is read-only.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    },
                    "search_hint": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "preview_edit",
            "Dry-run an edit intent against a workspace-relative file. This is read-only and never mutates files. Successful previews return a preview_id that can be applied with apply_preview.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "edit": {
                        "type": "object",
                        "properties": {
                            "apply_patch": {
                                "type": "object",
                                "properties": {
                                    "patch": { "type": "string" }
                                },
                                "required": ["patch"],
                                "additionalProperties": false
                            },
                            "replace_block": {
                                "type": "object",
                                "properties": {
                                    "search_block": { "type": "string" },
                                    "replace_block": { "type": "string" },
                                    "range": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 2
                                    }
                                },
                                "required": ["search_block", "replace_block"],
                                "additionalProperties": false
                            },
                            "replace_range": {
                                "type": "object",
                                "properties": {
                                    "range": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 2,
                                        "maxItems": 2
                                    },
                                    "expected_hash": { "type": "string" },
                                    "replacement": { "type": "string" }
                                },
                                "required": ["range", "expected_hash", "replacement"],
                                "additionalProperties": false
                            },
                            "modify_toml": {
                                "type": "object",
                                "properties": {
                                    "expected_hash": { "type": "string" },
                                    "operations": toml_operations_schema()
                                },
                                "required": ["expected_hash", "operations"],
                                "additionalProperties": false
                            }
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["path", "edit"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "replace_range",
            "Replace an already-read line range when the range content_hash still matches. Prefer this over unified diffs for small source edits.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "range": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 2,
                        "maxItems": 2
                    },
                    "expected_hash": { "type": "string" },
                    "replacement": { "type": "string" }
                },
                "required": ["path", "range", "expected_hash", "replacement"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "modify_toml",
            "Safely edit Cargo/TOML dependency tables using a full-file content_hash. Operations support set_dependency and remove_dependency.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "expected_hash": { "type": "string" },
                    "operations": toml_operations_schema()
                },
                "required": ["path", "expected_hash", "operations"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "apply_preview",
            "Apply a previously successful preview_id if the target file still has the same base hash.",
            json!({
                "type": "object",
                "properties": {
                    "preview_id": { "type": "string" }
                },
                "required": ["preview_id"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "write_file",
            "Write a full workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "apply_patch",
            "Apply a unified diff patch to a workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "patch": { "type": "string" }
                },
                "required": ["path", "patch"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "replace_block",
            "Replace an exact block inside a workspace-relative file.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "search_block": { "type": "string" },
                    "replace_block": { "type": "string" }
                },
                "required": ["path", "search_block", "replace_block"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "set_executable",
            "Mark an existing workspace-relative path executable.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        function_tool(
            "run_validation",
            "Run fmt, clippy, tests, or custom validation commands.",
            json!({
                "type": "object",
                "properties": {
                    "fmt": { "type": "boolean" },
                    "clippy": { "type": "boolean" },
                    "workspace_tests": { "type": "boolean" },
                    "tests": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "custom_commands": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

pub(crate) fn toml_operations_schema() -> serde_json::Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": {
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "op": { "const": "set_dependency" },
                        "table": {
                            "type": "string",
                            "enum": ["dependencies", "dev-dependencies", "build-dependencies"]
                        },
                        "name": { "type": "string", "minLength": 1 },
                        "version": { "type": "string" },
                        "features": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "default_features": { "type": "boolean" },
                        "optional": { "type": "boolean" },
                        "package": { "type": "string" },
                        "path": { "type": "string" }
                    },
                    "required": ["op", "table", "name"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": {
                        "op": { "const": "remove_dependency" },
                        "table": {
                            "type": "string",
                            "enum": ["dependencies", "dev-dependencies", "build-dependencies"]
                        },
                        "name": { "type": "string", "minLength": 1 }
                    },
                    "required": ["op", "table", "name"],
                    "additionalProperties": false
                }
            ]
        }
    })
}

pub(crate) fn native_tool_definitions_for_request(
    request: &StreamRequest,
) -> Vec<serde_json::Value> {
    let config =
        crate::quorp::tui::agent_context::load_agent_config(request.project_root.as_path());
    let tools = native_tool_definitions();
    let Some(allowed_tools) = native_tool_allowlist_for_request(request) else {
        return filter_native_tools_for_config(tools, &config);
    };
    filter_native_tools_for_config(tools, &config)
        .into_iter()
        .filter(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| allowed_tools.contains(&name))
        })
        .collect()
}

pub(crate) fn filter_native_tools_for_config(
    tools: Vec<serde_json::Value>,
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> Vec<serde_json::Value> {
    tools
        .into_iter()
        .filter(|tool| {
            let Some(name) = tool
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
            else {
                return false;
            };
            external_native_tool_enabled(name, config)
        })
        .collect()
}

pub(crate) fn external_native_tool_enabled(
    name: &str,
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> bool {
    let tools = &config.agent_tools;
    match name {
        "find_files" => tools.enabled && tools.fd.enabled,
        "structural_search" => {
            tools.enabled && tools.ast_grep.enabled && configured_ast_grep_command(tools).is_some()
        }
        "structural_edit_preview" => {
            tools.enabled
                && tools.ast_grep.enabled
                && tools.ast_grep.allow_rewrite_preview
                && configured_ast_grep_command(tools).is_some()
        }
        "cargo_diagnostics" => {
            tools.enabled
                && tools.cargo_diagnostics.enabled
                && quorp_agent_core::command_is_available(&tools.cargo_diagnostics.check_command)
        }
        _ => true,
    }
}

pub(crate) fn configured_ast_grep_command(
    tools: &crate::quorp::tui::agent_context::AgentToolsSettings,
) -> Option<String> {
    if quorp_agent_core::command_is_available(&tools.ast_grep.command) {
        return Some(tools.ast_grep.command.clone());
    }
    if tools.ast_grep.command == "ast-grep" && quorp_agent_core::command_is_available("sg") {
        return Some("sg".to_string());
    }
    None
}

pub(crate) fn native_tool_allowlist_for_request(
    request: &StreamRequest,
) -> Option<Vec<&'static str>> {
    if !nvidia_controller_benchmark_profile(request) {
        return None;
    }
    let transcript = request
        .messages
        .iter()
        .rev()
        .take(4)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if transcript.contains("exactly one `ApplyPreview`")
        || transcript.contains("clean manifest preview already exists")
    {
        return Some(vec!["apply_preview"]);
    }
    if transcript.contains("exactly one `PreviewEdit` with `modify_toml`")
        || transcript.contains("Manifest patch mode is still active")
        || transcript.contains("Manifest patch mode rejected the previous plan")
    {
        return Some(vec!["preview_edit"]);
    }
    if transcript.contains("NeedsFastLoopRerun")
        || transcript.contains("needs_fast_loop_rerun")
        || transcript.contains("rerun the smallest fast loop")
    {
        return Some(vec!["run_command", "run_validation"]);
    }
    None
}

pub(crate) fn native_tool_choice_for_tools(tools: &[serde_json::Value]) -> serde_json::Value {
    if tools.len() != 1 {
        return serde_json::json!("required");
    }
    let Some(name) = tools
        .first()
        .and_then(|tool| tool.get("function"))
        .and_then(|function| function.get("name"))
        .and_then(serde_json::Value::as_str)
    else {
        return serde_json::json!("required");
    };
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name
        }
    })
}

pub(crate) fn function_tool(
    name: &str,
    description: &str,
    parameters: serde_json::Value,
) -> serde_json::Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "strict": true,
            "parameters": parameters
        }
    })
}

pub(crate) fn build_completion_request_body(
    request: &StreamRequest,
    client_config: &RemoteClientConfig,
    stream: bool,
) -> serde_json::Value {
    let model_target = resolve_model_target(&request.model_id);
    let mut request_body = build_request_body(
        client_config,
        &RemoteChatRequest {
            messages: build_request_messages(request),
            max_tokens: request.max_completion_tokens.or(Some(4096)),
            reasoning_effort: if request.disable_reasoning {
                None
            } else {
                reasoning_effort_for_model(&model_target.provider_model_id)
            },
        },
    );
    request_body["stream"] = serde_json::json!(stream);
    if request.native_tool_calls {
        let native_tools = native_tool_definitions_for_request(request);
        request_body["tool_choice"] = native_tool_choice_for_tools(&native_tools);
        request_body["tools"] = serde_json::Value::Array(native_tools);
        request_body["parallel_tool_calls"] = serde_json::json!(false);
    }
    request_body
}

#[allow(dead_code)]
pub(crate) async fn request_single_completion(request: &StreamRequest) -> Result<String, String> {
    request_single_completion_details(request)
        .await
        .map(|result| result.content)
}

pub(crate) async fn request_single_completion_details(
    request: &StreamRequest,
) -> Result<SingleCompletionResult, String> {
    use futures::StreamExt;

    let started_at = std::time::Instant::now();
    let client_config = finalize_client_config_for_request(
        request,
        resolve_client_config(request).map_err(|error| error.to_string())?,
    )
    .await
    .map_err(|error| error.to_string())?;
    let use_stream = !request.native_tool_calls;
    let request_body = build_completion_request_body(request, &client_config.client, use_stream);
    let url =
        chat_completions_url_for_provider(client_config.provider, &client_config.client.base_url)
            .map_err(|error| error.to_string())?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(client_config.client.connect_timeout)
        .read_timeout(client_config.client.read_timeout)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))?;

    let max_rate_limit_retries = if client_config.provider == InteractiveProviderKind::Nvidia {
        nvidia_rate_limit_retries()
    } else {
        0
    };
    let mut attempt_index = 0_u64;
    let response = loop {
        let mut request_builder = http_client
            .post(url.clone())
            .header(
                reqwest::header::ACCEPT,
                if use_stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(&request_body);
        if let Some(bearer_token) = client_config.bearer_token.as_deref() {
            request_builder = request_builder.bearer_auth(bearer_token);
        }
        request_builder = apply_extra_headers(request_builder, &client_config.client.extra_headers);
        let response = request_builder.send().await.map_err(|error| {
            format!(
                "Failed to connect to {}: {error}",
                provider_connection_name(client_config.provider)
            )
        })?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS
            || attempt_index >= max_rate_limit_retries
        {
            break response;
        }
        let backoff_seconds = nvidia_rate_limit_backoff_seconds(response.headers(), attempt_index);
        tokio::time::sleep(Duration::from_secs(backoff_seconds)).await;
        attempt_index = attempt_index.saturating_add(1);
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<body unavailable>".to_string());
        return Err(format!(
            "{} returned {} while requesting agent completion: {}",
            provider_connection_name(client_config.provider),
            status,
            body.trim()
        ));
    }

    let watchdog = request.watchdog.clone().unwrap_or_default();

    if !use_stream {
        let response_json = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| format!("Failed to decode completion response JSON: {error}"))?;
        let mut raw_response = response_json.clone();
        let response_id = response_json
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let response_model = response_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let finish_reason = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let content = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reasoning_content = response_json
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| {
                message
                    .get("reasoning_content")
                    .or_else(|| message.get("reasoning"))
            })
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut native_tool_builders = BTreeMap::new();
        merge_native_tool_call_chunk(&response_json, &mut native_tool_builders);
        let usage_payload = response_json.get("usage").cloned();
        let mut usage = usage_payload.as_ref().and_then(|usage_payload| {
            parse_usage_payload(
                usage_payload,
                started_at.elapsed().as_millis() as u64,
                finish_reason.clone(),
                response_id.as_deref(),
            )
            .ok()
        });
        if usage.is_none() {
            let latency_ms = started_at.elapsed().as_millis() as u64;
            let input_tokens = estimate_token_count(&request_body["messages"]);
            let output_tokens = estimate_token_count(&(content.clone() + &reasoning_content));
            usage = Some(quorp_agent_core::TokenUsage {
                input_tokens,
                output_tokens,
                total_billed_tokens: input_tokens.saturating_add(output_tokens),
                reasoning_tokens: (!reasoning_content.is_empty()).then_some(output_tokens),
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
                provider_request_id: response_id.clone(),
                latency_ms,
                finish_reason: finish_reason.clone().or_else(|| Some("stop".to_string())),
                usage_source: quorp_agent_core::UsageSource::Estimated,
            });
        }

        let finalized_tool_calls = finalized_native_tool_calls(&native_tool_builders);
        let (native_turn, native_turn_error) = if finalized_tool_calls.is_empty() {
            match native_turn_from_content_fallback(&content) {
                Ok(turn) => (turn, None),
                Err(error) => (None, Some(error)),
            }
        } else {
            match native_turn_from_tool_calls(&content, &finalized_tool_calls) {
                Ok(turn) => (turn, None),
                Err(error) => (None, Some(error)),
            }
        };
        if !finalized_tool_calls.is_empty() {
            raw_response = json!({
                "id": response_id,
                "model": response_model,
                "choices": [{
                    "finish_reason": finish_reason,
                    "message": {
                        "content": content,
                        "tool_calls": finalized_tool_calls,
                    }
                }],
                "usage": usage_payload,
            });
        }

        let total_elapsed_ms = started_at.elapsed().as_millis() as u64;
        return Ok(SingleCompletionResult {
            content,
            reasoning_content,
            native_turn,
            native_turn_error,
            usage,
            raw_response: wrap_raw_provider_response(raw_response, &client_config.routing),
            watchdog: Some(quorp_agent_core::ModelRequestWatchdogReport {
                first_token_timeout_ms: watchdog.first_token_timeout_ms,
                idle_timeout_ms: watchdog.idle_timeout_ms,
                total_timeout_ms: watchdog.total_timeout_ms,
                first_token_latency_ms: Some(total_elapsed_ms),
                max_idle_gap_ms: None,
                total_elapsed_ms,
                near_limit: watchdog_near_limit(
                    &watchdog,
                    Some(total_elapsed_ms),
                    0,
                    total_elapsed_ms,
                ),
                triggered_reason: None,
            }),
            routing: client_config.routing,
        });
    }

    let mut bytes_stream = response.bytes_stream();
    let mut line_buffer = String::new();
    let mut content = String::new();
    let mut reasoning_content = String::new();
    let mut usage = None;
    let mut raw_response = serde_json::json!({});
    let mut usage_payload = None;
    let mut response_id = None;
    let mut response_model = None;
    let mut finish_reason = None;
    let mut native_tool_builders = BTreeMap::new();
    let mut stream_finished = false;
    let mut first_token_latency_ms = None;
    let mut last_progress_at = started_at;
    let mut max_idle_gap_ms = 0u64;

    if let Some(first_token_timeout_ms) = watchdog.first_token_timeout_ms
        && started_at.elapsed().as_millis() as u64 >= first_token_timeout_ms
    {
        return Err(format!(
            "first token timeout after {first_token_timeout_ms}ms"
        ));
    }

    loop {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        if let Some(total_timeout_ms) = watchdog.total_timeout_ms
            && elapsed_ms >= total_timeout_ms
        {
            return Err(format!("model request timeout after {total_timeout_ms}ms"));
        }
        if content.is_empty()
            && reasoning_content.is_empty()
            && let Some(first_token_timeout_ms) = watchdog.first_token_timeout_ms
            && elapsed_ms >= first_token_timeout_ms
        {
            return Err(format!(
                "first token timeout after {first_token_timeout_ms}ms"
            ));
        }

        let timeout_ms = if content.is_empty() && reasoning_content.is_empty() {
            watchdog.first_token_timeout_ms
        } else {
            watchdog.idle_timeout_ms
        };
        let next_chunk = if let Some(timeout_ms) = timeout_ms {
            match tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                bytes_stream.next(),
            )
            .await
            {
                Ok(chunk) => chunk,
                Err(_) if content.is_empty() && reasoning_content.is_empty() => {
                    return Err(format!("first token timeout after {timeout_ms}ms"));
                }
                Err(_) => {
                    return Err(format!("stream idle timeout after {timeout_ms}ms"));
                }
            }
        } else {
            bytes_stream.next().await
        };
        let Some(chunk) = next_chunk else {
            break;
        };
        let bytes = chunk.map_err(|error| format!("stream error: {error}"))?;
        let chunk_text = String::from_utf8_lossy(&bytes);
        line_buffer.push_str(&chunk_text);

        while let Some(newline_index) = line_buffer.find('\n') {
            let line = line_buffer[..newline_index].to_string();
            line_buffer.drain(..=newline_index);
            let Some(payload) = parse_sse_data_line(&line) else {
                continue;
            };

            if payload == "[DONE]" {
                stream_finished = true;
                break;
            }

            if let Ok(chunk_val) = serde_json::from_str::<serde_json::Value>(payload) {
                raw_response = chunk_val.clone(); // store the last chunk for raw response mapping
                if let Some(id) = chunk_val.get("id").and_then(|value| value.as_str()) {
                    response_id = Some(id.to_string());
                }
                if let Some(model) = chunk_val.get("model").and_then(|value| value.as_str()) {
                    response_model = Some(model.to_string());
                }
                if let Some(reason) = chunk_val
                    .get("choices")
                    .and_then(|choices| choices.get(0))
                    .and_then(|choice| choice.get("finish_reason"))
                    .and_then(|value| value.as_str())
                {
                    finish_reason = Some(reason.to_string());
                }
                merge_native_tool_call_chunk(&chunk_val, &mut native_tool_builders);
                if let Some(u) = chunk_val.get("usage") {
                    usage_payload = Some(u.clone());
                    let latency_ms = started_at.elapsed().as_millis() as u64;
                    let provider_request_id = chunk_val.get("id").and_then(|v| v.as_str());
                    usage = parse_usage_payload(
                        u,
                        latency_ms,
                        finish_reason.clone(),
                        provider_request_id,
                    )
                    .ok();
                }
            }

            if let Ok(events) = parse_sse_payload(payload) {
                for event in events {
                    match event {
                        RemoteStreamEvent::TextDelta(fragment) => {
                            let now = std::time::Instant::now();
                            let idle_gap_ms =
                                now.duration_since(last_progress_at).as_millis() as u64;
                            max_idle_gap_ms = max_idle_gap_ms.max(idle_gap_ms);
                            last_progress_at = now;
                            if first_token_latency_ms.is_none() {
                                first_token_latency_ms =
                                    Some(now.duration_since(started_at).as_millis() as u64);
                            }
                            content.push_str(&fragment);
                        }
                        RemoteStreamEvent::ReasoningDelta(fragment) => {
                            let now = std::time::Instant::now();
                            let idle_gap_ms =
                                now.duration_since(last_progress_at).as_millis() as u64;
                            max_idle_gap_ms = max_idle_gap_ms.max(idle_gap_ms);
                            last_progress_at = now;
                            if first_token_latency_ms.is_none() {
                                first_token_latency_ms =
                                    Some(now.duration_since(started_at).as_millis() as u64);
                            }
                            reasoning_content.push_str(&fragment);
                        }
                        RemoteStreamEvent::Finished => {}
                    }
                }
            }
        }

        if stream_finished {
            break;
        }
    }

    // Capture remaining if any
    if let Some(payload) = parse_sse_data_line(&line_buffer)
        && payload != "[DONE]"
        && let Ok(events) = parse_sse_payload(payload)
    {
        for event in events {
            match event {
                RemoteStreamEvent::TextDelta(fragment) => content.push_str(&fragment),
                RemoteStreamEvent::ReasoningDelta(fragment) => {
                    reasoning_content.push_str(&fragment)
                }
                RemoteStreamEvent::Finished => {}
            }
        }
    }

    if let Some(payload) = parse_sse_data_line(&line_buffer)
        && payload != "[DONE]"
        && let Ok(chunk_val) = serde_json::from_str::<serde_json::Value>(payload)
    {
        merge_native_tool_call_chunk(&chunk_val, &mut native_tool_builders);
        if let Some(id) = chunk_val.get("id").and_then(|value| value.as_str()) {
            response_id = Some(id.to_string());
        }
        if let Some(model) = chunk_val.get("model").and_then(|value| value.as_str()) {
            response_model = Some(model.to_string());
        }
        if let Some(reason) = chunk_val
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|value| value.as_str())
        {
            finish_reason = Some(reason.to_string());
        }
        if let Some(u) = chunk_val.get("usage") {
            usage_payload = Some(u.clone());
        }
    }

    if !stream_finished {
        return Err("stream ended before sending [DONE].".to_string());
    }

    // Fallback if no usage was streamed
    if usage.is_none() {
        let latency_ms = started_at.elapsed().as_millis() as u64;
        let input_tokens = estimate_token_count(&request_body["messages"]);
        let output_tokens = estimate_token_count(&(content.clone() + &reasoning_content));
        usage = Some(quorp_agent_core::TokenUsage {
            input_tokens,
            output_tokens,
            total_billed_tokens: input_tokens.saturating_add(output_tokens),
            reasoning_tokens: (!reasoning_content.is_empty()).then_some(output_tokens),
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            provider_request_id: None,
            latency_ms,
            finish_reason: Some("stop".to_string()),
            usage_source: quorp_agent_core::UsageSource::Estimated,
        });
    }

    let finalized_tool_calls = finalized_native_tool_calls(&native_tool_builders);
    let (native_turn, native_turn_error) = if finalized_tool_calls.is_empty() {
        match native_turn_from_content_fallback(&content) {
            Ok(turn) => (turn, None),
            Err(error) => (None, Some(error)),
        }
    } else {
        match native_turn_from_tool_calls(&content, &finalized_tool_calls) {
            Ok(turn) => (turn, None),
            Err(error) => (None, Some(error)),
        }
    };
    if !finalized_tool_calls.is_empty() {
        raw_response = json!({
            "id": response_id,
            "model": response_model,
            "choices": [{
                "finish_reason": finish_reason,
                "message": {
                    "content": content,
                    "tool_calls": finalized_tool_calls,
                }
            }],
            "usage": usage_payload,
        });
    }

    Ok(SingleCompletionResult {
        content,
        reasoning_content,
        native_turn,
        native_turn_error,
        usage,
        raw_response: wrap_raw_provider_response(raw_response, &client_config.routing),
        watchdog: Some(quorp_agent_core::ModelRequestWatchdogReport {
            first_token_timeout_ms: watchdog.first_token_timeout_ms,
            idle_timeout_ms: watchdog.idle_timeout_ms,
            total_timeout_ms: watchdog.total_timeout_ms,
            first_token_latency_ms,
            max_idle_gap_ms: (max_idle_gap_ms > 0).then_some(max_idle_gap_ms),
            total_elapsed_ms: started_at.elapsed().as_millis() as u64,
            near_limit: watchdog_near_limit(
                &watchdog,
                first_token_latency_ms,
                max_idle_gap_ms,
                started_at.elapsed().as_millis() as u64,
            ),
            triggered_reason: None,
        }),
        routing: client_config.routing,
    })
}
