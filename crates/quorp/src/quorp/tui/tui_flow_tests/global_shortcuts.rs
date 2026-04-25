use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::{Overlay, Pane};
use crate::quorp::tui::ssd_moe_tui::ModelStatus;

use super::harness::TuiTestHarness;

#[test]
fn question_opens_help_overlay() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.assert_overlay(Overlay::Help);
    h.draw();
    h.assert_buffer_contains("Keybindings");
}

#[test]
fn esc_dismisses_help_overlay() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.assert_overlay(Overlay::Help);
    h.key_press(KeyCode::Esc, KeyModifiers::NONE);
    h.assert_overlay(Overlay::None);
}

#[test]
fn question_toggles_help_overlay() {
    let mut h = TuiTestHarness::new(120, 40);
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.assert_overlay(Overlay::Help);
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.assert_overlay(Overlay::None);
}

#[test]
fn mouse_click_dismisses_help() {
    let mut h = TuiTestHarness::new(232, 64);
    h.draw();
    h.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    h.assert_overlay(Overlay::Help);
    h.mouse_left_down(5, 5);
    h.assert_overlay(Overlay::None);
}

#[test]
fn ctrl_m_toggles_model_picker() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    h.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    h.assert_overlay(Overlay::ModelPicker);
    h.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    h.assert_overlay(Overlay::None);
}

#[test]
fn ctrl_s_sets_flash_moe_ready_when_running() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.ssd_moe = crate::quorp::tui::ssd_moe_tui::SsdMoeManager::new_detached_for_test(59_999);
    h.app.ssd_moe.set_status_for_test(ModelStatus::Running);
    h.draw();
    assert!(
        matches!(h.app.ssd_moe.status(), ModelStatus::Running),
        "fixture app should report running flash status for tests"
    );
    h.key_press(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(matches!(h.app.ssd_moe.status(), ModelStatus::Ready));
    assert!(
        h.app.ssd_moe.last_transition_reason().is_some(),
        "Ctrl+S should record a transition reason"
    );
}

#[test]
fn tab_cycles_focus_forward() {
    let mut h = TuiTestHarness::new(80, 24);
    h.assert_focus(Pane::EditorPane);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Char('g'), KeyModifiers::CONTROL);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Chat);
}

#[test]
fn tab_stays_in_terminal_while_capture_mode_is_active() {
    let mut h = TuiTestHarness::new(80, 24);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Terminal);
    h.key_press(KeyCode::Tab, KeyModifiers::NONE);
    h.assert_focus(Pane::Terminal);
}

#[test]
fn shift_tab_cycles_focus_backward() {
    let mut h = TuiTestHarness::new(80, 24);
    h.key_press(KeyCode::Tab, KeyModifiers::SHIFT);
    h.assert_focus(Pane::FileTree);
}

#[test]
fn ctrl_c_quits_from_editor_pane() {
    let mut h = TuiTestHarness::new(80, 24);
    let flow = h.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(flow.is_break());
}

#[test]
fn ctrl_c_does_not_quit_from_terminal() {
    let mut h = TuiTestHarness::new(80, 24);
    h.app.focused = Pane::Terminal;
    let flow = h.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(flow.is_continue());
}
