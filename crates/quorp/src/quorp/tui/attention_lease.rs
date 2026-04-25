use crate::quorp::tui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct AttentionLease {
    pub title: String,
    pub description: String,
    pub severity: LeaseSeverity,
    pub options: Vec<String>,
}

impl AttentionLease {
    pub fn new(title: impl Into<String>, desc: impl Into<String>, severity: LeaseSeverity) -> Self {
        Self {
            title: title.into(),
            description: desc.into(),
            severity,
            options: vec![
                "Accept".to_string(),
                "Reject".to_string(),
                "Modify".to_string(),
            ],
        }
    }

    pub fn render(&self, theme: &Theme, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let border_style = match self.severity {
            LeaseSeverity::Info => Style::default().fg(theme.palette.accent_blue),
            LeaseSeverity::Warning => Style::default().fg(theme.palette.warning_yellow),
            LeaseSeverity::Critical => Style::default().fg(theme.palette.danger_orange),
        };

        lines.push(Line::from(Span::styled(
            "ATTENTION LEASE",
            border_style.add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled("? ", border_style),
            Span::styled(
                self.title.clone(),
                Style::default()
                    .fg(theme.palette.text_primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("│ ", border_style),
            Span::styled(
                self.description.clone(),
                Style::default().fg(theme.palette.text_muted),
            ),
        ]));

        lines.push(Line::from(Span::styled("├─ OPTIONS", border_style)));

        for (i, opt) in self.options.iter().enumerate() {
            let label = format!("│ [{}] {}", i + 1, opt);
            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(theme.palette.text_primary),
            )));
        }

        lines.push(Line::from(Span::styled("└────────────", border_style)));
        lines
    }
}
