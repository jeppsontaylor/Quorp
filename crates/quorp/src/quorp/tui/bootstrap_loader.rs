use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use image::GenericImageView;
use image::imageops::{FilterType, resize};
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::quorp::tui::shell::BrandArtCell;
use crate::quorp::tui::theme::Theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapLayoutMode {
    Compact,
    Standard,
    Full,
    Cinema,
}

impl BootstrapLayoutMode {
    pub fn for_area(area: Rect) -> Self {
        match (area.width, area.height) {
            (..=100, _) | (_, ..=30) => Self::Compact,
            (..=139, _) | (_, ..=39) => Self::Standard,
            (..=179, _) | (_, ..=49) => Self::Full,
            _ => Self::Cinema,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BootstrapWordmarkLine {
    pub text: String,
    pub offset: i16,
}

#[derive(Clone, Debug)]
pub struct BootstrapWordmark {
    pub lines: Vec<BootstrapWordmarkLine>,
    pub accent_phase: usize,
}

#[derive(Clone, Debug)]
pub struct BootstrapMascotRaster {
    pub rows: Vec<Vec<BrandArtCell>>,
}

#[derive(Clone, Debug)]
pub struct BootstrapAnimationState {
    pub frame_index: usize,
    pub progress: f32,
    pub phase_label: String,
}

#[derive(Clone, Debug)]
pub struct BootstrapFrame {
    pub wordmark: BootstrapWordmark,
    pub mascot: BootstrapMascotRaster,
    pub phase_badge: String,
}

pub struct BootstrapLoader;
pub const BOOTSTRAP_REVEAL_FRAMES: usize = 18;
const EMBEDDED_BOOTSTRAP_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/bootstrap/quorp.png"
));

struct BootstrapAssetCache {
    loading_started: bool,
    decoded: Option<image::RgbaImage>,
    rows_by_target_height: HashMap<u32, Vec<Vec<BrandArtCell>>>,
}

impl BootstrapAssetCache {
    fn new() -> Self {
        Self {
            loading_started: false,
            decoded: None,
            rows_by_target_height: HashMap::new(),
        }
    }
}

impl BootstrapLoader {
    pub fn warm_assets_async() {
        ensure_async_asset_load();
    }

    pub fn frame(
        area: Rect,
        frame_index: usize,
        phase_label: impl Into<String>,
        theme: &Theme,
    ) -> BootstrapFrame {
        let layout_mode = BootstrapLayoutMode::for_area(area);
        let phase_label = phase_label.into();
        let clamped_frame_index = frame_index.min(BOOTSTRAP_REVEAL_FRAMES.saturating_sub(1));
        let progress = ((clamped_frame_index + 1) as f32 / BOOTSTRAP_REVEAL_FRAMES.max(1) as f32)
            .clamp(0.0, 1.0);
        let animation = BootstrapAnimationState {
            frame_index: clamped_frame_index,
            progress,
            phase_label: phase_label.clone(),
        };
        let mascot_rows = target_mascot_rows(area, layout_mode);
        BootstrapFrame {
            wordmark: wordmark(animation.frame_index + animation.phase_label.len()),
            mascot: BootstrapMascotRaster {
                rows: render_mascot(mascot_rows, &animation, theme),
            },
            phase_badge: phase_label,
        }
    }
}

fn wordmark(frame_index: usize) -> BootstrapWordmark {
    let pulse = (frame_index % 4) as i16;
    BootstrapWordmark {
        lines: vec![
            BootstrapWordmarkLine {
                text: " ██████  ██    ██  ██████  ██████  ██████".to_string(),
                offset: pulse.min(1),
            },
            BootstrapWordmarkLine {
                text: "██    ██ ██    ██ ██    ██ ██   ██ ██   ██".to_string(),
                offset: (pulse / 2).min(1),
            },
            BootstrapWordmarkLine {
                text: "██    ██ ██    ██ ██    ██ ██████  ██████".to_string(),
                offset: 0,
            },
            BootstrapWordmarkLine {
                text: "██ ▄▄ ██ ██    ██ ██    ██ ██   ██ ██".to_string(),
                offset: (pulse / 2).min(1),
            },
            BootstrapWordmarkLine {
                text: " ██████   ██████   ██████  ██   ██ ██".to_string(),
                offset: pulse.min(1),
            },
        ],
        accent_phase: frame_index % 10,
    }
}

fn target_mascot_rows(area: Rect, layout_mode: BootstrapLayoutMode) -> u32 {
    match layout_mode {
        BootstrapLayoutMode::Compact => {
            let margin_y = 1.min(area.height / 12);
            let inner_height = area.height.saturating_sub(margin_y * 2);
            (inner_height / 3).max(10) as u32
        }
        BootstrapLayoutMode::Standard | BootstrapLayoutMode::Full | BootstrapLayoutMode::Cinema => {
            let margin_y = area.height / 12;
            area.height
                .saturating_sub(margin_y * 2)
                .saturating_sub(1)
                .max(20) as u32
        }
    }
}

fn render_mascot(
    target_rows: u32,
    animation: &BootstrapAnimationState,
    theme: &Theme,
) -> Vec<Vec<BrandArtCell>> {
    let source = mascot_source_rows_for_height(target_rows);
    if animation.progress >= 1.0 {
        return source;
    }
    let height = source.len();
    let width = source.first().map(|row| row.len()).unwrap_or(0);
    let focus_x = width as f32 * 0.5;
    let focus_y = height as f32 * 0.45;
    let reveal_radius = animation.progress * 1.15;
    let mut rendered = Vec::with_capacity(height);

    for (row_index, row) in source.iter().enumerate() {
        let mut rendered_row = Vec::with_capacity(row.len());
        for (column_index, cell) in row.iter().enumerate() {
            let dx = if width > 1 {
                (column_index as f32 - focus_x) / width.saturating_sub(1) as f32
            } else {
                0.0
            };
            let dy = if height > 1 {
                (row_index as f32 - focus_y) / height.saturating_sub(1) as f32
            } else {
                0.0
            };
            let distance = ((dx * 1.15).powi(2) + dy.powi(2)).sqrt();
            let noise = hash01(column_index, row_index, 17) * 0.18;
            let threshold = (distance + noise).clamp(0.0, 1.2);
            let visible = threshold <= reveal_radius || animation.progress > 0.96;

            if !visible {
                rendered_row.push(BrandArtCell {
                    symbol: ' ',
                    fg: Color::Reset,
                    bg: theme.palette.editor_bg,
                });
            } else {
                rendered_row.push(*cell);
            }
        }
        rendered.push(rendered_row);
    }

    rendered
}
fn mascot_source_rows_for_height(target_rows: u32) -> Vec<Vec<BrandArtCell>> {
    let cache = bootstrap_asset_cache();
    if let Ok(mut guard) = cache.lock() {
        if let Some(rows) = guard.rows_by_target_height.get(&target_rows) {
            return rows.clone();
        }
        if let Some(base) = guard.decoded.as_ref() {
            let rows = rows_from_base_image(base, target_rows);
            guard
                .rows_by_target_height
                .insert(target_rows, rows.clone());
            return rows;
        }
    }

    #[cfg(test)]
    if let Ok(mut guard) = cache.lock() {
        if guard.decoded.is_none() {
            guard.decoded = Some(load_base_mascot());
        }
        if let Some(base) = guard.decoded.as_ref() {
            let rows = rows_from_base_image(base, target_rows);
            guard
                .rows_by_target_height
                .insert(target_rows, rows.clone());
            return rows;
        }
    }

    ensure_async_asset_load();
    rows_from_base_image(&generated_fallback_mascot(), target_rows)
}

fn bootstrap_asset_cache() -> Arc<Mutex<BootstrapAssetCache>> {
    static CACHE: OnceLock<Arc<Mutex<BootstrapAssetCache>>> = OnceLock::new();
    CACHE
        .get_or_init(|| Arc::new(Mutex::new(BootstrapAssetCache::new())))
        .clone()
}

fn ensure_async_asset_load() {
    let cache = bootstrap_asset_cache();
    let should_spawn = if let Ok(mut guard) = cache.lock() {
        if guard.loading_started || guard.decoded.is_some() {
            false
        } else {
            guard.loading_started = true;
            true
        }
    } else {
        false
    };
    if !should_spawn {
        return;
    }

    std::thread::spawn(move || {
        let decoded = load_base_mascot();
        if let Ok(mut guard) = cache.lock() {
            guard.decoded = Some(decoded);
            guard.rows_by_target_height.clear();
        }
    });
}

fn rows_from_base_image(base: &image::RgbaImage, target_rows: u32) -> Vec<Vec<BrandArtCell>> {
    if base.width() == 0 || base.height() == 0 {
        return vec![];
    }
    let pixel_height = (target_rows * 2).max(16);
    let target_width = ((base.width() as f32) * (pixel_height as f32 / base.height() as f32))
        .round()
        .max(8.0) as u32;
    let scaled = if base.height() == pixel_height {
        base.clone()
    } else {
        resize(base, target_width, pixel_height, FilterType::CatmullRom)
    };
    rgba_to_cells(&scaled)
}

fn load_base_mascot() -> image::RgbaImage {
    let decoded = load_source_image();
    crop_to_presentable_bounds(&decoded)
}

fn load_source_image() -> image::RgbaImage {
    if let Ok(decoded) = image::load_from_memory(EMBEDDED_BOOTSTRAP_IMAGE) {
        return decoded.to_rgba8();
    }
    generated_fallback_mascot()
}

fn crop_to_presentable_bounds(image: &image::RgbaImage) -> image::RgbaImage {
    if image.width() < 64 || image.height() < 64 {
        return image.clone();
    }

    let stripless = trim_left_artifact_band(image);
    let left = (stripless.width() / 24).min(stripless.width().saturating_sub(2));
    let top = (stripless.height() / 14).min(stripless.height().saturating_sub(2));
    let right = (stripless.width() / 14).min(stripless.width().saturating_sub(left + 1));
    let bottom = (stripless.height() / 10).min(stripless.height().saturating_sub(top + 1));
    let width = stripless.width().saturating_sub(left + right).max(1);
    let height = stripless.height().saturating_sub(top + bottom).max(1);

    stripless.view(left, top, width, height).to_image()
}

fn trim_left_artifact_band(image: &image::RgbaImage) -> image::RgbaImage {
    let left_trim = (image.width() / 12).min(image.width().saturating_sub(1));
    if left_trim == 0 {
        return image.clone();
    }
    image
        .view(left_trim, 0, image.width() - left_trim, image.height())
        .to_image()
}

fn generated_fallback_mascot() -> image::RgbaImage {
    let width = 96;
    let height = 128;
    let mut image = image::RgbaImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let is_border = x < 4 || y < 4 || x >= width - 4 || y >= height - 4;
            let is_eye_band =
                (34..=50).contains(&y) && ((22..=34).contains(&x) || (61..=73).contains(&x));
            let is_eye_highlight =
                (38..=44).contains(&y) && ((26..=30).contains(&x) || (65..=69).contains(&x));
            let is_mouth = (84..=90).contains(&y) && (24..=71).contains(&x);
            let is_mouth_gap = (86..=88).contains(&y) && (30..=65).contains(&x);

            let pixel = if is_border {
                image::Rgba([32, 38, 50, 255])
            } else if is_eye_band {
                image::Rgba([91, 155, 255, 255])
            } else if is_eye_highlight {
                image::Rgba([223, 239, 255, 255])
            } else if is_mouth && !is_mouth_gap {
                image::Rgba([244, 177, 62, 255])
            } else {
                let blend = y as f32 / (height - 1) as f32;
                let red = (44.0 + 18.0 * blend).round() as u8;
                let green = (52.0 + 32.0 * blend).round() as u8;
                let blue = (70.0 + 54.0 * blend).round() as u8;
                image::Rgba([red, green, blue, 255])
            };

            image.put_pixel(x, y, pixel);
        }
    }

    image
}

fn rgba_to_cells(image: &image::RgbaImage) -> Vec<Vec<BrandArtCell>> {
    let mut rows = Vec::new();
    for y in (0..image.height()).step_by(2) {
        let mut row = Vec::new();
        for x in 0..image.width() {
            let top = image.get_pixel(x, y).0;
            let bottom = if y + 1 < image.height() {
                image.get_pixel(x, y + 1).0
            } else {
                [0, 0, 0, 0]
            };
            row.push(pixel_pair_to_cell(top, bottom));
        }
        rows.push(row);
    }
    rows
}

fn pixel_pair_to_cell(top: [u8; 4], bottom: [u8; 4]) -> BrandArtCell {
    let top_visible = top[3] > 24;
    let bottom_visible = bottom[3] > 24;
    match (top_visible, bottom_visible) {
        (false, false) => BrandArtCell {
            symbol: ' ',
            fg: Color::Reset,
            bg: Color::Reset,
        },
        (true, false) => BrandArtCell {
            symbol: '▀',
            fg: Color::Rgb(top[0], top[1], top[2]),
            bg: Color::Reset,
        },
        (false, true) => BrandArtCell {
            symbol: '▄',
            fg: Color::Rgb(bottom[0], bottom[1], bottom[2]),
            bg: Color::Reset,
        },
        (true, true) => BrandArtCell {
            symbol: '▀',
            fg: Color::Rgb(top[0], top[1], top[2]),
            bg: Color::Rgb(bottom[0], bottom[1], bottom[2]),
        },
    }
}

fn hash01(x: usize, y: usize, seed: u32) -> f32 {
    let mut value = (x as u64).wrapping_mul(374_761_393)
        ^ (y as u64).wrapping_mul(668_265_263)
        ^ (seed as u64).wrapping_mul(700_001);
    value = (value ^ (value >> 13)).wrapping_mul(1_274_126_177);
    value ^= value >> 16;
    (value as u32) as f32 / u32::MAX as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Pixel;

    #[test]
    fn mascot_raster_is_deterministic() {
        let left = mascot_source_rows_for_height(27);
        let right = mascot_source_rows_for_height(27);
        assert_eq!(left, right);
        assert!(left.len() > 10);
        assert!(left[0].len() > 10);
    }

    #[test]
    fn loader_frame_scales_with_layout() {
        let theme = Theme::core_tui();
        let compact = BootstrapLoader::frame(Rect::new(0, 0, 100, 30), 2, "Starting", &theme);
        let cinema = BootstrapLoader::frame(Rect::new(0, 0, 180, 55), 2, "Starting", &theme);
        assert!(cinema.mascot.rows.len() >= compact.mascot.rows.len());
        assert!(cinema.mascot.rows.first().is_some_and(|row| {
            compact
                .mascot
                .rows
                .first()
                .is_some_and(|compact_row| row.len() > compact_row.len())
        }));
    }

    #[test]
    fn loader_frame_freezes_after_reveal_finishes() {
        let theme = Theme::core_tui();
        let final_frame = BootstrapLoader::frame(
            Rect::new(0, 0, 160, 50),
            BOOTSTRAP_REVEAL_FRAMES - 1,
            "Starting",
            &theme,
        );
        let later_frame = BootstrapLoader::frame(
            Rect::new(0, 0, 160, 50),
            BOOTSTRAP_REVEAL_FRAMES + 12,
            "Starting",
            &theme,
        );

        assert_eq!(
            final_frame.wordmark.accent_phase,
            later_frame.wordmark.accent_phase
        );
        assert_eq!(
            final_frame.wordmark.lines[0].offset,
            later_frame.wordmark.lines[0].offset
        );
        assert_eq!(final_frame.mascot.rows, later_frame.mascot.rows);
    }

    #[test]
    fn loader_frame_shows_visible_art_on_first_frame() {
        let theme = Theme::core_tui();
        let first = BootstrapLoader::frame(Rect::new(0, 0, 120, 40), 0, "Starting", &theme);
        assert!(
            first
                .mascot
                .rows
                .iter()
                .flatten()
                .any(|cell| cell.symbol != ' '),
            "frame zero should begin revealing immediately"
        );
    }

    #[test]
    fn embedded_asset_left_edge_is_dark_after_crop() {
        let image = load_base_mascot();
        let pixel = image.get_pixel(0, 0).to_rgba().0;

        assert!(pixel[0] < 80, "{pixel:?}");
        assert!(pixel[1] < 80, "{pixel:?}");
        assert!(pixel[2] < 80, "{pixel:?}");
    }

    #[test]
    fn wide_layout_mascot_is_materially_larger() {
        let theme = Theme::core_tui();
        let standard = BootstrapLoader::frame(Rect::new(0, 0, 120, 40), 2, "Starting", &theme);
        let compact = BootstrapLoader::frame(Rect::new(0, 0, 80, 24), 2, "Starting", &theme);

        assert!(standard.mascot.rows.len() >= 30);
        assert!(standard.mascot.rows.len() > compact.mascot.rows.len());
    }

    #[test]
    fn wordmark_contains_quorp_block_text() {
        let mark = wordmark(4);
        assert!(mark.lines.iter().any(|line| line.text.contains("██████")));
        assert_eq!(mark.accent_phase, 4);
    }
}
