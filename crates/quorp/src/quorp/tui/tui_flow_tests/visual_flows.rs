//! Buffer-level checks for screens that also have PNG goldens elsewhere (`tests/tui_visual_regression.rs`).

use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Pane;

use super::harness::TuiTestHarness;

#[test]
fn welcome_fixture_buffer_contains_project_hint() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    h.assert_buffer_contains("fixture_project");
}

#[test]
fn harness_resize_keeps_status_bar_coherent() {
    let mut h = TuiTestHarness::new(80, 24);
    h.resize(120, 40);
    h.app.focused = Pane::EditorPane;
    h.draw();
    h.assert_status_contains("Mode: Preview");
}

#[test]
fn help_overlay_lists_vim_navigation_row() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.draw();
    h.assert_buffer_contains("Vim-style");
}

/// Resize from small (80×24) to large (200×60) redraws all panes without panic.
#[test]
fn resize_redraws_all_panes_without_panic() {
    let mut h = TuiTestHarness::new(80, 24);
    h.draw();
    h.assert_status_contains("Mode:");
    h.resize(200, 60);
    h.draw();
    h.assert_status_contains("Mode:");
    h.assert_buffer_contains("fixture_project");
}
