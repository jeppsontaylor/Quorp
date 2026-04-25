//! Chat composer, model brackets, Ctrl+T sessions, and tab-strip close-all/delete via harness.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;
use crate::quorp::tui::assistant_transcript;
use crate::quorp::tui::chat::{ChatMessage, ChatUiEvent};
use crate::quorp::tui::diagnostics;
use crate::quorp::tui::shell::{ShellGeometry, shell_composer_height_for_text};

use super::harness::TuiTestHarness;

fn long_chat_history() -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for index in 0..18 {
        messages.push(ChatMessage::User(format!("user message {index:02}")));
        messages.push(ChatMessage::Assistant(format!(
            "assistant line {index:02}\nassistant detail {index:02}\nassistant more {index:02}"
        )));
    }
    messages
}

fn assistant_feed_point(h: &mut TuiTestHarness, cols: u16, rows: u16) -> (u16, u16) {
    let shell = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, cols, rows));
    let geometry = ShellGeometry::for_state(ratatui::layout::Rect::new(0, 0, cols, rows), &shell);
    (geometry.center.x + 2, geometry.center.y + 4)
}

fn assistant_scrollbar_point(h: &mut TuiTestHarness, cols: u16, rows: u16) -> (u16, u16) {
    let shell = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, cols, rows));
    let geometry = ShellGeometry::for_state(ratatui::layout::Rect::new(0, 0, cols, rows), &shell);
    let inner = ratatui::layout::Rect::new(
        geometry.center.x.saturating_add(1),
        geometry.center.y.saturating_add(1),
        geometry.center.width.saturating_sub(2),
        geometry.center.height.saturating_sub(2),
    );
    let header_height = 2.min(inner.height);
    let composer_height = shell_composer_height_for_text(
        &shell.center.composer_text,
        inner.width,
        inner.height.saturating_sub(header_height),
    );
    let feed = ratatui::layout::Rect::new(
        inner.x,
        inner.y + header_height,
        inner.width,
        inner
            .height
            .saturating_sub(header_height)
            .saturating_sub(composer_height),
    );
    (
        feed.right().saturating_sub(1),
        feed.y + feed.height.saturating_sub(1) / 2,
    )
}

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

/// Simulates streaming lines from [`crate::quorp::tui::command_bridge`] before
/// `CommandFinished`. We do not apply `CommandFinished` here because it triggers the follow-up
/// assistant path, which is covered separately.
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
fn shell_composer_expands_for_long_prompt_and_resets_after_submit() {
    let mut h = TuiTestHarness::new(140, 40);
    h.app.focused = Pane::Chat;
    h.draw();

    let before = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 140, 40));
    let before_geometry =
        ShellGeometry::for_state(ratatui::layout::Rect::new(0, 0, 140, 40), &before);
    let before_inner = ratatui::layout::Rect::new(
        before_geometry.center.x.saturating_add(1),
        before_geometry.center.y.saturating_add(1),
        before_geometry.center.width.saturating_sub(2),
        before_geometry.center.height.saturating_sub(2),
    );
    let before_header = 2.min(before_inner.height);
    let before_height = shell_composer_height_for_text(
        &before.center.composer_text,
        before_inner.width,
        before_inner.height.saturating_sub(before_header),
    );

    for character in "This is a longer prompt that should wrap across multiple lines in the shell composer without feeling cramped."
        .chars()
    {
        h.key_press(KeyCode::Char(character), KeyModifiers::NONE);
    }
    h.draw();

    let expanded = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 140, 40));
    let expanded_geometry =
        ShellGeometry::for_state(ratatui::layout::Rect::new(0, 0, 140, 40), &expanded);
    let expanded_inner = ratatui::layout::Rect::new(
        expanded_geometry.center.x.saturating_add(1),
        expanded_geometry.center.y.saturating_add(1),
        expanded_geometry.center.width.saturating_sub(2),
        expanded_geometry.center.height.saturating_sub(2),
    );
    let expanded_header = 2.min(expanded_inner.height);
    let expanded_height = shell_composer_height_for_text(
        &expanded.center.composer_text,
        expanded_inner.width,
        expanded_inner.height.saturating_sub(expanded_header),
    );
    assert!(
        expanded_height > before_height,
        "expected expanded composer height {expanded_height} to exceed baseline {before_height}"
    );

    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.draw();

    let after = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 140, 40));
    let after_geometry =
        ShellGeometry::for_state(ratatui::layout::Rect::new(0, 0, 140, 40), &after);
    let after_inner = ratatui::layout::Rect::new(
        after_geometry.center.x.saturating_add(1),
        after_geometry.center.y.saturating_add(1),
        after_geometry.center.width.saturating_sub(2),
        after_geometry.center.height.saturating_sub(2),
    );
    let after_header = 2.min(after_inner.height);
    let after_height = shell_composer_height_for_text(
        &after.center.composer_text,
        after_inner.width,
        after_inner.height.saturating_sub(after_header),
    );
    assert_eq!(after.center.composer_text, "Streaming response...");
    assert_eq!(after_height, before_height);
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
    h.app.tab_strip_focus = Some(Pane::Chat);
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
    h.app.tab_strip_focus = Some(Pane::Chat);
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
    h.app.tab_strip_focus = Some(Pane::Chat);
    h.key_press(KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(h.app.chat.active_session_index(), 0);
}

#[test]
fn live_shell_renders_python_code_block_without_raw_fences() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Please write this python script".into()),
        ChatMessage::Assistant("```python\nprint('hello from quorp')\n```".into()),
    ]);

    h.draw();

    h.assert_buffer_contains("python");
    h.assert_buffer_contains("print('hello from quorp')");
    h.assert_buffer_not_contains("```python");
    h.assert_buffer_not_contains("```");
    h.assert_text_has_nondefault_fg("print('hello from quorp')");
    h.assert_text_bg(
        "print('hello from quorp')",
        h.app.theme.palette.code_block_bg,
    );
}

#[test]
fn submitting_chat_input_scrolls_assistant_feed_to_latest_user_message() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.draw();
    h.key_press(KeyCode::PageUp, KeyModifiers::NONE);
    assert!(!h.app.assistant_feed_follow_latest_for_test());

    h.app.chat.set_input_for_test("new prompt from submit");
    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.draw();

    assert!(h.app.assistant_feed_follow_latest_for_test());
    h.assert_buffer_contains("new prompt from submit");
}

#[test]
fn paging_up_disables_follow_and_streaming_delta_does_not_steal_position() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.app.chat.set_streaming_for_test(true);
    h.draw();

    let before = h.app.assistant_feed_scroll_top_for_test();
    h.key_press(KeyCode::PageUp, KeyModifiers::NONE);
    let paged = h.app.assistant_feed_scroll_top_for_test();
    assert!(paged < before);
    assert!(!h.app.assistant_feed_follow_latest_for_test());

    h.apply_chat_event(ChatUiEvent::AssistantDelta(0, "\nstream tail".into()));
    h.draw();

    assert_eq!(h.app.assistant_feed_scroll_top_for_test(), paged);
    assert!(!h.app.assistant_feed_follow_latest_for_test());
}

#[test]
fn mouse_wheel_scrolls_assistant_feed_from_pointer_location() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.draw();

    let before = h.app.assistant_feed_scroll_top_for_test();
    let point = assistant_feed_point(&mut h, 100, 24);
    h.mouse_scroll_up(point.0, point.1);

    assert!(h.app.assistant_feed_scroll_top_for_test() < before);
    assert!(!h.app.assistant_feed_follow_latest_for_test());
}

#[test]
fn mouse_wheel_down_to_bottom_reenables_follow_latest() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.draw();
    let point = assistant_feed_point(&mut h, 100, 24);
    h.mouse_scroll_up(point.0, point.1);
    assert!(!h.app.assistant_feed_follow_latest_for_test());

    for _ in 0..20 {
        h.mouse_scroll_down(point.0, point.1);
    }

    assert!(h.app.assistant_feed_follow_latest_for_test());
}

#[test]
fn clicking_assistant_scrollbar_jumps_feed_position() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.draw();

    let before = h.app.assistant_feed_scroll_top_for_test();
    let point = assistant_scrollbar_point(&mut h, 100, 24);
    h.mouse_left_down(point.0, point.1);

    assert_ne!(h.app.assistant_feed_scroll_top_for_test(), before);
}

#[test]
fn paging_down_to_bottom_reenables_follow_latest() {
    let mut h = TuiTestHarness::new(100, 24);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(long_chat_history());
    h.draw();

    h.key_press(KeyCode::PageUp, KeyModifiers::NONE);
    assert!(!h.app.assistant_feed_follow_latest_for_test());

    for _ in 0..20 {
        h.key_press(KeyCode::PageDown, KeyModifiers::NONE);
    }

    assert!(h.app.assistant_feed_follow_latest_for_test());
}

#[test]
fn help_overlay_mentions_assistant_feed_paging() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    let shell = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 120, 40));
    assert!(shell.status_hint.contains("Ctrl+k control deck"));
    assert!(shell.status_hint.contains("Alt+Enter open"));
}

#[test]
fn online_runtime_header_is_bright_and_pulses() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    let shell = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 120, 40));
    assert_eq!(
        shell.center.runtime_state_kind,
        crate::quorp::tui::shell::ShellRuntimeStateKind::Online
    );
    assert_eq!(shell.center.runtime_state_label, "Online");
    h.assert_text_bg(" Online ", h.app.theme.palette.runtime_online);
    h.assert_text_fg(" Online ", h.app.theme.palette.canvas_bg);
    h.draw();
    let refreshed = h
        .app
        .shell_state_snapshot(ratatui::layout::Rect::new(0, 0, 120, 40));
    assert_eq!(refreshed.center.runtime_state_label, "Online");
}

#[test]
fn typing_after_python_response_reuses_parse_and_highlight_cache() {
    assistant_transcript::reset_test_counters();
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Please write this python script".into()),
        ChatMessage::Assistant("```python\nprint('hello from quorp')\n```".into()),
    ]);

    h.draw();
    assert!(assistant_transcript::parse_count_for_test() > 0);
    let highlight_before = assistant_transcript::highlight_count_for_test();

    h.key_press(KeyCode::Char('x'), KeyModifiers::NONE);
    h.draw();

    assert_eq!(
        assistant_transcript::highlight_count_for_test(),
        highlight_before
    );
}

#[test]
fn diagnostics_log_python_code_classification() {
    diagnostics::clear_events_for_test();
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Please write this python script".into()),
        ChatMessage::Assistant("```python\nprint('hello from quorp')\n```".into()),
    ]);

    h.draw();

    let events = diagnostics::take_events_for_test();
    assert!(events.iter().any(|event| {
        event.get("event").and_then(|value| value.as_str()) == Some("assistant.segment_classified")
            && event.get("segment_kind").and_then(|value| value.as_str()) == Some("code")
            && event.get("language").and_then(|value| value.as_str()) == Some("python")
    }));
}

#[test]
fn repeated_draws_do_not_poll_runtime_health_every_frame() {
    let mut h = TuiTestHarness::new(120, 40);
    let before = h.app.ssd_moe.poll_health_count_for_test();
    h.app
        .set_last_runtime_health_poll_at_for_test(std::time::Instant::now());
    h.draw();
    h.draw();
    assert_eq!(h.app.ssd_moe.poll_health_count_for_test(), before);
}

#[test]
#[ignore = "manual perf smoke test"]
fn typing_perf_smoke_with_large_python_transcript() {
    assistant_transcript::reset_test_counters();
    let mut h = TuiTestHarness::new(140, 45);
    h.app.focused = Pane::Chat;
    let body = (0..200)
        .map(|index| format!("print('line {index}')"))
        .collect::<Vec<_>>()
        .join("\n");
    h.app.chat.seed_messages_for_test(vec![
        ChatMessage::User("Write python".into()),
        ChatMessage::Assistant(format!("```python\n{body}\n```")),
    ]);
    h.draw();

    let started_at = std::time::Instant::now();
    for _ in 0..25 {
        h.key_press(KeyCode::Char('x'), KeyModifiers::NONE);
        h.draw();
    }
    let elapsed_ms = started_at.elapsed().as_millis();
    println!("typing perf smoke elapsed_ms={elapsed_ms}");
}
