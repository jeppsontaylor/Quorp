use std::ops::ControlFlow;
use std::time::Duration;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::fixtures;
use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, PaneType, TuiApp};
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::tui_backend::SharedTuiBackend;

pub struct TuiTestHarness {
    pub app: TuiApp,
    pub terminal: Terminal<TestBackend>,
    _event_rx_keepalive: std::sync::mpsc::Receiver<TuiEvent>,
}

impl TuiTestHarness {
    pub fn screenshot_output_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tui_screenshots")
    }

    pub fn new(cols: u16, rows: u16) -> Self {
        let (app, event_rx) = TuiApp::new_for_flow_tests(fixtures::fixture_project_root());
        let backend = TestBackend::new(cols, rows);
        let terminal = Terminal::new(backend).expect("terminal");
        Self {
            app,
            terminal,
            _event_rx_keepalive: event_rx,
        }
    }

    pub fn new_with_terminal_bridge(
        cols: u16,
        rows: u16,
        bridge_tx: futures::channel::mpsc::UnboundedSender<
            crate::quorp::tui::bridge::TuiToBackendRequest,
        >,
    ) -> Self {
        let (mut app, event_rx) = TuiApp::new_for_flow_tests(fixtures::fixture_project_root());
        app.terminal = crate::quorp::tui::terminal_pane::TerminalPane::with_bridge(Some(bridge_tx));
        let backend = TestBackend::new(cols, rows);
        let terminal = Terminal::new(backend).expect("terminal");
        Self {
            app,
            terminal,
            _event_rx_keepalive: event_rx,
        }
    }

    pub fn new_with_root(cols: u16, rows: u16, root: std::path::PathBuf) -> Self {
        let (mut app, event_rx) = TuiApp::new_for_flow_tests(root.clone());
        app.file_tree = crate::quorp::tui::file_tree::FileTree::with_root(root);
        app.chat.ensure_project_root(app.file_tree.root());
        let backend = TestBackend::new(cols, rows);
        let terminal = Terminal::new(backend).expect("terminal");
        Self {
            app,
            terminal,
            _event_rx_keepalive: event_rx,
        }
    }

    /// Playwright-style harness with production-shaped wiring: file tree and code preview use backend
    /// senders (requests go to a sink). Tests inject [`crate::quorp::tui::TuiEvent`] via
    /// [`TuiTestHarness::apply_backend_event`] to mirror backend outputs without running `main`.
    /// Path index is **project-backed** so [`crate::quorp::tui::TuiEvent::PathIndexSnapshot`] is honored
    /// (unlike [`Self::new_with_root`], which uses a disk walk). Supply a snapshot before @-mention
    /// assertions, or only files from that snapshot appear — not whatever happens to be on disk.
    /// Chat list matches production (`provider/model` strings) and [`ChatPane`] uses the native
    /// backend request path. The bridge receiver is dropped; use
    /// [`TuiApp::new_for_flow_tests_with_registry_chat`] when a test needs to inspect backend
    /// requests directly.
    pub fn new_with_registry_chat(
        cols: u16,
        rows: u16,
        root: std::path::PathBuf,
        models: Vec<String>,
        model_index: usize,
    ) -> Self {
        let (app, event_rx, _bridge_rx) =
            TuiApp::new_for_flow_tests_with_registry_chat(root, models, model_index);
        let backend = TestBackend::new(cols, rows);
        let terminal = Terminal::new(backend).expect("terminal");
        Self {
            app,
            terminal,
            _event_rx_keepalive: event_rx,
        }
    }

    pub fn new_with_backend_state(cols: u16, rows: u16, root: std::path::PathBuf) -> Self {
        let (editor_pane_tx, _editor_pane_rx) = futures::channel::mpsc::unbounded();
        let (file_tree_tx, _file_tree_rx) = futures::channel::mpsc::unbounded();
        let (mut app, event_rx) = TuiApp::new_for_flow_tests(root.clone());
        let file_tree_backend =
            std::sync::Arc::new(crate::quorp::tui::bridge::UnifiedBridgeTuiBackend::new(
                file_tree_tx,
            )) as SharedTuiBackend;
        let editor_backend =
            std::sync::Arc::new(crate::quorp::tui::bridge::UnifiedBridgeTuiBackend::new(
                editor_pane_tx,
            )) as SharedTuiBackend;
        app.file_tree = crate::quorp::tui::file_tree::FileTree::with_root(root);
        app.file_tree.set_backend(file_tree_backend);
        app.editor_pane =
            crate::quorp::tui::editor_pane::EditorPane::with_buffer_bridge(Some(editor_backend));
        app.chat.ensure_project_root(app.file_tree.root());
        app.chat
            .use_project_backed_path_index_for_backend_flow_tests(
                app.file_tree.root().to_path_buf(),
            );
        let backend = TestBackend::new(cols, rows);
        let terminal = Terminal::new(backend).expect("terminal");
        Self {
            app,
            terminal,
            _event_rx_keepalive: event_rx,
        }
    }

    pub fn apply_backend_event(&mut self, event: crate::quorp::tui::TuiEvent) {
        self.app.apply_tui_backend_event(event);
    }

    pub fn draw(&mut self) {
        self.terminal
            .draw(|frame| {
                self.app.draw(frame);
            })
            .expect("draw");
    }

    pub fn buffer(&self) -> &Buffer {
        self.terminal.backend().buffer()
    }

    pub fn buffer_string(&self) -> String {
        let area = self.buffer().area();
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(
                    self.buffer()
                        .cell((x, y))
                        .map(|c| c.symbol())
                        .unwrap_or(" "),
                );
            }
        }
        out
    }

    /// Captures the current `ratatui` buffer deterministically as a an RGBA image array.
    /// Does not require a GPU or WindowServer (headless safe).
    pub fn screenshot(&mut self) -> image::RgbaImage {
        self.draw();
        crate::quorp::tui::buffer_png::buffer_to_rgba(self.buffer())
    }

    /// Captures the TUI frame and saves it to a persistent output directory for Playwright artifacts.
    pub fn save_screenshot(&mut self, base_name: &str) -> std::path::PathBuf {
        let img = self.screenshot();
        let target_dir = Self::screenshot_output_dir();
        std::fs::create_dir_all(&target_dir).expect("failed to mkdir target/tui_screenshots");
        let path = target_dir.join(format!("{}.png", base_name));
        img.save(&path).expect("failed to save screenshot png");
        path
    }

    pub fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> ControlFlow<(), ()> {
        self.app
            .handle_event(Event::Key(KeyEvent::new(code, modifiers)))
    }

    pub fn key_press(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        assert!(self.key(code, modifiers).is_continue());
    }

    pub fn mouse_left_down(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_click(column, row);
    }

    pub fn mouse_move_to(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    pub fn mouse_drag_left(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    pub fn mouse_left_up(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let area = Rect::new(0, 0, cols, rows);
        self.terminal.resize(area).expect("resize");
        let _ = self.app.handle_event(Event::Resize(cols, rows));
    }

    pub fn assert_buffer_contains(&self, needle: &str) {
        let hay = self.buffer_string();
        assert!(
            hay.contains(needle),
            "buffer missing {needle:?}; sample: {}",
            hay.chars().take(200).collect::<String>()
        );
    }

    pub fn assert_buffer_not_contains(&self, needle: &str) {
        let hay = self.buffer_string();
        assert!(
            !hay.contains(needle),
            "buffer unexpectedly contained {needle:?}; sample: {}",
            hay.chars().take(400).collect::<String>()
        );
    }

    pub fn assert_focus(&self, pane: PaneType) {
        assert_eq!(
            self.app.focused, pane,
            "expected focus {:?}, got {:?}",
            pane, self.app.focused
        );
    }

    pub fn assert_overlay(&self, overlay: Overlay) {
        assert_eq!(self.app.overlay, overlay);
    }

    pub fn assert_status_contains(&self, needle: &str) {
        let s = self.app.status_bar_text();
        assert!(s.contains(needle), "status {s:?} missing {needle:?}");
    }

    pub fn apply_chat_event(&mut self, event: ChatUiEvent) {
        let theme = self.app.theme.clone();
        self.app.chat.apply_chat_event(event, &theme);
    }

    pub fn recv_tui_event_timeout(&self, timeout: Duration) -> Option<TuiEvent> {
        match self._event_rx_keepalive.recv_timeout(timeout) {
            Ok(event) => Some(event),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        }
    }

    pub fn wait_path_index_ready(&mut self, timeout: Duration) -> bool {
        self.app.chat.blocking_wait_path_index_ready(timeout)
    }
}
