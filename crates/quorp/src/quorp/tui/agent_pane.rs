#![allow(unused)]
use ratatui::layout::Rect;
use ratatui::style::Style;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::quorp::tui::bridge::TuiToBackendRequest;
use crate::quorp::tui::theme;
use crate::quorp::tui::workbench::LeafRects;

pub struct AgentPane {
    bridge_tx: Option<futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>>,
    pub status_lines: Vec<String>,
}

impl AgentPane {
    pub fn new(bridge_tx: Option<futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>>) -> Self {
        Self {
            bridge_tx,
            status_lines: Vec::new(),
        }
    }

    pub fn try_handle_key(&mut self, key: &KeyEvent) -> anyhow::Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(true);
        }
        match key.code {
            KeyCode::Enter => {
                if let Some(tx) = &self.bridge_tx {
                    let _ = tx.unbounded_send(TuiToBackendRequest::StartAgentAction(
                        "Sample TUI Agent Request".to_string(),
                    ));
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn apply_status_update(&mut self, update: String) {
        self.status_lines.push(update);
    }

    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &theme::Theme,
    ) {
        self.render_in_leaf(frame.buffer_mut(), area, focused, theme);
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        focused: bool,
        theme: &theme::Theme,
    ) {
        use ratatui::widgets::Widget;

        crate::quorp::tui::paint::fill_rect(buf, area, theme.palette.editor_bg);

        if area.height == 0 || area.width == 0 {
            return;
        }

        let header_h: u16 = 1;
        let header_rect = Rect::new(area.x, area.y, area.width, header_h);
        let body_rect = Rect::new(
            area.x,
            area.y + header_h,
            area.width,
            area.height.saturating_sub(header_h),
        );

        let header_style = if focused {
            Style::default()
                .fg(theme.palette.text)
                .bg(theme.palette.raised_bg)
        } else {
            Style::default()
                .fg(theme.palette.text_muted)
                .bg(theme.palette.editor_bg)
        };
        crate::quorp::tui::paint::fill_rect(buf, header_rect, if focused { theme.palette.raised_bg } else { theme.palette.editor_bg });
        let header_text = " Agent [Press Enter to dispatch]";
        let header_line = Line::from(Span::styled(header_text, header_style));
        Paragraph::new(header_line).render(header_rect, buf);

        if body_rect.height == 0 {
            return;
        }

        let body_style = Style::default()
            .fg(theme.palette.text)
            .bg(theme.palette.editor_bg);

        let mut text = Vec::new();
        if self.status_lines.is_empty() {
            text.push(Line::from(Span::styled(
                "  Agent [Press Enter to dispatch]",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(theme.palette.editor_bg),
            )));
        } else {
            for line in &self.status_lines {
                text.push(Line::from(Span::styled(
                    format!("  {}", line),
                    body_style,
                )));
            }
        }

        Paragraph::new(text)
            .style(body_style)
            .render(body_rect, buf);
    }
}
