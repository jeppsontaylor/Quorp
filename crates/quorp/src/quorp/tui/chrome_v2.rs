#![allow(unused)]
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::quorp::tui::paint::{draw_text, fill_rect};
use crate::quorp::tui::theme::{Palette, Theme};

#[derive(Clone, Debug)]
pub enum TitleItem {
    Text(String),
    Icon(&'static str),
    Button { label: String, bg: Color },
    Spacer,
}

pub struct TitleBarVm {
    pub left: Vec<TitleItem>,
    pub center: String,
    pub right: Vec<TitleItem>,
}

pub struct ActivityItemVm {
    pub icon: &'static str,
    pub active: bool,
    pub badge: Option<u16>,
}

pub struct PanelTabVm {
    pub label: String,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub enum AgentBlockVm {
    Banner {
        icon: &'static str,
        text: String,
        link: Option<String>,
        trailing_icon: Option<&'static str>,
    },
    PromptCard {
        text: String,
        trailing_icon: Option<&'static str>,
    },
    Disclosure {
        open: bool,
        label: String,
    },
    Activity {
        icon: &'static str,
        label: String,
        target: String,
        accent: Option<Color>,
    },
    Paragraph(String),
    MutedLine(String),
}

pub struct ComposerVm {
    pub placeholder: String,
    pub input: String,
    pub mode_chips: Vec<String>,
    pub focused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MentionPopupVm {
    pub lines: Vec<String>,
    pub selected: usize,
}

pub fn render_mention_popup(buf: &mut Buffer, rect: Rect, vm: &MentionPopupVm, palette: &Palette) {
    if rect.height == 0 || rect.width < 3 {
        return;
    }
    fill_rect(buf, rect, palette.raised_bg);
    let max_rows = rect.height as usize;
    for (i, line) in vm.lines.iter().enumerate().take(max_rows) {
        let y = rect.y + i as u16;
        let style = if i == vm.selected {
            Style::default()
                .fg(palette.text)
                .bg(palette.pill_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text).bg(palette.raised_bg)
        };
        draw_text(
            buf,
            rect.x + 1,
            y,
            line.as_str(),
            style,
            rect.width.saturating_sub(2),
        );
    }
}

pub struct LeafTabVm {
    pub label: String,
    pub active: bool,
    pub icon: Option<&'static str>,
}

/// One tab for layout + hit-testing (file preview / chat sessions).
#[derive(Clone, Debug)]
pub struct LeafTabSpec {
    pub label: String,
    pub active: bool,
    pub icon: Option<&'static str>,
    pub show_close: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeafTabLayoutCell {
    pub tab_index: usize,
    pub truncated_label: String,
    /// Icon + padded label; clicking activates the tab.
    pub select_rect: Rect,
    pub close_rect: Option<Rect>,
}

const TAB_GAP: u16 = 1;
const OVERFLOW_HINT_MIN_W: u16 = 4;

fn fit_tab_label(available: u16, icon_w: u16, close_w: u16, label: &str) -> (String, u16, u16) {
    if available < icon_w + close_w + 3 {
        let t = crate::quorp::tui::text_width::truncate_fit(label, 1);
        let lw = UnicodeWidthStr::width(t.as_str()) as u16 + 2;
        let tw = icon_w + lw + close_w;
        return (t, lw, tw.min(available));
    }
    let mut label_max = (available
        .saturating_sub(icon_w)
        .saturating_sub(close_w)
        .saturating_sub(2))
    .max(1) as usize;
    loop {
        let truncated = crate::quorp::tui::text_width::truncate_fit(label, label_max);
        let label_inner = UnicodeWidthStr::width(truncated.as_str()) as u16 + 2;
        let tab_w = icon_w + label_inner + close_w;
        if tab_w <= available || label_max <= 1 {
            return (truncated, label_inner, tab_w.min(available));
        }
        label_max -= 1;
    }
}

/// Greedy left-to-right placement. Skips tabs that do not fit; `overflow_count` is how many
/// specs were not assigned a cell (including the first that did not fit and all after it).
pub fn layout_leaf_tabs(strip: Rect, tabs: &[LeafTabSpec]) -> (Vec<LeafTabLayoutCell>, usize) {
    if strip.width == 0 || strip.height == 0 {
        return (vec![], tabs.len());
    }
    let right = strip.x.saturating_add(strip.width);
    let mut col = strip.x;
    let mut cells = Vec::new();

    for (tab_index, tab) in tabs.iter().enumerate() {
        if !cells.is_empty() {
            col = col.saturating_add(TAB_GAP);
        }
        if col >= right {
            let placed = cells.len();
            return (cells, tabs.len().saturating_sub(placed));
        }

        let icon_w = tab.icon.map(|_| 2u16).unwrap_or(0);
        let close_w = u16::from(tab.show_close);

        let tabs_remaining = tabs.len() - tab_index;
        let available = right.saturating_sub(col);
        let reserve_overflow = tabs_remaining > 1 && available >= OVERFLOW_HINT_MIN_W + TAB_GAP;
        let max_for_tab = if reserve_overflow {
            available.saturating_sub(OVERFLOW_HINT_MIN_W + TAB_GAP)
        } else {
            available
        };

        let min_need = icon_w + 3 + close_w;
        if max_for_tab < min_need {
            let placed = cells.len();
            return (cells, tabs.len().saturating_sub(placed));
        }

        let (truncated, _label_inner, tab_w) =
            fit_tab_label(max_for_tab, icon_w, close_w, &tab.label);
        let tab_w = tab_w.min(max_for_tab);
        let label_inner = tab_w.saturating_sub(icon_w).saturating_sub(close_w);
        let select_w = icon_w + label_inner;

        let tab_start = col;
        col = col.saturating_add(tab_w);

        let close_rect = if tab.show_close && close_w > 0 {
            Some(Rect::new(
                tab_start.saturating_add(select_w),
                strip.y,
                close_w,
                strip.height,
            ))
        } else {
            None
        };

        cells.push(LeafTabLayoutCell {
            tab_index,
            truncated_label: truncated,
            select_rect: Rect::new(tab_start, strip.y, select_w, strip.height),
            close_rect,
        });
    }

    (cells, 0)
}

/// Paints tabs after [`layout_leaf_tabs`]. `close_glyph` is typically `theme.glyphs.close_icon`.
/// `active_underline` colors the second row under the active tab (e.g. editor or chat accent).
pub fn render_leaf_tabs_laid_out(
    buf: &mut Buffer,
    strip: Rect,
    cells: &[LeafTabLayoutCell],
    specs: &[LeafTabSpec],
    palette: &Palette,
    close_glyph: &str,
    active_underline: Color,
) {
    fill_rect(buf, strip, palette.raised_bg);

    for cell in cells {
        let Some(spec) = specs.get(cell.tab_index) else {
            continue;
        };
        let (bg, fg) = if spec.active {
            (palette.tab_active_bg, palette.text)
        } else {
            (palette.tab_inactive_bg, palette.text_muted)
        };

        let sr = cell.select_rect;
        for x in sr.left()..sr.right().min(strip.right()) {
            for y in strip.y..strip.y.saturating_add(strip.height) {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_symbol(" ").set_bg(bg);
                }
            }
        }

        let mut x = sr.x;
        if let Some(icon) = spec.icon {
            let icon_style = Style::default().fg(fg).bg(bg);
            draw_text(buf, x, strip.y, icon, icon_style, 2);
            x = x.saturating_add(2);
        }

        let label = cell.truncated_label.as_str();
        let style = Style::default().fg(fg).bg(bg);
        let label_w = UnicodeWidthStr::width(label) as u16;
        draw_text(buf, x + 1, strip.y, label, style, label_w);

        if spec.active && strip.height > 1 {
            for px in sr.left()..sr.right().min(strip.right()) {
                if let Some(c) = buf.cell_mut((px, strip.y + 1)) {
                    c.set_symbol(" ").set_bg(active_underline);
                }
            }
        } else {
            for px in sr.left()..sr.right().min(strip.right()) {
                if let Some(c) = buf.cell_mut((px, strip.y + 1)) {
                    c.set_symbol(" ").set_bg(bg);
                }
            }
        }

        if let Some(cr) = cell.close_rect {
            let close_bg = if spec.active {
                palette.tab_active_bg
            } else {
                palette.tab_inactive_bg
            };
            for cy in cr.y..cr.y.saturating_add(cr.height) {
                if let Some(c) = buf.cell_mut((cr.x, cy)) {
                    c.set_symbol(" ").set_bg(close_bg);
                }
            }
            let close_style = Style::default().fg(palette.text_muted).bg(close_bg);
            draw_text(buf, cr.x, strip.y, close_glyph, close_style, 1);
        }
    }
}

/// Renders a "+N" overflow hint at the right side of the strip when some tabs did not fit.
pub fn render_tab_overflow_hint(
    buf: &mut Buffer,
    strip: Rect,
    overflow_count: usize,
    palette: &Palette,
) {
    if overflow_count == 0 || strip.width < OVERFLOW_HINT_MIN_W {
        return;
    }
    let text = format!("+{}", overflow_count);
    let w = text.len() as u16;
    if w + 1 > strip.width {
        return;
    }
    let x = strip.right().saturating_sub(w + 1);
    let y = strip.y;
    let style = Style::default()
        .fg(palette.warning_yellow)
        .bg(palette.raised_bg);
    for px in x..x.saturating_add(w) {
        if let Some(c) = buf.cell_mut((px, y)) {
            c.set_symbol(" ").set_bg(palette.raised_bg);
        }
    }
    draw_text(buf, x, y, &text, style, w);
}

pub fn render_titlebar(buf: &mut Buffer, rect: Rect, vm: &TitleBarVm, palette: &Palette) {
    fill_rect(buf, rect, palette.titlebar_bg);
    let style = Style::default()
        .fg(palette.text_muted)
        .bg(palette.titlebar_bg);

    let mut left_col = rect.x + 2;
    for item in &vm.left {
        match item {
            TitleItem::Text(s) => {
                draw_text(buf, left_col, rect.y, s, style, rect.width);
                left_col += s.len() as u16 + 1;
            }
            TitleItem::Icon(s) => {
                draw_text(buf, left_col, rect.y, s, style, rect.width);
                left_col += 2;
            }
            TitleItem::Spacer => left_col += 1,
            TitleItem::Button { label, bg } => {
                let btn_style = Style::default().fg(Color::White).bg(*bg);
                draw_text(buf, left_col, rect.y, label, btn_style, rect.width);
                left_col += label.len() as u16 + 1;
            }
        }
    }

    let center_len = vm.center.len() as u16;
    let center_x = rect.x + rect.width.saturating_sub(center_len) / 2;
    let center_style = Style::default()
        .fg(palette.text)
        .bg(palette.titlebar_bg)
        .add_modifier(Modifier::BOLD);
    draw_text(buf, center_x, rect.y, &vm.center, center_style, rect.width);

    let mut right_col = rect.x + rect.width.saturating_sub(2);
    for item in vm.right.iter().rev() {
        match item {
            TitleItem::Button { label, bg } => {
                let w = label.len() as u16;
                let x = right_col.saturating_sub(w);
                let btn_style = Style::default().fg(Color::White).bg(*bg);
                draw_text(buf, x, rect.y, label, btn_style, w);
                right_col = x.saturating_sub(1);
            }
            TitleItem::Text(s) => {
                let w = s.len() as u16;
                let x = right_col.saturating_sub(w);
                draw_text(buf, x, rect.y, s, style, w);
                right_col = x.saturating_sub(1);
            }
            TitleItem::Icon(s) => {
                let x = right_col.saturating_sub(1);
                draw_text(buf, x, rect.y, s, style, 2);
                right_col = x.saturating_sub(1);
            }
            TitleItem::Spacer => right_col = right_col.saturating_sub(1),
        }
    }
}

pub fn render_statusbar(
    buf: &mut Buffer,
    rect: Rect,
    left: &str,
    right_chip: &str,
    palette: &Palette,
) {
    fill_rect(buf, rect, palette.status_blue);
    let style = Style::default().fg(Color::White).bg(palette.status_blue);
    draw_text(
        buf,
        rect.x + 1,
        rect.y,
        left,
        style,
        rect.width.saturating_sub(2),
    );

    if !right_chip.is_empty() {
        let chip_w = right_chip.len() as u16 + 2;
        let chip_x = rect.x + rect.width.saturating_sub(chip_w + 1);
        let chip_style = Style::default().fg(Color::White).bg(palette.status_gold);
        draw_text(
            buf,
            chip_x,
            rect.y,
            &format!(" {} ", right_chip),
            chip_style,
            chip_w,
        );
    }
}

pub fn render_activity_bar(
    buf: &mut Buffer,
    rect: Rect,
    items: &[ActivityItemVm],
    palette: &Palette,
) {
    fill_rect(buf, rect, palette.activity_bg);

    let mut row = rect.y + 1;
    for item in items {
        if row >= rect.bottom() {
            break;
        }

        let (fg, bg) = if item.active {
            (palette.icon_active, palette.pill_bg)
        } else {
            (palette.icon_inactive, palette.activity_bg)
        };

        if item.active {
            for x in rect.left()..rect.right() {
                if let Some(cell) = buf.cell_mut((x, row)) {
                    cell.set_symbol(" ").set_bg(bg);
                }
            }
        }

        let icon_x = rect.x + (rect.width.saturating_sub(1)) / 2;
        let icon_style = Style::default().fg(fg).bg(bg);
        draw_text(buf, icon_x, row, item.icon, icon_style, 2);

        if let Some(count) = item.badge {
            let badge = format!("{}", count);
            let badge_x = icon_x + 1;
            let badge_y = row;
            if badge_x < rect.right() {
                let badge_style = Style::default()
                    .fg(Color::White)
                    .bg(palette.accent_blue)
                    .add_modifier(Modifier::BOLD);
                draw_text(buf, badge_x, badge_y, &badge, badge_style, 2);
            }
        }

        row += 2;
    }
}

pub fn render_leaf_tab_strip(buf: &mut Buffer, rect: Rect, tabs: &[LeafTabVm], palette: &Palette) {
    fill_rect(buf, rect, palette.tab_inactive_bg);

    let mut col = rect.x;
    for tab in tabs {
        let (bg, fg) = if tab.active {
            (palette.tab_active_bg, palette.text)
        } else {
            (palette.tab_inactive_bg, palette.text_muted)
        };

        if let Some(icon) = tab.icon {
            let icon_style = Style::default().fg(fg).bg(bg);
            draw_text(buf, col, rect.y, icon, icon_style, 2);
            col += 2;
        }

        let label = &tab.label;
        let label_w = label.len() as u16;
        let tab_w = label_w + 2;

        for x in col..col.saturating_add(tab_w).min(rect.right()) {
            if let Some(cell) = buf.cell_mut((x, rect.y)) {
                cell.set_symbol(" ").set_bg(bg);
            }
            if rect.height > 1
                && let Some(cell) = buf.cell_mut((x, rect.y + 1))
            {
                cell.set_symbol(" ").set_bg(bg);
            }
        }

        let style = Style::default().fg(fg).bg(bg);
        draw_text(buf, col + 1, rect.y, label, style, label_w);

        if tab.active && rect.height > 1 {
            for x in col..col.saturating_add(tab_w).min(rect.right()) {
                if let Some(cell) = buf.cell_mut((x, rect.y + 1)) {
                    cell.set_symbol(" ").set_bg(palette.tab_active_bg);
                }
            }
        }

        col += tab_w + 1;
    }
}

pub fn render_panel_tabs(
    buf: &mut Buffer,
    rect: Rect,
    tabs: &[PanelTabVm],
    shell_badge: Option<&str>,
    palette: &Palette,
) {
    fill_rect(buf, rect, palette.editor_bg);

    let style_inactive = Style::default()
        .fg(palette.text_muted)
        .bg(palette.editor_bg);

    let mut col = rect.x + 1;
    for tab in tabs {
        let label = &tab.label;
        if tab.active {
            let pill_w = label.len() as u16 + 2;
            for x in col..col.saturating_add(pill_w).min(rect.right()) {
                if let Some(cell) = buf.cell_mut((x, rect.y)) {
                    cell.set_symbol(" ").set_bg(palette.pill_bg);
                }
            }
            let active_style = Style::default().fg(palette.text).bg(palette.pill_bg);
            draw_text(
                buf,
                col + 1,
                rect.y,
                label,
                active_style,
                label.len() as u16,
            );
            if rect.height > 1 {
                for x in col..col.saturating_add(pill_w).min(rect.right()) {
                    if let Some(cell) = buf.cell_mut((x, rect.y + 1)) {
                        cell.set_symbol(" ").set_bg(palette.terminal_accent);
                    }
                }
            }
            col += pill_w + 1;
        } else {
            draw_text(buf, col, rect.y, label, style_inactive, label.len() as u16);
            col += label.len() as u16 + 1;
        }
    }

    if let Some(badge) = shell_badge {
        let badge_w = badge.len() as u16 + 2;
        let badge_x = rect.right().saturating_sub(badge_w + 2);
        let badge_style = Style::default()
            .fg(palette.text_faint)
            .bg(palette.editor_bg);
        draw_text(buf, badge_x, rect.y, badge, badge_style, badge_w);
    }
}

pub fn render_agent_banner(
    buf: &mut Buffer,
    rect: Rect,
    icon: &str,
    text: &str,
    palette: &Palette,
) {
    fill_rect(buf, rect, palette.banner_bg);
    let style = Style::default().fg(palette.text).bg(palette.banner_bg);
    let icon_style = Style::default().fg(palette.link_blue).bg(palette.banner_bg);
    draw_text(buf, rect.x + 1, rect.y, icon, icon_style, 2);
    draw_text(
        buf,
        rect.x + 3,
        rect.y,
        text,
        style,
        rect.width.saturating_sub(4),
    );
}

pub fn render_agent_block(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    block: &AgentBlockVm,
    palette: &Palette,
) -> u16 {
    match block {
        AgentBlockVm::PromptCard {
            text,
            trailing_icon,
        } => {
            let bg = palette.raised_bg;
            for col in x..x.saturating_add(width) {
                if let Some(cell) = buf.cell_mut((col, y)) {
                    cell.set_symbol(" ").set_bg(bg);
                }
            }
            let style = Style::default().fg(palette.text).bg(bg);
            draw_text(buf, x + 2, y, text, style, width.saturating_sub(4));
            if let Some(icon) = trailing_icon {
                let icon_x = x + width.saturating_sub(3);
                let icon_style = Style::default().fg(palette.text_muted).bg(bg);
                draw_text(buf, icon_x, y, icon, icon_style, 2);
            }
            1
        }
        AgentBlockVm::Disclosure { open, label } => {
            let arrow = if *open { "▾" } else { "▸" };
            let style = Style::default().fg(palette.text_muted);
            draw_text(buf, x + 1, y, arrow, style, 2);
            draw_text(buf, x + 3, y, label, style, width.saturating_sub(4));
            1
        }
        AgentBlockVm::Activity {
            icon,
            label,
            target,
            accent,
        } => {
            let icon_style = Style::default().fg(accent.unwrap_or(palette.text_muted));
            draw_text(buf, x + 2, y, icon, icon_style, 2);
            let label_style = Style::default().fg(palette.text_muted);
            let label_len = label.len() as u16;
            draw_text(buf, x + 4, y, label, label_style, label_len);
            let target_style = Style::default().fg(palette.text);
            draw_text(
                buf,
                x + 4 + label_len + 1,
                y,
                target,
                target_style,
                width.saturating_sub(6 + label_len),
            );
            1
        }
        AgentBlockVm::Paragraph(text) => {
            let style = Style::default().fg(palette.text);
            draw_text(buf, x + 2, y, text, style, width.saturating_sub(4));
            1
        }
        AgentBlockVm::MutedLine(text) => {
            let style = Style::default().fg(palette.text_muted);
            draw_text(buf, x + 2, y, text, style, width.saturating_sub(4));
            1
        }
        AgentBlockVm::Banner { icon, text, .. } => {
            render_agent_banner(buf, Rect::new(x, y, width, 2), icon, text, palette);
            2
        }
    }
}

pub fn render_composer(buf: &mut Buffer, rect: Rect, vm: &ComposerVm, palette: &Palette) {
    fill_rect(buf, rect, palette.editor_bg);

    if rect.height < 3 || rect.width < 10 {
        return;
    }

    let inner_x = rect.x + 1;
    let inner_y = rect.y;
    let inner_w = rect.width.saturating_sub(2);
    let inner_h = rect.height.saturating_sub(1);

    let border_color = if vm.focused {
        palette.accent_blue
    } else {
        palette.input_border
    };

    let top_left = "╭";
    let top_right = "╮";
    let bottom_left = "╰";
    let bottom_right = "╯";

    let border_style = Style::default().fg(border_color).bg(palette.inset_bg);
    draw_text(buf, inner_x, inner_y, top_left, border_style, 1);
    for col in (inner_x + 1)..inner_x.saturating_add(inner_w).saturating_sub(1) {
        if let Some(cell) = buf.cell_mut((col, inner_y)) {
            cell.set_char('─').set_style(border_style);
        }
    }
    draw_text(
        buf,
        inner_x + inner_w.saturating_sub(1),
        inner_y,
        top_right,
        border_style,
        1,
    );

    for row in (inner_y + 1)..inner_y.saturating_add(inner_h).saturating_sub(1) {
        if let Some(cell) = buf.cell_mut((inner_x, row)) {
            cell.set_char('│').set_style(border_style);
        }
        for col in (inner_x + 1)..inner_x.saturating_add(inner_w).saturating_sub(1) {
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.set_symbol(" ").set_bg(palette.inset_bg);
            }
        }
        if let Some(cell) = buf.cell_mut((inner_x + inner_w.saturating_sub(1), row)) {
            cell.set_char('│').set_style(border_style);
        }
    }

    let bottom_y = inner_y + inner_h.saturating_sub(1);
    draw_text(buf, inner_x, bottom_y, bottom_left, border_style, 1);
    for col in (inner_x + 1)..inner_x.saturating_add(inner_w).saturating_sub(1) {
        if let Some(cell) = buf.cell_mut((col, bottom_y)) {
            cell.set_char('─').set_style(border_style);
        }
    }
    draw_text(
        buf,
        inner_x + inner_w.saturating_sub(1),
        bottom_y,
        bottom_right,
        border_style,
        1,
    );

    let text_x = inner_x + 2;
    let text_w = inner_w.saturating_sub(4);
    let text_y = inner_y + 1;
    let text_h = inner_h.saturating_sub(2);
    if vm.input.is_empty() {
        let placeholder_style = Style::default().fg(palette.text_muted).bg(palette.inset_bg);
        draw_text(
            buf,
            text_x,
            text_y,
            &vm.placeholder,
            placeholder_style,
            text_w,
        );
    } else {
        let text_style = Style::default().fg(palette.text).bg(palette.inset_bg);
        let wrapped = crate::quorp::tui::text_width::wrap_plain_lines(&vm.input, text_w as usize);
        let start_index = wrapped.len().saturating_sub(text_h as usize);
        for (row_index, line) in wrapped
            .into_iter()
            .skip(start_index)
            .take(text_h as usize)
            .enumerate()
        {
            draw_text(
                buf,
                text_x,
                text_y.saturating_add(row_index as u16),
                &line,
                text_style,
                text_w,
            );
        }
    }

    if !vm.mode_chips.is_empty() {
        let chips_y = bottom_y.saturating_sub(1);
        let chip_style = Style::default().fg(palette.text_faint).bg(palette.inset_bg);
        let mut col = text_x;
        for chip in &vm.mode_chips {
            draw_text(buf, col, chips_y, chip, chip_style, chip.len() as u16);
            col += chip.len() as u16 + 1;
        }
    }
}

pub fn default_titlebar_vm(theme: &Theme) -> TitleBarVm {
    TitleBarVm {
        left: vec![],
        center: "quorp-tui".to_string(),
        right: vec![TitleItem::Button {
            label: " Update ".to_string(),
            bg: theme.palette.toolbar_button_bg,
        }],
    }
}

pub fn default_activity_items() -> Vec<ActivityItemVm> {
    vec![
        ActivityItemVm {
            icon: "☰",
            active: true,
            badge: None,
        },
        ActivityItemVm {
            icon: "⌕",
            active: false,
            badge: None,
        },
        ActivityItemVm {
            icon: "◈",
            active: false,
            badge: Some(3),
        },
        ActivityItemVm {
            icon: "⚙",
            active: false,
            badge: None,
        },
    ]
}

#[cfg(test)]
mod tab_layout_tests {
    use super::*;

    #[test]
    fn layout_tabs_within_strip_bounds() {
        let strip = Rect::new(0, 0, 40, 2);
        let tabs = vec![
            LeafTabSpec {
                label: "short.rs".to_string(),
                active: true,
                icon: Some("f"),
                show_close: true,
            },
            LeafTabSpec {
                label: "very_long_filename_here.rs".to_string(),
                active: false,
                icon: Some("f"),
                show_close: true,
            },
            LeafTabSpec {
                label: "c.rs".to_string(),
                active: false,
                icon: None,
                show_close: true,
            },
        ];
        let (cells, overflow) = layout_leaf_tabs(strip, &tabs);
        assert!(overflow <= 3);
        for cell in &cells {
            assert!(cell.select_rect.x >= strip.x);
            assert!(cell.select_rect.right() <= strip.x + strip.width);
            if let Some(cr) = cell.close_rect {
                assert_eq!(cr.width, 1);
                assert!(cr.right() <= strip.x + strip.width);
            }
        }
    }

    #[test]
    fn truncated_label_fits_display_width_budget() {
        let strip = Rect::new(0, 0, 22, 2);
        let tabs = vec![LeafTabSpec {
            label: "abcdefghijklmnopqrstuvwxyz".to_string(),
            active: true,
            icon: Some("f"),
            show_close: true,
        }];
        let (cells, overflow) = layout_leaf_tabs(strip, &tabs);
        assert_eq!(overflow, 0);
        assert_eq!(cells.len(), 1);
        let w = UnicodeWidthStr::width(cells[0].truncated_label.as_str());
        let inner = cells[0]
            .select_rect
            .width
            .saturating_sub(2)
            .saturating_sub(2);
        assert!(w as u16 <= inner);
    }
}
