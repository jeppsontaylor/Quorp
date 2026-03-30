#![allow(unused)]
//! Chat pane: transcript, composer, and model row.
//!
//! The TUI-only build keeps chat state, transcript rendering, and tool output in this module.
//! Native chat and command services drive prompt submission and tool execution for the pane.
//!
//! Pane focus uses **Tab** / **Shift+Tab** for cycling panes (same as the rest of the TUI). Model
//! selection uses **`[`** and **`]`** while the Chat pane is focused (see the model row hint).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::quorp::tui::chat_service::{
    ChatServiceMessage, ChatServiceRequest, ChatServiceRole,
};
use crate::quorp::tui::mention_links::{expand_mentions_for_api_message, mention_link_for_path};
use crate::quorp::tui::path_index::{PathEntry, PathIndex, PathIndexProgress};

const MAX_MESSAGE_CHARS: usize = 512 * 1024;
const MAX_MESSAGES: usize = 500;

#[derive(Debug, Clone)]
pub enum ChatUiEvent {
    AssistantDelta(usize, String),
    StreamFinished(usize),
    Error(usize, String),
    CommandOutput(usize, String),
    CommandFinished(usize, String),
}

#[derive(Debug, Clone)]
pub enum ChatMessage {
    User(String),
    Assistant(String),
}

#[derive(Debug, Clone)]
pub struct PendingCommand {
    pub command: String,
    pub timeout: Duration,
}

impl ChatMessage {
    fn push_assistant(&mut self, delta: &str) {
        let ChatMessage::Assistant(s) = self else {
            return;
        };
        let remaining = MAX_MESSAGE_CHARS.saturating_sub(s.len());
        if delta.len() <= remaining {
            s.push_str(delta);
            return;
        }
        if remaining > 0 {
            let take = delta.floor_char_boundary(remaining);
            s.push_str(&delta[..take]);
        }
        s.push_str("\n… [truncated]");
    }
}

#[derive(Debug)]
struct MentionPopup {
    at_byte: usize,
    selected: usize,
    /// Index of the first visible row in `matches` (for lists taller than the viewport).
    scroll_top: usize,
    matches: Vec<PathEntry>,
    last_query: String,
}

pub struct ChatSession {
    pub title: String,
    pub messages: Vec<ChatMessage>,
    pub transcript_scroll: usize,
    pub stick_to_bottom: bool,
    pub input: String,
    pub cursor_char: usize,
    pub last_error: Option<String>,
    pub pending_command: Option<PendingCommand>,
    pub running_command: bool,
    pub running_command_name: Option<String>,
    pub command_output_lines: Vec<String>,
    mention_popup: Option<MentionPopup>,
    pub streaming: bool,
}

impl ChatSession {
    pub(crate) fn new(title: String) -> Self {
        Self {
            title,
            messages: Vec::new(),
            transcript_scroll: 0,
            stick_to_bottom: true,
            input: String::new(),
            cursor_char: 0,
            last_error: None,
            pending_command: None,
            running_command: false,
            running_command_name: None,
            command_output_lines: Vec::new(),
            mention_popup: None,
            streaming: false,
        }
    }
}

pub struct ChatPane {
    ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    sessions: Vec<ChatSession>,
    active_session: usize,
    models: Vec<String>,
    model_index: usize,
    viewport_transcript_lines: usize,
    last_text_width: usize,
    chat_service_tx: futures::channel::mpsc::UnboundedSender<ChatServiceRequest>,
    command_bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<crate::quorp::tui::command_bridge::CommandBridgeRequest>,
    >,
    project_root: PathBuf,
    path_index: Arc<PathIndex>,
    #[cfg(test)]
    base_url_override: Option<String>,
}

fn char_index_for_byte(input: &str, byte: usize) -> usize {
    let byte = byte.min(input.len());
    input[..byte].chars().count()
}

fn char_byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(input.len())
}

const MENTION_POPUP_MAX_VISIBLE: usize = 8;

fn clamp_mention_scroll(scroll_top: &mut usize, selected: usize, visible_rows: usize, total: usize) {
    if total == 0 {
        *scroll_top = 0;
        return;
    }
    let v = visible_rows.min(total).max(1);
    if selected < *scroll_top {
        *scroll_top = selected;
    }
    if selected >= *scroll_top + v {
        *scroll_top = selected + 1 - v;
    }
    let max_top = total.saturating_sub(v);
    *scroll_top = (*scroll_top).min(max_top);
}

fn active_mention_token(input: &str, cursor_byte: usize) -> Option<(usize, String)> {
    let before = input.get(..cursor_byte)?;
    let at_byte = before.rfind('@')?;
    if at_byte > 0 {
        let prev = input[..at_byte].chars().next_back()?;
        if prev.is_alphanumeric() || prev == '_' {
            return None;
        }
    }
    let after_at_start = at_byte + '@'.len_utf8();
    let after_at = input.get(after_at_start..cursor_byte)?;
    if after_at.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((at_byte, after_at.to_string()))
}

fn parse_command_timeout(timeout_ms: Option<&str>) -> Duration {
    timeout_ms
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(30))
}

impl ChatPane {
    pub fn new(
        ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        project_root: PathBuf,
        path_index: Arc<PathIndex>,
        unified_language_model: Option<(
            futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>,
            Vec<String>,
            usize,
        )>,
        command_bridge_tx: Option<
            futures::channel::mpsc::UnboundedSender<
                crate::quorp::tui::command_bridge::CommandBridgeRequest,
            >,
        >,
    ) -> Self {
        let chat_service_tx =
            crate::quorp::tui::chat_service::spawn_chat_service_loop(ui_tx.clone());
        let (mut models, mut model_index) = match unified_language_model {
            Some((_tx, m, idx)) => {
                let model_index = if m.is_empty() {
                    0
                } else {
                    idx.min(m.len() - 1)
                };
                (m, model_index)
            }
            None => {
                let mut models: Vec<String> = crate::quorp::tui::model_registry::local_moe_catalog()
                    .into_iter()
                    .map(|m| m.id.to_string())
                    .collect();
                let mut model_index = 1usize;
                if let Ok(m) = std::env::var("QUORP_TUI_MODEL") {
                    if !m.is_empty() {
                        if let Some(i) = models.iter().position(|x| x == &m) {
                            model_index = i;
                        } else {
                            models.insert(0, m);
                            model_index = 0;
                        }
                    }
                }
                (models, model_index)
            }
        };

        let first_session = ChatSession::new("Chat 1".to_string());

        Self {
            ui_tx: ui_tx.clone(),
            sessions: vec![first_session],
            active_session: 0,
            models,
            model_index,
            viewport_transcript_lines: 1,
            last_text_width: 60,
            chat_service_tx,
            command_bridge_tx: command_bridge_tx.or_else(|| {
                let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
                let _command_thread =
                    crate::quorp::tui::native_backend::spawn_command_service_loop(
                        ui_tx.clone(),
                        command_rx,
                    );
                Some(command_tx)
            }),
            project_root,
            path_index,
            #[cfg(test)]
            base_url_override: None,
        }
    }

    pub fn request_persist_default_model_to_agent_settings(&self, _registry_line: &str) {
    }

    fn active_session_mut(&mut self) -> &mut ChatSession {
        &mut self.sessions[self.active_session]
    }

    fn active_session_ref(&self) -> &ChatSession {
        &self.sessions[self.active_session]
    }

    fn abort_streaming(&mut self) {
        let session_id = self.active_session;
        self.active_session_mut().streaming = false;
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id })
            .is_err()
        {
            log::warn!("tui: failed to cancel chat stream for session {session_id}");
        }
    }

    fn cancel_stream_for_session(&mut self, session_id: usize) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.streaming = false;
        }
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id })
            .is_err()
        {
            log::warn!("tui: failed to cancel chat stream for session {session_id}");
        }
    }

    fn build_service_messages_for_session(&self, session_index: usize) -> Vec<ChatServiceMessage> {
        let Some(session) = self.sessions.get(session_index) else {
            return Vec::new();
        };
        session
            .messages
            .iter()
            .filter_map(|message| match message {
                ChatMessage::User(content) => Some(ChatServiceMessage {
                    role: ChatServiceRole::User,
                    content: expand_mentions_for_api_message(content, self.project_root.as_path()),
                }),
                ChatMessage::Assistant(content) if !content.trim().is_empty() => {
                    Some(ChatServiceMessage {
                        role: ChatServiceRole::Assistant,
                        content: content.clone(),
                    })
                }
                ChatMessage::Assistant(_) => None,
            })
            .collect()
    }

    fn base_url_override_for_service(&self) -> Option<String> {
        #[cfg(test)]
        {
            return self.base_url_override.clone();
        }
        #[cfg(not(test))]
        {
            None
        }
    }

    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    pub fn ensure_project_root(&mut self, root: &std::path::Path) {
        if self.project_root.as_path() == root {
            return;
        }
        self.project_root = root.to_path_buf();
        self.path_index.set_root(self.project_root.clone());
    }

    /// Match production + [`TuiTestHarness::new_with_backend_state`]: backend snapshots drive the index
    /// (no background `ignore` walk). Mention tests that need a disk scan use `new_with_root` instead.
    #[cfg(test)]
    pub fn use_project_backed_path_index_for_backend_flow_tests(&mut self, root: PathBuf) {
        let watch = std::sync::Arc::new(std::sync::RwLock::new(root.clone()));
        self.path_index = std::sync::Arc::new(crate::quorp::tui::path_index::PathIndex::new_project_backed(
            root,
            watch,
        ));
    }

    pub fn blocking_wait_path_index_ready(&self, timeout: Duration) -> bool {
        self.path_index
            .blocking_wait_for_ready(self.project_root.as_path(), timeout)
    }

    pub fn path_index_progress(&self) -> PathIndexProgress {
        self.path_index.snapshot_progress()
    }

    pub fn apply_path_index_snapshot(
        &mut self,
        root: std::path::PathBuf,
        entries: Arc<Vec<PathEntry>>,
        files_seen: u64,
    ) {
        self.path_index
            .apply_bridge_snapshot(root, entries, files_seen);
    }

    pub fn is_streaming(&self) -> bool {
        self.active_session_ref().streaming
    }

    pub fn active_session_index(&self) -> usize {
        self.active_session
    }

    pub fn apply_chat_event(&mut self, event: ChatUiEvent, theme: &crate::quorp::tui::theme::Theme) {
        match event {
            ChatUiEvent::AssistantDelta(idx, delta) => {
                if let Some(last) = self.sessions.get_mut(idx).and_then(|s| s.messages.last_mut()) {
                    last.push_assistant(delta.as_str());
                }
                let stick = self
                    .sessions
                    .get(idx)
                    .is_some_and(|s| s.stick_to_bottom);
                if stick && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::StreamFinished(idx) => {
                if let Some(s) = self.sessions.get_mut(idx) {
                    s.streaming = false;
                }
                self.try_extract_pending_command_for_session(idx);
                let stick = self
                    .sessions
                    .get(idx)
                    .is_some_and(|s| s.stick_to_bottom);
                if stick && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::Error(idx, err) => {
                if let Some(s) = self.sessions.get_mut(idx) {
                    s.last_error = Some(err.clone());
                    if let Some(ChatMessage::Assistant(content)) = s.messages.last_mut() {
                        if content.is_empty() {
                            *content = format!("Error: {err}");
                        } else {
                            content.push_str(&format!("\n[Error: {err}]"));
                        }
                    }
                    s.streaming = false;
                }
            }
            ChatUiEvent::CommandOutput(idx, line) => {
                let Some(s) = self.sessions.get_mut(idx) else { return };
                s.command_output_lines.push(line);
                if s.stick_to_bottom && idx == self.active_session {
                    self.scroll_transcript_to_bottom(theme);
                }
            }
            ChatUiEvent::CommandFinished(idx, output) => {
                let Some(s) = self.sessions.get_mut(idx) else { return };
                s.running_command = false;
                let command = s.running_command_name.take().unwrap_or_default();
                let context = format!(
                    "[Command Output]\n$ {}\n{}\n[End Output]",
                    command,
                    output
                );
                s.messages.push(ChatMessage::User(context));
                s.messages.push(ChatMessage::Assistant(String::new()));
                s.command_output_lines.clear();
                
                if idx == self.active_session {
                    self.submit_input_for_followup(theme, command, output);
                }
            }
        }
    }

    fn try_extract_pending_command_for_session(&mut self, session_index: usize) {
        let last_text = match self
            .sessions
            .get(session_index)
            .and_then(|s| s.messages.last())
        {
            Some(ChatMessage::Assistant(text)) => text.clone(),
            _ => return,
        };
        let segments = parse_assistant_segments(&last_text);
        for segment in segments {
            if let AssistantSegment::RunCommand { command, timeout_ms } = segment {
                let timeout = parse_command_timeout(Some(&timeout_ms.to_string()));
                if let Some(s) = self.sessions.get_mut(session_index) {
                    s.pending_command = Some(PendingCommand { command, timeout });
                }
                return;
            }
        }
    }

    fn execute_pending_command(&mut self) {
        let idx = self.active_session;
        let pending = self
            .sessions
            .get_mut(idx)
            .and_then(|session| session.pending_command.take());
        let Some(cmd) = pending else {
            return;
        };
        {
            let session = &mut self.sessions[idx];
            session.running_command = true;
            session.running_command_name = Some(cmd.command.clone());
            session.command_output_lines.clear();
        }
        if let Some(ref bridge_tx) = self.command_bridge_tx {
            let cwd = self.project_root.clone();
            let session_id = idx;
            let send_result = bridge_tx.unbounded_send(
                crate::quorp::tui::command_bridge::CommandBridgeRequest::Run {
                    session_id,
                    command: cmd.command,
                    cwd,
                    timeout: cmd.timeout,
                },
            );
            if send_result.is_err() {
                let session = &mut self.sessions[idx];
                session.running_command = false;
                session.running_command_name = None;
                session.messages.push(ChatMessage::User(
                    "Command bridge disconnected; could not run command.".to_string(),
                ));
                session.messages.push(ChatMessage::Assistant(String::new()));
            }
        }
    }

    fn cancel_pending_command(&mut self) {
        let s = self.active_session_mut();
        if s.pending_command.take().is_some() {
            s.messages.push(ChatMessage::User(
                "[Command cancelled by user]".to_string(),
            ));
        }
    }

    fn submit_input_for_followup(
        &mut self,
        theme: &crate::quorp::tui::theme::Theme,
        command: String,
        command_output: String,
    ) {
        if self.active_session_ref().messages.is_empty() || self.models.is_empty() {
            return;
        }
        let session_id = self.active_session;
        let model_id = self.current_model_id().to_string();
        let messages = self.build_service_messages_for_session(session_id);
        self.active_session_mut().streaming = true;
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::SummarizeCommandOutput {
                session_id,
                model_id,
                command,
                command_output,
                messages,
                project_root: self.project_root.clone(),
                base_url_override: self.base_url_override_for_service(),
            })
            .is_err()
        {
            let session = self.active_session_mut();
            session.streaming = false;
            session.last_error = Some("Chat service disconnected.".to_string());
            if let Some(ChatMessage::Assistant(text)) = session.messages.last_mut() {
                *text = "Chat service disconnected.".to_string();
            }
            self.scroll_transcript_to_bottom(theme);
        }
    }

    pub fn current_model_id(&self) -> &str {
        self.models
            .get(self.model_index)
            .map(|s| s.as_str())
            .unwrap_or("qwen3.5-35b-a3b")
    }

    pub fn model_list(&self) -> &[String] {
        &self.models
    }

    pub fn model_index(&self) -> usize {
        self.model_index
    }

    pub fn set_model_index(&mut self, index: usize) {
        if self.models.is_empty() {
            return;
        }
        self.model_index = index.min(self.models.len() - 1);
    }

    fn scroll_transcript_to_bottom(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, self.last_text_width);
        let v = self.viewport_transcript_lines.max(1);
        self.active_session_mut().transcript_scroll = wrapped.len().saturating_sub(v);
    }

    fn wrapped_line_count(&self, theme: &crate::quorp::tui::theme::Theme) -> usize {
        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, self.last_text_width);
        wrapped.len().max(1)
    }

    fn clamp_transcript_scroll(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let total = self.wrapped_line_count(theme);
        let v = self.viewport_transcript_lines.max(1);
        let max_scroll = total.saturating_sub(v);
        let s = self.active_session_mut();
        if s.transcript_scroll > max_scroll {
            s.transcript_scroll = max_scroll;
        }
    }

    fn cycle_model(&mut self, delta: isize) {
        if self.models.is_empty() {
            return;
        }
        let len = self.models.len() as isize;
        self.model_index = (self.model_index as isize + delta).rem_euclid(len) as usize;
    }

    pub fn handle_key_event(&mut self, key: &KeyEvent, theme: &crate::quorp::tui::theme::Theme) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Tab {
            return false;
        }

        if self.active_session_ref().pending_command.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.execute_pending_command();
                    return true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.cancel_pending_command();
                    return true;
                }
                _ => return true,
            }
        }

        if self.active_session_ref().running_command {
            return true;
        }

        match key.code {
            KeyCode::Esc => {
                if self.active_session_ref().mention_popup.is_some() {
                    self.active_session_mut().mention_popup = None;
                    return true;
                }
                false
            }
            KeyCode::Tab => {
                if self
                    .active_session_ref()
                    .mention_popup
                    .as_ref()
                    .is_some_and(|p| !p.matches.is_empty())
                {
                    self.accept_mention();
                    return true;
                }
                false
            }
            KeyCode::Char('[') if key.modifiers.is_empty() => {
                self.cycle_model(-1);
                true
            }
            KeyCode::Char(']') if key.modifiers.is_empty() => {
                self.cycle_model(1);
                true
            }
            KeyCode::Enter => {
                if self
                    .active_session_ref()
                    .mention_popup
                    .as_ref()
                    .is_some_and(|p| !p.matches.is_empty())
                {
                    self.accept_mention();
                    return true;
                }
                if self.active_session_ref().mention_popup.is_some() {
                    self.active_session_mut().mention_popup = None;
                    return true;
                }
                self.submit_input(theme);
                true
            }
            KeyCode::Left => {
                let s = self.active_session_mut();
                if s.cursor_char > 0 {
                    s.cursor_char -= 1;
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Right => {
                let max_c = {
                    let s = self.active_session_ref();
                    s.input.chars().count()
                };
                let s = self.active_session_mut();
                if s.cursor_char < max_c {
                    s.cursor_char += 1;
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Home => {
                self.active_session_mut().cursor_char = 0;
                self.sync_mention_popup();
                true
            }
            KeyCode::End => {
                let count = self.active_session_ref().input.chars().count();
                self.active_session_mut().cursor_char = count;
                self.sync_mention_popup();
                true
            }
            KeyCode::Backspace => {
                let s = self.active_session_mut();
                if s.cursor_char > 0 {
                    let end = char_byte_index(&s.input, s.cursor_char);
                    s.cursor_char -= 1;
                    let start = char_byte_index(&s.input, s.cursor_char);
                    s.input.replace_range(start..end, "");
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Delete => {
                let (byte, len) = {
                    let s = self.active_session_ref();
                    let byte = char_byte_index(&s.input, s.cursor_char);
                    (byte, s.input.len())
                };
                let s = self.active_session_mut();
                if byte < len {
                    let end = s.input.ceil_char_boundary(byte + 1);
                    s.input.replace_range(byte..end, "");
                }
                self.sync_mention_popup();
                true
            }
            KeyCode::Up => {
                let handled = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            p.selected = p.selected.saturating_sub(1);
                            clamp_mention_scroll(
                                &mut p.scroll_top,
                                p.selected,
                                MENTION_POPUP_MAX_VISIBLE,
                                p.matches.len(),
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if handled {
                    return true;
                }
                let s = self.active_session_mut();
                s.stick_to_bottom = false;
                s.transcript_scroll = s.transcript_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                let handled = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let max_sel = p.matches.len().saturating_sub(1);
                            p.selected = (p.selected + 1).min(max_sel);
                            clamp_mention_scroll(
                                &mut p.scroll_top,
                                p.selected,
                                MENTION_POPUP_MAX_VISIBLE,
                                p.matches.len(),
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if handled {
                    return true;
                }
                self.clamp_transcript_scroll(theme);
                let max = self
                    .wrapped_line_count(theme)
                    .saturating_sub(self.viewport_transcript_lines.max(1));
                let s = self.active_session_mut();
                s.transcript_scroll = (s.transcript_scroll + 1).min(max);
                if s.transcript_scroll >= max {
                    s.stick_to_bottom = true;
                }
                true
            }
            KeyCode::PageUp => {
                let popup_page = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let v = MENTION_POPUP_MAX_VISIBLE.min(p.matches.len()).max(1);
                            let max_top = p.matches.len().saturating_sub(v);
                            p.scroll_top = p.scroll_top.saturating_sub(v).min(max_top);
                            if p.selected < p.scroll_top {
                                p.selected = p.scroll_top;
                            }
                            let last_visible = (p.scroll_top + v).min(p.matches.len()).saturating_sub(1);
                            if p.selected > last_visible {
                                p.selected = last_visible;
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if popup_page {
                    return true;
                }
                let step = self.viewport_transcript_lines.saturating_sub(1).max(1);
                let s = self.active_session_mut();
                s.stick_to_bottom = false;
                s.transcript_scroll = s.transcript_scroll.saturating_sub(step);
                true
            }
            KeyCode::PageDown => {
                let popup_page = {
                    let s = self.active_session_mut();
                    if let Some(ref mut p) = s.mention_popup {
                        if !p.matches.is_empty() {
                            let v = MENTION_POPUP_MAX_VISIBLE.min(p.matches.len()).max(1);
                            let max_top = p.matches.len().saturating_sub(v);
                            p.scroll_top = (p.scroll_top + v).min(max_top);
                            if p.selected < p.scroll_top {
                                p.selected = p.scroll_top;
                            }
                            let last_visible = (p.scroll_top + v).min(p.matches.len()).saturating_sub(1);
                            if p.selected > last_visible {
                                p.selected = last_visible;
                            }
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if popup_page {
                    return true;
                }
                let step = self.viewport_transcript_lines.saturating_sub(1).max(1);
                self.clamp_transcript_scroll(theme);
                let max = self
                    .wrapped_line_count(theme)
                    .saturating_sub(self.viewport_transcript_lines.max(1));
                let s = self.active_session_mut();
                s.transcript_scroll = (s.transcript_scroll + step).min(max);
                if s.transcript_scroll >= max {
                    s.stick_to_bottom = true;
                }
                true
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.insert_char(c);
                true
            }
            _ => false,
        }
    }

    fn byte_at_char_index(&self) -> usize {
        let s = self.active_session_ref();
        char_byte_index(&s.input, s.cursor_char)
    }

    fn sync_mention_popup(&mut self) {
        let cursor_byte = self.byte_at_char_index();
        let token = {
            let s = self.active_session_ref();
            active_mention_token(&s.input, cursor_byte)
        };
        let Some((at_byte, query)) = token else {
            self.active_session_mut().mention_popup = None;
            return;
        };
        let matches = self.path_index.match_query(&query, 80);
        let s = self.active_session_mut();
        let prev = s.mention_popup.take();
        let (selected, mut scroll_top) = match prev {
            Some(p) if p.at_byte == at_byte && p.last_query == query => {
                let sel = p.selected.min(matches.len().saturating_sub(1));
                (sel, p.scroll_top)
            }
            _ => (0, 0),
        };
        clamp_mention_scroll(
            &mut scroll_top,
            selected,
            MENTION_POPUP_MAX_VISIBLE,
            matches.len(),
        );
        s.mention_popup = Some(MentionPopup {
            at_byte,
            selected,
            scroll_top,
            matches,
            last_query: query,
        });
    }

    fn accept_mention(&mut self) {
        let Some(pop) = self.active_session_mut().mention_popup.take() else {
            return;
        };
        if pop.matches.is_empty() {
            self.active_session_mut().mention_popup = Some(pop);
            return;
        }
        let sel = pop.selected.min(pop.matches.len().saturating_sub(1));
        let entry = &pop.matches[sel];
        let link = match mention_link_for_path(&entry.abs_path, &entry.relative_display) {
            Ok(l) => format!("{l} "),
            Err(_) => {
                self.active_session_mut().mention_popup = Some(pop);
                return;
            }
        };
        let end_byte = self.byte_at_char_index();
        let s = self.active_session_mut();
        if end_byte < pop.at_byte || pop.at_byte > s.input.len() {
            self.sync_mention_popup();
            return;
        }
        s.input.replace_range(pop.at_byte..end_byte, &link);
        s.cursor_char = char_index_for_byte(&s.input, pop.at_byte + link.len());
        self.sync_mention_popup();
    }

    fn insert_char(&mut self, c: char) {
        let byte = self.byte_at_char_index();
        let mut buf = [0u8; 4];
        let slice = c.encode_utf8(&mut buf);
        let s = self.active_session_mut();
        s.input.insert_str(byte, slice);
        s.cursor_char += 1;
        self.sync_mention_popup();
    }

    fn title_from_user_message(text: &str) -> String {
        let t = text.trim();
        let line = t.lines().next().unwrap_or("");
        let mut s: String = line.chars().take(32).collect();
        s = s.trim().to_string();
        if s.is_empty() {
            "Chat".to_string()
        } else {
            s
        }
    }

    pub fn chat_tab_specs(&self, theme: &crate::quorp::tui::theme::Theme) -> Vec<crate::quorp::tui::chrome_v2::LeafTabSpec> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(i, session)| crate::quorp::tui::chrome_v2::LeafTabSpec {
                label: session.title.clone(),
                active: i == self.active_session,
                icon: Some(theme.glyphs.activity_agent),
                show_close: self.sessions.len() > 1,
            })
            .collect()
    }

    pub fn draw_tab_strip(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        strip: ratatui::layout::Rect,
        theme: &crate::quorp::tui::theme::Theme,
    ) -> (Vec<crate::quorp::tui::chrome_v2::LeafTabLayoutCell>, usize) {
        let specs = self.chat_tab_specs(theme);
        let (cells, overflow) = crate::quorp::tui::chrome_v2::layout_leaf_tabs(strip, &specs);
        crate::quorp::tui::chrome_v2::render_leaf_tabs_laid_out(
            buf,
            strip,
            &cells,
            &specs,
            &theme.palette,
            theme.glyphs.close_icon,
            theme.palette.chat_accent,
        );
        if overflow > 0 {
            crate::quorp::tui::chrome_v2::render_tab_overflow_hint(buf, strip, overflow, &theme.palette);
        }
        (cells, overflow)
    }

    pub fn activate_chat_session(&mut self, index: usize, theme: &crate::quorp::tui::theme::Theme) -> bool {
        if index >= self.sessions.len() {
            return false;
        }
        if index == self.active_session {
            return true;
        }
        self.active_session = index;
        self.clamp_transcript_scroll(theme);
        true
    }

    pub fn close_chat_session_at(&mut self, index: usize, theme: &crate::quorp::tui::theme::Theme) -> bool {
        if index >= self.sessions.len() || self.sessions.len() <= 1 {
            return false;
        }
        if self.sessions.get(index).is_some_and(|s| s.streaming) {
            self.cancel_stream_for_session(index);
        }
        let was_active = self.active_session == index;
        self.sessions.remove(index);
        let new_len = self.sessions.len();
        if self.active_session > index {
            self.active_session -= 1;
        } else if was_active {
            self.active_session = self.active_session.min(new_len.saturating_sub(1));
        }
        self.active_session = self.active_session.min(new_len.saturating_sub(1));
        self.clamp_transcript_scroll(theme);
        true
    }

    pub fn close_all_chat_sessions(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let streaming_sessions: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| session.streaming.then_some(index))
            .collect();
        for session_id in streaming_sessions {
            self.cancel_stream_for_session(session_id);
        }
        self.sessions = vec![ChatSession::new("Chat 1".to_string())];
        self.active_session = 0;
        self.clamp_transcript_scroll(theme);
    }

    pub fn cycle_chat_session(&mut self, delta: isize, theme: &crate::quorp::tui::theme::Theme) {
        if self.sessions.len() <= 1 {
            return;
        }
        if self.active_session_ref().streaming {
            self.abort_streaming();
        }
        let len = self.sessions.len() as isize;
        self.active_session = (self.active_session as isize + delta).rem_euclid(len) as usize;
        self.clamp_transcript_scroll(theme);
    }

    pub fn new_chat_session(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        if self.active_session_ref().streaming {
            self.abort_streaming();
        }
        let n = self.sessions.len() + 1;
        self.sessions.push(ChatSession::new(format!("Chat {n}")));
        self.active_session = self.sessions.len() - 1;
        self.clamp_transcript_scroll(theme);
    }

    fn submit_input(&mut self, theme: &crate::quorp::tui::theme::Theme) {
        let trimmed = {
            let s = self.active_session_ref();
            s.input.trim()
        };
        if trimmed.is_empty() {
            return;
        }
        let trimmed = trimmed.to_string();

        self.abort_streaming();

        let user_text = trimmed;
        let request_input = user_text.clone();
        let s = self.active_session_mut();
        let was_empty = s.messages.is_empty();
        s.input.clear();
        s.cursor_char = 0;
        s.last_error = None;
        s.stick_to_bottom = true;
        if was_empty {
            s.title = Self::title_from_user_message(&user_text);
        }
        s.messages.push(ChatMessage::User(user_text));
        s.messages.push(ChatMessage::Assistant(String::new()));
        if s.messages.len() > MAX_MESSAGES {
            let excess = s.messages.len() - MAX_MESSAGES;
            let drop_count = if excess % 2 != 0 { excess + 1 } else { excess };
            s.messages.drain(0..drop_count);
        }

        if self.models.is_empty() {
            if let Some(ChatMessage::Assistant(text)) = self.active_session_mut().messages.last_mut()
            {
                *text = "No configured chat models are available.".to_string();
            }
            self.scroll_transcript_to_bottom(theme);
            return;
        }

        self.active_session_mut().streaming = true;

        let session_id = self.active_session;
        let model_id = self.current_model_id().to_string();
        let messages = self.build_service_messages_for_session(session_id);
        if self
            .chat_service_tx
            .unbounded_send(ChatServiceRequest::SubmitPrompt {
                session_id,
                model_id,
                latest_input: request_input,
                messages,
                project_root: self.project_root.clone(),
                base_url_override: self.base_url_override_for_service(),
            })
            .is_err()
        {
            let session = self.active_session_mut();
            session.streaming = false;
            session.last_error = Some("Chat service disconnected.".to_string());
            if let Some(ChatMessage::Assistant(text)) = session.messages.last_mut() {
                *text = "Chat service disconnected.".to_string();
            }
        }
        self.scroll_transcript_to_bottom(theme);
    }

    pub fn render_in_leaf(
        &mut self,
        buf: &mut ratatui::buffer::Buffer,
        rects: &crate::quorp::tui::workbench::LeafRects,
        is_focused: bool,
        theme: &crate::quorp::tui::theme::Theme,
    ) {
        if let Some(banner_rect) = rects.banner {
            let text = format!("Flash-MOE {}", self.current_model_id());
            crate::quorp::tui::chrome_v2::render_agent_banner(buf, banner_rect, "◈", &text, &theme.palette);
        }

        let body_rect = rects.body;
        self.viewport_transcript_lines = body_rect.height as usize;

        let show_scrollbar_hint = body_rect.width > 2;
        let text_width = if show_scrollbar_hint {
            body_rect.width.saturating_sub(1)
        } else {
            body_rect.width
        };
        self.last_text_width = text_width as usize;

        let lines = self.build_transcript_lines(theme);
        let wrapped = wrap_lines(lines, text_width as usize);
        let total_lines = wrapped.len().max(1);
        let v = self.viewport_transcript_lines.max(1);

        self.clamp_transcript_scroll(theme);
        let max_scroll = total_lines.saturating_sub(v);
        let scroll = self
            .active_session_ref()
            .transcript_scroll
            .min(max_scroll);

        let visible: Vec<Line> = wrapped
            .into_iter()
            .skip(scroll)
            .take(v)
            .collect();

        let transcript_block = ratatui::widgets::Paragraph::new(visible);
        let show_scrollbar = total_lines > v && body_rect.width > 1;

        let text_area = if show_scrollbar {
            ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Horizontal)
                .constraints([ratatui::layout::Constraint::Min(1), ratatui::layout::Constraint::Length(1)])
                .split(body_rect)[0]
        } else {
            body_rect
        };

        ratatui::widgets::Widget::render(transcript_block, text_area, buf);

        if show_scrollbar {
            let mut state = ratatui::widgets::ScrollbarState::new(total_lines).position(scroll);
            ratatui::widgets::StatefulWidget::render(
                ratatui::widgets::Scrollbar::new(ratatui::widgets::ScrollbarOrientation::VerticalRight),
                body_rect,
                buf,
                &mut state,
            );
        }

        if let Some(composer_rect) = rects.composer {
            if let Some(ref mp) = self.active_session_ref().mention_popup {
                if is_focused && !mp.matches.is_empty() {
                    let space_above = composer_rect.y.saturating_sub(body_rect.y);
                    if space_above > 0 && composer_rect.width > 2 {
                        let max_rows = MENTION_POPUP_MAX_VISIBLE.min(mp.matches.len());
                        let popup_rows = (max_rows as u16).min(space_above).max(1);
                        let popup_y = composer_rect.y.saturating_sub(popup_rows);
                        let line_budget =
                            (composer_rect.width.saturating_sub(2)) as usize;
                        let slice = mp
                            .matches
                            .iter()
                            .skip(mp.scroll_top)
                            .take(popup_rows as usize)
                            .map(|e| {
                                let raw = if e.is_directory {
                                    format!("{}/", e.relative_display)
                                } else {
                                    e.relative_display.clone()
                                };
                                crate::quorp::tui::text_width::truncate_middle_fit(&raw, line_budget.max(1))
                            })
                            .collect::<Vec<_>>();
                        let vm = crate::quorp::tui::chrome_v2::MentionPopupVm {
                            lines: slice,
                            selected: mp.selected.saturating_sub(mp.scroll_top),
                        };
                        let popup_rect = ratatui::layout::Rect::new(
                            composer_rect.x,
                            popup_y,
                            composer_rect.width,
                            popup_rows,
                        );
                        crate::quorp::tui::chrome_v2::render_mention_popup(
                            buf,
                            popup_rect,
                            &vm,
                            &theme.palette,
                        );
                    }
                }
            }

            let composer = crate::quorp::tui::chrome_v2::ComposerVm {
                placeholder: " Ask… @ files · / commands".to_string(),
                input: self.active_session_ref().input.clone(),
                mode_chips: vec![],
                focused: is_focused,
            };
            crate::quorp::tui::chrome_v2::render_composer(buf, composer_rect, &composer, &theme.palette);
        }
    }

    fn build_transcript_lines(&self, theme: &crate::quorp::tui::theme::Theme) -> Vec<Line<'static>> {
        let session = self.active_session_ref();
        let mut lines = Vec::new();
        if let Some(ref err) = session.last_error {
            if session.messages.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("Error: {err}"),
                    Style::default().fg(theme.palette.success_green).fg(Color::Red),
                )));
                return lines;
            }
        }

        for m in &session.messages {
            match m {
                ChatMessage::User(u) => {
                    if !lines.is_empty() {
                        lines.push(Line::from(""));
                    }
                    for line in u.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(theme.palette.text).add_modifier(Modifier::BOLD)),
                        ]));
                    }
                    if u.is_empty() {
                        lines.push(Line::from(Span::styled("  ", Style::default())));
                    }
                }
                ChatMessage::Assistant(a) => {
                    if a.starts_with("Error:") || a.contains("\n[Error:") {
                        lines.push(Line::from(Span::styled(
                            a.clone(),
                            Style::default().fg(Color::Red),
                        )));
                    } else {
                        let segments = parse_assistant_segments(a);
                        let mut first = true;
                        if !segments.is_empty() { lines.push(Line::from("")); }
                        for segment in segments {
                            match segment {
                                AssistantSegment::Text(text) => {
                                    for line in text.lines() {
                                        let prefix = if first { "  " } else { "  " };
                                        first = false;
                                        lines.push(Line::from(vec![
                                            Span::styled(prefix, Style::default()),
                                            Span::styled(line.to_string(), Style::default().fg(theme.palette.text)),
                                        ]));
                                    }
                                    if text.is_empty() && first {
                                        first = false;
                                        lines.push(Line::from(Span::styled("  ", Style::default())));
                                    }
                                }
                                AssistantSegment::Think(content) => {
                                    let think_style = Style::default()
                                        .fg(theme.palette.text_faint)
                                        .add_modifier(Modifier::ITALIC);
                                    lines.push(Line::from(Span::styled(
                                        "  💭 thinking…",
                                        think_style,
                                    )));
                                    for line in content.lines() {
                                        lines.push(Line::from(Span::styled(
                                            format!("  │ {line}"),
                                            think_style,
                                        )));
                                    }
                                    first = false;
                                }
                                AssistantSegment::Code { language, body } => {
                                    let lang_label = if language.is_empty() { "code" } else { &language };
                                    let border_color = theme.palette.subtle_border;
                                    let bg = theme.palette.code_block_bg;
                                    
                                    lines.push(Line::from(Span::styled(
                                        format!("  ┌── {lang_label} "),
                                        Style::default().fg(border_color),
                                    )));
                                    
                                    let highlighted = highlight_code(&body, &language, theme);
                                    for hl_line in highlighted {
                                        let mut spans: Vec<Span<'static>> = vec![Span::styled(
                                            "  │ ",
                                            Style::default().fg(border_color),
                                        )];
                                        for hl_span in hl_line {
                                            spans.push(Span::styled(
                                                hl_span.content.to_string(),
                                                hl_span.style.bg(bg),
                                            ));
                                        }
                                        lines.push(Line::from(spans));
                                    }
                                    lines.push(Line::from(Span::styled(
                                        "  └────────────",
                                        Style::default().fg(border_color),
                                    )));
                                    first = false;
                                }
                                AssistantSegment::RunCommand { command, .. } => {
                                    let cmd_style = Style::default().fg(theme.palette.success_green);
                                    lines.push(Line::from(Span::styled(
                                        "  ┌── command",
                                        cmd_style,
                                    )));
                                    for cmd_line in command.lines() {
                                        lines.push(Line::from(vec![
                                            Span::styled("  │ $ ", cmd_style),
                                            Span::styled(
                                                cmd_line.to_string(),
                                                Style::default().fg(theme.palette.text).bg(theme.palette.code_block_bg),
                                            ),
                                        ]));
                                    }
                                    lines.push(Line::from(Span::styled(
                                        "  └────────────",
                                        cmd_style,
                                    )));
                                    first = false;
                                }
                            }
                        }
                        if a.is_empty()
                            && self.active_session_ref().streaming
                        {
                            lines.push(Line::from(Span::styled(
                                "  …",
                                Style::default().fg(theme.palette.text_muted),
                            )));
                        }
                    }
                }
            }
        }

        if let Some(cmd) = &session.pending_command {
            lines.push(Line::from(vec![
                Span::styled("⚠ Run: ", Style::default().fg(theme.palette.success_green)),
                Span::styled(cmd.command.clone(), Style::default().fg(theme.palette.text)),
                Span::styled(" ? [y/n]", Style::default().fg(theme.palette.text_muted)),
            ]));
        }

        if session.running_command {
            lines.push(Line::from(Span::styled(
                "⏳ Running command…",
                Style::default().fg(theme.palette.text_muted),
            )));
            for output_line in &session.command_output_lines {
                lines.push(Line::from(vec![
                    Span::styled("  │ ", Style::default().fg(theme.palette.subtle_border)),
                    Span::styled(output_line.clone(), Style::default().fg(theme.palette.text_faint)),
                ]));
            }
        }

        lines
    }



    #[cfg(test)]
    pub fn input_for_test(&self) -> &str {
        self.active_session_ref().input.as_str()
    }

    #[cfg(test)]
    pub fn set_input_for_test(&mut self, s: &str) {
        let s_mut = self.active_session_mut();
        s_mut.input = s.to_string();
        s_mut.cursor_char = s.chars().count();
    }

    #[cfg(test)]
    pub fn set_base_url_for_test(&mut self, base_url: String) {
        self.base_url_override = Some(base_url);
    }

    #[cfg(test)]
    pub fn mention_popup_open_for_test(&self) -> bool {
        self.active_session_ref().mention_popup.is_some()
    }

    #[cfg(test)]
    pub fn mention_match_count_for_test(&self) -> usize {
        self.active_session_ref()
            .mention_popup
            .as_ref()
            .map(|p| p.matches.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn mention_selected_label_for_test(&self) -> Option<String> {
        let p = self.active_session_ref().mention_popup.as_ref()?;
        let e = p.matches.get(p.selected)?;
        Some(e.relative_display.clone())
    }

    #[cfg(test)]
    pub fn mention_scroll_top_for_test(&self) -> Option<usize> {
        Some(self.active_session_ref().mention_popup.as_ref()?.scroll_top)
    }

    #[cfg(test)]
    pub fn transcript_scroll_for_test(&self) -> usize {
        self.active_session_ref().transcript_scroll
    }

    /// Lines accumulated from [`ChatUiEvent::CommandOutput`] before [`ChatUiEvent::CommandFinished`].
    #[cfg(test)]
    pub fn command_output_lines_for_test(&self) -> Vec<String> {
        self.active_session_ref().command_output_lines.clone()
    }

    /// Output lines are drawn in the transcript only while a command is marked running (see
    /// [`ChatPane::execute_pending_command`]).
    #[cfg(test)]
    pub fn set_running_command_for_test(&mut self, running: bool) {
        let session = self.active_session_mut();
        session.running_command = running;
        if !running {
            session.running_command_name = None;
        }
    }

    #[cfg(test)]
    pub fn model_index_for_test(&self) -> usize {
        self.model_index
    }

    pub fn set_model_index_for_test(&mut self, index: usize) {
        if !self.models.is_empty() {
            self.model_index = index % self.models.len();
        }
    }

    #[cfg(test)]
    pub fn seed_messages_for_test(&mut self, messages: Vec<ChatMessage>) {
        self.active_session_mut().messages = messages;
    }

    #[cfg(test)]
    pub fn last_assistant_text_for_test(&self) -> Option<&str> {
        self.active_session_ref()
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                ChatMessage::Assistant(s) => Some(s.as_str()),
                ChatMessage::User(_) => None,
            })
    }

    #[cfg(test)]
    pub fn set_streaming_for_test(&mut self, streaming: bool) {
        self.active_session_mut().streaming = streaming;
    }
}

enum AssistantSegment {
    Text(String),
    Think(String),
    Code { language: String, body: String },
    RunCommand { command: String, timeout_ms: u64 },
}

fn parse_assistant_segments(text: &str) -> Vec<AssistantSegment> {
    let mut segments = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        let run_cmd_pos = remaining.find("<run_command");
        let think_pos = remaining.find("<think>");
        let fence_pos = remaining.find("```");

        let next = [run_cmd_pos, think_pos, fence_pos]
            .iter()
            .filter_map(|p| *p)
            .min();

        let Some(next_pos) = next else {
            segments.push(AssistantSegment::Text(remaining.to_string()));
            break;
        };

        if next_pos > 0 {
            segments.push(AssistantSegment::Text(remaining[..next_pos].to_string()));
            remaining = &remaining[next_pos..];
            continue;
        }

        if run_cmd_pos == Some(next_pos) {
            let after_open = &remaining["<run_command".len()..];
            let close_bracket = after_open.find('>').unwrap_or(0);
            let attrs = &after_open[..close_bracket];
            let timeout_ms = extract_attr(attrs, "timeout_ms")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(30000);
            let body_start = &after_open[close_bracket + 1..];
            let (command, rest) = if let Some(end) = body_start.find("</run_command>") {
                (body_start[..end].trim().to_string(), &body_start[end + 14..])
            } else {
                (body_start.trim().to_string(), "")
            };
            segments.push(AssistantSegment::RunCommand { command, timeout_ms });
            remaining = rest;
            continue;
        }

        if think_pos == Some(next_pos) {
            let after_tag = &remaining[7..];
            let (content, rest) = if let Some(end) = after_tag.find("</think>") {
                (after_tag[..end].to_string(), &after_tag[end + 8..])
            } else {
                (after_tag.to_string(), "")
            };
            segments.push(AssistantSegment::Think(content));
            remaining = rest;
            continue;
        }

        if fence_pos == Some(next_pos) {
            let after_fence = &remaining[3..];
            let lang_end = after_fence.find('\n').unwrap_or(after_fence.len());
            let language = after_fence[..lang_end].trim().to_string();
            let body_start = if lang_end < after_fence.len() { lang_end + 1 } else { lang_end };
            let body_rest = &after_fence[body_start..];
            let (body, rest) = if let Some(end) = body_rest.find("```") {
                (body_rest[..end].to_string(), &body_rest[end + 3..])
            } else {
                (body_rest.to_string(), "")
            };
            segments.push(AssistantSegment::Code { language, body });
            remaining = rest;
            continue;
        }

        segments.push(AssistantSegment::Text(remaining.to_string()));
        break;
    }
    segments
}

fn extract_attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("{name}=\"");
    let start = attrs.find(&pattern)?;
    let value_start = start + pattern.len();
    let end = attrs[value_start..].find('"')?;
    Some(&attrs[value_start..value_start + end])
}

fn highlight_code(code: &str, language: &str, _theme: &crate::quorp::tui::theme::Theme) -> Vec<Vec<Span<'static>>> {
    use syntect::highlighting::ThemeSet;
    use syntect::parsing::SyntaxSet;
    use syntect::easy::HighlightLines;
    use syntect::highlighting::Style as SyntectStyle;

    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["base16-ocean.dark"];

    let syntax = syntax_set.find_syntax_by_token(language)
        .or_else(|| syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut output = Vec::new();

    for line in code.lines() {
        let ranges: Vec<(SyntectStyle, &str)> = highlighter.highlight_line(line, &syntax_set)
            .unwrap_or_default();
        let spans: Vec<Span<'static>> = ranges.into_iter().map(|(style, text)| {
            let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
            Span::styled(text.to_string(), Style::default().fg(fg))
        }).collect();
        output.push(spans);
    }
    output
}



fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return lines;
    }
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= max_width {
            result.push(line);
            continue;
        }
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        let mut current_width = 0usize;
        for span in line.spans {
            let content = span.content.to_string();
            let style = span.style;
            let mut remaining = content.as_str();
            while !remaining.is_empty() {
                let mut chunk_end = 0usize;
                let mut chunk_width = 0usize;
                for ch in remaining.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if current_width + chunk_width + cw > max_width {
                        break;
                    }
                    chunk_width += cw;
                    chunk_end += ch.len_utf8();
                }
                if chunk_end == 0 {
                    result.push(Line::from(std::mem::take(&mut current_spans)));
                    let cw = remaining.chars().next().map(|c| c.width().unwrap_or(0)).unwrap_or(1);
                    let len = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    current_spans.push(Span::styled(remaining[..len].to_string(), style));
                    current_width = cw;
                    remaining = &remaining[len..];
                } else {
                    current_spans.push(Span::styled(remaining[..chunk_end].to_string(), style));
                    current_width += chunk_width;
                    remaining = &remaining[chunk_end..];
                    if current_width >= max_width && !remaining.is_empty() {
                        result.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                }
            }
        }
        result.push(Line::from(current_spans));
    }
    result
}
