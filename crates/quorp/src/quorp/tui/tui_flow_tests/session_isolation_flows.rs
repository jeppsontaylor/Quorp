use super::harness::TuiTestHarness;
use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::{ChatMessage, ChatUiEvent};

#[test]
fn cross_talk_tokens_stay_in_originating_session() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;

    let theme = h.app.theme;
    h.app.chat.new_chat_session(&theme); // session 1
    assert_eq!(h.app.chat.active_session_index(), 1);

    // Switch to session 0
    h.app.chat.activate_chat_session(0, &theme);
    h.app
        .chat
        .seed_messages_for_test(vec![ChatMessage::Assistant(String::new())]);

    h.app.chat.activate_chat_session(1, &theme);
    h.app
        .chat
        .seed_messages_for_test(vec![ChatMessage::Assistant(String::new())]);

    // Make session 0 active in UI
    h.app.chat.activate_chat_session(0, &theme);

    h.apply_chat_event(ChatUiEvent::AssistantDelta(1, "tok".into()));

    h.app.chat.activate_chat_session(0, &theme);
    assert_eq!(h.app.chat.last_assistant_text_for_test(), Some("")); // session 0 unchanged
    h.app.chat.activate_chat_session(1, &theme);
    assert_eq!(h.app.chat.last_assistant_text_for_test(), Some("tok"));
}

#[test]
fn stream_finished_clears_streaming_on_correct_session() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;

    let theme = h.app.theme;
    h.app.chat.new_chat_session(&theme);

    h.app.chat.activate_chat_session(0, &theme);
    h.app.chat.set_streaming_for_test(true);

    h.app.chat.activate_chat_session(1, &theme);
    h.app.chat.set_streaming_for_test(true);

    h.apply_chat_event(ChatUiEvent::StreamFinished(0));

    h.app.chat.activate_chat_session(0, &theme);
    assert!(!h.app.chat.is_streaming());

    h.app.chat.activate_chat_session(1, &theme);
    assert!(h.app.chat.is_streaming());
}

#[test]
fn error_event_routes_to_correct_session() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    let theme = h.app.theme;
    h.app.chat.new_chat_session(&theme);

    h.app.chat.activate_chat_session(0, &theme);
    h.app
        .chat
        .seed_messages_for_test(vec![ChatMessage::Assistant(String::new())]);

    h.app.chat.activate_chat_session(1, &theme);
    h.app
        .chat
        .seed_messages_for_test(vec![ChatMessage::Assistant(String::new())]);

    h.apply_chat_event(ChatUiEvent::Error(1, "fail".into()));

    h.app.chat.activate_chat_session(0, &theme);
    assert_eq!(h.app.chat.last_assistant_text_for_test(), Some(""));

    h.app.chat.activate_chat_session(1, &theme);
    assert_eq!(
        h.app.chat.last_assistant_text_for_test(),
        Some("Error: fail")
    ); // The prefix depends on implementation
}

#[test]
fn command_output_routes_to_correct_session() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    let theme = h.app.theme;
    h.app.chat.new_chat_session(&theme);

    h.apply_chat_event(ChatUiEvent::CommandOutput(1, "line".into()));

    h.app.chat.activate_chat_session(0, &theme);
    assert_eq!(
        h.app.chat.command_output_lines_for_test(),
        Vec::<String>::new()
    );

    h.app.chat.activate_chat_session(1, &theme);
    assert_eq!(
        h.app.chat.command_output_lines_for_test(),
        vec!["line".to_string()]
    );
}

#[test]
fn full_chat_lifecycle() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;

    h.app.chat.set_input_for_test("hello");
    h.key_press(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );

    assert!(h.app.chat.is_streaming());

    for _ in 0..3 {
        h.apply_chat_event(ChatUiEvent::AssistantDelta(0, "A".into()));
    }

    h.apply_chat_event(ChatUiEvent::StreamFinished(0));
    assert!(!h.app.chat.is_streaming());
    assert_eq!(h.app.chat.last_assistant_text_for_test(), Some("AAA"));
}

#[test]
fn screenshot_chat_streaming() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Write a loop".into()),
        ChatMessage::Assistant("```rust\nfor".into()),
    ]);
    h.app.chat.set_streaming_for_test(true);
    if std::env::var("VISUAL_TEST_OUTPUT_DIR").is_ok() {
        h.save_screenshot("chat_streaming");
    }
}

#[test]
fn screenshot_chat_finished() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Write a loop".into()),
        ChatMessage::Assistant("```rust\nfor i in 0..10 {\n    println!(\"{i}\");\n}\n```".into()),
    ]);
    if std::env::var("VISUAL_TEST_OUTPUT_DIR").is_ok() {
        h.save_screenshot("chat_complete");
    }
}
