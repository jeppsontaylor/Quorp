use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use regex::Regex;
use serde::Serialize;

use super::fixtures;
use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, PaneType, TuiApp};
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::tui_backend::SharedTuiBackend;

pub struct TuiTestHarness {
    pub app: TuiApp,
    pub terminal: Terminal<TestBackend>,
    _event_rx_keepalive: std::sync::mpsc::Receiver<TuiEvent>,
    replay_log: Vec<ScenarioReplayEntry>,
    replay_started_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
struct ScenarioReplayEntry {
    step_index: usize,
    elapsed_ms: u128,
    kind: String,
    detail: String,
    focus: String,
    overlay: String,
    proof_mode: String,
    confidence: f32,
    status: String,
    buffer_excerpt: String,
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
            replay_log: Vec::new(),
            replay_started_at: Instant::now(),
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
            replay_log: Vec::new(),
            replay_started_at: Instant::now(),
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
            replay_log: Vec::new(),
            replay_started_at: Instant::now(),
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
            replay_log: Vec::new(),
            replay_started_at: Instant::now(),
        }
    }

    pub fn new_with_backend_state(cols: u16, rows: u16, root: std::path::PathBuf) -> Self {
        let (editor_pane_tx, _editor_pane_rx) = futures::channel::mpsc::unbounded();
        let (file_tree_tx, _file_tree_rx) = futures::channel::mpsc::unbounded();
        let (mut app, event_rx) = TuiApp::new_for_flow_tests(root.clone());
        let file_tree_backend = std::sync::Arc::new(
            crate::quorp::tui::bridge::UnifiedBridgeTuiBackend::new(file_tree_tx),
        ) as SharedTuiBackend;
        let editor_backend = std::sync::Arc::new(
            crate::quorp::tui::bridge::UnifiedBridgeTuiBackend::new(editor_pane_tx),
        ) as SharedTuiBackend;
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
            replay_log: Vec::new(),
            replay_started_at: Instant::now(),
        }
    }

    pub fn apply_backend_event(&mut self, event: crate::quorp::tui::TuiEvent) {
        let detail = describe_tui_event(&event);
        self.app.apply_tui_backend_event(event);
        self.record_replay_step("backend_event", detail);
    }

    pub fn draw(&mut self) {
        self.terminal
            .draw(|frame| {
                self.app.draw(frame);
            })
            .expect("draw");
        self.record_replay_step("draw", "render current frame");
    }

    fn draw_silent(&mut self) {
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

    /// Captures the current `ratatui` buffer deterministically as a PNG plus plain-text dump.
    /// Does not require a GPU or WindowServer (headless safe).
    pub fn screenshot_artifact(&mut self) -> crate::quorp::tui::buffer_png::RenderedFrameArtifact {
        self.draw();
        crate::quorp::tui::buffer_png::render_frame_artifact(
            self.buffer(),
            crate::quorp::tui::buffer_png::CellRasterConfig::default(),
        )
    }

    pub fn screenshot(&mut self) -> image::RgbaImage {
        self.screenshot_artifact().png
    }

    pub fn shell_preview_artifact(
        &mut self,
    ) -> crate::quorp::tui::buffer_png::RenderedFrameArtifact {
        self.terminal
            .draw(|frame| {
                self.app.draw_shell_preview(frame);
            })
            .expect("draw shell preview");
        crate::quorp::tui::buffer_png::render_frame_artifact(
            self.buffer(),
            crate::quorp::tui::buffer_png::CellRasterConfig::default(),
        )
    }

    /// Captures the TUI frame and saves it to a persistent output directory for Playwright artifacts.
    pub fn save_screenshot(&mut self, base_name: &str) -> std::path::PathBuf {
        let artifact = self.screenshot_artifact();
        let target_dir = Self::screenshot_output_dir();
        Self::save_render_artifact(&target_dir, base_name, artifact)
    }

    pub fn save_shell_preview_screenshot(&mut self, base_name: &str) -> std::path::PathBuf {
        let artifact = self.shell_preview_artifact();
        let target_dir = Self::screenshot_output_dir();
        Self::save_render_artifact(&target_dir, base_name, artifact)
    }

    pub fn save_failure_artifacts(
        &mut self,
        output_dir: &std::path::Path,
        base_name: &str,
    ) -> std::path::PathBuf {
        let artifact = self.screenshot_artifact();
        let path = Self::save_render_artifact(output_dir, base_name, artifact);

        let terminal_trace = self.app.terminal.trace_dump_for_test();
        if !terminal_trace.trim().is_empty() {
            let trace_path = output_dir.join(format!("{}.terminal-trace.txt", base_name));
            std::fs::write(&trace_path, terminal_trace)
                .expect("failed to save terminal trace dump");
        }

        let snapshot_path = output_dir.join(format!("{}.terminal-snapshot.txt", base_name));
        std::fs::write(&snapshot_path, self.app.terminal.snapshot_dump_for_test())
            .expect("failed to save terminal snapshot dump");

        let replay_path = output_dir.join(format!("{}.replay.jsonl", base_name));
        std::fs::write(&replay_path, self.replay_log_jsonl()).expect("failed to save replay log");

        path
    }

    pub fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> ControlFlow<(), ()> {
        let flow = self
            .app
            .handle_event(Event::Key(KeyEvent::new(code, modifiers)));
        self.record_replay_step("key", format!("{} {:?}", key_label(code), modifiers));
        flow
    }

    pub fn key_press(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        assert!(self.key(code, modifiers).is_continue());
    }

    #[allow(dead_code)]
    pub fn paste(&mut self, text: impl Into<String>) {
        let text = text.into();
        let _ = self.app.handle_event(Event::Paste(text.clone()));
        self.record_replay_step("paste", text);
    }

    pub fn mouse_left_down(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_click(column, row);
        self.record_replay_step("mouse_left_down", format!("{column},{row}"));
    }

    pub fn mouse_move_to(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        self.record_replay_step("mouse_move", format!("{column},{row}"));
    }

    pub fn mouse_drag_left(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        self.record_replay_step("mouse_drag_left", format!("{column},{row}"));
    }

    pub fn mouse_left_up(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        self.record_replay_step("mouse_left_up", format!("{column},{row}"));
    }

    pub fn mouse_scroll_up(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        self.record_replay_step("mouse_scroll_up", format!("{column},{row}"));
    }

    pub fn mouse_scroll_down(&mut self, column: u16, row: u16) {
        self.app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        self.record_replay_step("mouse_scroll_down", format!("{column},{row}"));
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let area = Rect::new(0, 0, cols, rows);
        self.terminal.resize(area).expect("resize");
        let _ = self.app.handle_event(Event::Resize(cols, rows));
        self.record_replay_step("resize", format!("{cols}x{rows}"));
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

    pub fn find_text(&self, needle: &str) -> Option<(u16, u16)> {
        let area = self.buffer().area();
        for y in area.top()..area.bottom() {
            let row = (area.left()..area.right())
                .map(|x| {
                    self.buffer()
                        .cell((x, y))
                        .map(|cell| cell.symbol())
                        .unwrap_or(" ")
                })
                .collect::<String>();
            if let Some(index) = row.find(needle) {
                return Some((area.left() + index as u16, y));
            }
        }
        None
    }

    pub fn assert_text_has_nondefault_fg(&self, needle: &str) {
        let (x, y) = self.find_text(needle).unwrap_or_else(|| {
            panic!(
                "buffer missing {needle:?}; sample: {}",
                self.buffer_string()
            )
        });
        let Some(cell) = self.buffer().cell((x, y)) else {
            panic!("missing cell for {needle:?} at ({x}, {y})");
        };
        assert_ne!(cell.fg, Color::Reset, "expected styled fg for {needle:?}");
    }

    pub fn assert_text_bg(&self, needle: &str, color: Color) {
        let (x, y) = self.find_text(needle).unwrap_or_else(|| {
            panic!(
                "buffer missing {needle:?}; sample: {}",
                self.buffer_string()
            )
        });
        let Some(cell) = self.buffer().cell((x, y)) else {
            panic!("missing cell for {needle:?} at ({x}, {y})");
        };
        assert_eq!(cell.bg, color, "unexpected bg for {needle:?}");
    }

    pub fn assert_text_fg(&self, needle: &str, color: Color) {
        let (x, y) = self.find_text(needle).unwrap_or_else(|| {
            panic!(
                "buffer missing {needle:?}; sample: {}",
                self.buffer_string()
            )
        });
        let Some(cell) = self.buffer().cell((x, y)) else {
            panic!("missing cell for {needle:?} at ({x}, {y})");
        };
        assert_eq!(cell.fg, color, "unexpected fg for {needle:?}");
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
        let detail = format!("{event:?}");
        self.app.handle_chat_ui_event(event);
        self.record_replay_step("chat_event", detail);
    }

    pub fn save_replay_log(
        &self,
        output_dir: &std::path::Path,
        base_name: &str,
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(output_dir).expect("failed to mkdir replay directory");
        let replay_path = output_dir.join(format!("{}.replay.jsonl", base_name));
        std::fs::write(&replay_path, self.replay_log_jsonl()).expect("failed to save replay log");
        replay_path
    }

    pub fn clear_replay_log(&mut self) {
        self.replay_log.clear();
        self.replay_started_at = Instant::now();
    }

    pub fn recv_tui_event_timeout(&self, timeout: Duration) -> Option<TuiEvent> {
        match self._event_rx_keepalive.recv_timeout(timeout) {
            Ok(event) => Some(event),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        }
    }

    pub fn recv_tui_event_until(
        &self,
        timeout: Duration,
        mut predicate: impl FnMut(&TuiEvent) -> bool,
    ) -> Option<TuiEvent> {
        let started_at = Instant::now();
        loop {
            let elapsed = Instant::now().duration_since(started_at);
            if elapsed >= timeout {
                return None;
            }
            let remaining = timeout.saturating_sub(elapsed);
            match self
                ._event_rx_keepalive
                .recv_timeout(remaining.min(Duration::from_millis(50)))
            {
                Ok(event) if predicate(&event) => return Some(event),
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    pub fn drain_tui_events(&mut self) -> usize {
        let mut drained = 0usize;
        while let Ok(event) = self._event_rx_keepalive.try_recv() {
            let detail = describe_tui_event(&event);
            self.app.apply_tui_backend_event(event);
            self.record_replay_step("drain_tui_event", detail);
            drained += 1;
        }
        drained
    }

    fn drain_tui_events_silent(&mut self) -> usize {
        let mut drained = 0usize;
        while let Ok(event) = self._event_rx_keepalive.try_recv() {
            self.app.apply_tui_backend_event(event);
            drained += 1;
        }
        drained
    }

    pub fn pump_tui_events_for(&mut self, timeout: Duration) -> usize {
        let started_at = Instant::now();
        let mut drained = 0usize;
        loop {
            drained += self.drain_tui_events();
            if Instant::now().duration_since(started_at) >= timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        drained
    }

    pub fn wait_for_buffer_contains(&mut self, timeout: Duration, needle: &str) -> bool {
        let started_at = Instant::now();
        loop {
            self.drain_tui_events();
            self.draw();
            if self.buffer_string().contains(needle) {
                return true;
            }
            if Instant::now().duration_since(started_at) >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn wait_for_buffer_contains_silent(&mut self, timeout: Duration, needle: &str) -> bool {
        let started_at = Instant::now();
        loop {
            self.drain_tui_events_silent();
            self.draw_silent();
            if self.buffer_string().contains(needle) {
                return true;
            }
            if Instant::now().duration_since(started_at) >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn wait_path_index_ready(&mut self, timeout: Duration) -> bool {
        self.app.chat.blocking_wait_path_index_ready(timeout)
    }

    fn save_render_artifact(
        output_dir: &std::path::Path,
        base_name: &str,
        artifact: crate::quorp::tui::buffer_png::RenderedFrameArtifact,
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(output_dir).expect("failed to mkdir render artifact directory");
        let path = output_dir.join(format!("{}.png", base_name));
        artifact
            .png
            .save(&path)
            .expect("failed to save screenshot png");
        let dump_path = output_dir.join(format!("{}.txt", base_name));
        std::fs::write(&dump_path, artifact.plain_text_dump)
            .expect("failed to save screenshot text dump");
        path
    }

    pub fn replay_log_jsonl(&self) -> String {
        self.replay_log
            .iter()
            .map(|entry| {
                serde_json::to_string(entry)
                    .unwrap_or_else(|error| format!(r#"{{"error":"{error}"}}"#))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn replay_log_jsonl_normalized(&self) -> String {
        self.replay_log
            .iter()
            .cloned()
            .map(|mut entry| {
                entry.elapsed_ms = 0;
                entry.detail = normalize_replay_text(&entry.detail);
                entry.status = normalize_replay_text(&entry.status);
                entry.buffer_excerpt = normalize_replay_text(&entry.buffer_excerpt);
                serde_json::to_string(&entry)
                    .unwrap_or_else(|error| format!(r#"{{"error":"{error}"}}"#))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn assert_screenshot_golden(&mut self, name: &str) {
        let img = self.screenshot();
        let baseline_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/quorp/tui/tui_flow_tests/baselines");
        let baseline_path = baseline_dir.join(format!("{}.png", name));
        let actual_dir = Self::screenshot_output_dir().join("visual_regression_failures");

        if std::env::var("UPDATE_BASELINES").is_ok() {
            std::fs::create_dir_all(&baseline_dir).expect("create baseline dir");
            img.save(&baseline_path).expect("save baseline png");
            return;
        }

        if !baseline_path.exists() {
            let actual_name = format!("{name}.missing_baseline");
            let actual_path = self.save_failure_artifacts(&actual_dir, &actual_name);
            panic!(
                "Baseline {} missing. Run with UPDATE_BASELINES=1. Failure artifacts saved to {}",
                name,
                actual_path.display()
            );
        }

        let baseline = image::open(&baseline_path)
            .expect("open baseline image")
            .to_rgba8();
        let fraction = crate::quorp::tui::buffer_png::pixel_mismatch_fraction(&baseline, &img)
            .expect("diff screenshot");
        if fraction >= 0.05 {
            let actual_name = format!("{name}.actual");
            let actual_path = self.save_failure_artifacts(&actual_dir, &actual_name);
            panic!(
                "Visual regression for {}: mismatch fraction {}. Failure artifacts saved to {}",
                name,
                fraction,
                actual_path.display()
            );
        }
    }

    pub fn assert_replay_golden(&mut self, name: &str) {
        let baseline_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/quorp/tui/tui_flow_tests/baselines");
        let baseline_path = baseline_dir.join(format!("{}.replay.jsonl", name));
        let actual_dir = Self::screenshot_output_dir().join("visual_regression_failures");
        let replay = self.replay_log_jsonl_normalized();

        if std::env::var("UPDATE_BASELINES").is_ok() {
            std::fs::create_dir_all(&baseline_dir).expect("create baseline dir");
            std::fs::write(&baseline_path, replay).expect("save replay baseline");
            return;
        }

        if !baseline_path.exists() {
            let actual_name = format!("{name}.missing_replay_baseline");
            let actual_path = self.save_failure_artifacts(&actual_dir, &actual_name);
            panic!(
                "Replay baseline {} missing. Run with UPDATE_BASELINES=1. Failure artifacts saved to {}",
                name,
                actual_path.display()
            );
        }

        let baseline = std::fs::read_to_string(&baseline_path).expect("read replay baseline file");
        if baseline != replay {
            let actual_name = format!("{name}.replay_mismatch");
            let actual_path = self.save_failure_artifacts(&actual_dir, &actual_name);
            panic!(
                "Replay regression for {}. Failure artifacts saved to {}",
                name,
                actual_path.display()
            );
        }
    }

    pub fn record_replay_step(&mut self, kind: impl Into<String>, detail: impl Into<String>) {
        let buffer_excerpt = self
            .buffer_string()
            .chars()
            .filter(|character| !character.is_control())
            .take(120)
            .collect::<String>();
        self.replay_log.push(ScenarioReplayEntry {
            step_index: self.replay_log.len(),
            elapsed_ms: self.replay_log.len() as u128,
            kind: kind.into(),
            detail: detail.into(),
            focus: format!("{:?}", self.app.focused),
            overlay: format!("{:?}", self.app.overlay),
            proof_mode: format!("{:?}", self.app.proof_rail.effective_mode()),
            confidence: self.app.proof_rail.snapshot.confidence_composite,
            status: self.app.status_bar_text(),
            buffer_excerpt,
        });
    }
}

fn normalize_replay_text(text: &str) -> String {
    static TMP_DIR_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static RUN_DIR_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static PRIVATE_VAR_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static ELAPSED_MS_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let tmp_dir_re =
        TMP_DIR_RE.get_or_init(|| Regex::new(r"\.tmp[a-zA-Z0-9]+").expect("valid tmp-dir regex"));
    let run_dir_re = RUN_DIR_RE.get_or_init(|| {
        Regex::new(r"/full-auto/\d+-[A-Za-z0-9._-]+").expect("valid run-dir regex")
    });
    let private_var_re = PRIVATE_VAR_RE
        .get_or_init(|| Regex::new(r"/private/var").expect("valid private var regex"));
    let elapsed_ms_re = ELAPSED_MS_RE
        .get_or_init(|| Regex::new(r#""elapsed_ms":\d+"#).expect("valid elapsed regex"));

    let normalized = tmp_dir_re.replace_all(text, ".tmpFIXTURE");
    let normalized = run_dir_re.replace_all(&normalized, "/full-auto/<run-id>");
    let normalized = private_var_re.replace_all(&normalized, "/var");
    elapsed_ms_re
        .replace_all(&normalized, r#""elapsed_ms":0"#)
        .into_owned()
}

fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "backtab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(number) => format!("f{number}"),
        other => format!("{other:?}"),
    }
}

fn describe_tui_event(event: &TuiEvent) -> String {
    match event {
        TuiEvent::Crossterm(_) => "crossterm".to_string(),
        TuiEvent::Chat(chat_event) => format!("chat::{chat_event:?}"),
        TuiEvent::TerminalFrame(frame) => format!(
            "terminal_frame(cwd={:?}, shell={:?}, title={:?})",
            frame.cwd, frame.shell_label, frame.window_title
        ),
        TuiEvent::TerminalClosed => "terminal_closed".to_string(),
        TuiEvent::BootstrapTick => "bootstrap_tick".to_string(),
        TuiEvent::RuntimeHealthTick => "runtime_health_tick".to_string(),
        TuiEvent::FileTreeListed(listing) => format!(
            "file_tree_listed(parent={}, ok={})",
            listing.parent.display(),
            listing.result.is_ok()
        ),
        TuiEvent::PathIndexSnapshot(snapshot) => format!(
            "path_index_snapshot(root={}, files_seen={})",
            snapshot.root.display(),
            snapshot.files_seen
        ),
        TuiEvent::BufferSnapshot(snapshot) => format!(
            "buffer_snapshot(path={:?}, lines={})",
            snapshot.path,
            snapshot.lines.len()
        ),
        TuiEvent::BackendResponse(response) => format!("backend_response::{response:?}"),
        TuiEvent::AgentRuntime(event) => format!("agent_runtime::{event:?}"),
        TuiEvent::StartAgentTask(task) => format!("start_agent_task(goal={})", task.goal),
        TuiEvent::RailEvent(event) => format!("rail_event::{event:?}"),
    }
}
