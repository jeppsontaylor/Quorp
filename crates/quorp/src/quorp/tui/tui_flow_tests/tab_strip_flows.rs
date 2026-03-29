//! Tab-strip sub-focus (Alt+Up), code/chat multi-tab cycling, Delete, Ctrl+W / Ctrl+Shift+W.
//! Uses [`TuiTestHarness`](super::harness::TuiTestHarness): scripted keys + `draw` + buffer/state checks.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::fixtures;
use super::harness::TuiTestHarness;

#[test]
fn alt_up_then_left_cycles_code_tabs() {
    let dir = fixtures::temp_project_with_files(&[
        ("a.rs", "// a\n"),
        ("b.rs", "// b\n"),
    ]);
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    let root = dir.path();
    h.app.focused = Pane::EditorPane;
    h.app.editor_pane.sync_tree_selection(Some(&root.join("a.rs")), root);
    h.app.editor_pane.sync_tree_selection(Some(&root.join("b.rs")), root);
    h.app
        .file_tree
        .set_selected_file(Some(root.join("b.rs")));
    h.draw();
    h.app.tab_strip_focus = None;
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    assert!(h.app.tab_strip_focus.is_some());
    let tab_before = h.app.editor_pane.active_tab_index();
    h.key_press(KeyCode::Left, KeyModifiers::NONE);
    let tab_after = h.app.editor_pane.active_tab_index();
    assert_ne!(tab_before, tab_after);
    h.key_press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.app.tab_strip_focus.is_none());
}

#[test]
fn alt_up_chat_strip_esc_clears_focus() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(h.app.tab_strip_focus, Some(Pane::Chat));
    h.key_press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.app.tab_strip_focus.is_none());
}

#[test]
fn harness_draw_shows_two_editor_pane_tab_labels() {
    let dir = fixtures::temp_project_with_files(&[
        ("alpha.rs", "// a\n"),
        ("beta.rs", "// b\n"),
    ]);
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    let root = dir.path();
    h.app.focused = Pane::EditorPane;
    h.app
        .file_tree
        .set_selected_file(Some(root.join("alpha.rs")));
    h.app.editor_pane.sync_tree_selection(Some(&root.join("alpha.rs")), root);
    h.app.editor_pane.sync_tree_selection(Some(&root.join("beta.rs")), root);
    h.app
        .editor_pane
        .ensure_active_loaded(root);
    h.draw();
    h.assert_buffer_contains("alpha.rs");
    h.assert_buffer_contains("beta.rs");
}

#[test]
fn strip_focused_delete_closes_active_code_tab() {
    let dir = fixtures::temp_project_with_files(&[
        ("a.rs", "//\n"),
        ("b.rs", "//\n"),
    ]);
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    let root = dir.path();
    h.app.focused = Pane::EditorPane;
    h.app.editor_pane.sync_tree_selection(Some(&root.join("a.rs")), root);
    h.app.editor_pane.sync_tree_selection(Some(&root.join("b.rs")), root);
    assert_eq!(h.app.editor_pane.tab_count(), 3);
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    h.key_press(KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(h.app.editor_pane.tab_count(), 2);
}

#[test]
fn strip_focused_ctrl_shift_w_closes_all_code_tabs_to_welcome() {
    let dir = fixtures::temp_project_with_files(&[("only.rs", "//\n")]);
    let mut h = TuiTestHarness::new_with_root(120, 40, dir.path().to_path_buf());
    let root = dir.path();
    h.app.focused = Pane::EditorPane;
    h.app.editor_pane.sync_tree_selection(Some(&root.join("only.rs")), root);
    assert!(h.app.editor_pane.tab_count() >= 2);
    h.key_press(KeyCode::Up, KeyModifiers::ALT);
    h.key_press(
        KeyCode::Char('w'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(h.app.editor_pane.tab_count(), 1);
    h.draw();
    h.assert_buffer_contains("Welcome");
}
