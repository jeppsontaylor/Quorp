#![allow(unused)]
//! Terminal pane: ratatui paint from integrated snapshots; shell I/O goes through
//! native backend requests sent over [`crate::quorp::tui::bridge::TuiToBackendRequest`].

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::path::{Path, PathBuf};

use crate::quorp::tui::terminal_trace::{
    SharedTerminalTraceBuffer, new_shared_terminal_trace, record_trace,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum TerminalInteractionMode {
    #[default]
    Capture,
    Navigate,
}

pub struct TerminalPane {
    last_grid: (u16, u16),
    pub pty_exited: bool,
    bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
    >,
    snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot,
    latest_cwd: Option<PathBuf>,
    shell_label: Option<String>,
    window_title: Option<String>,
    interaction_mode: TerminalInteractionMode,
    trace: Option<SharedTerminalTraceBuffer>,
}

impl TerminalPane {
    pub fn new() -> Self {
        Self {
            last_grid: (80, 24),
            pty_exited: false,
            bridge_tx: None,
            snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::blank(24, 80),
            latest_cwd: None,
            shell_label: None,
            window_title: None,
            interaction_mode: TerminalInteractionMode::Capture,
            trace: if cfg!(any(test, debug_assertions)) {
                Some(new_shared_terminal_trace())
            } else {
                None
            },
        }
    }

    pub fn with_bridge(
        bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
        >,
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
            self.trace(format!("sync-grid cols={cols} rows={rows}"));
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
        self.trace(format!("spawn-pty cols={cols} rows={rows}"));
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

        self.render_snapshot(frame.buffer_mut(), area, focused, Color::Reset);
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

        self.render_snapshot(buf, inner, focused, theme.palette.editor_bg);
    }

    pub fn try_handle_key(&mut self, key: &KeyEvent) -> anyhow::Result<bool> {
        if self.pty_exited {
            if key.code == KeyCode::Enter
                && key.kind == KeyEventKind::Press
                && let Some(tx) = self.bridge_tx.clone()
            {
                let (c, r) = self.last_grid;
                self.pty_exited = false;
                self.trace(format!("restart-after-exit cols={c} rows={r}"));
                let _ = tx.unbounded_send(
                    crate::quorp::tui::bridge::TuiToBackendRequest::TerminalResize {
                        cols: c,
                        rows: r,
                    },
                );
            }
            return Ok(true);
        }

        if self.interaction_mode == TerminalInteractionMode::Navigate {
            return Ok(false);
        }

        if self.bridge_tx.is_some() {
            if key.kind == KeyEventKind::Release {
                return Ok(true);
            }
            use crate::quorp::tui::bridge::TuiToBackendRequest;
            match key.code {
                KeyCode::PageUp if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if let Some(tx) = &self.bridge_tx {
                        self.trace("scroll-page-up");
                        let _ = tx.unbounded_send(TuiToBackendRequest::TerminalScrollPageUp);
                    }
                    return Ok(true);
                }
                KeyCode::PageDown if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if let Some(tx) = &self.bridge_tx {
                        self.trace("scroll-page-down");
                        let _ = tx.unbounded_send(TuiToBackendRequest::TerminalScrollPageDown);
                    }
                    return Ok(true);
                }
                _ => {}
            }

            if let Some(ks) = crate::quorp::tui::bridge::crossterm_key_event_to_keystroke(key) {
                if let Some(tx) = &self.bridge_tx {
                    self.trace(format!(
                        "keystroke key={} ctrl={} alt={} shift={}",
                        ks.key, ks.modifiers.control, ks.modifiers.alt, ks.modifiers.shift
                    ));
                    let _ = tx.unbounded_send(TuiToBackendRequest::TerminalKeystroke(ks));
                }
                return Ok(true);
            }

            let bytes = key_event_to_pty_bytes(key);
            if bytes.is_empty() {
                return Ok(false);
            }
            if let Some(tx) = &self.bridge_tx {
                self.trace(format!("input-bytes len={}", bytes.len()));
                let _ = tx.unbounded_send(TuiToBackendRequest::TerminalInput(bytes));
            }
            return Ok(true);
        }

        if key.kind == KeyEventKind::Release {
            return Ok(true);
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => Ok(false),
            KeyCode::Char('h' | 'j' | 'k' | 'l')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                Ok(false)
            }
            KeyCode::Char('?') if key.modifiers.is_empty() => Ok(false),
            _ => Ok(false),
        }
    }

    pub fn apply_integrated_frame(&mut self, frame: crate::quorp::tui::bridge::TerminalFrame) {
        self.snapshot = frame.snapshot;
        if let Some(cwd) = frame.cwd {
            self.latest_cwd = Some(cwd);
        }
        if let Some(shell_label) = frame.shell_label {
            self.shell_label = Some(shell_label);
        }
        self.window_title = frame.window_title;
        self.trace(format!(
            "frame rows={} cols={} scrollback={} alt={} paste={} cursor={:?}",
            self.snapshot.rows,
            self.snapshot.cols,
            self.snapshot.scrollback,
            self.snapshot.alternate_screen,
            self.snapshot.bracketed_paste,
            self.snapshot.cursor
        ));
    }

    pub fn shell_title(&self) -> String {
        "Terminal".to_string()
    }

    pub fn shell_label(&self) -> String {
        self.shell_label.clone().unwrap_or_else(default_shell_label)
    }

    pub fn shell_window_title(&self) -> Option<String> {
        self.window_title.clone()
    }

    pub fn shell_path_label(&self, fallback_root: &Path) -> String {
        self.latest_cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| fallback_root.display().to_string())
    }

    pub fn shell_lines(&self, max_lines: usize) -> Vec<String> {
        self.snapshot.row_strings(max_lines)
    }

    pub fn snapshot(&self) -> crate::quorp::tui::terminal_surface::TerminalSnapshot {
        self.snapshot.clone()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.snapshot.alternate_screen
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.snapshot.bracketed_paste
    }

    pub fn in_capture_mode(&self) -> bool {
        self.interaction_mode == TerminalInteractionMode::Capture
    }

    pub fn enter_capture_mode(&mut self) {
        self.interaction_mode = TerminalInteractionMode::Capture;
        self.trace("mode=capture");
    }

    pub fn enter_navigation_mode(&mut self) {
        self.interaction_mode = TerminalInteractionMode::Navigate;
        self.trace("mode=navigate");
    }

    pub fn toggle_interaction_mode(&mut self) {
        self.interaction_mode = match self.interaction_mode {
            TerminalInteractionMode::Capture => TerminalInteractionMode::Navigate,
            TerminalInteractionMode::Navigate => TerminalInteractionMode::Capture,
        };
    }

    pub fn interaction_hint(&self) -> &'static str {
        match self.interaction_mode {
            TerminalInteractionMode::Capture => "Shell capture  Ctrl+g navigate  Ctrl+` hide dock",
            TerminalInteractionMode::Navigate => {
                "Terminal navigation  Enter capture  Tab next focus  Ctrl+` hide dock"
            }
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> anyhow::Result<bool> {
        let Some(tx) = &self.bridge_tx else {
            return Ok(false);
        };
        let mut bytes = Vec::new();
        if self.bracketed_paste_enabled() {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if self.bracketed_paste_enabled() {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.trace(format!(
            "paste len={} bracketed={}",
            text.len(),
            self.bracketed_paste_enabled()
        ));
        let _ =
            tx.unbounded_send(crate::quorp::tui::bridge::TuiToBackendRequest::TerminalInput(bytes));
        Ok(true)
    }

    pub fn notify_focus_changed(&self, focused: bool) {
        if let Some(tx) = &self.bridge_tx {
            record_trace(self.trace.as_ref(), format!("focus={focused}"));
            let _ = tx.unbounded_send(
                crate::quorp::tui::bridge::TuiToBackendRequest::TerminalFocusChanged { focused },
            );
        }
    }

    pub fn mark_integrated_session_closed(&mut self) {
        self.pty_exited = true;
        self.trace("closed");
    }

    #[cfg(test)]
    pub fn pty_exited_for_test(&self) -> bool {
        self.pty_exited
    }

    #[cfg(test)]
    pub fn trace_dump_for_test(&self) -> String {
        crate::quorp::tui::terminal_trace::dump_trace(self.trace.as_ref())
    }

    #[cfg(test)]
    pub fn snapshot_dump_for_test(&self) -> String {
        let rows = self.snapshot.row_strings(usize::from(self.snapshot.rows));
        format!(
            "rows={} cols={} cursor={:?} hide_cursor={} scrollback={} alternate_screen={} bracketed_paste={} cwd={:?} shell_label={:?} window_title={:?}\n{}",
            self.snapshot.rows,
            self.snapshot.cols,
            self.snapshot.cursor,
            self.snapshot.hide_cursor,
            self.snapshot.scrollback,
            self.snapshot.alternate_screen,
            self.snapshot.bracketed_paste,
            self.latest_cwd,
            self.shell_label,
            self.window_title,
            rows.join("\n")
        )
    }

    fn render_snapshot(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        focused: bool,
        bg: Color,
    ) {
        self.snapshot
            .render(buf, area, bg, focused && self.in_capture_mode());
    }

    fn trace(&self, entry: impl Into<String>) {
        record_trace(self.trace.as_ref(), entry);
    }
}

fn cols_rows_nonzero(cols: u16, rows: u16) -> bool {
    cols > 0 && rows > 0
}

fn default_shell_label() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|shell| {
            Path::new(&shell)
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "shell".to_string())
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
    let mut bytes = if modifiers.contains(KeyModifiers::CONTROL) {
        let lower = c.to_ascii_lowercase();
        match lower {
            'a'..='z' => vec![(lower as u8).saturating_sub(b'a') + 1],
            ' ' => vec![0],
            '[' => vec![0x1b],
            '\\' => vec![0x1c],
            ']' => vec![0x1d],
            '^' => vec![0x1e],
            '_' | '?' => vec![0x1f],
            '8' | '@' => vec![0],
            _ => Vec::new(),
        }
    } else {
        let mut buf = [0u8; 4];
        c.encode_utf8(&mut buf).as_bytes().to_vec()
    };
    if modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        let mut alt_prefixed = vec![0x1b];
        alt_prefixed.append(&mut bytes);
        alt_prefixed
    } else {
        bytes
    }
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
