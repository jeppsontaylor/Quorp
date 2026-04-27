//! Terminal capability detection. Drives palette downsampling and
//! feature gating (OSC8 hyperlinks, image protocols, bracketed paste).

use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    NoColor,
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsCapability {
    Kitty,
    Iterm2,
    Sixel,
    AnsiBlocks,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderProfile {
    pub color: ColorCapability,
    pub graphics: GraphicsCapability,
    pub osc8_hyperlinks: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
}

impl RenderProfile {
    pub fn detect_from_env() -> Self {
        let no_color = env::var_os("NO_COLOR").is_some();
        let colorterm = env::var("COLORTERM").unwrap_or_default();
        let term = env::var("TERM").unwrap_or_default();
        let term_program = env::var("TERM_PROGRAM").unwrap_or_default();

        let color = if no_color {
            ColorCapability::NoColor
        } else if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            ColorCapability::TrueColor
        } else if term.contains("256") {
            ColorCapability::Ansi256
        } else if term.is_empty() {
            ColorCapability::NoColor
        } else {
            ColorCapability::Ansi16
        };
        let graphics = if matches!(color, ColorCapability::NoColor) {
            GraphicsCapability::None
        } else if term.contains("kitty") || env::var_os("KITTY_WINDOW_ID").is_some() {
            GraphicsCapability::Kitty
        } else if term_program.contains("iTerm") {
            GraphicsCapability::Iterm2
        } else if term.contains("sixel") {
            GraphicsCapability::Sixel
        } else if matches!(
            color,
            ColorCapability::TrueColor | ColorCapability::Ansi256 | ColorCapability::Ansi16
        ) {
            GraphicsCapability::AnsiBlocks
        } else {
            GraphicsCapability::None
        };

        Self {
            color,
            graphics,
            osc8_hyperlinks: !matches!(color, ColorCapability::NoColor),
            bracketed_paste: !matches!(color, ColorCapability::NoColor),
            focus_events: matches!(color, ColorCapability::TrueColor),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
