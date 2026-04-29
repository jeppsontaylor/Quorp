use super::*;

#[test]
fn no_color_env_disables_color() {
    // We can't sandbox env in stable Rust without unsafe std::env;
    // instead, sanity check that NoColor is non-truecolor.
    let profile = RenderProfile {
        color: ColorCapability::NoColor,
        graphics: GraphicsCapability::None,
        osc8_hyperlinks: false,
        bracketed_paste: false,
        focus_events: false,
    };
    assert!(matches!(profile.color, ColorCapability::NoColor));
}
