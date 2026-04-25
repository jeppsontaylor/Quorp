use crossterm::event::{KeyCode, KeyModifiers};

use crate::quorp::tui::app::Overlay;
use crate::quorp::tui::model_registry::{self, push_test_model_config_root};

use super::fixtures;
use super::harness::TuiTestHarness;

#[test]
fn model_picker_enter_writes_active_model_under_test_config_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let _guard = push_test_model_config_root(root.clone());

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

    let path = root.join(".config/quorp-tui/active_model.txt");
    let saved = std::fs::read_to_string(&path).expect("saved model file");
    assert_eq!(saved.trim(), picked);
    let resolved = model_registry::get_saved_model().expect("saved broker model");
    assert_eq!(resolved.id, picked.as_str());
}

#[test]
fn model_picker_enter_cloud_registry_id_skips_active_model_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_home = tmp.path().to_path_buf();
    let _guard = push_test_model_config_root(config_home.clone());

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

#[test]
fn model_picker_persists_and_restores_registry_chat_model_selection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_home = tmp.path().to_path_buf();
    let _guard = push_test_model_config_root(config_home.clone());

    let models = vec![
        "open_ai/registry-flow-test".to_string(),
        "anthropic/registry-flow-other".to_string(),
        "openai-compatible/registry-flow-third".to_string(),
    ];
    let mut harness = TuiTestHarness::new_with_registry_chat(
        120,
        40,
        fixtures::fixture_project_root(),
        models.clone(),
        0,
    );
    harness.key_press(KeyCode::Char('m'), KeyModifiers::CONTROL);
    harness.assert_overlay(Overlay::ModelPicker);
    harness.key_press(KeyCode::Down, KeyModifiers::NONE);
    harness.key_press(KeyCode::Down, KeyModifiers::NONE);
    harness.key_press(KeyCode::Enter, KeyModifiers::NONE);

    let path = config_home.join(".config/quorp-tui/default_chat_model.txt");
    let saved = std::fs::read_to_string(&path).expect("saved chat model file");
    assert_eq!(saved.trim(), "openai-compatible/registry-flow-third");

    let restored = TuiTestHarness::new_with_registry_chat(
        120,
        40,
        fixtures::fixture_project_root(),
        models,
        0,
    );
    assert_eq!(restored.app.chat.model_index_for_test(), 2);
}
