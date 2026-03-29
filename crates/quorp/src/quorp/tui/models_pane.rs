use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::quorp::tui::chat::ChatPane;
use crate::quorp::tui::model_registry;
use crate::quorp::tui::ssd_moe_tui::ModelStatus;
use crate::quorp::tui::theme::Theme;

#[derive(Debug, Clone)]
pub struct ModelsPaneEntry {
    pub registry_id: String,
    pub title: String,
    pub subtitle: String,
    pub disk_gb: Option<f32>,
}

impl ModelsPaneEntry {
    fn from_registry_line(full_id: &str) -> Self {
        let title = if let Some((provider, model)) = full_id.split_once('/') {
            format!("{model} · {provider}")
        } else {
            full_id.to_string()
        };
        let subtitle = model_registry::local_moe_spec_for_registry_id(full_id)
            .map(|s| s.description.to_string())
            .unwrap_or_else(|| "Cloud or remote model (no local MoE bundle).".to_string());
        let disk_gb = model_registry::local_moe_spec_for_registry_id(full_id)
            .map(|s| s.estimated_disk_gb);
        Self {
            registry_id: full_id.to_string(),
            title,
            subtitle,
            disk_gb,
        }
    }
}

pub struct ModelsPane {
    pub selected_index: usize,
    pub entries: Vec<ModelsPaneEntry>,
}

impl ModelsPane {
    pub fn sync_from_chat(chat: &ChatPane) -> Self {
        let entries: Vec<ModelsPaneEntry> = chat
            .model_list()
            .iter()
            .map(|id| ModelsPaneEntry::from_registry_line(id))
            .collect();
        let selected_index = if entries.is_empty() {
            0
        } else {
            chat.model_index().min(entries.len() - 1)
        };
        Self {
            selected_index,
            entries,
        }
    }

    pub fn handle_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn handle_down(&mut self) {
        if self.selected_index + 1 < self.entries.len() {
            self.selected_index += 1;
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        focused: bool,
        ssd_moe_active_spec_id: Option<&str>,
        active_model_status: &ModelStatus,
    ) {
        let bg = if focused {
            theme.palette.sidebar_bg
        } else {
            theme.palette.editor_bg
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                theme.palette.accent_blue
            } else {
                theme.palette.subtle_border
            }))
            .style(Style::default().bg(bg))
            .title(Span::styled(
                " Models (Ctrl+M to close) ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(theme.palette.text_muted),
            ));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 10 || inner.height < 5 {
            return;
        }

        let mut constraints = Vec::new();
        for _ in &self.entries {
            constraints.push(Constraint::Length(6));
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Min(1));
        constraints.push(Constraint::Length(1));

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        for (i, entry) in self.entries.iter().enumerate() {
            let chunk = chunks[i * 2];
            let is_selected = i == self.selected_index;
            let is_moe_server_active = ssd_moe_active_spec_id.is_some_and(|active_id| {
                model_registry::local_moe_spec_for_registry_id(&entry.registry_id)
                    .map(|s| s.id == active_id)
                    .unwrap_or(false)
            });

            let card_bg = if is_selected && focused {
                theme.palette.editor_bg
            } else {
                theme.palette.sidebar_bg
            };
            let mut border_style = Style::default().fg(theme.palette.subtle_border);
            if is_selected {
                border_style = border_style.fg(theme.palette.accent_blue);
            }

            let card_block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .style(Style::default().bg(card_bg));

            let card_inner = card_block.inner(chunk);
            frame.render_widget(card_block, chunk);

            let status_text = if is_moe_server_active {
                format!(
                    "{} {}",
                    active_model_status.indicator(),
                    active_model_status.label()
                )
            } else {
                "⬜ Offline".to_string()
            };

            let prefix = if is_moe_server_active { "● " } else { "○ " };

            let disk_label = entry
                .disk_gb
                .map(|g| format!("{g} GB"))
                .unwrap_or_else(|| "—".to_string());

            let line1 = Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default().fg(if is_moe_server_active {
                        theme.palette.accent_blue
                    } else {
                        theme.palette.text_muted
                    }),
                ),
                Span::styled(
                    entry.title.as_str(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled(
                    disk_label,
                    Style::default().fg(theme.palette.text_muted),
                ),
            ]);

            let line2 = Line::from(vec![Span::raw("Status: "), Span::raw(status_text)]);

            let desc = Line::from(Span::styled(
                entry.subtitle.as_str(),
                Style::default().fg(theme.palette.text_muted),
            ));

            let hint = if is_selected && !is_moe_server_active {
                Line::from(Span::styled(
                    "[Enter] to select / download",
                    Style::default().fg(theme.palette.accent_blue),
                ))
            } else {
                Line::default()
            };

            let paragraph = Paragraph::new(vec![line1, line2, desc, hint]);
            frame.render_widget(paragraph, card_inner);
        }

        let footer_chunk = chunks.last().unwrap();
        let disk_text = Line::from(Span::styled(
            "Disk: ~47 GB free / 926 GB total",
            Style::default().fg(theme.palette.text_muted),
        ));
        frame.render_widget(Paragraph::new(disk_text), *footer_chunk);
    }
}
