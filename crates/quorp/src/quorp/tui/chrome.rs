#![allow(unused)]
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Paragraph, Widget};

use crate::quorp::tui::theme::Theme;

pub struct TitleBar<'a> {
    pub text: &'a str,
    pub theme: &'a Theme,
}

impl Widget for TitleBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bg = Style::default()
            .bg(self.theme.palette.titlebar_bg)
            .fg(self.theme.palette.text_muted);
        buf.set_style(area, bg);

        let line = Line::from(Span::styled(self.text, bg));
        let x = area.x + area.width.saturating_sub(line.width() as u16) / 2;
        buf.set_line(x, area.y, &line, area.width);
    }
}

pub struct StatusBar<'a> {
    pub left: &'a str,
    pub center: &'a str,
    pub right_status: &'a str,
    pub theme: &'a Theme,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let bg_blue = Style::default()
            .bg(self.theme.palette.status_blue)
            .fg(self.theme.palette.icon_active);
        let bg_center = Style::default()
            .bg(self.theme.palette.inset_bg)
            .fg(self.theme.palette.text_muted);
        let bg_gold = Style::default()
            .bg(self.theme.palette.status_gold)
            .fg(self.theme.palette.icon_active);

        let left_text = format!(" {} ", self.left);
        let right_text = format!(" {} ", self.right_status);
        let left_w = left_text.chars().count() as u16;
        let right_w = right_text.chars().count() as u16;
        let left_take = left_w.min(area.width.saturating_sub(1));
        let right_take = right_w.min(area.width.saturating_sub(left_take).saturating_sub(1));
        let center_w = area
            .width
            .saturating_sub(left_take)
            .saturating_sub(right_take)
            .max(1);

        let center_display =
            crate::quorp::tui::text_width::truncate_fit(self.center, center_w as usize);

        buf.set_style(
            Rect::new(area.x, area.y, left_take, area.height),
            bg_blue,
        );
        buf.set_line(
            area.x,
            area.y,
            &Line::from(Span::styled(
                crate::quorp::tui::text_width::truncate_fit(&left_text, left_take as usize),
                bg_blue,
            )),
            left_take,
        );

        let center_x = area.x + left_take;
        buf.set_style(
            Rect::new(center_x, area.y, center_w, area.height),
            bg_center,
        );
        buf.set_line(
            center_x,
            area.y,
            &Line::from(Span::styled(center_display, bg_center)),
            center_w,
        );

        let right_x = area.x + left_take + center_w;
        buf.set_style(
            Rect::new(right_x, area.y, right_take, area.height),
            bg_gold,
        );
        buf.set_line(
            right_x,
            area.y,
            &Line::from(Span::styled(
                crate::quorp::tui::text_width::truncate_fit(&right_text, right_take as usize),
                bg_gold,
            )),
            right_take,
        );
    }
}

pub struct ExplorerHeader<'a> {
    pub theme: &'a Theme,
}

impl Widget for ExplorerHeader<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let bg = Style::default()
            .bg(self.theme.palette.sidebar_bg)
            .fg(self.theme.palette.text_faint);
        buf.set_style(area, bg);

        let line = Line::from(Span::styled(" EXPLORER", bg));
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

pub struct TabStrip<'a> {
    pub tabs: Vec<(&'a str, bool)>, // (filename, is_active)
    pub theme: &'a Theme,
}

impl Widget for TabStrip<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let inactive_bg = Style::default()
            .bg(self.theme.palette.tab_inactive_bg)
            .fg(self.theme.palette.text_muted);
        buf.set_style(area, inactive_bg);

        let mut x = area.x;
        for (label, is_active) in self.tabs {
            let style = if is_active {
                Style::default()
                    .bg(self.theme.palette.editor_bg)
                    .fg(self.theme.palette.text)
            } else {
                inactive_bg
            };
            
            let text = format!(" {} {} ", self.theme.glyphs.file_icon, label);
            let width = text.chars().count() as u16;
            let close_text = format!("{} ", self.theme.glyphs.close_icon);
            let close_width = close_text.chars().count() as u16;
            
            if x + width + close_width > area.right() {
                break;
            }
            
            buf.set_string(x, area.y, text, style);
            x += width;
            
            buf.set_string(x, area.y, close_text, style);
            x += close_width;
            
            // right border for tab
            if x < area.right() {
                buf.set_string(x, area.y, " ", inactive_bg);
                x += 1;
            }
        }
    }
}

pub struct PanelTabs<'a> {
    pub tabs: Vec<(&'a str, bool)>, // (label, is_active)
    pub theme: &'a Theme,
}

impl Widget for PanelTabs<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let bg = Style::default()
            .bg(self.theme.palette.editor_bg)
            .fg(self.theme.palette.text_faint);
        buf.set_style(area, bg);

        let mut x = area.x + 1; // Left padding
        for (label, is_active) in self.tabs {
            let style = if is_active {
                Style::default()
                    .bg(self.theme.palette.pill_bg)
                    .fg(self.theme.palette.text)
            } else {
                bg
            };
            let text = format!(" {} ", label);
            let width = text.chars().count() as u16;
            if x + width > area.right() {
                break;
            }
            buf.set_string(x, area.y, text, style);
            x += width + 1; // 1 space gap between tabs
        }
        
        // Toolbar icons on the right
        let toolbar = " ×  ^ ";
        let toolbar_width = toolbar.chars().count() as u16;
        if area.width >= toolbar_width {
            buf.set_string(area.right() - toolbar_width, area.y, toolbar, bg);
        }
    }
}

pub struct AssistantHeader<'a> {
    pub label: &'a str,
    pub model: &'a str,
    pub theme: &'a Theme,
}

impl Widget for AssistantHeader<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let bg = Style::default()
            .bg(self.theme.palette.sidebar_bg)
            .fg(self.theme.palette.text);
        buf.set_style(area, bg);

        let left_padding = "  ";
        let label_text = format!("{} ", self.label);
        buf.set_string(area.x, area.y, left_padding, bg);
        buf.set_string(area.x + 2, area.y, label_text, bg.add_modifier(Modifier::BOLD));
        
        let pill_style = Style::default()
            .bg(self.theme.palette.pill_bg)
            .fg(self.theme.palette.text_muted);
        let model_text = format!(" {} ", self.model);
        buf.set_string(area.x + 2 + self.label.len() as u16 + 1, area.y, model_text, pill_style);
    }
}

pub struct Composer<'a> {
    pub placeholder: &'a str,
    pub footer_text: &'a str,
    pub is_focused: bool,
    pub theme: &'a Theme,
}

impl Widget for Composer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 4 {
            return;
        }

        let border_color = if self.is_focused {
            self.theme.palette.text_muted // More visible when focused
        } else {
            self.theme.palette.subtle_border
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(self.theme.palette.inset_bg))
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        block.render(area, buf);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

        Paragraph::new(self.placeholder)
            .style(
                Style::default()
                    .fg(self.theme.palette.text_muted)
                    .bg(self.theme.palette.inset_bg),
            )
            .render(chunks[0], buf);

        Paragraph::new(self.footer_text)
            .style(
                Style::default()
                    .fg(self.theme.palette.text_faint)
                    .bg(self.theme.palette.inset_bg),
            )
            .render(chunks[1], buf);
    }
}
