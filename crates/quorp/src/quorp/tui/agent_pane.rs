use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::quorp::tui::agent_runtime::{AgentRuntimeStatus, AgentUiEvent};
use crate::quorp::tui::theme;

pub struct AgentPane {
    pub status_lines: Vec<String>,
}

impl AgentPane {
    pub fn new() -> Self {
        Self {
            status_lines: Vec::new(),
        }
    }

    pub fn try_handle_key(&mut self, key: &KeyEvent) -> anyhow::Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(true);
        }
        match key.code {
            KeyCode::Enter => {
                self.apply_status_update(
                    "Launch autonomous runs from the Assistant pane with `/agent <goal>`. This pane shows runtime status and verifier progress.".to_string(),
                );
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn apply_status_update(&mut self, update: String) {
        self.status_lines.push(update);
    }

    pub fn apply_event(&mut self, event: AgentUiEvent) {
        match event {
            AgentUiEvent::StatusUpdate(status) => match status {
                AgentRuntimeStatus::Idle => {
                    self.apply_status_update("[Idle] Waiting for background tasks.".to_string())
                }
                AgentRuntimeStatus::Thinking => {
                    self.apply_status_update("[Thinking] Generating actions...".to_string())
                }
                AgentRuntimeStatus::ExecutingTool(tool) => {
                    self.apply_status_update(format!("[Executing] {}", tool))
                }
                AgentRuntimeStatus::Validating(check) => {
                    self.apply_status_update(format!("[Validating] {}", check))
                }
                AgentRuntimeStatus::Failed(err) => {
                    self.apply_status_update(format!("[Failed] {}", err))
                }
                AgentRuntimeStatus::Success => {
                    self.apply_status_update("[Success] Task completed!".to_string())
                }
            },
            AgentUiEvent::TurnCompleted(_) => {
                self.apply_status_update("[Turn Completed]".to_string())
            }
            AgentUiEvent::ArtifactsReady(path) => {
                self.apply_status_update(format!("[Artifacts] {}", path.display()))
            }
            AgentUiEvent::FatalError(err) => {
                self.apply_status_update(format!("[Fatal Error] {}", err))
            }
        }
    }

    #[allow(dead_code)]
    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        focused: bool,
        theme: &theme::Theme,
    ) {
        self.render_in_leaf(frame.buffer_mut(), area, focused, theme);
    }

    #[allow(dead_code)]
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
        crate::quorp::tui::paint::fill_rect(
            buf,
            header_rect,
            if focused {
                theme.palette.raised_bg
            } else {
                theme.palette.editor_bg
            },
        );
        let header_text = " Agent Status [Enter explains current limitation]";
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
                "  Launch autonomous work from the Assistant pane with `/agent <goal>`.",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(theme.palette.editor_bg),
            )));
            text.push(Line::from(Span::styled(
                "  This pane visualizes background runtime status, verifier progress, and failures.",
                Style::default()
                    .fg(theme.palette.text_muted)
                    .bg(theme.palette.editor_bg),
            )));
        } else {
            for line in &self.status_lines {
                text.push(Line::from(Span::styled(format!("  {}", line), body_style)));
            }
        }

        Paragraph::new(text)
            .style(body_style)
            .render(body_rect, buf);
    }
}
