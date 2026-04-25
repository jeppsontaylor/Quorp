//! Four-pane TUI (ratatui + crossterm): file tree, code preview, integrated terminal, chat.

pub mod action_discovery;
pub mod agent_context;
pub mod local_model_program;
pub mod model_registry;
pub mod models_pane;
pub mod native_backend;
use std::io::stdout;
use std::ops::ControlFlow;
use std::panic;
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

pub mod agent_protocol;
pub mod agent_turn;
pub mod app;
pub mod assistant_transcript;
pub mod bootstrap_loader;
#[cfg(any(test, feature = "test-support"))]
pub mod buffer_png;
pub mod chat;
pub mod chat_service;
pub mod chrome;
pub mod editor_pane;
pub mod engage_target;
pub mod file_tree;
pub mod openai_compatible_client;
pub mod ssd_moe_client;
pub mod ssd_moe_tui;

pub mod agent_pane;
pub mod agent_runtime;
pub mod chrome_v2;
pub mod hitmap;
pub mod mcp_client;
pub mod mention_links;
pub mod paint;
pub mod path_guard;
pub mod path_index;
pub mod reason_ledger;
pub mod shell;
pub mod terminal_pane;
pub mod terminal_surface;
pub mod terminal_trace;
pub mod theme;
pub mod tui_backend;
pub mod workbench;
pub mod workspace_state;

pub mod attention_lease;
pub mod command_bridge;
pub mod diagnostics;
pub mod diff_reactor;
pub mod proof_rail;
pub mod rail_event;
pub mod slash_commands;
pub mod tool_orchestra;

pub mod bridge;

mod text_width;

use app::TuiApp;
use chat::ChatUiEvent;
use crossterm::event::Event;

/// Unified events for the main thread (crossterm input, chat, integrated terminal frames).
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub enum TuiEvent {
    Crossterm(Event),
    Chat(ChatUiEvent),
    TerminalFrame(bridge::TerminalFrame),
    TerminalClosed,
    BootstrapTick,
    RuntimeHealthTick,
    FileTreeListed(file_tree::DirectoryListing),
    PathIndexSnapshot(path_index::PathIndexSnapshot),
    BufferSnapshot(editor_pane::BufferSnapshot),
    BackendResponse(bridge::BackendToTuiResponse),
    AgentRuntime(agent_runtime::AgentUiEvent),
    StartAgentTask(agent_runtime::AgentTaskRequest),
    RailEvent(rail_event::RailEvent),
}

/// Bounded queue capacity for [`std::sync::mpsc::sync_channel`] to the TUI thread.
pub const TUI_EVENT_QUEUE_CAPACITY: usize = 128;
const INPUT_IDLE_WAIT: Duration = Duration::from_millis(8);
const RENDER_FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug)]
struct UiScheduler {
    dirty: bool,
    immediate: bool,
    last_draw_at: Instant,
}

impl UiScheduler {
    fn new() -> Self {
        Self {
            dirty: true,
            immediate: true,
            last_draw_at: Instant::now(),
        }
    }

    fn request_draw(&mut self) {
        self.dirty = true;
    }

    fn request_immediate_draw(&mut self) {
        self.dirty = true;
        self.immediate = true;
    }

    fn should_draw(&self, now: Instant) -> bool {
        self.dirty
            && (self.immediate || now.duration_since(self.last_draw_at) >= RENDER_FRAME_INTERVAL)
    }

    fn mark_drawn(&mut self, now: Instant) {
        self.dirty = false;
        self.immediate = false;
        self.last_draw_at = now;
    }
}

#[derive(Debug, Default)]
struct PendingBackendEvents {
    deferred_input_events: Vec<Event>,
    chat_events: Vec<ChatUiEvent>,
    latest_terminal_frame: Option<bridge::TerminalFrame>,
    terminal_closed: bool,
    bootstrap_tick_count: usize,
    runtime_health_tick: bool,
    file_tree_listings: Vec<file_tree::DirectoryListing>,
    latest_path_index_snapshot: Option<path_index::PathIndexSnapshot>,
    latest_buffer_snapshot: Option<editor_pane::BufferSnapshot>,
    backend_updates: Vec<bridge::BackendToTuiResponse>,
    agent_runtime_events: Vec<agent_runtime::AgentUiEvent>,
    start_agent_tasks: Vec<agent_runtime::AgentTaskRequest>,
    rail_events: Vec<rail_event::RailEvent>,
}

impl PendingBackendEvents {
    fn push(&mut self, event: TuiEvent) {
        match event {
            TuiEvent::Crossterm(event) => self.deferred_input_events.push(event),
            TuiEvent::Chat(event) => self.chat_events.push(event),
            TuiEvent::TerminalFrame(frame) => {
                self.latest_terminal_frame = Some(frame);
            }
            TuiEvent::TerminalClosed => {
                self.terminal_closed = true;
            }
            TuiEvent::BootstrapTick => {
                self.bootstrap_tick_count = self.bootstrap_tick_count.saturating_add(1);
            }
            TuiEvent::RuntimeHealthTick => {
                self.runtime_health_tick = true;
            }
            TuiEvent::FileTreeListed(listing) => self.file_tree_listings.push(listing),
            TuiEvent::PathIndexSnapshot(snapshot) => {
                self.latest_path_index_snapshot = Some(snapshot);
            }
            TuiEvent::BufferSnapshot(snapshot) => {
                self.latest_buffer_snapshot = Some(snapshot);
            }
            TuiEvent::BackendResponse(response) => self.backend_updates.push(response),
            TuiEvent::AgentRuntime(event) => self.agent_runtime_events.push(event),
            TuiEvent::StartAgentTask(task) => self.start_agent_tasks.push(task),
            TuiEvent::RailEvent(event) => self.rail_events.push(event),
        }
    }

    fn is_empty(&self) -> bool {
        self.chat_events.is_empty()
            && self.deferred_input_events.is_empty()
            && self.latest_terminal_frame.is_none()
            && !self.terminal_closed
            && self.bootstrap_tick_count == 0
            && !self.runtime_health_tick
            && self.file_tree_listings.is_empty()
            && self.latest_path_index_snapshot.is_none()
            && self.latest_buffer_snapshot.is_none()
            && self.backend_updates.is_empty()
            && self.agent_runtime_events.is_empty()
            && self.start_agent_tasks.is_empty()
            && self.rail_events.is_empty()
    }
}

fn restore_terminal() {
    let mut stdout = stdout();
    if let Err(e) = execute!(stdout, Show) {
        eprintln!("quorp: restore terminal (show cursor): {e}");
    }
    if let Err(e) = execute!(stdout, DisableMouseCapture) {
        eprintln!("quorp: restore terminal (disable mouse capture): {e}");
    }
    if let Err(e) = execute!(stdout, LeaveAlternateScreen) {
        eprintln!("quorp: restore terminal (leave alternate screen): {e}");
    }
    if let Err(e) = disable_raw_mode() {
        eprintln!("quorp: restore terminal (disable raw mode): {e}");
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

static INSTALL_PANIC_HOOK: Once = Once::new();

fn install_panic_hook() {
    INSTALL_PANIC_HOOK.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            restore_terminal();
            default_hook(panic_info);
        }));
    });
}

fn init_terminal() -> Result<TerminalGuard> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        anyhow::bail!("TUI requires a real terminal (not a pipe). Run in a terminal emulator.");
    }
    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, Hide, EnableMouseCapture)
        .context("enter alternate screen")?;
    Ok(TerminalGuard)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    workspace_root: std::path::PathBuf,
    event_rx: std::sync::mpsc::Receiver<TuiEvent>,
    input_rx: std::sync::mpsc::Receiver<Event>,
    input_tx: std::sync::mpsc::Sender<Event>,
    chat_tx: std::sync::mpsc::SyncSender<TuiEvent>,

    unified_language_model: Option<(
        futures::channel::mpsc::UnboundedSender<bridge::TuiToBackendRequest>,
        Vec<String>,
        usize,
    )>,

    path_index_display_root: Option<std::sync::Arc<std::sync::RwLock<std::path::PathBuf>>>,
    command_bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<command_bridge::CommandBridgeRequest>,
    >,
    unified_bridge_tx: Option<futures::channel::mpsc::UnboundedSender<bridge::TuiToBackendRequest>>,
) -> Result<()> {
    install_panic_hook();
    let _guard = init_terminal()?;

    let stdout = stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;

    let runtime = tokio::runtime::Runtime::new().context("tokio runtime")?;
    let handle = runtime.handle().clone();

    let native_backend_tx = if unified_bridge_tx.is_none() {
        let (native_tx, native_rx) = futures::channel::mpsc::unbounded();
        let _backend_thread = native_backend::spawn_native_backend_loop(
            workspace_root.clone(),
            chat_tx.clone(),
            native_rx,
        );
        Some(native_tx)
    } else {
        unified_bridge_tx.clone()
    };

    let native_command_tx = if command_bridge_tx.is_none() {
        let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
        let _command_thread =
            native_backend::spawn_command_service_loop(chat_tx.clone(), command_rx);
        Some(command_tx)
    } else {
        command_bridge_tx.clone()
    };

    let mut app = TuiApp::new_with_backend(
        workspace_root,
        chat_tx.clone(),
        handle,
        unified_language_model,
        path_index_display_root,
        native_command_tx,
        native_backend_tx,
    );
    let _runtime_guard = runtime;

    crate::quorp::tui::bootstrap_loader::BootstrapLoader::warm_assets_async();

    let _crossterm_reader = std::thread::spawn(move || {
        loop {
            match event::read() {
                Ok(ev) => {
                    if input_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("tui: crossterm::event::read error: {e}");
                    break;
                }
            }
        }
    });

    terminal
        .draw(|frame| {
            app.draw(frame);
        })
        .context("initial draw")?;
    let mut scheduler = UiScheduler::new();
    scheduler.mark_drawn(Instant::now());

    let bootstrap_tx = chat_tx.clone();
    let bootstrap_done = Arc::new(AtomicBool::new(false));
    let bootstrap_done_thread = Arc::clone(&bootstrap_done);
    let _bootstrap_tick_thread = std::thread::spawn(move || {
        while !bootstrap_done_thread.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(120));
            if bootstrap_done_thread.load(Ordering::Relaxed) {
                break;
            }
            if bootstrap_tx.send(TuiEvent::BootstrapTick).is_err() {
                break;
            }
        }
    });

    let runtime_tick_tx = chat_tx.clone();
    let runtime_done_thread = Arc::clone(&bootstrap_done);
    let _runtime_health_thread = std::thread::spawn(move || {
        while !runtime_done_thread.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(400));
            if runtime_done_thread.load(Ordering::Relaxed) {
                break;
            }
            if runtime_tick_tx.send(TuiEvent::RuntimeHealthTick).is_err() {
                break;
            }
        }
    });

    if let Ok(size) = terminal.size() {
        let full = Rect::new(0, 0, size.width, size.height);
        if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
            let pty_result = app.terminal.spawn_pty(cols, rows);
            app.mark_bootstrap_pty_result(
                pty_result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| anyhow::anyhow!(error.to_string())),
            );
            if let Err(e) = pty_result {
                log::error!("tui: failed to init terminal pane: {e:#}");
            }
        }
    }

    loop {
        let mut should_exit = false;
        let mut processed_input = false;
        let mut pending = PendingBackendEvents::default();

        match input_rx.recv_timeout(INPUT_IDLE_WAIT) {
            Ok(event) => {
                let should_sync_pty = matches!(event, Event::Resize(_, _));
                if let ControlFlow::Break(()) = app.handle_event(event) {
                    should_exit = true;
                } else {
                    scheduler.request_immediate_draw();
                    processed_input = true;
                    if should_sync_pty && let Ok(size) = terminal.size() {
                        let full = Rect::new(0, 0, size.width, size.height);
                        if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
                            let _ = app.terminal.sync_grid(cols, rows);
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        while !should_exit {
            match input_rx.try_recv() {
                Ok(event) => {
                    let should_sync_pty = matches!(event, Event::Resize(_, _));
                    if let ControlFlow::Break(()) = app.handle_event(event) {
                        should_exit = true;
                        break;
                    }
                    processed_input = true;
                    scheduler.request_immediate_draw();
                    if should_sync_pty && let Ok(size) = terminal.size() {
                        let full = Rect::new(0, 0, size.width, size.height);
                        if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
                            let _ = app.terminal.sync_grid(cols, rows);
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    should_exit = true;
                    break;
                }
            }
        }
        if should_exit {
            break;
        }

        while let Ok(event) = event_rx.try_recv() {
            pending.push(event);
        }
        if !processed_input && pending.is_empty() {
            match event_rx.recv_timeout(Duration::from_millis(1)) {
                Ok(event) => pending.push(event),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        for event in pending.deferred_input_events {
            let should_sync_pty = matches!(event, Event::Resize(_, _));
            if let ControlFlow::Break(()) = app.handle_event(event) {
                should_exit = true;
                break;
            }
            scheduler.request_immediate_draw();
            if should_sync_pty && let Ok(size) = terminal.size() {
                let full = Rect::new(0, 0, size.width, size.height);
                if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
                    let _ = app.terminal.sync_grid(cols, rows);
                }
            }
        }
        if should_exit {
            break;
        }

        for event in pending.chat_events {
            app.handle_chat_ui_event(event);
            scheduler.request_draw();
        }
        if let Some(frame) = pending.latest_terminal_frame {
            app.terminal.apply_integrated_frame(frame);
            scheduler.request_draw();
        }
        if pending.terminal_closed {
            app.terminal.mark_integrated_session_closed();
            scheduler.request_draw();
        }
        if pending.bootstrap_tick_count > 0 {
            for _ in 0..pending.bootstrap_tick_count {
                app.advance_bootstrap_tick();
            }
            scheduler.request_draw();
        }
        if pending.runtime_health_tick {
            app.poll_runtime_health();
            scheduler.request_draw();
        }
        for listing in pending.file_tree_listings {
            app.file_tree
                .apply_project_listing(listing.parent, listing.result);
            scheduler.request_draw();
        }
        if let Some(snapshot) = pending.latest_buffer_snapshot {
            app.editor_pane.apply_editor_pane_buffer_snapshot(
                snapshot.path,
                snapshot.lines,
                snapshot.error,
                snapshot.truncated,
            );
            scheduler.request_draw();
        }
        if let Some(snapshot) = pending.latest_path_index_snapshot {
            app.chat.apply_path_index_snapshot(
                snapshot.root,
                snapshot.entries,
                snapshot.files_seen,
            );
            scheduler.request_draw();
        }
        for response in pending.backend_updates {
            app.handle_backend_response(response);
            scheduler.request_draw();
        }
        for event in pending.agent_runtime_events {
            app.agent_pane.apply_event(event);
            scheduler.request_draw();
        }
        for task in pending.start_agent_tasks {
            if let Some(tx) = &app.agent_runtime_tx {
                let _ = tx.unbounded_send(agent_runtime::AgentRuntimeCommand::StartTask(task));
                app.focused = app::Pane::Agent;
                app.agent_pane
                    .apply_status_update("Agent loop started.".to_string());
            }
            scheduler.request_draw();
        }
        for event in pending.rail_events {
            app.proof_rail.apply_event(&event);
            scheduler.request_draw();
        }

        if app.bootstrap_completed() {
            bootstrap_done.store(true, Ordering::Relaxed);
        }
        if scheduler.should_draw(Instant::now()) {
            app.persist_workspace_state();
            terminal
                .draw(|frame| {
                    app.draw(frame);
                })
                .context("draw")?;
            scheduler.mark_drawn(Instant::now());
        }
    }
    bootstrap_done.store(true, Ordering::Relaxed);
    app.persist_workspace_state();
    app.ssd_moe.stop();

    Ok(())
}

#[cfg(test)]
mod tui_flow_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn tui_event_channel_preserves_order() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(16);
        let join = std::thread::spawn(move || {
            for index in 0..1000 {
                let ev = ChatUiEvent::AssistantDelta(0, format!("d{index}"));
                tx.send(TuiEvent::Chat(ev)).expect("send");
            }
        });
        for index in 0..1000 {
            match rx.recv().expect("recv") {
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, s)) => {
                    assert_eq!(s, format!("d{index}"));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        join.join().expect("join");
    }

    #[test]
    fn crossterm_key_events_preserve_order() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(16);
        let join = std::thread::spawn(move || {
            for index in 0..1000 {
                let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
                tx.send(TuiEvent::Crossterm(Event::Key(key)))
                    .unwrap_or_else(|_| panic!("send failed at {index}"));
            }
        });
        for _ in 0..1000 {
            match rx.recv().expect("recv") {
                TuiEvent::Crossterm(Event::Key(_)) => {}
                other => panic!("unexpected event: {other:?}"),
            }
        }
        join.join().expect("join");
    }

    #[test]
    fn sync_channel_backpressure_blocks_second_send_until_recv() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(1);
        let join = std::thread::spawn(move || {
            tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(0)))
                .expect("first send");
            tx.send(TuiEvent::Chat(ChatUiEvent::Error(0, "second".to_string())))
                .expect("second send unblocks after recv drains queue");
        });
        match rx.recv() {
            Ok(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {}
            other => panic!("unexpected: {other:?}"),
        }
        match rx.recv() {
            Ok(TuiEvent::Chat(ChatUiEvent::Error(0, s))) => assert_eq!(s, "second"),
            other => panic!("unexpected: {other:?}"),
        }
        join.join().expect("join");
    }

    #[test]
    fn chat_and_crossterm_events_preserve_interleaved_order() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(32);
        let join = std::thread::spawn(move || {
            for index in 0..100 {
                tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
                    0,
                    format!("c{index}"),
                )))
                .expect("chat");
                let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
                tx.send(TuiEvent::Crossterm(Event::Key(key)))
                    .expect("crossterm");
            }
        });
        for index in 0..100 {
            match rx.recv().expect("recv") {
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, s)) => {
                    assert_eq!(s, format!("c{index}"));
                }
                other => panic!("expected chat at {index}: {other:?}"),
            }
            match rx.recv().expect("recv") {
                TuiEvent::Crossterm(Event::Key(_)) => {}
                other => panic!("expected key at {index}: {other:?}"),
            }
        }
        join.join().expect("join");
    }

    #[test]
    fn chat_crossterm_and_terminal_frame_events_preserve_interleaved_order() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(32);
        let join = std::thread::spawn(move || {
            for index in 0..50 {
                tx.send(TuiEvent::TerminalFrame(bridge::TerminalFrame {
                    snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::from_lines(&[
                        ratatui::text::Line::from(index.to_string()),
                    ]),
                    cwd: None,
                    shell_label: None,
                    window_title: None,
                }))
                .expect("terminal frame");
                tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
                    0,
                    format!("c{index}"),
                )))
                .expect("chat");
                let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
                tx.send(TuiEvent::Crossterm(Event::Key(key)))
                    .expect("crossterm");
            }
        });
        for index in 0..50 {
            match rx.recv().expect("recv") {
                TuiEvent::TerminalFrame(frame) => {
                    let text = frame
                        .snapshot
                        .row_strings(1)
                        .into_iter()
                        .next()
                        .unwrap_or_default();
                    assert_eq!(text, index.to_string(), "frame at {index}");
                }
                other => panic!("expected terminal frame at {index}: {other:?}"),
            }
            match rx.recv().expect("recv") {
                TuiEvent::Chat(ChatUiEvent::AssistantDelta(0, s)) => {
                    assert_eq!(s, format!("c{index}"));
                }
                other => panic!("expected chat at {index}: {other:?}"),
            }
            match rx.recv().expect("recv") {
                TuiEvent::Crossterm(Event::Key(_)) => {}
                other => panic!("expected key at {index}: {other:?}"),
            }
        }
        join.join().expect("join");
    }

    #[test]
    fn pending_backend_events_keep_latest_terminal_frame() {
        let mut pending = PendingBackendEvents::default();
        pending.push(TuiEvent::TerminalFrame(bridge::TerminalFrame {
            snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::from_lines(&[
                ratatui::text::Line::from("old"),
            ]),
            cwd: None,
            shell_label: None,
            window_title: None,
        }));
        pending.push(TuiEvent::TerminalFrame(bridge::TerminalFrame {
            snapshot: crate::quorp::tui::terminal_surface::TerminalSnapshot::from_lines(&[
                ratatui::text::Line::from("new"),
            ]),
            cwd: None,
            shell_label: None,
            window_title: None,
        }));

        let latest = pending.latest_terminal_frame.expect("latest frame");
        let text = latest
            .snapshot
            .row_strings(1)
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(text, "new");
    }

    #[test]
    fn scheduler_defers_non_immediate_redraw_until_budget() {
        let now = Instant::now();
        let mut scheduler = UiScheduler {
            dirty: false,
            immediate: false,
            last_draw_at: now,
        };
        scheduler.request_draw();
        assert!(!scheduler.should_draw(now + Duration::from_millis(5)));
        assert!(scheduler.should_draw(now + RENDER_FRAME_INTERVAL));
    }

    #[test]
    fn scheduler_draws_immediately_for_input() {
        let now = Instant::now();
        let mut scheduler = UiScheduler {
            dirty: false,
            immediate: false,
            last_draw_at: now,
        };
        scheduler.request_immediate_draw();
        assert!(scheduler.should_draw(now));
    }

    #[test]
    fn sync_channel_recv_blocks_until_event() {
        let (tx, rx) = mpsc::sync_channel::<TuiEvent>(1);
        let tx_clone = tx.clone();
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            tx_clone
                .send(TuiEvent::Chat(ChatUiEvent::StreamFinished(0)))
                .expect("send");
        });
        drop(tx);
        let start = Instant::now();
        match rx.recv_timeout(Duration::from_millis(30)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("unexpected recv before timeout");
            }
        }
        assert!(start.elapsed() >= Duration::from_millis(25));
        match rx.recv() {
            Ok(TuiEvent::Chat(ChatUiEvent::StreamFinished(0))) => {}
            other => panic!("unexpected: {other:?}"),
        }
        join.join().expect("join");
    }
}
