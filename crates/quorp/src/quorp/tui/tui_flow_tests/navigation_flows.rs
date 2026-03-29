use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::harness::TuiTestHarness;

#[test]
fn ctrl_h_from_editor_pane_focuses_file_tree() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::EditorPane;
    h.key_press(KeyCode::Char('h'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::FileTree);
}

#[test]
fn ctrl_l_from_file_tree_returns_to_last_left_pane() {
    let mut h = TuiTestHarness::new(80, 24);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Chat);
    h.key_press(KeyCode::Char('h'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::FileTree);
    h.key_press(KeyCode::Char('l'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Chat);
}

#[test]
fn ctrl_j_from_editor_pane_moves_to_terminal() {
    let mut h = TuiTestHarness::new(80, 24);
    h.assert_focus(Pane::EditorPane);
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Terminal);
}

#[test]
fn ctrl_j_from_terminal_moves_to_chat() {
    let mut h = TuiTestHarness::new(80, 24);
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Chat);
}

#[test]
fn ctrl_k_from_terminal_moves_to_editor_pane() {
    let mut h = TuiTestHarness::new(80, 24);
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Char('k'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::EditorPane);
}

/// Full pane cycle via Tab, then ShiftTab back to origin.
#[test]
fn full_pane_cycle_returns_to_origin() {
    let mut h = TuiTestHarness::new(80, 24);
    h.assert_focus(Pane::EditorPane);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Chat);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Agent);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::FileTree);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::EditorPane);

    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::FileTree);
    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::Agent);
    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::Chat);
    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::EditorPane);
}

/// Ctrl+H to file tree, then Ctrl+L back — verifies the return-to-last-left-pane contract.
#[test]
fn ctrl_h_then_ctrl_l_roundtrip() {
    let mut h = TuiTestHarness::new(80, 24);
    h.assert_focus(Pane::EditorPane);
    h.key_press(KeyCode::Char('h'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::FileTree);
    h.key_press(KeyCode::Char('l'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::EditorPane);
}

/// Ctrl+J down to terminal, Ctrl+K back up — vim J/K roundtrip from editor.
#[test]
fn vim_j_then_k_roundtrip_from_editor() {
    let mut h = TuiTestHarness::new(80, 24);
    h.assert_focus(Pane::EditorPane);
    h.key_press(KeyCode::Char('j'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Char('k'), KeyModifiers::CONTROL);
    h.assert_focus(Pane::EditorPane);
}
