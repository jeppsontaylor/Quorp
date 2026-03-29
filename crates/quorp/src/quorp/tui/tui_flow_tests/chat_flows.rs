//! Chat composer, model brackets, Ctrl+T sessions, and tab-strip close-all/delete via harness.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;
use crate::quorp::tui::chat::{ChatMessage, ChatUiEvent};

use super::harness::TuiTestHarness;

#[test]
fn bracket_keys_cycle_model_in_chat() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Chat;
    let before = h.app.chat.model_index_for_test();
    h.key_press(KeyCode::Char(']'), KeyModifiers::NONE);
    assert_ne!(h.app.chat.model_index_for_test(), before);
    h.assert_focus(Pane::Chat);
}

#[test]
fn ctrl_t_opens_new_chat_session() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.draw();
    h.assert_buffer_contains("Chat 1");
    h.key_press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    h.draw();
    h.assert_buffer_contains("Chat 2");
}

#[test]
fn harness_apply_chat_event_appends_assistant_delta() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("u".into()),
        ChatMessage::Assistant(String::new()),
    ]);
    h.app.chat.set_streaming_for_test(true);
    h.apply_chat_event(ChatUiEvent::AssistantDelta(0, "tok".into()));
    assert_eq!(h.app.chat.last_assistant_text_for_test(), Some("tok"));
}

/// Simulates streaming lines from [`crate::quorp::tui::command_bridge`] (or `command_runner`) before
/// `CommandFinished`. We do not apply `CommandFinished` here: it triggers an LLM follow-up round,
/// which would open the HTTP completion path in harnesses without a chat bridge.
#[test]
fn harness_apply_chat_event_command_output_appends_lines() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.set_running_command_for_test(true);
    assert!(h.app.chat.command_output_lines_for_test().is_empty());
    h.apply_chat_event(ChatUiEvent::CommandOutput(0, "line one".into()));
    h.apply_chat_event(ChatUiEvent::CommandOutput(0, "line two".into()));
    assert_eq!(
        h.app.chat.command_output_lines_for_test(),
        vec!["line one".to_string(), "line two".to_string()]
    );
    h.draw();
    h.assert_buffer_contains("line one");
}

#[test]
fn typing_in_chat_composer_updates_input() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('h'), KeyModifiers::NONE);
    h.key_press(KeyCode::Char('i'), KeyModifiers::NONE);
    assert_eq!(h.app.chat.input_for_test(), "hi");
}

#[test]
fn harness_draw_shows_two_chat_tab_labels_after_ctrl_t() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    h.draw();
    h.assert_buffer_contains("Chat 1");
    h.assert_buffer_contains("Chat 2");
}

#[test]
fn alt_up_left_cycles_chat_session_index() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(h.app.chat.active_session_index(), 1);
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    h.key_press(KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(h.app.chat.active_session_index(), 0);
    h.key_press(KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(h.app.chat.active_session_index(), 1);
}

#[test]
fn strip_focused_ctrl_shift_w_leaves_single_chat_tab() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    h.draw();
    h.assert_buffer_contains("Chat 2");
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    h.key_press(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    h.draw();
    h.assert_buffer_not_contains("Chat 2");
    h.assert_buffer_contains("Chat 1");
}

#[test]
fn strip_focused_delete_closes_active_chat_session() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(h.app.chat.active_session_index(), 1);
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    h.key_press(KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(h.app.chat.active_session_index(), 0);
}
