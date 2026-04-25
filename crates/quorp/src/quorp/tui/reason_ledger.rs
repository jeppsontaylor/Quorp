use crate::quorp::tui::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone)]
pub struct ReasonLedgerEntry {
    pub objective: Option<String>,
    pub evidence: Vec<String>,
    pub action: Option<String>,
    pub rollback: Option<String>,
    pub raw_trace: Vec<String>,
}

impl ReasonLedgerEntry {
    pub fn parse_from_think(content: &str) -> Self {
        // A simple heuristic parser for Reason Ledger components.
        // It tries to extract structure if the model followed the grammar,
        // otherwise falls back to a raw trace.
        let mut entry = ReasonLedgerEntry {
            objective: None,
            evidence: Vec::new(),
            action: None,
            rollback: None,
            raw_trace: Vec::new(),
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("objective:") || lower.starts_with("[objective]") {
                entry.objective = Some(
                    line.split_once(':')
                        .map(|(_, val)| val)
                        .unwrap_or(line)
                        .trim()
                        .to_string(),
                );
            } else if lower.starts_with("evidence:") || lower.starts_with("[evidence]") {
                entry.evidence.push(
                    line.split_once(':')
                        .map(|(_, val)| val)
                        .unwrap_or(line)
                        .trim()
                        .to_string(),
                );
            } else if lower.starts_with("action:") || lower.starts_with("[action]") {
                entry.action = Some(
                    line.split_once(':')
                        .map(|(_, val)| val)
                        .unwrap_or(line)
                        .trim()
                        .to_string(),
                );
            } else if lower.starts_with("rollback:") || lower.starts_with("[rollback]") {
                entry.rollback = Some(
                    line.split_once(':')
                        .map(|(_, val)| val)
                        .unwrap_or(line)
                        .trim()
                        .to_string(),
                );
            } else {
                entry.raw_trace.push(line.to_string());
            }
        }
        entry
    }

    pub fn render(&self, theme: &Theme, left_pad: &str) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let border_style = Style::default().fg(theme.palette.secondary_teal);
        let header_style = Style::default()
            .fg(theme.palette.secondary_teal)
            .add_modifier(Modifier::BOLD);

        lines.push(Line::from(Span::styled(
            format!("{}┌── REASON LEDGER", left_pad),
            header_style,
        )));

        if let Some(obj) = &self.objective {
            lines.push(Line::from(vec![
                Span::styled(format!("{}│ ", left_pad), border_style),
                Span::styled(
                    "OBJECTIVE ",
                    Style::default()
                        .fg(theme.palette.text_muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    obj.to_string(),
                    Style::default().fg(theme.palette.text_primary),
                ),
            ]));
        }

        for ev in &self.evidence {
            lines.push(Line::from(vec![
                Span::styled(format!("{}│ ", left_pad), border_style),
                Span::styled(
                    "EVIDENCE  ",
                    Style::default()
                        .fg(theme.palette.text_muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    ev.to_string(),
                    Style::default().fg(theme.palette.text_faint),
                ),
            ]));
        }

        if let Some(act) = &self.action {
            lines.push(Line::from(vec![
                Span::styled(format!("{}│ ", left_pad), border_style),
                Span::styled(
                    "ACTION    ",
                    Style::default()
                        .fg(theme.palette.text_muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    act.to_string(),
                    Style::default().fg(theme.palette.accent_blue),
                ),
            ]));
        }

        if let Some(roll) = &self.rollback {
            lines.push(Line::from(vec![
                Span::styled(format!("{}│ ", left_pad), border_style),
                Span::styled(
                    "ROLLBACK  ",
                    Style::default()
                        .fg(theme.palette.text_muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    roll.to_string(),
                    Style::default().fg(theme.palette.warning_yellow),
                ),
            ]));
        }

        if self.objective.is_none() && self.action.is_none() && !self.raw_trace.is_empty() {
            // Render a condensed trace instead of giant dumps
            lines.push(Line::from(vec![
                Span::styled(format!("{}│ ", left_pad), border_style),
                Span::styled(
                    "AUDIT TRACE",
                    Style::default()
                        .fg(theme.palette.text_muted)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            let sample = self
                .raw_trace
                .iter()
                .filter(|s| !s.is_empty())
                .take(3)
                .collect::<Vec<_>>();
            for line in sample {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}│ ", left_pad), border_style),
                    Span::styled(
                        line.to_string(),
                        Style::default()
                            .fg(theme.palette.text_faint)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
            if self.raw_trace.len() > 3 {
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "{}│ ... ({} lines folded)",
                        left_pad,
                        self.raw_trace.len() - 3
                    ),
                    border_style,
                )]));
            }
        }

        lines.push(Line::from(Span::styled(
            format!("{}└────────────", left_pad),
            border_style,
        )));
        lines
    }
}
