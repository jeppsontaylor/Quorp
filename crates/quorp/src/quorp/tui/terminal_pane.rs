#![allow(unused)]
//! Terminal pane: ratatui paint from integrated snapshots; shell I/O goes through
//! [`crate::quorp::tui::terminal_bridge`] to Quorp's `terminal::Terminal` entity.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

pub struct TerminalPane {
    last_grid: (u16, u16),
    pub pty_exited: bool,
    bridge_tx:
        Option<futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>>,
    integrated_lines: Vec<Line<'static>>,
}

impl TerminalPane {
    pub fn new() -> Self {
        Self {
            last_grid: (80, 24),
            pty_exited: false,
            bridge_tx: None,
            integrated_lines: Vec::new(),
        }
    }

    pub fn with_bridge(
        bridge_tx: Option<futures::channel::mpsc::UnboundedSender<
            crate::quorp::tui::bridge::TuiToBackendRequest,
        >>,
    ) -> Self {
        Self {
            bridge_tx,
            ..Self::new()
        }
    }

    pub fn sync_grid(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        if let Some(tx) = &self.bridge_tx {
            if (cols, rows) == self.last_grid {
                return Ok(());
            }
            self.last_grid = (cols, rows);
            let _ = tx.unbounded_send(
                crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize { cols, rows },
            );
        } else {
            self.last_grid = (cols, rows);
        }
        Ok(())
    }

    pub fn spawn_pty(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        self.last_grid = (cols, rows);
        self.pty_exited = false;
        if let Some(tx) = &self.bridge_tx {
            let _ = tx.unbounded_send(
                crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize { cols, rows },
            );
        }
        Ok(())
    }

    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        if self.pty_exited {
            let msg = Paragraph::new(vec![
                Line::from("Shell exited."),
                Line::from("Press Enter to restart."),
            ])
            .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(msg, area);
            return;
        }

        let w = self.render_integrated(area, focused, Color::Reset);
        frame.render_widget(w, area);
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        rects: &crate::quorp::tui::workbench::LeafRects,
        focused: bool,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        use ratatui::widgets::Widget;

        crate::quorp::tui::paint::fill_rect(buf, rects.body, theme.palette.editor_bg);
        crate::quorp::tui::paint::fill_rect(buf, rects.scrollbar, theme.palette.editor_bg);

        if let Some(panel_tabs_rect) = rects.panel_tabs {
            let tabs = vec![crate::quorp::tui::chrome_v2::PanelTabVm {
                label: "Terminal".to_string(),
                active: true,
            }];
            crate::quorp::tui::chrome_v2::render_panel_tabs(
                buf,
                panel_tabs_rect,
                &tabs,
                Some("zsh"),
                &theme.palette,
            );
        }

        if rects.body.height == 0 || rects.body.width == 0 {
            return;
        }

        let inner = rects.body;

        if self.pty_exited {
            let msg = Paragraph::new(vec![
                Line::from("Shell exited."),
                Line::from("Press Enter to restart."),
            ])
            .style(Style::default().fg(Color::DarkGray));
            msg.render(inner, buf);
            return;
        }

        if cols_rows_nonzero(inner.width, inner.height) {
            let _ = self.sync_grid(inner.width, inner.height);
        }

        let w = self.render_integrated(inner, focused, theme.palette.editor_bg);
        w.render(inner, buf);
    }

    pub fn try_handle_key(&mut self, key: &KeyEvent) -> anyhow::Result<bool> {
        if self.pty_exited {
            if key.code == KeyCode::Enter && key.kind == KeyEventKind::Press {
                if let Some(tx) = self.bridge_tx.clone() {
                    let (c, r) = self.last_grid;
                    self.pty_exited = false;
                    let _ = tx.unbounded_send(
                        crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize {
                            cols: c,
                            rows: r,
                        },
                    );
                }
            }
            return Ok(true);
        }

        if self.bridge_tx.is_some() {
            if key.kind == KeyEventKind::Release {
                return Ok(true);
            }
            use crate::quorp::tui::bridge::TuiToBackendRequest;
            match key.code {
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => return Ok(false),
                KeyCode::Char('h' | 'j' | 'k' | 'l')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Ok(false);
                }
                KeyCode::Char('?') if key.modifiers.is_empty() => return Ok(false),
                KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if let Some(tx) = &self.bridge_tx {
                        let _ = tx.unbounded_send(TuiToBackendRequest::TerminalScrollPageUp);
                    }
                    return Ok(true);
                }
                KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if let Some(tx) = &self.bridge_tx {
                        let _ = tx.unbounded_send(TuiToBackendRequest::TerminalScrollPageDown);
                    }
                    return Ok(true);
                }
                _ => {}
            }

            if let Some(ks) =
                crate::quorp::tui::bridge::crossterm_key_event_to_keystroke(key)
            {
                if let Some(tx) = &self.bridge_tx {
                    let _ = tx.unbounded_send(TuiToBackendRequest::TerminalKeystroke(ks));
                }
                return Ok(true);
            }

            let bytes = key_event_to_pty_bytes(key);
            if bytes.is_empty() {
                return Ok(false);
            }
            if let Some(tx) = &self.bridge_tx {
                let _ = tx.unbounded_send(TuiToBackendRequest::TerminalInput(bytes));
            }
            return Ok(true);
        }

        if key.kind == KeyEventKind::Release {
            return Ok(true);
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => Ok(false),
            KeyCode::Char('h' | 'j' | 'k' | 'l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Ok(false)
            }
            KeyCode::Char('?') if key.modifiers.is_empty() => Ok(false),
            _ => Ok(false),
        }
    }

    pub fn apply_integrated_frame(&mut self, frame: crate::quorp::tui::bridge::TerminalFrame) {
        self.integrated_lines = frame.lines;
    }

    pub fn mark_integrated_session_closed(&mut self) {
        self.pty_exited = true;
    }

    #[cfg(test)]
    pub fn pty_exited_for_test(&self) -> bool {
        self.pty_exited
    }

    fn render_integrated(&self, area: Rect, focused: bool, bg: Color) -> Paragraph<'static> {
        let cols = area.width as usize;
        let rows = area.height as usize;
        let pad_line = || Line::from(" ".repeat(cols.max(1)));
        let mut lines: Vec<Line> = self
            .integrated_lines
            .iter()
            .take(rows)
            .cloned()
            .collect();
        while lines.len() < rows {
            lines.push(pad_line());
        }
        let _ = focused;
        Paragraph::new(lines).style(Style::default().bg(bg))
    }
}

fn cols_rows_nonzero(cols: u16, rows: u16) -> bool {
    cols > 0 && rows > 0
}

pub fn key_event_to_pty_bytes(key: &KeyEvent) -> Vec<u8> {
    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }
    match key.code {
        KeyCode::Char(c) => char_to_pty_bytes(c, key.modifiers),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::F(n) => f_key_bytes(n),
        _ => Vec::new(),
    }
}

fn f_key_bytes(n: u8) -> Vec<u8> {
    match n {
        1 => vec![0x1b, b'O', b'P'],
        2 => vec![0x1b, b'O', b'Q'],
        3 => vec![0x1b, b'O', b'R'],
        4 => vec![0x1b, b'O', b'S'],
        5 => vec![0x1b, b'[', b'1', b'5', b'~'],
        6 => vec![0x1b, b'[', b'1', b'7', b'~'],
        7 => vec![0x1b, b'[', b'1', b'8', b'~'],
        8 => vec![0x1b, b'[', b'1', b'9', b'~'],
        9 => vec![0x1b, b'[', b'2', b'0', b'~'],
        10 => vec![0x1b, b'[', b'2', b'1', b'~'],
        11 => vec![0x1b, b'[', b'2', b'3', b'~'],
        12 => vec![0x1b, b'[', b'2', b'4', b'~'],
        _ => Vec::new(),
    }
}

fn char_to_pty_bytes(c: char, modifiers: KeyModifiers) -> Vec<u8> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        let lower = c.to_ascii_lowercase();
        return match lower {
            'a'..='z' => vec![(lower as u8).saturating_sub(b'a') + 1],
            ' ' => vec![0],
            '[' => vec![0x1b],
            '\\' => vec![0x1c],
            ']' => vec![0x1d],
            '^' => vec![0x1e],
            '_' | '?' => vec![0x1f],
            '8' | '@' => vec![0],
            _ => Vec::new(),
        };
    }
    if modifiers.contains(KeyModifiers::ALT) {
        let mut v = vec![0x1b];
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        v.extend_from_slice(s.as_bytes());
        return v;
    }
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use futures::StreamExt as _;

    #[test]
    fn key_forwarding_arrow_and_ctrl_c() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(key_event_to_pty_bytes(&up), vec![0x1b, b'[', b'A']);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_pty_bytes(&ctrl_c), vec![0x03]);
    }

    #[test]
    fn bridge_shift_page_up_sends_scroll_request() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let mut pane = TerminalPane::with_bridge(Some(tx));
        pane.spawn_pty(10, 10).expect("spawn");
        let page_up = KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT);
        assert!(pane.try_handle_key(&page_up).expect("key"));
        let mut msg = futures::executor::block_on(rx.next());
        if matches!(
            msg,
            Some(crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize { .. })
        ) {
            msg = futures::executor::block_on(rx.next());
        }
        assert!(
            matches!(
                msg,
                Some(crate::quorp::tui::bridge::TuiToBackendRequest::TerminalScrollPageUp)
            ),
            "expected ScrollPageUp, got {msg:?}"
        );
    }

    #[test]
    fn bridge_maps_arrow_to_keystroke() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let mut pane = TerminalPane::with_bridge(Some(tx));
        pane.spawn_pty(10, 10).expect("spawn");
        let _ = futures::executor::block_on(rx.next());
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert!(pane.try_handle_key(&up).expect("key"));
        let msg = futures::executor::block_on(rx.next());
        match msg {
            Some(crate::quorp::tui::bridge::TuiToBackendRequest::TerminalKeystroke(ks)) => {
                assert_eq!(ks.key, "up");
            }
            other => panic!("expected Keystroke(up), got {other:?}"),
        }
    }
}
