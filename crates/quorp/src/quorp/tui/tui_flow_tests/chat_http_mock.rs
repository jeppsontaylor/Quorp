use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::ChatUiEvent;

use super::fixtures;
use super::harness::TuiTestHarness;

fn spawn_single_response_server(response_body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let address = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).expect("read request");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        socket
            .write_all(response.as_bytes())
            .expect("write response");
    });
    format!("http://127.0.0.1:{}/v1", address.port())
}

#[test]
fn chat_streams_mock_sse_into_transcript() {
    let base_url = spawn_single_response_server(concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"from-mock\"},\"finish_reason\":null}]}\n\n",
        "data: [DONE]\n\n",
    ));

    let project_dir = fixtures::temp_project_with_files(&[("stub.rs", "")]);
    let root = project_dir.path().to_path_buf();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness.app.chat.set_base_url_for_test(base_url);
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("hi");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_delta = false;
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, text)))
                if text.contains("from-mock") =>
            {
                saw_delta = true;
                harness.apply_chat_event(ChatUiEvent::AssistantDelta(0, text));
            }
            Some(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {
                harness.apply_chat_event(ChatUiEvent::StreamFinished(0));
                break;
            }
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                panic!("unexpected chat stream error: {error}");
            }
            Some(event) => {
                harness.apply_backend_event(event);
            }
            None => {}
        }
    }

    assert!(saw_delta, "timed out waiting for streamed assistant delta");
    harness.draw();
    harness.assert_buffer_contains("from-mock");
}

#[test]
fn chat_streams_structured_agent_turn_and_renders_receipts() {
    let content = serde_json::json!({
        "assistant_message": "I found the file that needs updating.",
        "actions": [
            {
                "WriteFile": {
                    "path": "stub.rs",
                    "content": "fn main() {}"
                }
            }
        ],
        "task_updates": [
            {
                "title": "Inspect stub.rs",
                "status": "completed"
            }
        ],
        "memory_updates": [],
        "requested_mode_change": null,
        "verifier_plan": {
            "fmt": true,
            "clippy": false,
            "tests": [],
            "custom_commands": []
        }
    })
    .to_string();
    let response_body = format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":null}}]}}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&content).expect("quote json content")
    );
    let leaked_response: &'static str = Box::leak(response_body.into_boxed_str());
    let base_url = spawn_single_response_server(leaked_response);

    let project_dir = fixtures::temp_project_with_files(&[("stub.rs", "")]);
    let root = project_dir.path().to_path_buf();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness.app.chat.set_base_url_for_test(base_url);
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("update stub");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, text))) => {
                harness.apply_chat_event(ChatUiEvent::AssistantDelta(0, text));
            }
            Some(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {
                harness.apply_chat_event(ChatUiEvent::StreamFinished(0));
                break;
            }
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                panic!("unexpected chat stream error: {error}");
            }
            Some(event) => {
                harness.apply_backend_event(event);
            }
            None => {}
        }
    }

    harness.draw();
    harness.assert_buffer_contains("I found the file");
    harness.assert_buffer_contains("Action receipts");
    harness.assert_buffer_not_contains("\"assistant_message\"");
}

#[test]
fn chat_structured_search_text_turn_auto_executes_and_renders_results() {
    let content = serde_json::json!({
        "assistant_message": "I searched the repo for the render helper.",
        "actions": [
            {
                "SearchText": {
                    "query": "render_agent_turn_text",
                    "limit": 4
                }
            }
        ],
        "task_updates": [],
        "memory_updates": [],
        "requested_mode_change": null,
        "verifier_plan": null
    })
    .to_string();
    let response_body = format!(
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":null}}]}}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&content).expect("quote json content")
    );
    let leaked_response: &'static str = Box::leak(response_body.into_boxed_str());
    let base_url = spawn_single_response_server(leaked_response);

    let project_dir =
        fixtures::temp_project_with_files(&[("stub.rs", "fn render_agent_turn_text() {}\n")]);
    let root = project_dir.path().to_path_buf();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness.app.chat.set_base_url_for_test(base_url);
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("search the repo");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_search_output = false;
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, text))) => {
                harness.apply_chat_event(ChatUiEvent::AssistantDelta(0, text));
            }
            Some(TuiEvent::Chat(ChatUiEvent::CommandOutput(0, line))) => {
                if line.contains("Text search results")
                    || line.contains("stub.rs:1")
                    || line.contains("render_agent_turn_text")
                {
                    saw_search_output = true;
                }
                harness.apply_chat_event(ChatUiEvent::CommandOutput(0, line));
            }
            Some(TuiEvent::Chat(ChatUiEvent::CommandFinished(0, outcome))) => {
                harness.apply_chat_event(ChatUiEvent::CommandFinished(0, outcome));
                break;
            }
            Some(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {
                harness.apply_chat_event(ChatUiEvent::StreamFinished(0));
            }
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                panic!("unexpected chat stream error: {error}");
            }
            Some(event) => {
                harness.apply_backend_event(event);
            }
            None => {}
        }
    }

    assert!(saw_search_output, "expected auto-executed search output");
    harness.draw();
    harness.assert_buffer_contains("Text search results");
}

#[test]
fn chat_surfaces_loopback_connection_errors() {
    let root = fixtures::fixture_project_root();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness
        .app
        .chat
        .set_base_url_for_test("http://127.0.0.1:65534/v1".to_string());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("hello");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_error = false;
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                saw_error = true;
                harness.apply_chat_event(ChatUiEvent::Error(0, error));
                break;
            }
            Some(TuiEvent::Chat(event)) => harness.apply_chat_event(event),
            Some(other) => harness.apply_backend_event(other),
            None => {}
        }
    }

    assert!(
        saw_error,
        "expected visible chat error when loopback server is unavailable"
    );
    assert!(
        harness
            .app
            .chat
            .last_assistant_text_for_test()
            .is_some_and(|text| text.contains("Failed to connect to")),
        "expected assistant transcript to retain the provider connection error"
    );
    harness.draw();
    harness.assert_buffer_contains("Failed to connect");
}

#[test]
fn chat_surfaces_invalid_base_urls_in_transcript() {
    let root = fixtures::fixture_project_root();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness
        .app
        .chat
        .set_base_url_for_test("ftp://example.com".to_string());
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("hello");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_error = false;
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                saw_error = true;
                harness.apply_chat_event(ChatUiEvent::Error(0, error));
                break;
            }
            Some(TuiEvent::Chat(event)) => harness.apply_chat_event(event),
            Some(other) => harness.apply_backend_event(other),
            None => {}
        }
    }

    assert!(
        saw_error,
        "expected visible chat error for invalid base URL"
    );
    assert!(
        harness
            .app
            .chat
            .last_error_for_test()
            .is_some_and(|error| error.contains("unsupported base URL scheme")),
        "expected base URL validation error in chat state"
    );
}

#[test]
fn chat_surfaces_truncated_stream_disconnect_after_partial_output() {
    let base_url = spawn_single_response_server(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial-output\"},\"finish_reason\":null}]}\n\n",
    );

    let project_dir = fixtures::temp_project_with_files(&[("stub.rs", "")]);
    let root = project_dir.path().to_path_buf();
    let mut harness = TuiTestHarness::new_with_root(120, 40, root);
    harness.app.chat.set_base_url_for_test(base_url);
    harness.app.focused = Pane::Chat;
    harness.app.chat.set_input_for_test("hi");
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_partial = false;
    let mut saw_error = false;
    while std::time::Instant::now() < deadline {
        match harness.recv_tui_event_timeout(Duration::from_millis(100)) {
            Some(TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, text))) => {
                if text.contains("partial-output") {
                    saw_partial = true;
                }
                harness.apply_chat_event(ChatUiEvent::AssistantDelta(0, text));
            }
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, error))) => {
                saw_error = error.contains("before sending [DONE]");
                harness.apply_chat_event(ChatUiEvent::Error(0, error));
            }
            Some(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {
                harness.apply_chat_event(ChatUiEvent::StreamFinished(0));
                break;
            }
            Some(TuiEvent::Chat(event)) => harness.apply_chat_event(event),
            Some(other) => harness.apply_backend_event(other),
            None => {}
        }
    }

    assert!(
        saw_partial,
        "expected partial transcript text before disconnect"
    );
    assert!(saw_error, "expected visible truncated-stream error");
    let assistants = harness.app.chat.assistant_messages_for_test();
    assert!(
        assistants
            .iter()
            .any(|message| message.contains("partial-output")),
        "expected assistant transcript to preserve partial output"
    );
    assert!(
        harness
            .app
            .chat
            .last_error_for_test()
            .is_some_and(|error| error.contains("before sending [DONE]")),
        "expected disconnect error in chat state"
    );
}
