//! Deterministic rasterization of a ratatui [`Buffer`] to PNG for visual regression and scoring.
//!
//! Glyphs are approximated with per-codepoint dot patterns (not a real font) so output is stable
//! across platforms and still changes when cell text or colors change.

use std::io::Cursor;
use std::path::Path;

use image::imageops::{resize, FilterType};
use image::{ImageFormat, Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

const CELL_W: u32 = 8;
const CELL_H: u32 = 16;

/// Standard VGA-ish ANSI 0–7 (normal) and 8–15 (bright) RGB values.
const ANSI16: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 49, 49],
    [13, 188, 121],
    [229, 224, 113],
    [36, 114, 200],
    [188, 63, 188],
    [17, 168, 205],
    [229, 229, 229],
    [102, 102, 102],
    [241, 76, 76],
    [35, 209, 139],
    [245, 245, 67],
    [59, 142, 234],
    [214, 112, 214],
    [41, 184, 219],
    [255, 255, 255],
];

fn xterm_256_to_rgb(i: u8) -> [u8; 3] {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let i = i - 16;
            let r = (i / 36) % 6;
            let g = (i / 6) % 6;
            let b = i % 6;
            let v = |n: u8| -> u8 {
                if n == 0 {
                    0
                } else {
                    (55 + (n as u16 - 1) * 40) as u8
                }
            };
            [v(r), v(g), v(b)]
        }
        _ => {
            let g = 8 + (i as u16 - 232) * 10;
            let g = g.min(255) as u8;
            [g, g, g]
        }
    }
}

fn color_to_rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Reset => [198, 198, 198],
        Color::Black => [0, 0, 0],
        Color::Red => ANSI16[1],
        Color::Green => ANSI16[2],
        Color::Yellow => ANSI16[3],
        Color::Blue => ANSI16[4],
        Color::Magenta => ANSI16[5],
        Color::Cyan => ANSI16[6],
        Color::Gray => ANSI16[7],
        Color::DarkGray => ANSI16[8],
        Color::LightRed => ANSI16[9],
        Color::LightGreen => ANSI16[10],
        Color::LightYellow => ANSI16[11],
        Color::LightBlue => ANSI16[12],
        Color::LightMagenta => ANSI16[13],
        Color::LightCyan => ANSI16[14],
        Color::White => ANSI16[15],
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) => xterm_256_to_rgb(i),
    }
}

fn brighten(mut rgb: [u8; 3], amount: f32) -> [u8; 3] {
    for c in &mut rgb {
        *c = ((*c as f32) + (255.0 - *c as f32) * amount).round() as u8;
    }
    rgb
}

/// 4×4 patterns keyed by `code_point % 8` for non-space glyphs.
const PATTERNS: [[u16; 4]; 8] = [
    [0b1111, 0b1001, 0b1001, 0b1111],
    [0b0110, 0b1111, 0b1111, 0b0110],
    [0b1010, 0b0101, 0b1010, 0b0101],
    [0b1100, 0b1100, 0b0011, 0b0011],
    [0b0001, 0b0011, 0b0111, 0b1111],
    [0b1111, 0b0111, 0b0011, 0b0001],
    [0b0101, 0b1010, 0b0101, 0b1010],
    [0b0011, 0b0110, 0b1100, 0b1001],
];

fn draw_glyph_for_symbol(
    img: &mut RgbaImage,
    origin_x: u32,
    origin_y: u32,
    cell_w: u32,
    cell_h: u32,
    symbol: &str,
    fg: [u8; 3],
    bg: [u8; 3],
) {
    for py in 0..cell_h {
        for px in 0..cell_w {
            let x = origin_x + px;
            let y = origin_y + py;
            if x < img.width() && y < img.height() {
                img.put_pixel(x, y, Rgba([bg[0], bg[1], bg[2], 255]));
            }
        }
    }
    let ch = symbol.chars().next();
    let Some(ch) = ch else {
        return;
    };
    if ch == ' ' || ch == '\u{0}' {
        return;
    }
    let idx = (ch as u32) as usize % PATTERNS.len();
    let pat = PATTERNS[idx];
    let ox = (cell_w.saturating_sub(4)) / 2;
    let oy = (cell_h.saturating_sub(4)) / 2;
    for row in 0..4u32 {
        let bits = pat[row as usize];
        for col in 0..4u32 {
            if (bits >> (3 - col)) & 1 != 0 {
                let px = origin_x + ox + col;
                let py = origin_y + oy + row;
                if px < img.width() && py < img.height() {
                    img.put_pixel(px, py, Rgba([fg[0], fg[1], fg[2], 255]));
                }
            }
        }
    }
}

/// Rasterize the buffer using `CELL_W`×`CELL_H` pixels per terminal cell.
pub fn buffer_to_rgba(buffer: &Buffer) -> RgbaImage {
    buffer_to_rgba_scaled(buffer, CELL_W, CELL_H)
}

/// Rasterize with a custom cell size (width and height in pixels).
pub fn buffer_to_rgba_scaled(buffer: &Buffer, cell_w: u32, cell_h: u32) -> RgbaImage {
    let w = buffer.area.width as u32 * cell_w;
    let h = buffer.area.height as u32 * cell_h;
    let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));

    for y in 0..buffer.area.height {
        let mut x = 0u16;
        while x < buffer.area.width {
            let cell = &buffer[(x, y)];
            if cell.skip {
                x += 1;
                continue;
            }
            let sym = cell.symbol();
            let sym_w = UnicodeWidthStr::width(sym).max(1).min(2) as u16;
            let mut fg = color_to_rgb(cell.fg);
            let bg = color_to_rgb(cell.bg);
            if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                fg = brighten(fg, 0.15);
            }

            let px0 = x as u32 * cell_w;
            let py0 = y as u32 * cell_h;
            let span_w = sym_w as u32 * cell_w;

            for py in 0..cell_h {
                for px in 0..span_w {
                    let ix = px0 + px;
                    let iy = py0 + py;
                    if ix < w && iy < h {
                        img.put_pixel(ix, iy, Rgba([bg[0], bg[1], bg[2], 255]));
                    }
                }
            }

            if sym != " " && sym.chars().next().is_some() {
                draw_glyph_for_symbol(&mut img, px0, py0, span_w, cell_h, sym, fg, bg);
            }

            x += sym_w;
        }
    }

    img
}

/// Encode `RGBA8` as PNG bytes.
pub fn rgba_to_png_bytes(img: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
    let mut cursor = Cursor::new(Vec::new());
    img.write_to(&mut cursor, ImageFormat::Png)?;
    Ok(cursor.into_inner())
}

/// Load a PNG from disk as RGBA8.
pub fn load_png_rgba(path: &Path) -> Result<RgbaImage, image::ImageError> {
    let bytes = std::fs::read(path)?;
    let dyn_img = image::load_from_memory(&bytes)?;
    Ok(dyn_img.to_rgba8())
}

/// Fraction of pixels that differ between two equal-siquorp RGBA images (alpha ignored).
pub fn pixel_mismatch_fraction(a: &RgbaImage, b: &RgbaImage) -> Result<f64, String> {
    if a.dimensions() != b.dimensions() {
        return Err(format!(
            "dimension mismatch: {:?} vs {:?}",
            a.dimensions(),
            b.dimensions()
        ));
    }
    let mut diff = 0u64;
    let total = a.width() as u64 * a.height() as u64;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        if pa.0[0..3] != pb.0[0..3] {
            diff += 1;
        }
    }
    Ok(diff as f64 / total as f64)
}

/// Bottom `strip_frac` of image height (at least 4 px), full width.
fn bottom_strip(img: &RgbaImage, strip_frac: f32) -> RgbaImage {
    let h = img.height();
    let strip_h = ((h as f32) * strip_frac).round() as u32;
    let strip_h = strip_h.max(4).min(h);
    let y0 = h - strip_h;
    image::imageops::crop_imm(img, 0, y0, img.width(), strip_h).to_image()
}

/// Right `width_frac` of a horizontal strip.
fn right_fraction(strip: &RgbaImage, width_frac: f32) -> RgbaImage {
    let w = strip.width();
    let rw = ((w as f32) * width_frac).round() as u32;
    let rw = rw.max(4).min(w);
    let x0 = w - rw;
    image::imageops::crop_imm(strip, x0, 0, rw, strip.height()).to_image()
}

fn dist_rgb(a: [u8; 3], b: [u8; 3]) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    (dr * dr + dg * dg + db * db).sqrt()
}

fn blue_and_warm_fractions(img: &RgbaImage) -> (f64, f64) {
    let status_blue = [0u8, 122, 204];
    let status_gold = [123u8, 100, 0];
    let strip = bottom_strip(img, 0.08);
    let total = strip.width() as u64 * strip.height() as u64;
    if total == 0 {
        return (0.0, 0.0);
    }
    let mut blue_hits = 0u64;
    for p in strip.pixels() {
        let c = [p.0[0], p.0[1], p.0[2]];
        if dist_rgb(c, status_blue) < 90.0 {
            blue_hits += 1;
        }
    }
    let blue_frac = blue_hits as f64 / total as f64;

    let right = right_fraction(&strip, 0.38);
    let rt = right.width() as u64 * right.height() as u64;
    let mut warm_hits = 0u64;
    if rt > 0 {
        for p in right.pixels() {
            let c = [p.0[0], p.0[1], p.0[2]];
            if dist_rgb(c, status_gold) < 100.0 || (c[0] > 140 && c[1] > 90 && c[2] < 120) {
                warm_hits += 1;
            }
        }
    }
    let warm_frac = if rt > 0 {
        warm_hits as f64 / rt as f64
    } else {
        0.0
    };
    (blue_frac, warm_frac)
}

/// Heuristic 0.0–1.0 comparing bottom status strip of `tui` to `reference` (e.g. Core TUI IDE).
///
/// Uses blue-bar and warm-chip pixel fractions; not a perceptual match to the whole IDE frame.
pub fn core_tui_likeness(tui: &RgbaImage, reference: &RgbaImage) -> f64 {
    let (tb, tw) = blue_and_warm_fractions(tui);
    let (rb, rw) = blue_and_warm_fractions(reference);
    let blue_closeness = (1.0 - (tb - rb).abs()).clamp(0.0, 1.0);
    let warm_closeness = (1.0 - (tw - rw).abs()).clamp(0.0, 1.0);
    (blue_closeness * 0.4 + warm_closeness * 0.6).clamp(0.0, 1.0)
}

/// Subscores for [`prismforge_likeness`]: layout (edge structure), palette (role colors in ROIs), status strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrismScore {
    pub layout_score: f64,
    pub palette_score: f64,
    pub status_bar_score: f64,
    pub composite: f64,
}

const PRISM_NORM_WIDTH: u32 = 960;

fn normalize_pair_to_same_size(a: &RgbaImage, b: &RgbaImage) -> (RgbaImage, RgbaImage) {
    let mut ca = resize_to_width(a, PRISM_NORM_WIDTH);
    let mut cb = resize_to_width(b, PRISM_NORM_WIDTH);
    let h = ca.height().max(cb.height());
    if ca.height() != h {
        ca = resize(&ca, ca.width(), h, FilterType::Triangle);
    }
    if cb.height() != h {
        cb = resize(&cb, cb.width(), h, FilterType::Triangle);
    }
    (ca, cb)
}

fn roi_rect(w: u32, h: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> (u32, u32, u32, u32) {
    let x0p = ((w as f32) * x0).round() as u32;
    let y0p = ((h as f32) * y0).round() as u32;
    let x1p = ((w as f32) * x1).round().max(1.0) as u32;
    let y1p = ((h as f32) * y1).round().max(1.0) as u32;
    let x1p = x1p.min(w).max(x0p.saturating_add(1));
    let y1p = y1p.min(h).max(y0p.saturating_add(1));
    (x0p, y0p, x1p - x0p, y1p - y0p)
}

fn crop_roi(img: &RgbaImage, x: u32, y: u32, rw: u32, rh: u32) -> RgbaImage {
    let w = img.width();
    let h = img.height();
    if x >= w || y >= h || rw == 0 || rh == 0 {
        return RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
    }
    let rw = rw.min(w - x);
    let rh = rh.min(h - y);
    image::imageops::crop_imm(img, x, y, rw, rh).to_image()
}

fn gray_u8(p: &Rgba<u8>) -> f32 {
    let r = p.0[0] as f32;
    let g = p.0[1] as f32;
    let b = p.0[2] as f32;
    0.299 * r + 0.587 * g + 0.114 * b
}

fn edge_map_l1_similarity(a: &RgbaImage, b: &RgbaImage, small: u32) -> f64 {
    let ma = resize(a, small, small, FilterType::Triangle);
    let mb = resize(b, small, small, FilterType::Triangle);
    if ma.width() < 2 || ma.height() < 2 {
        return 0.5;
    }
    let mut sum_diff = 0.0f64;
    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut count = 0u64;
    for y in 0..ma.height() - 1 {
        for x in 0..ma.width() - 1 {
            let ga = gray_u8(ma.get_pixel(x, y));
            let e_ax = (gray_u8(ma.get_pixel(x + 1, y)) - ga).abs();
            let e_ay = (gray_u8(ma.get_pixel(x, y + 1)) - ga).abs();
            let edge_a = e_ax + e_ay;
            let gb = gray_u8(mb.get_pixel(x, y));
            let e_bx = (gray_u8(mb.get_pixel(x + 1, y)) - gb).abs();
            let e_by = (gray_u8(mb.get_pixel(x, y + 1)) - gb).abs();
            let edge_b = e_bx + e_by;
            sum_diff += (edge_a - edge_b).abs() as f64;
            sum_a += edge_a as f64;
            sum_b += edge_b as f64;
            count += 1;
        }
    }
    if count == 0 {
        return 0.5;
    }
    let mean_diff = sum_diff / count as f64;
    let mean_a = sum_a / count as f64;
    let mean_b = sum_b / count as f64;
    let shape = (1.0 - (mean_diff / 510.0).min(1.0)).clamp(0.0, 1.0);
    const MIN_REF_EDGE: f64 = 2.0;
    if mean_b < MIN_REF_EDGE {
        return shape;
    }
    let recall = (mean_a / mean_b).min(1.0);
    (shape * recall).clamp(0.0, 1.0)
}

fn fraction_near_color(img: &RgbaImage, target: [u8; 3], max_dist: f32) -> f64 {
    let w = img.width();
    let h = img.height();
    let total = (w * h) as u64;
    if total == 0 {
        return 0.0;
    }
    let mut hits = 0u64;
    for p in img.pixels() {
        let c = [p.0[0], p.0[1], p.0[2]];
        if dist_rgb(c, target) <= max_dist {
            hits += 1;
        }
    }
    hits as f64 / total as f64
}

fn palette_roi_closeness(cand: &RgbaImage, reference: &RgbaImage, target: [u8; 3]) -> f64 {
    let fc = fraction_near_color(cand, target, 60.0);
    let fr = fraction_near_color(reference, target, 60.0);
    const NEGLIGIBLE_FRAC: f64 = 0.02;
    if fr < NEGLIGIBLE_FRAC {
        (1.0 - fc).clamp(0.0, 1.0)
    } else {
        (fc / fr).min(1.0)
    }
}

fn prismforge_status_triple(img: &RgbaImage) -> (f64, f64, f64) {
    let status_blue = [0u8, 122, 204];
    let lime = [0xA7u8, 0xF4, 0x32];
    let gold = [0xFFu8, 0xB2, 0x24];
    let strip = bottom_strip(img, 0.08);
    let t = strip.width() as u64 * strip.height() as u64;
    if t == 0 {
        return (0.0, 0.0, 0.0);
    }
    let mut b = 0u64;
    let mut l = 0u64;
    let mut g = 0u64;
    for p in strip.pixels() {
        let c = [p.0[0], p.0[1], p.0[2]];
        if dist_rgb(c, status_blue) < 90.0 {
            b += 1;
        }
        if dist_rgb(c, lime) < 85.0 {
            l += 1;
        }
        if dist_rgb(c, gold) < 95.0 {
            g += 1;
        }
    }
    (
        b as f64 / t as f64,
        l as f64 / t as f64,
        g as f64 / t as f64,
    )
}

fn prismforge_status_score(cand: &RgbaImage, reference: &RgbaImage) -> f64 {
    let (cb, cl, cg) = prismforge_status_triple(cand);
    let (rb, rl, rg) = prismforge_status_triple(reference);
    let s0 = (1.0 - (cb - rb).abs()).clamp(0.0, 1.0);
    let s1 = (1.0 - (cl - rl).abs()).clamp(0.0, 1.0);
    let s2 = (1.0 - (cg - rg).abs()).clamp(0.0, 1.0);
    (s0 * 0.5 + s1 * 0.25 + s2 * 0.25).clamp(0.0, 1.0)
}

/// Heuristic comparison vs a PrismForge / Mock1 reference PNG (normaliquorp width [`PRISM_NORM_WIDTH`]).
///
/// Dot-pattern TUI raster vs a photographic mock will not reach 1.0; use for directional ratcheting.
pub fn prismforge_likeness(candidate: &RgbaImage, reference: &RgbaImage) -> PrismScore {
    let (c, r) = normalize_pair_to_same_size(candidate, reference);
    let w = c.width();
    let h = c.height();

    let rois_layout: &[(f32, f32, f32, f32)] = &[
        (0.0, 0.0, 1.0, 0.025),
        (0.0, 0.025, 0.25, 0.95),
        (0.25, 0.025, 0.60, 0.60),
        (0.60, 0.025, 1.0, 0.95),
        (0.0, 0.975, 1.0, 1.0),
    ];
    let mut layout_acc = 0.0f64;
    for &(xf, yf, xt, yt) in rois_layout {
        let (x, y, rw, rh) = roi_rect(w, h, xf, yf, xt, yt);
        let ca = crop_roi(&c, x, y, rw, rh);
        let ra = crop_roi(&r, x, y, rw, rh);
        layout_acc += edge_map_l1_similarity(&ca, &ra, 64);
    }
    let layout_score = (layout_acc / rois_layout.len() as f64).clamp(0.0, 1.0);

    let emerald = [0x34u8, 0xE7, 0xA5];
    let violet = [0x8Bu8, 0x5C, 0xF6];
    let cyan = [0x35u8, 0xD7, 0xFF];
    let amber = [0xFFu8, 0xB2, 0x24];

    let (x, y, rw, rh) = roi_rect(w, h, 0.0, 0.025, 0.25, 0.95);
    let p0 = palette_roi_closeness(&crop_roi(&c, x, y, rw, rh), &crop_roi(&r, x, y, rw, rh), emerald);
    let (x, y, rw, rh) = roi_rect(w, h, 0.25, 0.025, 0.60, 0.60);
    let p1 = palette_roi_closeness(&crop_roi(&c, x, y, rw, rh), &crop_roi(&r, x, y, rw, rh), violet);
    let (x, y, rw, rh) = roi_rect(w, h, 0.60, 0.025, 1.0, 0.95);
    let p2 = palette_roi_closeness(&crop_roi(&c, x, y, rw, rh), &crop_roi(&r, x, y, rw, rh), cyan);
    let (x, y, rw, rh) = roi_rect(w, h, 0.25, 0.55, 0.60, 0.95);
    let p3 = palette_roi_closeness(&crop_roi(&c, x, y, rw, rh), &crop_roi(&r, x, y, rw, rh), amber);

    let palette_score = ((p0 + p1 + p2 + p3) / 4.0).clamp(0.0, 1.0);

    let status_bar_score = prismforge_status_score(&c, &r);

    let composite = (layout_score * 0.35 + palette_score * 0.45 + status_bar_score * 0.20).clamp(0.0, 1.0);

    PrismScore {
        layout_score,
        palette_score,
        status_bar_score,
        composite,
    }
}

/// Horizontal mirror (for layout sensitivity tests).
pub fn flip_horizontal_rgba(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.put_pixel(w - 1 - x, y, *img.get_pixel(x, y));
        }
    }
    out
}

/// Resize for lightweight comparison (e.g. regression at a fixed scale).
pub fn resize_to_width(img: &RgbaImage, width: u32) -> RgbaImage {
    let h = img.height();
    let w = img.width();
    if w == 0 || h == 0 {
        return img.clone();
    }
    let nh = ((h as u64 * width as u64) / w as u64).max(1) as u32;
    resize(img, width, nh, FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    #[test]
    fn buffer_png_smoke_single_cell() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf[(0, 0)].set_symbol("X");
        buf[(0, 0)].fg = Color::Rgb(255, 0, 0);
        buf[(0, 0)].bg = Color::Rgb(0, 0, 255);
        let img = buffer_to_rgba_scaled(&buf, 4, 4);
        assert_eq!(img.dimensions(), (4, 4));
        let has_red_fg = img.pixels().any(|p| p.0[0] > 200 && p.0[2] < 100);
        assert!(has_red_fg, "expected some red foreground pixels");
    }

    #[test]
    fn png_roundtrip_preserves_pixels() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        buf[(1, 0)].set_symbol("a");
        buf[(1, 0)].fg = Color::Rgb(10, 20, 30);
        buf[(1, 0)].bg = Color::Rgb(40, 50, 60);
        let img = buffer_to_rgba_scaled(&buf, 2, 2);
        let png = rgba_to_png_bytes(&img).expect("encode");
        let back = image::load_from_memory(&png)
            .expect("decode")
            .to_rgba8();
        let frac = pixel_mismatch_fraction(&img, &back).expect("cmp");
        assert_eq!(frac, 0.0, "png roundtrip mismatch {frac}");
    }

    #[test]
    fn prismforge_all_black_scores_lower_than_self() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/visual/prismforge_target.png");
        if !path.exists() {
            return;
        }
        let reference = load_png_rgba(&path).expect("load prismforge target");
        let black = RgbaImage::from_pixel(reference.width(), reference.height(), Rgba([0u8, 0, 0, 255]));
        let s_black = prismforge_likeness(&black, &reference);
        let s_self = prismforge_likeness(&reference, &reference);
        assert!(
            s_black.composite < s_self.composite,
            "black {} vs self {}",
            s_black.composite,
            s_self.composite
        );
        assert!(
            s_self.composite - s_black.composite > 0.2,
            "black should trail self by a margin: black {} self {}",
            s_black.composite,
            s_self.composite
        );
        assert!(
            s_black.composite < 0.7,
            "uniform black should stay well below a good match: {}",
            s_black.composite
        );
    }

    #[test]
    fn prismforge_reference_vs_self_is_high() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/visual/prismforge_target.png");
        if !path.exists() {
            return;
        }
        let reference = load_png_rgba(&path).expect("load");
        let s = prismforge_likeness(&reference, &reference);
        assert!(s.composite > 0.85, "self composite {}", s.composite);
        assert!(s.layout_score > 0.9);
    }

    #[test]
    fn prismforge_flipped_reference_lowers_layout() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/visual/prismforge_target.png");
        if !path.exists() {
            return;
        }
        let reference = load_png_rgba(&path).expect("load");
        let flipped = flip_horizontal_rgba(&reference);
        let s = prismforge_likeness(&flipped, &reference);
        assert!(
            s.layout_score < prismforge_likeness(&reference, &reference).layout_score,
            "layout should drop when mirrored"
        );
    }
}
