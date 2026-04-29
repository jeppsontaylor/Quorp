use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

#[test]
fn includes_lsp_tool_definitions() {
    let names = native_tool_definitions()
        .into_iter()
        .filter_map(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect::<std::collections::BTreeSet<_>>();
    for name in [
        "lsp_diagnostics",
        "lsp_definition",
        "lsp_references",
        "lsp_hover",
        "lsp_workspace_symbols",
        "lsp_document_symbols",
        "lsp_code_actions",
        "lsp_rename_preview",
        "process_start",
        "process_read",
        "process_write",
        "process_stop",
        "process_wait_for_port",
        "browser_open",
        "browser_screenshot",
        "browser_console_logs",
        "browser_network_errors",
        "browser_accessibility_snapshot",
        "browser_close",
    ] {
        assert!(names.contains(name), "missing tool {name}");
    }
}

#[tokio::test]
async fn native_tool_schema_rejection_retries_with_json_contract() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr");
    let (body_tx, body_rx) = mpsc::channel::<String>();

    let server = thread::spawn(move || {
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .expect("read timeout");
            let request_text = read_http_request_body(&mut stream).expect("request body");
            body_tx.send(request_text.clone()).expect("send body");
            if attempt == 0 {
                write_http_response(
                    &mut stream,
                    400,
                    "application/json",
                    r#"{"error":{"message":"unsupported tool schema"}}"#,
                )
                .expect("rejection response");
            } else {
                write_http_response(
                    &mut stream,
                    200,
                    "text/event-stream",
                    "data: {\"id\":\"chatcmpl-1\",\"model\":\"qwen\",\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\ndata: [DONE]\n\n",
                )
                .expect("success response");
            }
        }
    });

    let request = StreamRequest {
        request_id: 1,
        session_id: 1,
        model_id: crate::quorp::provider_config::NVIDIA_QWEN_MODEL.to_string(),
        agent_mode: crate::quorp::tui::agent_protocol::AgentMode::Act,
        latest_input: "hello".to_string(),
        messages: vec![ChatServiceMessage {
            role: ChatServiceRole::User,
            content: "hello".to_string(),
        }],
        project_root: std::env::current_dir().expect("cwd"),
        base_url_override: Some(format!("http://{}", address)),
        max_completion_tokens: Some(16),
        include_repo_capsule: false,
        disable_reasoning: true,
        native_tool_calls: true,
        watchdog: None,
        safety_mode_label: None,
        prompt_compaction_policy: None,
        capture_scope: None,
        capture_call_class: None,
    };

    let completion = request_single_completion_details(&request)
        .await
        .expect("completion");
    assert_eq!(completion.content, "hello");

    let first_request_text = body_rx.recv().expect("first body");
    let fallback_request_text = body_rx.recv().expect("fallback body");
    let first_request_lower = first_request_text.to_ascii_lowercase();
    let fallback_request_lower = fallback_request_text.to_ascii_lowercase();
    assert!(first_request_lower.contains("\"tools\""));
    assert!(first_request_lower.contains("\"tool_choice\""));
    assert!(first_request_lower.contains("\"parallel_tool_calls\":false"));
    assert!(first_request_lower.contains("run_command"));
    assert!(first_request_lower.contains("x-quorp-action-contract-mode: native_tool_calls_v1"));
    assert!(!fallback_request_text.contains("\"tools\""));
    assert!(
        fallback_request_lower
            .contains("x-quorp-action-contract-mode: json_action_contract_v1")
    );
    server.join().expect("join");
}

fn read_http_request_body(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
    let mut reader = BufReader::new(stream);
    let mut request_text = String::new();
    let mut content_length = 0usize;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    request_text.push_str(&line);
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        request_text.push_str(&line);
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    request_text.push_str(&String::from_utf8_lossy(&body));
    Ok(request_text)
}

fn write_http_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        422 => "Unprocessable Entity",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}
