use crate::quorp::tui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct OrchestraAgentState {
    pub name: String,
    pub task: String,
    pub is_active: bool,
    pub progress: u8, // 0 to 100
}

#[derive(Debug, Clone, Default)]
pub struct ToolOrchestra {
    pub agents: Vec<OrchestraAgentState>,
}

impl ToolOrchestra {
    pub fn render(&self, theme: &Theme, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if self.agents.is_empty() {
            return lines;
        }

        let header_style = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled(
            "TOOL ORCHESTRA (Swarm)",
            header_style,
        )));

        for agent in &self.agents {
            let status_color = if agent.is_active {
                theme.palette.success_green
            } else {
                theme.palette.text_muted
            };

            let name_style = Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD);

            lines.push(Line::from(vec![
                Span::styled(format!("⠼ {} ", agent.name.clone()), name_style),
                Span::styled(
                    agent.task.clone(),
                    Style::default().fg(theme.palette.text_primary),
                ),
            ]));

            // Render a mini progress bar
            let bar_len = (width.saturating_sub(6) as usize * agent.progress as usize) / 100;
            let bar = "█".repeat(bar_len);
            let empty = "░".repeat(width.saturating_sub(6) as usize - bar_len);
            lines.push(Line::from(vec![
                Span::styled("  └─", Style::default().fg(theme.palette.subtle_border)),
                Span::styled(bar, Style::default().fg(theme.palette.link_blue)),
                Span::styled(empty, Style::default().fg(theme.palette.subtle_border)),
            ]));
        }

        lines
    }
}
