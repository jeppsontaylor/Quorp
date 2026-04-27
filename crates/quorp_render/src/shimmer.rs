//! Oscillating shimmer renderer for active-turn indicators.
//!
//! Each visible character at column `i` takes a colour from a sine-shifted
//! gradient driven by elapsed wall time `t`. The renderer is pure — no
//! crossterm — so frame snapshots are deterministic for tests.

use unicode_width::UnicodeWidthStr;

use crate::caps::ColorCapability;
use crate::palette::{RESET, SHIMMER_COOL, SHIMMER_WARM, lerp_rgb};

#[derive(Debug, Clone, Copy)]
pub struct ShimmerStyle {
    /// Phase shift per column. Larger spreads the gradient across more
    /// characters; default 0.35 matches Claude/Codex CLI cadence.
    pub phase_per_column: f32,
    /// Time scale. Larger speeds up the oscillation. Default 4.0 → 18 fps.
    pub time_scale: f32,
}

impl Default for ShimmerStyle {
    fn default() -> Self {
        Self {
            phase_per_column: 0.35,
            time_scale: 4.0,
        }
    }
}

/// Render a shimmer frame for the given verb at time `t`. `colors`
/// determines whether we degrade to braille spinner / dimmed text.
pub fn render_shimmer(verb: &str, t: f32, style: ShimmerStyle, colors: ColorCapability) -> String {
    if matches!(colors, ColorCapability::NoColor) {
        return braille_fallback(verb, t);
    }
    let mut out = String::with_capacity(verb.len() * 24);
    for (i, ch) in verb.chars().enumerate() {
        let theta = (i as f32) * style.phase_per_column - t * style.time_scale;
        let m = 0.5 + 0.5 * theta.sin();
        let rgb = match colors {
            ColorCapability::TrueColor => lerp_rgb(SHIMMER_COOL, SHIMMER_WARM, m),
            ColorCapability::Ansi256 => quantize_to_256(lerp_rgb(SHIMMER_COOL, SHIMMER_WARM, m)),
            _ => SHIMMER_COOL,
        };
        out.push_str(&rgb.fg());
        out.push(ch);
    }
    out.push_str(RESET);
    out
}

fn quantize_to_256(rgb: crate::palette::Rgb) -> crate::palette::Rgb {
    // Simple 6x6x6 cube quantization, returned as an Rgb. The escape
    // emitted is still 24-bit because it stays visually faithful even on
    // 256-colour terminals. A future pass can emit the 256-colour escape
    // form.
    crate::palette::Rgb::new(
        (rgb.r as u16 * 5 / 255 * 51) as u8,
        (rgb.g as u16 * 5 / 255 * 51) as u8,
        (rgb.b as u16 * 5 / 255 * 51) as u8,
    )
}

fn braille_fallback(verb: &str, t: f32) -> String {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = ((t * 10.0).abs() as usize) % FRAMES.len();
    format!("{} {}", FRAMES[idx], verb)
}

/// Width of the rendered string in display columns. Used to size the
/// dirty rectangle for the live region repaint.
pub fn shimmer_visible_width(verb: &str) -> usize {
    UnicodeWidthStr::width(verb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_frame_contains_escapes_and_verb_chars() {
        let s = render_shimmer(
            "Cogitating",
            0.5,
            ShimmerStyle::default(),
            ColorCapability::TrueColor,
        );
        assert!(s.contains("\x1b[38;2"));
        assert!(s.contains('C'));
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn no_color_falls_back_to_braille() {
        let s = render_shimmer(
            "Reading",
            0.0,
            ShimmerStyle::default(),
            ColorCapability::NoColor,
        );
        assert!(s.contains("Reading"));
        assert!(
            s.starts_with('⠋') || s.starts_with('⠙') || s.starts_with('⠹') || s.starts_with('⠸')
        );
    }

    #[test]
    fn frame_at_zero_time_has_stable_first_color() {
        let a = render_shimmer(
            "X",
            0.0,
            ShimmerStyle::default(),
            ColorCapability::TrueColor,
        );
        let b = render_shimmer(
            "X",
            0.0,
            ShimmerStyle::default(),
            ColorCapability::TrueColor,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn width_is_grapheme_aware() {
        assert_eq!(shimmer_visible_width("hello"), 5);
        // Wide kana counts as 2 columns each.
        assert_eq!(shimmer_visible_width("こんにちは"), 10);
    }
}
