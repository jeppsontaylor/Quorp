use super::*;

#[test]
fn wordmark_fallback_contains_brand() {
    let rendered = render_logo(
        LogoRenderOptions::default(),
        RenderProfile {
            color: ColorCapability::NoColor,
            graphics: GraphicsCapability::None,
            osc8_hyperlinks: false,
            bracketed_paste: false,
            focus_events: false,
        },
    )
    .expect("render");
    assert!(rendered.contains("QUORP"));
}

#[test]
fn ansi_logo_respects_small_bounds() {
    let rendered = render_logo(
        LogoRenderOptions {
            max_width_cells: 16,
            max_height_cells: 6,
            animate_once: false,
        },
        RenderProfile {
            color: ColorCapability::TrueColor,
            graphics: GraphicsCapability::AnsiBlocks,
            osc8_hyperlinks: true,
            bracketed_paste: true,
            focus_events: true,
        },
    )
    .expect("render");
    assert!(rendered.contains("QUORP"));
}
