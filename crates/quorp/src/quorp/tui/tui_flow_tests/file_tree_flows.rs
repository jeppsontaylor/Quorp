use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::fixtures;
use super::harness::TuiTestHarness;

#[test]
fn enter_on_file_loads_editor_pane() {
    let dir = fixtures::temp_project_with_files(&[
        ("readme.txt", "hello fixture"),
        ("sample.rs", "fn main() {}\n"),
    ]);
    let mut h = TuiTestHarness::new_with_root(100, 32, dir.path().to_path_buf());
    let _ = h.wait_path_index_ready(Duration::from_secs(6));
    h.app.focused = Pane::FileTree;
    h.draw();

    // Visible rows: worktree root, then `readme.txt`, then `sample.rs` (dirs-first, then name sort).
    // `selected_file` is only set after Enter on a file, so navigate by row count — not by `selected_file`.
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.draw();
    h.assert_buffer_contains("fn main()");
}
