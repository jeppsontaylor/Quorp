use crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;

use crate::quorp::tui::app::Overlay;
use crate::quorp::tui::model_registry::{self, set_test_model_config_root};

use super::fixtures;
use super::harness::TuiTestHarness;

struct ClearTestModelRoot;

impl Drop for ClearTestModelRoot {
    fn drop(&mut self) {
        set_test_model_config_root(None);
    }
}

#[test]
fn model_picker_enter_writes_active_model_under_test_config_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    set_test_model_config_root(Some(root.clone()));
    let _clear = ClearTestModelRoot;

    let mut h = TuiTestHarness::new(120, 40);
    h.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    h.assert_overlay(Overlay::ModelPicker);
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    h.key_press(KeyCode::Down, KeyModifiers::NONE);
    let picked = h.app.models_pane.entries[h.app.models_pane.selected_index]
        .registry_id
        .clone();
    h.key_press(KeyCode::Enter, KeyModifiers::NONE);
    h.assert_overlay(Overlay::None);

    let path = PathBuf::from(root).join(".config/quorp-tui/active_model.txt");
    let saved = std::fs::read_to_string(&path).expect("saved model file");
    assert_eq!(saved.trim(), picked);
    let resolved = model_registry::get_saved_model();
    assert_eq!(resolved.id, picked.as_str());
}

#[test]
fn model_picker_enter_cloud_registry_id_skips_active_model_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_home = tmp.path().to_path_buf();
    set_test_model_config_root(Some(config_home.clone()));
    let _clear = ClearTestModelRoot;

    let models = vec![
        "open_ai/registry-flow-test".to_string(),
        "anthropic/registry-flow-other".to_string(),
    ];
    let mut harness = TuiTestHarness::new_with_registry_chat(
        120,
        40,
        fixtures::fixture_project_root(),
        models,
        0,
    );
    harness.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    harness.assert_overlay(Overlay::ModelPicker);
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);
    harness.assert_overlay(Overlay::None);

    let path = config_home.join(".config/quorp-tui/active_model.txt");
    assert!(
        !path.exists(),
        "active_model.txt is for local SSD-MOE weights only, not provider/model ids"
    );
}
