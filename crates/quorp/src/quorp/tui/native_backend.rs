use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::text::Line;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::bridge::{
    BackendToTuiResponse, TerminalFrame, TuiKeystroke, TuiToBackendRequest,
};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::editor_pane::buffer_snapshot_from_disk;
use crate::quorp::tui::file_tree::{DirectoryListing, read_children};

const TERMINAL_MAX_LINES: usize = 500;
const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;

pub fn spawn_native_backend_loop(
    workspace_root: PathBuf,
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    mut request_rx: futures::channel::mpsc::UnboundedReceiver<TuiToBackendRequest>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut terminal = NativeTerminalService::new(event_tx.clone());
        futures::executor::block_on(async move {
            while let Some(request) = request_rx.next().await {
                match request {
                    TuiToBackendRequest::ListDirectory(path) => {
                        let listing = DirectoryListing {
                            parent: path.clone(),
                            result: read_children(&path, &workspace_root),
                        };
                        if let Err(error) = event_tx.send(TuiEvent::FileTreeListed(listing)) {
                            log::error!("tui: file-tree event channel closed: {error}");
                            break;
                        }
                    }
                    TuiToBackendRequest::OpenBuffer(path) => {
                        let snapshot = buffer_snapshot_from_disk(Some(path), &workspace_root);
                        if let Err(error) = event_tx.send(TuiEvent::BufferSnapshot(snapshot)) {
                            log::error!("tui: buffer snapshot channel closed: {error}");
                            break;
                        }
                    }
                    TuiToBackendRequest::CloseBuffer => {
                        let snapshot = buffer_snapshot_from_disk(None, &workspace_root);
                        if let Err(error) = event_tx.send(TuiEvent::BufferSnapshot(snapshot)) {
                            log::error!("tui: close-buffer channel closed: {error}");
                            break;
                        }
                    }
                    TuiToBackendRequest::TerminalResize { cols, rows } => {
                        if let Err(error) = terminal.ensure_session(cols, rows) {
                            log::error!("tui: terminal resize/spawn failed: {error:#}");
                        }
                    }
                    TuiToBackendRequest::TerminalInput(bytes) => {
                        if let Err(error) = terminal.write_bytes(&bytes) {
                            log::error!("tui: terminal input failed: {error:#}");
                        }
                    }
                    TuiToBackendRequest::TerminalKeystroke(keystroke) => {
                        if let Err(error) = terminal.write_keystroke(&keystroke) {
                            log::error!("tui: terminal keystroke failed: {error:#}");
                        }
                    }
                    TuiToBackendRequest::TerminalScrollPageUp
                    | TuiToBackendRequest::TerminalScrollPageDown => {}
                    TuiToBackendRequest::StartAgentAction(action) => {
                        let response = BackendToTuiResponse::AgentStatusUpdate(format!(
                            "Agent request queued: {action}"
                        ));
                        if let Err(error) = event_tx.send(TuiEvent::BackendResponse(response)) {
                            log::error!("tui: agent status channel closed: {error}");
                            break;
                        }
                    }
                }
            }
        });
    })
}

pub fn spawn_command_service_loop(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    mut request_rx: futures::channel::mpsc::UnboundedReceiver<CommandBridgeRequest>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        futures::executor::block_on(async move {
            while let Some(request) = request_rx.next().await {
                match request {
                    CommandBridgeRequest::Run {
                        session_id,
                        command,
                        cwd,
                        timeout,
                    } => {
                        spawn_command_task(event_tx.clone(), session_id, command, cwd, timeout);
                    }
                }
            }
        });
    })
}

fn spawn_command_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    command: String,
    cwd: PathBuf,
    timeout: Duration,
) {
    std::thread::spawn(move || {
        let result = run_command_streaming(&event_tx, session_id, &command, &cwd, timeout);
        if let Err(error) = result {
            let message = format!("Command failed: {error:#}");
            let _ = event_tx.send(TuiEvent::Chat(crate::quorp::tui::chat::ChatUiEvent::Error(
                session_id,
                message.clone(),
            )));
            let _ = event_tx.send(TuiEvent::Chat(
                crate::quorp::tui::chat::ChatUiEvent::CommandFinished(session_id, message),
            ));
        }
    });
}

fn run_command_streaming(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut builder = CommandBuilder::new(shell);
    builder.arg("-lc");
    builder.arg(command);
    builder.cwd(cwd);

    let mut child = pair.slave.spawn_command(builder)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    drop(pair.master);

    let output = Arc::new(Mutex::new(String::new()));
    let output_for_reader = Arc::clone(&output);
    let event_tx_for_reader = event_tx.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read_len) => {
                    let text = String::from_utf8_lossy(&chunk[..read_len]).to_string();
                    if let Ok(mut full_output) = output_for_reader.lock() {
                        if full_output.len() < COMMAND_OUTPUT_LIMIT {
                            full_output.push_str(&text);
                            if full_output.len() > COMMAND_OUTPUT_LIMIT {
                                full_output.truncate(COMMAND_OUTPUT_LIMIT);
                            }
                        }
                    }
                    let _ = event_tx_for_reader.send(TuiEvent::Chat(
                        crate::quorp::tui::chat::ChatUiEvent::CommandOutput(session_id, text),
                    ));
                }
                Err(_) => break,
            }
        }
    });

    let deadline = std::time::Instant::now() + timeout;
    let exit_code = loop {
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            break Some(-1);
        }
        match child.try_wait()? {
            Some(status) => break Some(status.exit_code() as i32),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let _ = reader_thread.join();

    let mut final_output = output
        .lock()
        .map(|captured| captured.clone())
        .unwrap_or_default();
    if exit_code == Some(-1) {
        final_output.push_str("\n[Command timed out]");
    } else {
        final_output.push_str(&format!(
            "\n[Exit code: {}]",
            exit_code.unwrap_or_default()
        ));
    }
    let _ = event_tx.send(TuiEvent::Chat(
        crate::quorp::tui::chat::ChatUiEvent::CommandFinished(session_id, final_output),
    ));
    Ok(())
}

struct NativeTerminalService {
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session: Option<TerminalSession>,
}

impl NativeTerminalService {
    fn new(event_tx: std::sync::mpsc::SyncSender<TuiEvent>) -> Self {
        Self {
            event_tx,
            session: None,
        }
    }

    fn ensure_session(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(session) = self.session.as_mut() {
            session.resize(cols, rows)?;
            return Ok(());
        }
        self.session = Some(TerminalSession::spawn(self.event_tx.clone(), cols, rows)?);
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session.write(bytes)
    }

    fn write_keystroke(&mut self, keystroke: &TuiKeystroke) -> anyhow::Result<()> {
        let bytes = keystroke_to_bytes(keystroke);
        self.write_bytes(&bytes)
    }
}

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    _reader_thread: std::thread::JoinHandle<()>,
}

impl TerminalSession {
    fn spawn(
        event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = pair.master;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut builder = CommandBuilder::new(shell);
        builder.arg("-i");
        let child = pair.slave.spawn_command(builder)?;
        drop(pair.slave);

        let reader_thread = std::thread::spawn(move || {
            let _child = child;
            let mut chunk = [0u8; 4096];
            let mut lines = VecDeque::<String>::new();
            let mut tail = String::new();
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        let _ = event_tx.send(TuiEvent::TerminalClosed);
                        break;
                    }
                    Ok(read_len) => {
                        tail.push_str(&String::from_utf8_lossy(&chunk[..read_len]));
                        while let Some(pos) = tail.find('\n') {
                            let line = tail.drain(..=pos).collect::<String>();
                            lines.push_back(line.trim_end_matches(&['\r', '\n'][..]).to_string());
                            while lines.len() > TERMINAL_MAX_LINES {
                                lines.pop_front();
                            }
                        }
                        if !tail.is_empty() {
                            let preview = tail.trim_end_matches('\r').to_string();
                            let mut snapshot = lines.clone();
                            if !preview.is_empty() {
                                snapshot.push_back(preview);
                            }
                            let frame = TerminalFrame {
                                lines: snapshot.into_iter().map(Line::from).collect(),
                            };
                            let _ = event_tx.send(TuiEvent::TerminalFrame(frame));
                        } else {
                            let frame = TerminalFrame {
                                lines: lines.iter().cloned().map(Line::from).collect(),
                            };
                            let _ = event_tx.send(TuiEvent::TerminalFrame(frame));
                        }
                    }
                    Err(_) => {
                        let _ = event_tx.send(TuiEvent::TerminalClosed);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master,
            writer,
            _reader_thread: reader_thread,
        })
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }
}

fn keystroke_to_bytes(keystroke: &TuiKeystroke) -> Vec<u8> {
    match keystroke.key.as_str() {
        "enter" => vec![b'\r'],
        "tab" => {
            if keystroke.modifiers.shift {
                vec![0x1b, b'[', b'Z']
            } else {
                vec![b'\t']
            }
        }
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "up" => vec![0x1b, b'[', b'A'],
        "down" => vec![0x1b, b'[', b'B'],
        "right" => vec![0x1b, b'[', b'C'],
        "left" => vec![0x1b, b'[', b'D'],
        "home" => vec![0x1b, b'[', b'H'],
        "end" => vec![0x1b, b'[', b'F'],
        "pageup" => vec![0x1b, b'[', b'5', b'~'],
        "pagedown" => vec![0x1b, b'[', b'6', b'~'],
        "delete" => vec![0x1b, b'[', b'3', b'~'],
        "insert" => vec![0x1b, b'[', b'2', b'~'],
        "space" => vec![b' '],
        key if key.len() == 1 => {
            let byte = key.as_bytes()[0];
            if keystroke.modifiers.control && byte.is_ascii_alphabetic() {
                vec![byte.to_ascii_lowercase() - b'a' + 1]
            } else if keystroke.modifiers.alt {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        }
        function if function.starts_with('f') => Vec::new(),
        _ => Vec::new(),
    }
}
