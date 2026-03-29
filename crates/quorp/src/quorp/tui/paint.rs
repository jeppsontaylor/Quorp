#![allow(unused)]
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

pub fn fill_rect(buf: &mut Buffer, rect: Rect, bg: Color) {
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ").set_bg(bg);
            }
        }
    }
}

pub fn draw_text(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, max_width: u16) {
    let mut col = x;
    let end = x.saturating_add(max_width);
    for ch in text.chars() {
        if col >= end {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(style);
        }
        col += 1;
    }
}

pub fn draw_scrollbar(
    buf: &mut Buffer,
    rect: Rect,
    thumb_top: u16,
    thumb_height: u16,
    track_color: Color,
    thumb_color: Color,
) {
    fill_rect(buf, rect, track_color);
    let thumb_bottom = thumb_top.saturating_add(thumb_height).min(rect.bottom());
    for y in thumb_top..thumb_bottom {
        for x in rect.left()..rect.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ").set_bg(thumb_color);
            }
        }
    }
}

pub fn hline(buf: &mut Buffer, x: u16, y: u16, width: u16, ch: char, style: Style) {
    let end = x.saturating_add(width);
    for col in x..end {
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch).set_style(style);
        }
    }
}
