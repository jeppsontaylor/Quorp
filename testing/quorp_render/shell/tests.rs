use super::*;

#[test]
fn composer_uses_chevron_not_quorp_prompt() {
    let rendered = render_composer(
        &ComposerView {
            prompt: ">".to_string(),
            buffer: "/plan".to_string(),
            blink_on: true,
        },
        ColorCapability::NoColor,
    );
    assert_eq!(rendered, "> /plan");
    assert!(!rendered.contains("quorp>"));
}

#[test]
fn slash_overlay_renders_palette_rows() {
    let lines = render_shell_overlay(
        &Some(ShellOverlay::SlashPalette {
            selected: 0,
            entries: vec![PaletteRow {
                value: "/plan".to_string(),
                detail: "command".to_string(),
                description: "Enter plan mode".to_string(),
            }],
        }),
        80,
        ColorCapability::NoColor,
    );
    assert_eq!(
        lines[0],
        "  > /plan              command      Enter plan mode"
    );
}
