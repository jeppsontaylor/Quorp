use super::*;

fn sample() -> PermissionPrompt {
    PermissionPrompt {
        tool: "run_command".into(),
        command_repr: "cargo test -p quorp_term".into(),
        cwd: "crates/quorp_term".into(),
        sandbox: "tmp-copy".into(),
        rationale: "validate slash parser".into(),
    }
}

#[test]
fn no_color_includes_options() {
    let s = render_permission_modal(&sample(), ColorCapability::NoColor);
    assert!(s.contains("[y] approve once"));
    assert!(s.contains("[n] deny"));
    assert!(s.contains("cargo test"));
}

#[test]
fn truecolor_uses_red_for_deny() {
    let s = render_permission_modal(&sample(), ColorCapability::TrueColor);
    assert!(s.contains("\x1b[38;2"));
    assert!(s.contains("[n] deny"));
}
