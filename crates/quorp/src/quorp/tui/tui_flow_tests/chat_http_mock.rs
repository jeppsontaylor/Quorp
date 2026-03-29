use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::ChatUiEvent;

use super::fixtures;
use super::harness::TuiTestHarness;

/// Exercises the OpenAI-compatible HTTP fallback when [`ChatPane`] is constructed without a
/// [`crate::quorp::tui::chat_bridge`] (flow harnesses). SSE lines must match
/// [`open_ai::ResponseStreamResult`] / [`open_ai::ChoiceDelta`] (including `index` on each choice).
///
/// Sync test: `TuiTestHarness` owns a Tokio `Runtime`; an outer `#[tokio::test]` would panic when
/// dropping that runtime from an async context.
#[test]
fn chat_http_mock_sse() {
    let setup_rt = tokio::runtime::Runtime::new().expect("setup runtime");
    let (_mock_server, base) = setup_rt.block_on(async {
        let mock_server = MockServer::start().await;
        let body = concat!(
            "data: {\"id\":\"mock1\",\"object\":\"chat.completion.chunk\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"from-mock\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("hi"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&mock_server)
            .await;
        let base = format!("{}/v1", mock_server.uri().trim_end_matches('/'));
        (mock_server, base)
    });

    let _project_dir = fixtures::temp_project_with_files(&[("stub.rs", "")]);
    let root = _project_dir.path().to_path_buf();
    let mut h = TuiTestHarness::new_with_root(120, 40, root);
    h.app.chat.set_api_base_for_test(base);
    h.app.focused = Pane::Chat;
    h.app.chat.set_input_for_test("hi");
    let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(h.app.handle_event(enter).is_continue());

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut saw_delta = false;
    while std::time::Instant::now() < deadline {
        match h.recv_tui_event_timeout(Duration::from_millis(200)) {
            Some(TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, s))) if s.contains("from-mock") => {
                saw_delta = true;
                break;
            }
            Some(TuiEvent::Chat(ChatUiEvent::Error(0, e))) => {
                panic!("chat error from mock flow: {e}");
            }
            Some(_) => continue,
            None => continue,
        }
    }

    assert!(
        saw_delta,
        "timed out waiting for AssistantDelta from wiremock"
    );
}
