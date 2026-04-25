use crate::quorp::tui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Default)]
pub struct DiffReactorState {
    pub active_patch_path: Option<String>,
    pub additions: usize,
    pub deletions: usize,
    pub is_dirty: bool,
}

impl DiffReactorState {
    pub fn render(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(path) = &self.active_patch_path {
            lines.push(Line::from(Span::styled(
                "Diff Reactor",
                Style::default()
                    .fg(theme.palette.secondary_teal)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("Target: {}", path),
                Style::default().fg(theme.palette.text_primary),
            )));

            lines.push(Line::from(vec![
                Span::styled("Impact: ", Style::default().fg(theme.palette.text_muted)),
                Span::styled(
                    format!("+{}", self.additions),
                    Style::default().fg(theme.palette.success_green),
                ),
                Span::styled(
                    format!(" -{}", self.deletions),
                    Style::default().fg(theme.palette.danger_orange),
                ),
            ]));

            if self.is_dirty {
                lines.push(Line::from(Span::styled(
                    "⚠ Requires Testing",
                    Style::default()
                        .fg(theme.palette.warning_yellow)
                        .add_modifier(Modifier::BOLD),
                )));
            }
        }

        lines
    }
}
