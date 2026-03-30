use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;

use crate::quorp::tui::tui_backend::TuiBackend;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TuiKeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TuiKeystroke {
    pub modifiers: TuiKeyModifiers,
    pub key: String,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub enum TuiToBackendRequest {
    ListDirectory(PathBuf),
    OpenBuffer(PathBuf),
    CloseBuffer,
    TerminalKeystroke(TuiKeystroke),
    TerminalInput(Vec<u8>),
    TerminalResize {
        cols: u16,
        rows: u16,
    },
    TerminalScrollPageUp,
    TerminalScrollPageDown,
    StartAgentAction(String),
}

pub struct UnifiedBridgeTuiBackend {
    request_tx: futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>,
}

impl UnifiedBridgeTuiBackend {
    pub fn new(request_tx: futures::channel::mpsc::UnboundedSender<TuiToBackendRequest>) -> Self {
        Self { request_tx }
    }
}

impl TuiBackend for UnifiedBridgeTuiBackend {
    fn request_list_directory(&self, path: PathBuf) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::ListDirectory(path))
            .map_err(|error| error.to_string())
    }

    fn request_open_buffer(&self, path: PathBuf) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::OpenBuffer(path))
            .map_err(|error| error.to_string())
    }

    fn request_close_buffer(&self) -> Result<(), String> {
        self.request_tx
            .unbounded_send(TuiToBackendRequest::CloseBuffer)
            .map_err(|error| error.to_string())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub enum BackendToTuiResponse {
    AgentStatusUpdate(String),
}

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub lines: Vec<Line<'static>>,
}

pub fn crossterm_key_event_to_keystroke(key: &KeyEvent) -> Option<TuiKeystroke> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let mut modifiers = TuiKeyModifiers::default();
    modifiers.control = key.modifiers.contains(KeyModifiers::CONTROL);
    modifiers.alt = key.modifiers.contains(KeyModifiers::ALT);
    modifiers.shift = key.modifiers.contains(KeyModifiers::SHIFT);

    let key_text = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(character) => {
            if character.is_ascii_uppercase() {
                modifiers.shift = true;
            }
            character.to_ascii_lowercase().to_string()
        }
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => {
            modifiers.shift = true;
            "tab".to_string()
        }
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
        _ => return None,
    };

    Some(TuiKeystroke {
        modifiers,
        key: key_text,
    })
}
