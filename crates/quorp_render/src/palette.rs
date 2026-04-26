//! 24-bit truecolor palette + interpolation helpers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Render as ANSI 24-bit foreground escape, e.g. `\x1b[38;2;255;90;0m`.
    pub fn fg(self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }

    pub fn bg(self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.r, self.g, self.b)
    }
}

/// Linear-RGB interpolation. Approximate but visually pleasant for short
/// ranges; sRGB-aware lerp can swap in later.
pub fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    Rgb {
        r: lerp_u8(a.r, b.r, t),
        g: lerp_u8(a.g, b.g, t),
        b: lerp_u8(a.b, b.b, t),
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let af = a as f32;
    let bf = b as f32;
    (af + (bf - af) * t).round().clamp(0.0, 255.0) as u8
}

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

// Brand palette for the brilliant CLI:
pub const SHIMMER_COOL: Rgb = Rgb::new(0x6F, 0xE3, 0xFF);
pub const SHIMMER_WARM: Rgb = Rgb::new(0xC7, 0x7D, 0xFF);
pub const ACCENT_CYAN: Rgb = Rgb::new(0x00, 0xC8, 0xE6);
pub const ACCENT_VIOLET: Rgb = Rgb::new(0xA8, 0x7E, 0xFF);
pub const ACCENT_GREEN: Rgb = Rgb::new(0x39, 0xFF, 0x88);
pub const ACCENT_RED: Rgb = Rgb::new(0xFF, 0x4D, 0x6D);
pub const ACCENT_YELLOW: Rgb = Rgb::new(0xFF, 0xD1, 0x66);
pub const DIFF_ADD_BG: Rgb = Rgb::new(0x06, 0x2D, 0x1F);
pub const DIFF_DEL_BG: Rgb = Rgb::new(0x35, 0x10, 0x16);
pub const FG_TEXT: Rgb = Rgb::new(0xE6, 0xEA, 0xF2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fg_escape_is_correct() {
        let red = Rgb::new(255, 0, 0);
        assert_eq!(red.fg(), "\x1b[38;2;255;0;0m");
    }

    #[test]
    fn lerp_endpoints_unchanged() {
        let a = Rgb::new(0, 0, 0);
        let b = Rgb::new(255, 255, 255);
        assert_eq!(lerp_rgb(a, b, 0.0), a);
        assert_eq!(lerp_rgb(a, b, 1.0), b);
        let mid = lerp_rgb(a, b, 0.5);
        assert!(mid.r >= 126 && mid.r <= 129);
    }
}
