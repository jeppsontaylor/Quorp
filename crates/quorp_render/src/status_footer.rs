//! Three-segment status footer: left (model/provider/mode), center
//! (state-machine phase pill), right (token/cost/task usage).

use crate::caps::ColorCapability;
use crate::palette::{ACCENT_CYAN, ACCENT_VIOLET, ACCENT_YELLOW, FG_TEXT, RESET};

#[derive(Debug, Clone)]
pub struct StatusFooter {
    pub model_provider: String,
    pub mode_label: String,
    pub phase_pill: String,
    pub usage_summary: String,
}

pub fn render_status_footer(footer: &StatusFooter, color: ColorCapability) -> String {
    if matches!(color, ColorCapability::NoColor) {
        return format!(
            "[{} | {}] {}  {}",
            footer.model_provider, footer.mode_label, footer.phase_pill, footer.usage_summary
        );
    }
    let mut out = String::new();
    out.push_str(&ACCENT_CYAN.fg());
    out.push_str(&footer.model_provider);
    out.push(' ');
    out.push_str(&ACCENT_YELLOW.fg());
    out.push_str(&footer.mode_label);
    out.push(' ');
    out.push_str(&ACCENT_VIOLET.fg());
    out.push_str(&footer.phase_pill);
    out.push_str(&FG_TEXT.fg());
    out.push_str("  ");
    out.push_str(&footer.usage_summary);
    out.push_str(RESET);
    out
}
#[cfg(test)]
#[path = "../../../testing/quorp_render/status_footer/tests.rs"]
mod tests;
