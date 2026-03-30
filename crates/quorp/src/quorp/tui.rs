//! Four-pane TUI (ratatui + crossterm): file tree, code preview, integrated terminal, chat.

pub mod model_registry;
pub mod models_pane;
pub mod native_backend;
use std::io::stdout;
use std::ops::ControlFlow;
use std::panic;
use std::sync::Once;

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

pub mod app;
#[cfg(any(test, feature = "test-support"))]
pub mod buffer_png;
pub mod chat;
pub mod chat_service;
pub mod chrome;
pub mod editor_pane;
pub mod file_tree;
pub mod ssd_moe_client;
pub mod ssd_moe_tui;

pub mod agent_pane;
pub mod chrome_v2;
pub mod hitmap;
pub mod mention_links;
pub mod paint;
pub mod path_guard;
pub mod path_index;
pub mod terminal_pane;
pub mod theme;
pub mod tui_backend;
pub mod workbench;

pub mod command_bridge;

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
    FileTreeListed(file_tree::DirectoryListing),
    PathIndexSnapshot(path_index::PathIndexSnapshot),
    BufferSnapshot(editor_pane::BufferSnapshot),
    BackendResponse(bridge::BackendToTuiResponse),
}

/// Bounded queue capacity for [`std::sync::mpsc::sync_channel`] to the TUI thread.
pub const TUI_EVENT_QUEUE_CAPACITY: usize = 128;

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

pub fn run(
    workspace_root: std::path::PathBuf,
    event_rx: std::sync::mpsc::Receiver<TuiEvent>,
    crossterm_tx: std::sync::mpsc::SyncSender<TuiEvent>,
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
        let _command_thread = native_backend::spawn_command_service_loop(chat_tx.clone(), command_rx);
        Some(command_tx)
    } else {
        command_bridge_tx.clone()
    };

    let mut app = TuiApp::new_with_backend(
        workspace_root,
        chat_tx,
        handle,
        unified_language_model,
        path_index_display_root,
        native_command_tx,
        native_backend_tx,
    );
    let _runtime_guard = runtime;

    let _crossterm_reader = std::thread::spawn(move || {
        loop {
            match event::read() {
                Ok(ev) => {
                    if crossterm_tx.send(TuiEvent::Crossterm(ev)).is_err() {
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

    if let Ok(size) = terminal.size() {
        let full = Rect::new(0, 0, size.width, size.height);
        if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
            if let Err(e) = app.terminal.spawn_pty(cols, rows) {
                log::error!("tui: failed to init terminal pane: {e:#}");
            }
        }
    }

    loop {
        match event_rx.recv() {
            Ok(TuiEvent::Chat(ev)) => {
                app.chat.apply_chat_event(ev, &app.theme);
            }
            Ok(TuiEvent::TerminalFrame(frame)) => {
                app.terminal.apply_integrated_frame(frame);
            }
            Ok(TuiEvent::TerminalClosed) => {
                app.terminal.mark_integrated_session_closed();
            }
            Ok(TuiEvent::FileTreeListed(listing)) => {
                app.file_tree
                    .apply_project_listing(listing.parent, listing.result);
            }
            Ok(TuiEvent::BufferSnapshot(snapshot)) => {
                app.editor_pane
                    .apply_editor_pane_buffer_snapshot(
                        snapshot.path,
                        snapshot.lines,
                        snapshot.error,
                        snapshot.truncated,
                    );
            }
            Ok(TuiEvent::PathIndexSnapshot(snapshot)) => {
                app.chat
                    .apply_path_index_snapshot(snapshot.root, snapshot.entries, snapshot.files_seen);
            }
            Ok(TuiEvent::Crossterm(ev)) => {
                let should_sync_pty = matches!(ev, Event::Resize(_, _));
                if let ControlFlow::Break(()) = app.handle_event(ev) {
                    break;
                }
                if should_sync_pty {
                    if let Ok(size) = terminal.size() {
                        let full = Rect::new(0, 0, size.width, size.height);
                        if let Some((cols, rows)) = app.terminal_pane_content_size(full) {
                            let _ = app.terminal.sync_grid(cols, rows);
                        }
                    }
                }
            }
            Ok(TuiEvent::BackendResponse(bridge::BackendToTuiResponse::AgentStatusUpdate(
                update,
            ))) => {
                app.agent_pane.apply_status_update(update);
            }
            Err(_) => break,
        }
        terminal
            .draw(|frame| {
                app.draw(frame);
            })
            .context("draw")?;
    }
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
                    lines: vec![ratatui::text::Line::from(index.to_string())],
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
                        .lines
                        .first()
                        .map(|line| {
                            line.spans
                                .iter()
                                .map(|span| span.content.as_ref())
                                .collect::<String>()
                        })
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
