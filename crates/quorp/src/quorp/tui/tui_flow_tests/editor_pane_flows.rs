use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::fixtures;
use super::harness::TuiTestHarness;

#[test]
fn page_down_scrolls_long_file() {
    let dir = fixtures::temp_project_with_files(&[(
        "long.rs",
        &(0..40)
            .map(|i| format!("// line {i}\n"))
            .collect::<String>(),
    )]);
    let mut h = TuiTestHarness::new_with_root(80, 24, dir.path().to_path_buf());
    let file = dir.path().join("long.rs");
    h.app.file_tree.set_selected_file(Some(file));
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_from_selected_file(h.app.file_tree.selected_file(), h.app.file_tree.root());
    assert_eq!(h.app.editor_pane.vertical_scroll_for_test(), 0);
    h.key_press(KeyCode::PageDown, KeyModifiers::NONE);
    assert!(h.app.editor_pane.vertical_scroll_for_test() > 0);
}

#[test]
fn home_resets_scroll() {
    let dir = fixtures::temp_project_with_files(&[(
        "x.rs",
        &(0..30).map(|i| format!("// {i}\n")).collect::<String>(),
    )]);
    let mut h = TuiTestHarness::new_with_root(80, 24, dir.path().to_path_buf());
    let file = dir.path().join("x.rs");
    h.app.file_tree.set_selected_file(Some(file));
    h.app.focused = Pane::EditorPane;
    h.app
        .editor_pane
        .sync_from_selected_file(h.app.file_tree.selected_file(), h.app.file_tree.root());
    h.key_press(KeyCode::PageDown, KeyModifiers::NONE);
    h.key_press(KeyCode::PageDown, KeyModifiers::NONE);
    assert!(h.app.editor_pane.vertical_scroll_for_test() > 0);
    h.key_press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(h.app.editor_pane.vertical_scroll_for_test(), 0);
}
