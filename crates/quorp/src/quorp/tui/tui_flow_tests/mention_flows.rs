//! Playwright-style flows for @ file mentions: full `TuiApp::handle_event` + `draw` + buffer inspection.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::fixtures;
use super::harness::TuiTestHarness;

fn harness_with_mention_file() -> (TuiTestHarness, tempfile::TempDir) {
    let dir = fixtures::temp_project_with_files(&[
        ("mention_target.rs", "// mention flow\n"),
        ("other.txt", "x\n"),
    ]);
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    assert!(
        h.wait_path_index_ready(Duration::from_secs(8)),
        "path index should be ready for @ mentions"
    );
    h.app.focused = Pane::Chat;
    (h, dir)
}

#[test]
fn app_typing_at_and_tab_inserts_file_link_in_composer() {
    let (mut h, _dir) = harness_with_mention_file();
    for ch in "@mention_target".chars() {
        h.key_press(KeyCode::Char(ch), KeyModifiers::NONE);
    }
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    assert!(
        h.app.chat.input_for_test().contains("mention_target"),
        "input: {:?}",
        h.app.chat.input_for_test()
    );
    assert!(
        h.app.chat.input_for_test().contains("file:"),
        "expected file URL in link, got {:?}",
        h.app.chat.input_for_test()
    );
}

#[test]
fn draw_after_mention_filter_shows_filename_in_buffer() {
    let (mut h, _dir) = harness_with_mention_file();
    for ch in "@mention".chars() {
        h.key_press(KeyCode::Char(ch), KeyModifiers::NONE);
    }
    h.draw();
    h.assert_buffer_contains("mention_target.rs");
}

#[test]
fn esc_dismisses_mention_popup_second_esc_quits() {
    let (mut h, _dir) = harness_with_mention_file();
    h.key_press(KeyCode::Char('@'), KeyModifiers::NONE);
    assert!(h.app.chat.mention_popup_open_for_test());
    h.key_press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!h.app.chat.mention_popup_open_for_test());
    assert!(h.key(KeyCode::Esc, KeyModifiers::NONE).is_break());
}

#[test]
fn mention_list_scrolls_after_many_down_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..12 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").expect("write");
    }
    std::fs::write(dir.path().join("needle.txt"), "n").expect("write");
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    assert!(h.wait_path_index_ready(Duration::from_secs(8)));
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('@'), KeyModifiers::NONE);
    h.key_press(KeyCode::Char('f'), KeyModifiers::NONE);
    assert!(h.app.chat.mention_match_count_for_test() > 8);
    for _ in 0..8 {
        h.key_press(KeyCode::Down, KeyModifiers::NONE);
    }
    assert!(
        h.app.chat.mention_scroll_top_for_test().unwrap_or(0) > 0,
        "popup should scroll after paging down the match list"
    );
    h.assert_focus(Pane::Chat);
}
