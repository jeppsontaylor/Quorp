use crossterm::event::{KeyCode, KeyModifiers};

use super::harness::TuiTestHarness;
use crate::quorp::tui::app::{Overlay, Pane};

#[test]
fn user_flow_smoke_navigation_and_overlays() {
    let mut harness = TuiTestHarness::new(120, 40);
    harness.draw();

    harness.key_press(KeyCode::Char('l'), KeyModifiers::CONTROL);
    assert_eq!(harness.app.focused, Pane::Chat);

    harness.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(harness.app.overlay, Overlay::Help);

    harness.key_press(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(harness.app.overlay, Overlay::None);

    harness.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    assert_eq!(harness.app.overlay, Overlay::ModelPicker);
}

#[test]
fn rust_screenshot_export_for_core_tui_flows() {
    let mut harness = TuiTestHarness::new(120, 40);

    let default_path = harness.save_screenshot("core_tui_default");
    assert!(default_path.exists(), "missing screenshot {:?}", default_path);

    harness.key_press(KeyCode::Char('?'), KeyModifiers::NONE);
    let help_path = harness.save_screenshot("core_tui_help_overlay");
    assert!(help_path.exists(), "missing screenshot {:?}", help_path);

    harness.key_press(KeyCode::Esc, KeyModifiers::NONE);
    harness.app.focused = Pane::Chat;
    harness.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    let models_path = harness.save_screenshot("core_tui_model_picker");
    assert!(models_path.exists(), "missing screenshot {:?}", models_path);
}
