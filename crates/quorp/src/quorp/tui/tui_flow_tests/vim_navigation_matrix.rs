use crate::quorp::tui::app::Pane;
use super::harness::TuiTestHarness;
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn ctrl_l_from_editor_goes_to_right_pane() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::EditorPane;
    h.key_press(KeyCode::Char('l'), KeyModifiers::CONTROL);
    assert_eq!(h.app.focused, h.app.right_pane);
}

#[test]
fn ctrl_l_from_terminal_goes_to_right_pane() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Terminal;
    h.key_press(KeyCode::Char('l'), KeyModifiers::CONTROL);
    assert_eq!(h.app.focused, h.app.right_pane);
}

#[test]
fn ctrl_k_from_chat_goes_to_terminal() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert_eq!(h.app.focused, Pane::Terminal);
}

#[test]
fn ctrl_j_from_chat_goes_to_agent() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(h.app.focused, Pane::Agent);
}

#[test]
fn ctrl_j_from_terminal_goes_to_chat() {
    let mut h = TuiTestHarness::new(100, 40);
    h.app.focused = Pane::Terminal;
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(h.app.focused, Pane::Chat);
}

#[test]
fn exhaustive_5_pane_vim_grid() {
    let mut h = TuiTestHarness::new(100, 40);

    // Initial state assumption
    h.app.focused = Pane::EditorPane;

    let moves = vec![
        // h: Editor -> FileTree
        (KeyCode::Char('h'), Pane::FileTree),
        // l: FileTree -> Editor (last_left_pane)
        (KeyCode::Char('l'), Pane::EditorPane),
        // j: Editor -> Terminal
        (KeyCode::Char('j'), Pane::Terminal),
        // j: Terminal -> Chat
        (KeyCode::Char('j'), Pane::Chat),
        // j: Chat -> Agent
        (KeyCode::Char('j'), Pane::Agent),
        // l: Agent -> right_pane (assume Chat since right_pane=Chat)
        (KeyCode::Char('l'), Pane::Chat),
        // k: Chat -> Terminal
        (KeyCode::Char('k'), Pane::Terminal),
        // k: Terminal -> Editor
        (KeyCode::Char('k'), Pane::EditorPane),
        // l: Editor -> right_pane
        (KeyCode::Char('l'), Pane::Chat),
        // h: Chat -> file tree
        (KeyCode::Char('h'), Pane::FileTree),
    ];

    for (key, expected_pane) in moves {
        h.key_press(key, KeyModifiers::CONTROL);
        assert_eq!(h.app.focused, expected_pane, "Failed after moving with Ctrl+{:?}", key);
    }
}
