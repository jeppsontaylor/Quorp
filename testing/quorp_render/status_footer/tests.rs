use super::*;

fn sample() -> StatusFooter {
    StatusFooter {
        model_provider: "qwen3-coder@nvidia".into(),
        mode_label: "Act".into(),
        phase_pill: "thinking".into(),
        usage_summary: "ctx 12.4k/64k".into(),
    }
}

#[test]
fn no_color_uses_brackets() {
    let s = render_status_footer(&sample(), ColorCapability::NoColor);
    assert!(s.contains("[qwen3-coder@nvidia | Act]"));
}

#[test]
fn truecolor_emits_escapes() {
    let s = render_status_footer(&sample(), ColorCapability::TrueColor);
    assert!(s.contains("\x1b[38;2"));
    assert!(s.contains("qwen3-coder@nvidia"));
    assert!(s.ends_with("\x1b[0m"));
}
