#![allow(unused)]
//! Standalone PTY runner for `<run_command>` XML blocks (Phase 3h fallback).
//!
//! **Integrated `quorp`:** [`crate::quorp::tui::chat::ChatPane`] sends commands through
//! [`crate::quorp::tui::command_bridge`] (`Project::create_terminal_task`, agent tool permissions).
//! This module runs only when `ChatPane` is built **without** `command_bridge_tx` (flow harnesses,
//! `ui_lab`).

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::quorp::tui::ChatUiEvent;

const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct CommandRunner {
    project_root: PathBuf,
}

#[derive(Debug)]
pub struct PendingCommand {
    pub command: String,
    pub timeout: Duration,
}

impl CommandRunner {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    pub fn set_project_root(&mut self, project_root: PathBuf) {
        self.project_root = project_root;
    }

    pub fn execute(
        &self,
        session_id: usize,
        command: &str,
        timeout: Duration,
        ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let command = command.to_string();
        let project_root = self.project_root.clone();

        tokio::spawn(async move {
            let result = run_in_pty(&command, &project_root, timeout);
            match result {
                Ok(output) => {
                    send(&ui_tx, ChatUiEvent::CommandOutput(session_id, output.clone()));
                    send(&ui_tx, ChatUiEvent::CommandFinished(session_id, output));
                }
                Err(err) => {
                    send(&ui_tx, ChatUiEvent::CommandOutput(session_id, format!("Error: {err}")));
                    send(&ui_tx, ChatUiEvent::CommandFinished(session_id, format!("Error: {err}")));
                }
            }
        })
    }

    pub fn parse_timeout(timeout_ms: Option<&str>) -> Duration {
        timeout_ms
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT)
    }
}

fn send(tx: &std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>, event: ChatUiEvent) {
    let _ = tx.try_send(crate::quorp::tui::TuiEvent::Chat(event));
}

fn run_in_pty(command: &str, working_dir: &PathBuf, timeout: Duration) -> anyhow::Result<String> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-c");
    cmd.arg(command);
    cmd.cwd(working_dir);

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    drop(pair.master);

    let output_handle = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    if buffer.len() > COMMAND_OUTPUT_LIMIT {
                        buffer.truncate(COMMAND_OUTPUT_LIMIT);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    });

    let deadline = std::time::Instant::now() + timeout;
    let exit_status = loop {
        if std::time::Instant::now() > deadline {
            child.kill()?;
            let output = output_handle.join().unwrap_or_default();
            return Ok(format!("{output}\n[Command timed out after {}s]", timeout.as_secs()));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.into()),
        }
    };

    let output = output_handle.join().unwrap_or_default();
    let code = exit_status.exit_code();
    Ok(format!("{output}\n[Exit code: {code}]"))
}
