use crate::quorp::tui::app::{Pane, Overlay};
use super::harness::TuiTestHarness;

fn diff_baseline(harness: &mut TuiTestHarness, name: &str) {
    let img = harness.screenshot();
    let baseline_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/quorp/tui/tui_flow_tests/baselines");
    let baseline_path = baseline_dir.join(format!("{}.png", name));

    if std::env::var("UPDATE_BASELINES").is_ok() {
        std::fs::create_dir_all(&baseline_dir).unwrap();
        img.save(&baseline_path).unwrap();
        println!("updated baseline {}", name);
        return;
    }

    if !baseline_path.exists() {
        panic!("Baseline {} missing. Run with UPDATE_BASELINES=1", name);
    }

    let baseline = image::open(&baseline_path).unwrap().to_rgba8();
    let fraction = crate::quorp::tui::buffer_png::pixel_mismatch_fraction(&baseline, &img).unwrap();
    assert!(fraction < 0.05, "Visual regression for {}: mismatch fraction {}", name, fraction);
}

#[test]
fn baseline_default_workspace() {
    let mut h = TuiTestHarness::new(120, 40);
    h.draw();
    diff_baseline(&mut h, "baseline_default_workspace");
}

#[test]
fn baseline_help_overlay() {
    let mut h = TuiTestHarness::new(120, 40);
    h.key_press(crossterm::event::KeyCode::Char('?'), crossterm::event::KeyModifiers::NONE);
    assert_eq!(h.app.overlay, Overlay::Help);
    
    h.draw();
    diff_baseline(&mut h, "baseline_help_overlay");
}

#[test]
fn baseline_model_picker() {
    let mut h = TuiTestHarness::new(120, 40);
    h.app.focused = Pane::Chat;
    h.key_press(crossterm::event::KeyCode::Char('m'), crossterm::event::KeyModifiers::CONTROL);
    assert_eq!(h.app.overlay, Overlay::ModelPicker);
    
    h.draw();
    diff_baseline(&mut h, "baseline_model_picker");
}
