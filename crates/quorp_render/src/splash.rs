//! Startup splash: 6-row checklist that streams in as phases complete.

use crate::caps::ColorCapability;
use crate::palette::{ACCENT_CYAN, ACCENT_GREEN, ACCENT_YELLOW, FG_TEXT, RESET};

#[derive(Debug, Clone)]
pub struct SplashStep {
    pub name: String,
    pub detail: String,
    pub status: SplashStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashStatus {
    Pending,
    Running,
    Done,
    Warn,
}

pub fn render_splash(title: &str, steps: &[SplashStep], color: ColorCapability) -> String {
    let mut out = String::new();
    if matches!(color, ColorCapability::NoColor) {
        out.push_str(title);
        out.push('\n');
        for step in steps {
            let symbol = match step.status {
                SplashStatus::Pending => "·",
                SplashStatus::Running => "*",
                SplashStatus::Done => "✓",
                SplashStatus::Warn => "!",
            };
            out.push_str(&format!("  {symbol} {:14} {}\n", step.name, step.detail));
        }
        return out;
    }

    out.push_str(&FG_TEXT.fg());
    out.push_str(title);
    out.push_str(RESET);
    out.push('\n');
    for step in steps {
        let (symbol, color_code) = match step.status {
            SplashStatus::Pending => ("·", ACCENT_CYAN),
            SplashStatus::Running => ("*", ACCENT_CYAN),
            SplashStatus::Done => ("✓", ACCENT_GREEN),
            SplashStatus::Warn => ("!", ACCENT_YELLOW),
        };
        out.push_str("  ");
        out.push_str(&color_code.fg());
        out.push_str(symbol);
        out.push(' ');
        out.push_str(&FG_TEXT.fg());
        out.push_str(&format!("{:<14} ", step.name));
        out.push_str(&step.detail);
        out.push_str(RESET);
        out.push('\n');
    }
    out
}
#[cfg(test)]
#[path = "../../../testing/quorp_render/splash/tests.rs"]
mod tests;
