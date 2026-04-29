use super::*;

fn step(name: &str, detail: &str, status: SplashStatus) -> SplashStep {
    SplashStep {
        name: name.into(),
        detail: detail.into(),
        status,
    }
}

#[test]
fn no_color_renders_ascii_symbols() {
    let s = render_splash(
        "quorp",
        &[step("workspace", "~/q", SplashStatus::Done)],
        ColorCapability::NoColor,
    );
    assert!(s.contains("✓ workspace"));
    assert!(!s.contains("\x1b["));
}

#[test]
fn truecolor_includes_escapes() {
    let s = render_splash(
        "quorp",
        &[step("provider", "nvidia/qwen3", SplashStatus::Running)],
        ColorCapability::TrueColor,
    );
    assert!(s.contains("\x1b[38;2"));
    assert!(s.contains("provider"));
}
