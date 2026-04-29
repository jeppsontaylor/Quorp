use super::*;

#[test]
fn test_deserialize_initialize_result() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            }
        }
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, json!(1));
    let init_res: InitializeResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(init_res.protocol_version, "2024-11-05");
    assert_eq!(init_res.server_info.name, "test-server");
}

#[test]
fn test_deserialize_tools_list() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": "abc",
        "result": {
            "tools": [
                {
                    "name": "get_repo_map",
                    "description": "Returns a repo map",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
        }
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.id, json!("abc"));
    let list_res: ToolsListResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(list_res.tools.len(), 1);
    assert_eq!(list_res.tools[0].name, "get_repo_map");
}

#[test]
fn json_rpc_request_round_trips() {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(7),
        method: "tools/call".to_string(),
        params: Some(json!({"name": "search"})),
    };
    let serialised = serde_json::to_string(&request).unwrap();
    let parsed: JsonRpcRequest = serde_json::from_str(&serialised).unwrap();
    assert_eq!(parsed.jsonrpc, "2.0");
    assert_eq!(parsed.id, json!(7));
    assert_eq!(parsed.method, "tools/call");
    assert_eq!(parsed.params.unwrap()["name"], "search");
}

#[test]
fn json_rpc_request_omits_none_params() {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(null),
        method: "ping".to_string(),
        params: None,
    };
    let serialised = serde_json::to_string(&request).unwrap();
    assert!(!serialised.contains("\"params\""));
}

#[test]
fn json_rpc_error_round_trips() {
    let err = JsonRpcError {
        code: -32601,
        message: "Method not found".to_string(),
        data: Some(json!({"hint": "check the method name"})),
    };
    let serialised = serde_json::to_string(&err).unwrap();
    let parsed: JsonRpcError = serde_json::from_str(&serialised).unwrap();
    assert_eq!(parsed.code, -32601);
    assert_eq!(parsed.message, "Method not found");
    assert_eq!(parsed.data.unwrap()["hint"], "check the method name");
}

#[test]
fn incoming_message_distinguishes_response_from_notification() {
    let response_json = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
    let parsed: IncomingMessage = serde_json::from_str(response_json).unwrap();
    assert!(matches!(parsed, IncomingMessage::Response(_)));

    let notification_json = r#"{"jsonrpc":"2.0","method":"resources/updated"}"#;
    let parsed: IncomingMessage = serde_json::from_str(notification_json).unwrap();
    assert!(matches!(parsed, IncomingMessage::Notification(_)));

    // Note: the `untagged` IncomingMessage enum has a known greedy-match
    // quirk where a server-to-client Request (with id + method but no
    // result/error) deserialises as Response. This is pre-existing
    // behaviour we capture explicitly so callers know to detect Request
    // shape via `method.is_some()` after a Response match. The dispatch
    // loop in McpClient already only treats id-bearing messages as
    // responses, so the runtime impact is nil today.
    let request_shaped_json =
        r#"{"jsonrpc":"2.0","id":42,"method":"sampling/createMessage","params":{}}"#;
    let parsed: IncomingMessage = serde_json::from_str(request_shaped_json).unwrap();
    assert!(matches!(parsed, IncomingMessage::Response(_)));
}

#[test]
fn call_tool_result_supports_all_content_variants() {
    // The CallToolResultContent enum uses `tag = "type"` plus snake_case
    // field names (no per-variant rename_all attribute), so wire JSON uses
    // `mime_type`, not `mimeType`.
    let json = r#"{
        "content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "AQID", "mime_type": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///tmp/a"}}
        ],
        "isError": false
    }"#;
    let parsed: CallToolResult = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.content.len(), 3);
    assert_eq!(parsed.is_error, Some(false));
    assert!(matches!(&parsed.content[0], CallToolResultContent::Text { text } if text == "hello"));
    assert!(matches!(
        &parsed.content[1],
        CallToolResultContent::Image { mime_type, data } if mime_type == "image/png" && data == "AQID"
    ));
    assert!(
        matches!(&parsed.content[2], CallToolResultContent::Resource { resource } if resource["uri"] == "file:///tmp/a")
    );
}

#[test]
fn call_tool_result_is_error_default_is_none() {
    let json = r#"{"content": [{"type": "text", "text": "ok"}]}"#;
    let parsed: CallToolResult = serde_json::from_str(json).unwrap();
    assert!(parsed.is_error.is_none());
}

#[tokio::test]
async fn take_stdio_pipes_returns_an_error_when_stdio_is_not_piped() {
    let mut child = tokio::process::Command::new("sh")
        .arg("-lc")
        .arg("exit 0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let error = take_stdio_pipes(&mut child).expect_err("missing pipes should fail");
    assert!(error.to_string().contains("not piped"));
    let _ = child.wait().await;
}

#[test]
fn call_tool_request_round_trips_camel_case() {
    let request = CallToolRequest {
        name: "search".to_string(),
        arguments: Some(json!({"query": "owner"})),
    };
    let serialised = serde_json::to_string(&request).unwrap();
    let parsed: CallToolRequest = serde_json::from_str(&serialised).unwrap();
    assert_eq!(parsed.name, "search");
    assert_eq!(parsed.arguments.unwrap()["query"], "owner");
}

#[test]
fn read_resource_result_parses_text_and_blob_variants() {
    let json = r#"{
        "contents": [
            {"uri": "file:///a", "mimeType": "text/plain", "text": "hi"},
            {"uri": "file:///b", "mimeType": "application/octet-stream", "blob": "AQID"}
        ]
    }"#;
    let parsed: ReadResourceResult = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.contents.len(), 2);
    assert_eq!(parsed.contents[0].text.as_deref(), Some("hi"));
    assert!(parsed.contents[0].blob.is_none());
    assert_eq!(parsed.contents[1].blob.as_deref(), Some("AQID"));
    assert!(parsed.contents[1].text.is_none());
}

#[test]
fn broker_truncates_large_text_and_redacts_large_resources() {
    let broker = McpBroker::new(
        McpClient {
            request_tx: tokio::sync::mpsc::unbounded_channel().0,
            workspace_roots: Vec::new(),
            server_info: Implementation {
                name: "test".into(),
                version: "1".into(),
            },
            server_capabilities: ServerCapabilities {
                resources: None,
                tools: None,
                logging: None,
                prompts: None,
            },
        },
        McpBrokerPolicy {
            max_text_chars: 4,
            max_resource_chars: 32,
            allowed_resource_schemes: vec!["file".into()],
        },
    );

    let result = broker
        .sanitize_call_tool_result(CallToolResult {
            content: vec![
                CallToolResultContent::Text {
                    text: "abcdef".into(),
                },
                CallToolResultContent::Resource {
                    resource: json!({
                        "uri": "file:///tmp/example",
                        "payload": "x".repeat(64)
                    }),
                },
            ],
            is_error: Some(false),
        })
        .expect("sanitize");

    match &result.content[0] {
        CallToolResultContent::Text { text } => {
            assert!(text.contains("[truncated]"));
        }
        _ => panic!("expected text content"),
    }
    match &result.content[1] {
        CallToolResultContent::Resource { resource } => {
            assert_eq!(resource["redacted"], true);
        }
        _ => panic!("expected resource content"),
    }
}

#[test]
fn broker_rejects_disallowed_resource_scheme() {
    let broker = McpBroker::new(
        McpClient {
            request_tx: tokio::sync::mpsc::unbounded_channel().0,
            workspace_roots: Vec::new(),
            server_info: Implementation {
                name: "test".into(),
                version: "1".into(),
            },
            server_capabilities: ServerCapabilities {
                resources: None,
                tools: None,
                logging: None,
                prompts: None,
            },
        },
        McpBrokerPolicy::default(),
    );

    let error = broker
        .ensure_resource_scheme_allowed("ssh://example.com/secret")
        .expect_err("scheme should be rejected");
    assert!(error.to_string().contains("not allowed"));
}

#[test]
fn roots_prompts_and_resources_round_trip() {
    let root = Root {
        uri: "file:///tmp/workspace".to_string(),
        name: Some("workspace".to_string()),
    };
    let roots = RootsListResult {
        roots: vec![root.clone()],
    };
    let prompts = ListPromptsResult {
        prompts: vec![json!({"name": "build"})],
        next_cursor: Some("next".to_string()),
    };
    let prompt = GetPromptResult {
        description: Some("Build prompt".to_string()),
        messages: vec![json!({"role": "user", "content": "hello"})],
    };
    let resources = ListResourcesResult {
        resources: vec![json!({"uri": "file:///tmp/workspace/readme.md"})],
        next_cursor: None,
    };

    let serialized_root = serde_json::to_string(&root).unwrap();
    let parsed_root: Root = serde_json::from_str(&serialized_root).unwrap();
    assert_eq!(parsed_root.uri, root.uri);

    let serialized_roots = serde_json::to_string(&roots).unwrap();
    let parsed_roots: RootsListResult = serde_json::from_str(&serialized_roots).unwrap();
    assert_eq!(parsed_roots.roots.len(), 1);

    let serialized_prompts = serde_json::to_string(&prompts).unwrap();
    let parsed_prompts: ListPromptsResult = serde_json::from_str(&serialized_prompts).unwrap();
    assert_eq!(parsed_prompts.next_cursor.as_deref(), Some("next"));

    let serialized_prompt = serde_json::to_string(&prompt).unwrap();
    let parsed_prompt: GetPromptResult = serde_json::from_str(&serialized_prompt).unwrap();
    assert_eq!(parsed_prompt.messages.len(), 1);

    let serialized_resources = serde_json::to_string(&resources).unwrap();
    let parsed_resources: ListResourcesResult =
        serde_json::from_str(&serialized_resources).unwrap();
    assert_eq!(parsed_resources.resources.len(), 1);
}
