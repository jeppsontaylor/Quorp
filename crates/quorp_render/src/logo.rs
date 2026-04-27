use image::{GenericImageView, imageops::FilterType};

use crate::caps::{ColorCapability, GraphicsCapability, RenderProfile};
use crate::palette::{ACCENT_CYAN, ACCENT_GREEN, ACCENT_YELLOW, BOLD, DIM, FG_TEXT, RESET, Rgb};

const QUORP_MASCOT_PNG: &[u8] = include_bytes!("../../../assets/images/quorp-mascot.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoMode {
    TerminalImage,
    AnsiHalfBlock,
    Wordmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogoRenderOptions {
    pub max_width_cells: u16,
    pub max_height_cells: u16,
    pub animate_once: bool,
}

impl Default for LogoRenderOptions {
    fn default() -> Self {
        Self {
            max_width_cells: 48,
            max_height_cells: 14,
            animate_once: true,
        }
    }
}

pub fn render_logo(
    options: LogoRenderOptions,
    profile: RenderProfile,
) -> Result<String, image::ImageError> {
    let mode = detect_logo_mode(profile);
    match mode {
        LogoMode::TerminalImage | LogoMode::AnsiHalfBlock => render_ansi_half_block_logo(
            options.max_width_cells,
            options.max_height_cells,
            profile.color,
        ),
        LogoMode::Wordmark => Ok(render_wordmark(profile.color)),
    }
}

pub fn detect_logo_mode(profile: RenderProfile) -> LogoMode {
    match profile.graphics {
        GraphicsCapability::Kitty | GraphicsCapability::Iterm2 | GraphicsCapability::Sixel => {
            LogoMode::TerminalImage
        }
        GraphicsCapability::AnsiBlocks => LogoMode::AnsiHalfBlock,
        GraphicsCapability::None => LogoMode::Wordmark,
    }
}

fn render_ansi_half_block_logo(
    max_width_cells: u16,
    max_height_cells: u16,
    color: ColorCapability,
) -> Result<String, image::ImageError> {
    if matches!(color, ColorCapability::NoColor) {
        return Ok(render_wordmark(color));
    }

    let image = image::load_from_memory(QUORP_MASCOT_PNG)?;
    let (source_width, source_height) = image.dimensions();
    let max_width = u32::from(max_width_cells.max(12));
    let max_pixel_height = u32::from(max_height_cells.max(4)) * 2;
    let scale = (max_width as f32 / source_width as f32)
        .min(max_pixel_height as f32 / source_height as f32)
        .min(1.0);
    let width = ((source_width as f32 * scale).round() as u32).max(8);
    let height = ((source_height as f32 * scale).round() as u32).max(4);
    let height = if height % 2 == 0 { height } else { height + 1 };
    let resized = image
        .resize_exact(width, height, FilterType::Triangle)
        .to_rgba8();

    let mut out = String::new();
    for y in (0..height).step_by(2) {
        for x in 0..width {
            let top = resized.get_pixel(x, y);
            let bottom = resized.get_pixel(x, y + 1);
            if top[3] < 12 && bottom[3] < 12 {
                out.push(' ');
                continue;
            }
            if top[3] < 12 {
                out.push_str(&rgb(bottom).fg());
                out.push('▄');
                out.push_str(RESET);
            } else if bottom[3] < 12 {
                out.push_str(&rgb(top).fg());
                out.push('▀');
                out.push_str(RESET);
            } else {
                out.push_str(&rgb(top).fg());
                out.push_str(&rgb(bottom).bg());
                out.push('▀');
                out.push_str(RESET);
            }
        }
        out.push('\n');
    }
    out.push_str(&render_wordmark(color));
    Ok(out)
}

fn rgb(pixel: &image::Rgba<u8>) -> Rgb {
    Rgb::new(pixel[0], pixel[1], pixel[2])
}

fn render_wordmark(color: ColorCapability) -> String {
    if matches!(color, ColorCapability::NoColor) {
        return "QUORP\nterminal agent online\n".to_string();
    }
    format!(
        "{}{}QUORP{}{}  terminal agent online{}",
        ACCENT_YELLOW.fg(),
        BOLD,
        RESET,
        DIM,
        RESET
    )
}

pub fn render_boot_card(
    workspace: &str,
    model: &str,
    sandbox: &str,
    profile: RenderProfile,
) -> String {
    let mut out = render_logo(LogoRenderOptions::default(), profile)
        .unwrap_or_else(|_| render_wordmark(profile.color));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let rows = [
        ("workspace", workspace),
        ("model", model),
        ("sandbox", sandbox),
        ("commands", "/plan /act /diff /status"),
    ];
    for (label, value) in rows {
        if matches!(profile.color, ColorCapability::NoColor) {
            out.push_str(&format!("✓ {:<9} {}\n", label, value));
        } else {
            out.push_str(&format!(
                "{}✓{} {}{:<9}{} {}{}{}\n",
                ACCENT_GREEN.fg(),
                RESET,
                ACCENT_CYAN.fg(),
                label,
                RESET,
                FG_TEXT.fg(),
                value,
                RESET
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
}
