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
        socket.write_all(response.as_bytes()).expect("write response");
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

    assert!(saw_error, "expected visible chat error when loopback server is unavailable");
    harness.draw();
    harness.assert_buffer_contains("Failed to connect to SSD-MOE");
}
