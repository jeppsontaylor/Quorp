#![allow(unused)]
//! Chat pane: transcript, composer, and model row.
//!
//! When the TUI is started from `main` with a GPUI [`crate::quorp::tui::chat_bridge`] task, all
//! completions go through [`language_model::LanguageModelRegistry`] and
//! [`language_model::LanguageModel::stream_completion`]. The OpenAI HTTP path in this module exists
//! only for harnesses and tools that construct [`ChatPane`] without a bridge (flow tests, `ui_lab`).
//!
//! Pane focus uses **Tab** / **Shift+Tab** for cycling panes (same as the rest of the TUI). Model
//! selection uses **`[`** and **`]`** while the Chat pane is focused (see the model row hint).

use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use language_model::{
    CompletionIntent, LanguageModelRequest, LanguageModelRequestMessage, MessageContent, Role,
    SelectedModel,
};
use open_ai::RequestMessage;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use reqwest_client::ReqwestClient;
use tokio::task::AbortHandle;
use unicode_width::UnicodeWidthChar;

use crate::quorp::tui::command_runner::{CommandRunner, PendingCommand};
use crate::quorp::tui::mention_links::{expand_mentions_for_api_message, mention_link_for_path};
use crate::quorp::tui::path_index::{PathEntry, PathIndex, PathIndexProgress};

const MAX_MESSAGE_CHARS: usize = 512 * 1024;
const MAX_MESSAGES: usize = 500;

const SYSTEM_PROMPT: &str = r#"You are an expert coding assistant running inside a terminal IDE.
You can read files, write code, and execute shell commands.

When the model API exposes tools, prefer the `terminal`, `read_file`, and `list_directory` tools (with correct `cd` / project-relative paths) instead of the XML format below.

When you need to run a shell command without tools, wrap it in XML tags like this:

<run_command>
python3 hello.py
</run_command>

You can optionally specify a timeout in milliseconds:

<run_command timeout_ms="60000">
cargo build
</run_command>

Rules:
- Commands run in the project root directory
- Each command is a fresh shell (no state carries over between commands)
- Output will be shown to the user and returned to you for analysis
- Always run code when the user asks you to — never simulate or fake output
- For scripts you just wrote, use the filename directly (it runs from project root)
- Prefer short, focused commands over long pipelines
- Do NOT run destructive commands (rm -rf /, etc) without explicit user confirmation"#;

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

pub fn normalize_api_base(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

/// Rejects `http://` for non-local hosts so remote calls use TLS; allows `http://` only for
/// `localhost` and loopback IPs (parsed host, not substring — avoids `http://localhost.evil.com`).
pub fn validate_api_base_url(normaliquorp: &str) -> Result<(), String> {
    let parsed = url::Url::parse(normaliquorp).map_err(|e| format!("Invalid API base URL: {e}"))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let allow_plaintext = match parsed.host() {
                Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => {
                    return Err("API base URL must include a host".to_string());
                }
            };
            if allow_plaintext {
                Ok(())
            } else {
                Err(
                    "Insecure HTTP is only allowed for localhost (e.g. http://127.0.0.1:11434/v1). Use https:// for remote hosts."
                        .to_string(),
                )
            }
        }
        other => Err(format!(
            "API base URL scheme must be http or https, got {other}"
        )),
    }
}

/// Pushes UI work to the main thread via a bounded `sync_channel`. Uses blocking `send` on
/// purpose: when the queue is full, the SSE task waits until the UI drains events (backpressure).
/// If Tokio worker blocking becomes an issue, consider `spawn_blocking` around `send` or running the
/// stream loop on a dedicated `std::thread`.
fn send_chat_ui(tx: &std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>, event: ChatUiEvent) {
    if let Err(e) = tx.send(crate::quorp::tui::TuiEvent::Chat(event)) {
        log::error!("tui: chat UI channel closed: {e}");
    }
}

/// When no API key is configured but the base URL is loopback, send `Bearer local`, matching Quorp's
/// OpenAI-compatible localhost behavior in `language_models`.
fn effective_api_key(api_base: &str, stored_key: &str) -> String {
    let trimmed = stored_key.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    if let Ok(url) = url::Url::parse(api_base) {
        match url.host_str() {
            Some(host) if host.eq_ignore_ascii_case("localhost") => return "local".to_string(),
            Some("127.0.0.1") | Some("[::1]") => return "local".to_string(),
            _ => {}
        }
    }
    String::new()
}

async fn drive_chat_completion_stream(
    session_id: usize,
    http: Arc<ReqwestClient>,
    api_base: String,
    api_key_effective: String,
    request: open_ai::Request,
    ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
) {
    let stream_result = open_ai::stream_completion(
        http.as_ref(),
        "quorp-tui",
        &api_base,
        &api_key_effective,
        request,
    )
    .await;

    let mut stream = match stream_result {
        Ok(stream) => stream,
        Err(error) => {
            send_chat_ui(
                &ui_tx,
                ChatUiEvent::Error(session_id, format!("stream start failed: {error}")),
            );
            send_chat_ui(&ui_tx, ChatUiEvent::StreamFinished(session_id));
            return;
        }
    };

    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                for choice in event.choices {
                    if let Some(delta) = choice.delta {
                        if let Some(text) = delta.content.filter(|fragment| !fragment.is_empty()) {
                            send_chat_ui(&ui_tx, ChatUiEvent::AssistantDelta(session_id, text));
                        }
                        if let Some(text) = delta
                            .reasoning_content
                            .filter(|fragment| !fragment.is_empty())
                        {
                            send_chat_ui(&ui_tx, ChatUiEvent::AssistantDelta(session_id, text));
                        }
                    }
                }
            }
            Err(error) => {
                send_chat_ui(
                    &ui_tx,
                    ChatUiEvent::Error(session_id, format!("stream error: {error}")),
                );
                break;
            }
        }
    }

    send_chat_ui(&ui_tx, ChatUiEvent::StreamFinished(session_id));
}

/// Returns `Some(payload)` for a non-empty `data:` line; `None` for comments/empty.
pub fn sse_data_payload(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let rest = line.strip_prefix("data:")?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    Some(rest)
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
    pub command_output_lines: Vec<String>,
    mention_popup: Option<MentionPopup>,
    pub streaming: bool,
    pub streaming_abort: Option<AbortHandle>,
    pub streaming_cancel: Option<Arc<AtomicBool>>,
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
            command_output_lines: Vec::new(),
            mention_popup: None,
            streaming: false,
            streaming_abort: None,
            streaming_cancel: None,
        }
    }
}

pub struct ChatPane {
    http_client: Arc<ReqwestClient>,
    runtime: tokio::runtime::Handle,
    ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
    sessions: Vec<ChatSession>,
    active_session: usize,
    unified_bridge_tx:
        Option<futures::channel::mpsc::UnboundedSender<crate::quorp::tui::bridge::TuiToBackendRequest>>,
    models: Vec<String>,
    model_index: usize,
    viewport_transcript_lines: usize,
    api_base: String,
    api_key: String,
    last_text_width: usize,
    command_runner: CommandRunner,
    command_bridge_tx: Option<
        futures::channel::mpsc::UnboundedSender<crate::quorp::tui::command_bridge::CommandBridgeRequest>,
    >,
    project_root: PathBuf,
    path_index: Arc<PathIndex>,
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

impl ChatPane {
    pub fn new(
        ui_tx: std::sync::mpsc::SyncSender<crate::quorp::tui::TuiEvent>,
        runtime: tokio::runtime::Handle,
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
        let http_client = Arc::new(ReqwestClient::new());

        let default_api_base = format!(
            "http://127.0.0.1:{}/v1",
            flash_moe_defaults::DEFAULT_INFER_SERVE_PORT
        );
        let api_base = std::env::var("QUORP_TUI_API_BASE")
            .or_else(|_| std::env::var("QUORP_TUI_API_URL"))
            .map(|s| normalize_api_base(&s))
            .unwrap_or(default_api_base);
        let mut last_error = None;
        let use_registry = unified_language_model.is_some();
        if !use_registry {
            if let Err(e) = validate_api_base_url(&api_base) {
                last_error = Some(e);
            }
        }
        let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();

        let (unified_bridge_tx, mut models, mut model_index) = match unified_language_model {
            Some((tx, m, idx)) => {
                let model_index = if m.is_empty() {
                    0
                } else {
                    idx.min(m.len() - 1)
                };
                (Some(tx), m, model_index)
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
                (None, models, model_index)
            }
        };

        if use_registry {
            if let Ok(m) = std::env::var("QUORP_TUI_MODEL") {
                if !m.is_empty() {
                    if let Some(i) = models.iter().position(|x| x == &m) {
                        model_index = i;
                    } else if m.contains('/') {
                        models.insert(0, m);
                        model_index = 0;
                    }
                }
            }
        }

        let mut first_session = ChatSession::new("Chat 1".to_string());
        if use_registry && models.is_empty() {
            first_session.last_error = Some(
                "No authenticated language models. Configure a provider in Quorp settings.".to_string(),
            );
        } else if let Some(e) = last_error {
            first_session.last_error = Some(e);
        }

        Self {
            http_client,
            runtime,
            ui_tx,
            sessions: vec![first_session],
            active_session: 0,
            unified_bridge_tx,
            models,
            model_index,
            viewport_transcript_lines: 1,
            api_base,
            api_key,
            last_text_width: 60,
            command_runner: CommandRunner::new(project_root.clone()),
            command_bridge_tx,
            project_root,
            path_index,
        }
    }

    fn uses_language_model_registry(&self) -> bool {
        self.unified_bridge_tx.is_some()
    }

    /// When the chat bridge is active, persist `provider/model` to Quorp agent settings and the global registry.
    pub fn request_persist_default_model_to_agent_settings(&self, registry_line: &str) {
        let Some(bridge_tx) = self.unified_bridge_tx.as_ref() else {
            return;
        };
        if SelectedModel::from_str(registry_line).is_err() {
            log::trace!(
                "chat: skip agent settings persist for non-registry model id `{registry_line}`"
            );
            return;
        }
        let _ = bridge_tx.unbounded_send(
            crate::quorp::tui::bridge::TuiToBackendRequest::PersistDefaultModel {
                registry_line: registry_line.to_string(),
            },
        );
    }

    fn build_language_model_request(&self) -> LanguageModelRequest {
        let mut messages = vec![LanguageModelRequestMessage {
            role: Role::System,
            content: vec![MessageContent::Text(SYSTEM_PROMPT.to_string())],
            cache: false,
            reasoning_details: None,
        }];
        let chat_messages = &self.active_session_ref().messages;
        let len = chat_messages.len();
        for (i, m) in chat_messages.iter().enumerate() {
            match m {
                ChatMessage::User(user_text) => {
                    messages.push(LanguageModelRequestMessage {
                        role: Role::User,
                        content: vec![MessageContent::Text(
                            expand_mentions_for_api_message(user_text, &self.project_root),
                        )],
                        cache: false,
                        reasoning_details: None,
                    });
                }
                ChatMessage::Assistant(assistant_text) => {
                    let is_trailing_empty = self.active_session_ref().streaming
                        && i + 1 == len
                        && assistant_text.is_empty();
                    if is_trailing_empty {
                        continue;
                    }
                    if !assistant_text.is_empty() {
                        messages.push(LanguageModelRequestMessage {
                            role: Role::Assistant,
                            content: vec![MessageContent::Text(assistant_text.clone())],
                            cache: false,
                            reasoning_details: None,
                        });
                    }
                }
            }
        }
        LanguageModelRequest {
            thread_id: None,
            prompt_id: None,
            intent: Some(CompletionIntent::UserPrompt),
            messages,
            tools: crate::quorp::tui::tui_tool_runtime::tui_chat_tools(),
            tool_choice: None,
            stop: vec![],
            temperature: None,
            thinking_allowed: true,
            thinking_effort: None,
            speed: None,
        }
    }

    fn enqueue_registry_completion(&mut self) {
        let Some(bridge_tx) = self.unified_bridge_tx.clone() else {
            log::error!("chat: registry completion requested without bridge");
            return;
        };
        let request = self.build_language_model_request();
        let preferred_model = SelectedModel::from_str(self.current_model_id()).ok();
        let cancel = Arc::new(AtomicBool::new(false));
        
        let session_id = self.active_session;
        self.active_session_mut().streaming_cancel = Some(cancel.clone());
        self.active_session_mut().streaming = true;
        
        if bridge_tx
            .unbounded_send(crate::quorp::tui::bridge::TuiToBackendRequest::StreamChat {
                request,
                preferred_model,
                cancel,
                session_id,
            })
            .is_err()
        {
            log::error!("chat: chat bridge send failed (disconnected)");
            self.active_session_mut().streaming_cancel = None;
            self.active_session_mut().streaming = false;
            let _ = self.ui_tx.send(crate::quorp::tui::TuiEvent::Chat(ChatUiEvent::Error(
                session_id,
                "Chat bridge disconnected.".to_string(),
            )));
            let _ = self
                .ui_tx
                .send(crate::quorp::tui::TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
        }
    }

    fn active_session_mut(&mut self) -> &mut ChatSession {
        &mut self.sessions[self.active_session]
    }

    fn active_session_ref(&self) -> &ChatSession {
        &self.sessions[self.active_session]
    }

    fn abort_streaming(&mut self) {
        let s = self.active_session_mut();
        if let Some(cancel) = s.streaming_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if let Some(h) = s.streaming_abort.take() {
            h.abort();
        }
        s.streaming = false;
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
        self.command_runner.set_project_root(self.project_root.clone());
    }

    /// Match production + [`TuiTestHarness::new_with_backend_state`]: bridge snapshots drive the index
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
                    s.streaming_abort = None;
                    s.streaming_cancel = None;
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
                    s.streaming_abort = None;
                    s.streaming_cancel = None;
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
                let context = format!(
                    "[Command Output]\n$ {}\n{}\n[End Output]",
                    s.command_output_lines.first().cloned().unwrap_or_default(),
                    output
                );
                s.messages.push(ChatMessage::User(context));
                s.messages.push(ChatMessage::Assistant(String::new()));
                s.command_output_lines.clear();
                
                if idx == self.active_session {
                    self.submit_input_for_followup(theme);
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
                let timeout = CommandRunner::parse_timeout(Some(&timeout_ms.to_string()));
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
            session.command_output_lines.clear();
        }
        // Production `quorp` TUI always wires `command_bridge_tx` (Quorp `Terminal` / task stack).
        // `command_runner` is only used when `ChatPane` is constructed without a bridge (flow tests, ui_lab).
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
                session.messages.push(ChatMessage::User(
                    "Command bridge disconnected; could not run command.".to_string(),
                ));
                session.messages.push(ChatMessage::Assistant(String::new()));
            }
        } else {
            self.command_runner.execute(
                idx,
                &cmd.command,
                cmd.timeout,
                self.ui_tx.clone(),
            );
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

    fn submit_input_for_followup(&mut self, _theme: &crate::quorp::tui::theme::Theme) {
        if self.active_session_ref().messages.is_empty() {
            return;
        }
        if self.uses_language_model_registry() && self.models.is_empty() {
            return;
        }
        self.active_session_mut().streaming = true;

        if self.uses_language_model_registry() {
            self.enqueue_registry_completion();
            return;
        }

        let request = self.build_open_ai_request();
        let api_key_effective = effective_api_key(&self.api_base, &self.api_key);
        let http = self.http_client.clone();
        let api_base = self.api_base.clone();
        let ui_tx = self.ui_tx.clone();

        let session_id = self.active_session;
        let task = self.runtime.spawn(async move {
            drive_chat_completion_stream(session_id, http, api_base, api_key_effective, request, ui_tx).await;
        });
        self.active_session_mut().streaming_abort = Some(task.abort_handle());
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

    fn requires_api_key(&self) -> bool {
        url::Url::parse(&self.api_base).ok().is_some_and(|url| {
            url.host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
        })
    }

    fn build_request_messages(&self) -> Vec<RequestMessage> {
        let mut out = vec![RequestMessage::System {
            content: open_ai::MessageContent::Plain(SYSTEM_PROMPT.to_string()),
        }];
        let messages = &self.active_session_ref().messages;
        let len = messages.len();
        for (i, m) in messages.iter().enumerate() {
            match m {
                ChatMessage::User(user_text) => {
                    out.push(RequestMessage::User {
                        content: open_ai::MessageContent::Plain(
                            expand_mentions_for_api_message(user_text, &self.project_root),
                        ),
                    });
                }
                ChatMessage::Assistant(assistant_text) => {
                    let is_trailing_empty = self.active_session_ref().streaming
                        && i + 1 == len
                        && assistant_text.is_empty();
                    if is_trailing_empty {
                        continue;
                    }
                    if !assistant_text.is_empty() {
                        out.push(RequestMessage::Assistant {
                            content: Some(open_ai::MessageContent::Plain(assistant_text.clone())),
                            tool_calls: Vec::new(),
                        });
                    }
                }
            }
        }
        out
    }

    fn build_open_ai_request(&self) -> open_ai::Request {
        open_ai::Request {
            model: self.current_model_id().to_string(),
            messages: self.build_request_messages(),
            stream: true,
            stream_options: Some(open_ai::StreamOptions::default()),
            max_completion_tokens: None,
            stop: Vec::new(),
            temperature: None,
            tool_choice: None,
            parallel_tool_calls: None,
            tools: Vec::new(),
            prompt_cache_key: None,
            reasoning_effort: None,
        }
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
            if let Some(s) = self.sessions.get_mut(index) {
                if let Some(cancel) = s.streaming_cancel.take() {
                    cancel.store(true, Ordering::Release);
                }
                if let Some(h) = s.streaming_abort.take() {
                    h.abort();
                }
                s.streaming = false;
            }
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
        self.abort_streaming();
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

        if !self.uses_language_model_registry() {
            if validate_api_base_url(&self.api_base).is_err() {
                let s = self.active_session_mut();
                s.messages.push(ChatMessage::User(trimmed));
                s.messages.push(ChatMessage::Assistant(
                    "Invalid API base URL. Use https:// for remote hosts or http://127.0.0.1 for local."
                        .to_string(),
                ));
                s.input.clear();
                s.cursor_char = 0;
                self.sync_mention_popup();
                self.scroll_transcript_to_bottom(theme);
                return;
            }

            if self.requires_api_key() && self.api_key.is_empty() {
                let s = self.active_session_mut();
                s.messages.push(ChatMessage::User(trimmed));
                s.messages.push(ChatMessage::Assistant(
                    "Error: OPENAI_API_KEY is not set.".to_string(),
                ));
                s.input.clear();
                s.cursor_char = 0;
                s.last_error = Some("OPENAI_API_KEY is not set".to_string());
                self.sync_mention_popup();
                self.scroll_transcript_to_bottom(theme);
                return;
            }
        }

        self.abort_streaming();

        let user_text = trimmed;
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

        if self.uses_language_model_registry() && self.models.is_empty() {
            if let Some(ChatMessage::Assistant(text)) = self.active_session_mut().messages.last_mut()
            {
                *text = "No authenticated language models. Configure a provider in Quorp settings."
                    .to_string();
            }
            self.scroll_transcript_to_bottom(theme);
            return;
        }

        if self.uses_language_model_registry() {
            self.active_session_mut().streaming = true;
            self.enqueue_registry_completion();
            self.scroll_transcript_to_bottom(theme);
            return;
        }

        let request = self.build_open_ai_request();
        let api_key_effective = effective_api_key(&self.api_base, &self.api_key);
        let http = self.http_client.clone();
        let api_base = self.api_base.clone();
        let ui_tx = self.ui_tx.clone();

        self.active_session_mut().streaming = true;

        // `send_chat_ui` may block this task when the UI queue is full; see `send_chat_ui`.
        let session_id = self.active_session;
        let task = self.runtime.spawn(async move {
            drive_chat_completion_stream(session_id, http, api_base, api_key_effective, request, ui_tx).await;
        });

        self.active_session_mut().streaming_abort = Some(task.abort_handle());
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
        self.active_session_mut().running_command = running;
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
    pub fn set_api_base_for_test(&mut self, base: String) {
        self.api_base = base;
    }

    #[cfg(test)]
    pub fn requires_api_key_for_test(&self) -> bool {
        self.requires_api_key()
    }

    #[cfg(test)]
    pub fn set_streaming_for_test(&mut self, streaming: bool) {
        self.active_session_mut().streaming = streaming;
    }

    #[cfg(test)]
    pub fn request_roles_and_contents_for_test(&self) -> Vec<(String, String)> {
        self.build_request_messages()
            .iter()
            .map(Self::request_message_role_content_for_test)
            .collect()
    }

    #[cfg(test)]
    fn request_message_role_content_for_test(message: &RequestMessage) -> (String, String) {
        match message {
            RequestMessage::System { content } => (
                "system".into(),
                plain_message_content_for_test(content),
            ),
            RequestMessage::User { content } => ("user".into(), plain_message_content_for_test(content)),
            RequestMessage::Assistant { content, .. } => (
                "assistant".into(),
                content
                    .as_ref()
                    .map(plain_message_content_for_test)
                    .unwrap_or_default(),
            ),
            RequestMessage::Tool { content, .. } => ("tool".into(), plain_message_content_for_test(content)),
        }
    }
}

#[cfg(test)]
fn plain_message_content_for_test(content: &open_ai::MessageContent) -> String {
    use open_ai::{MessageContent, MessagePart};
    match content {
        MessageContent::Plain(text) => text.clone(),
        MessageContent::Multipart(parts) => parts
            .iter()
            .filter_map(|part| {
                if let MessagePart::Text { text } = part {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    fn chat_pane_with_temp_root() -> ChatPane {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, _rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(8);
        let root = std::env::temp_dir();
        let path_index = Arc::new(PathIndex::new(root.clone()));
        ChatPane::new(tx, runtime.handle().clone(), root, path_index, None, None)
    }

    fn key_char(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn key_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn chat_with_indexed_project() -> (tokio::runtime::Runtime, ChatPane, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..12 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "").expect("write");
        }
        std::fs::write(dir.path().join("needle.txt"), "").expect("write");
        let path_index = Arc::new(PathIndex::new(dir.path().to_path_buf()));
        assert!(
            path_index.blocking_wait_for_ready(dir.path(), Duration::from_secs(8)),
            "path index"
        );
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, _rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(8);
        let chat = ChatPane::new(
            tx,
            runtime.handle().clone(),
            dir.path().to_path_buf(),
            path_index,
            None,
            None,
        );
        (runtime, chat, dir)
    }

    #[test]
    fn active_mention_token_requires_word_boundary_before_at() {
        let s = "user@host.com ";
        let cursor = s.trim_end().len();
        assert!(active_mention_token(s, cursor).is_none());
    }

    #[test]
    fn active_mention_token_after_whitespace() {
        let s = "hi @src";
        assert_eq!(
            active_mention_token(s, s.len()),
            Some((3usize, "src".to_string()))
        );
    }

    #[test]
    fn active_mention_token_rejects_space_inside_query() {
        let s = "hi @a b";
        let cursor = s.len();
        assert!(active_mention_token(s, cursor).is_none());
    }

    #[test]
    fn normalize_api_base_strips_slash() {
        assert_eq!(
            normalize_api_base(" https://api.openai.com/v1/ "),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn validate_api_base_rejects_insecure_remote_http() {
        assert!(validate_api_base_url("http://api.openai.com/v1").is_err());
    }

    #[test]
    fn validate_api_base_rejects_localhost_prefix_spoof() {
        assert!(validate_api_base_url("http://localhost.evil.com/v1").is_err());
    }

    #[test]
    fn validate_api_base_allows_localhost_http() {
        assert!(validate_api_base_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_api_base_url("http://localhost:8080/v1").is_ok());
        assert!(validate_api_base_url("http://[::1]:8080/v1").is_ok());
    }

    #[test]
    fn validate_api_base_allows_https() {
        assert!(validate_api_base_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn sse_data_payload_skips_empty() {
        assert_eq!(sse_data_payload(""), None);
        assert_eq!(sse_data_payload("data: "), None);
        assert_eq!(sse_data_payload("data: {}").unwrap(), "{}");
        assert_eq!(sse_data_payload("data:[DONE]").unwrap(), "[DONE]");
    }

    #[test]
    fn build_request_messages_skips_trailing_empty_assistant_while_streaming() {
        let mut pane = chat_pane_with_temp_root();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("hello".to_string()),
            ChatMessage::Assistant(String::new()),
        ]);
        pane.set_streaming_for_test(true);
        let rows = pane.request_roles_and_contents_for_test();
        let non_system: Vec<_> = rows.into_iter().filter(|(r, _)| r != "system").collect();
        assert_eq!(
            non_system,
            vec![("user".to_string(), "hello".to_string())],
            "in-flight empty assistant must not be sent to the API"
        );
        pane.set_streaming_for_test(false);
        let rows = pane.request_roles_and_contents_for_test();
        let non_system: Vec<_> = rows.into_iter().filter(|(r, _)| r != "system").collect();
        assert_eq!(non_system, vec![("user".to_string(), "hello".to_string())]);
    }

    #[test]
    fn build_request_messages_includes_nonempty_assistant_while_streaming() {
        let mut pane = chat_pane_with_temp_root();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("hello".to_string()),
            ChatMessage::Assistant("partial".to_string()),
        ]);
        pane.set_streaming_for_test(true);
        let rows = pane.request_roles_and_contents_for_test();
        let non_system: Vec<_> = rows.into_iter().filter(|(r, _)| r != "system").collect();
        assert_eq!(
            non_system,
            vec![
                ("user".to_string(), "hello".to_string()),
                ("assistant".to_string(), "partial".to_string()),
            ]
        );
    }

    #[test]
    fn apply_chat_event_assistant_delta_appends() {
        let mut pane = chat_pane_with_temp_root();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("hi".to_string()),
            ChatMessage::Assistant(String::new()),
        ]);
        pane.set_streaming_for_test(true);
        pane.apply_chat_event(ChatUiEvent::AssistantDelta(0, "world".to_string()), &crate::quorp::tui::theme::Theme::antigravity());
        assert_eq!(pane.last_assistant_text_for_test(), Some("world"));
        pane.apply_chat_event(ChatUiEvent::StreamFinished(0), &crate::quorp::tui::theme::Theme::antigravity());
        assert!(!pane.is_streaming());
    }

    #[test]
    fn requires_api_key_matches_openai_host_not_substring() {
        let mut pane = chat_pane_with_temp_root();
        pane.set_api_base_for_test("https://proxy.example.com/v1".to_string());
        assert!(
            !pane.requires_api_key_for_test(),
            "substring must not imply OpenAI host"
        );
        pane.set_api_base_for_test("https://api.openai.com/v1".to_string());
        assert!(pane.requires_api_key_for_test());
    }

    #[test]
    fn chunked_sse_lines() {
        let mut buf = String::new();
        let mut lines_out = Vec::new();
        let json_line = r#"{"choices":[{"delta":{"content":"hi"}}]}"#;
        let full = format!("data: {json_line}\ndata: [DONE]\n");
        for chunk in [full.as_str()] {
            buf.push_str(chunk);
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf.drain(..=pos);
                if let Some(p) = sse_data_payload(&line) {
                    lines_out.push(p.to_string());
                }
            }
        }
        assert!(lines_out.iter().any(|s| s.contains("choices")));
        assert!(lines_out.iter().any(|s| *s == "[DONE]"));

        let mut buf2 = String::new();
        let mut lines2 = Vec::new();
        let s = format!("data: {json_line}\n");
        let mid = s.len() / 2;
        for chunk in [&s[..mid], &s[mid..], "data: [DONE]\n"] {
            buf2.push_str(chunk);
            while let Some(pos) = buf2.find('\n') {
                let line = buf2[..pos].to_string();
                buf2.drain(..=pos);
                if let Some(p) = sse_data_payload(&line) {
                    lines2.push(p.to_string());
                }
            }
        }
        assert_eq!(lines2, lines_out);
    }

    #[test]
    fn chat_messages_capped_at_max() {
        let mut pane = chat_pane_with_temp_root();
        pane.api_base = "https://api.openai.com/v1".to_string();
        pane.api_key = "test".to_string();

        for i in 0..600 {
            pane.set_input_for_test(&format!("Message {}", i));
            pane.submit_input(&crate::quorp::tui::theme::Theme::antigravity());
            // submit_input sets streaming to true. Reset it so we can submit again.
            pane.active_session_mut().streaming = false;
        }

        let msg_len = pane.active_session_ref().messages.len();
        assert!(msg_len <= super::MAX_MESSAGES);
        assert_eq!(msg_len, super::MAX_MESSAGES);
    }

    #[test]
    fn new_chat_session_does_not_see_prior_session_assistant() {
        let mut pane = chat_pane_with_temp_root();
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        pane.seed_messages_for_test(vec![
            ChatMessage::User("u".to_string()),
            ChatMessage::Assistant("prior".to_string()),
        ]);
        pane.new_chat_session(&theme);
        assert_eq!(pane.active_session_index(), 1);
        assert_eq!(pane.last_assistant_text_for_test(), None);
    }

    #[test]
    fn close_chat_session_keeps_other_tab_intact() {
        let mut pane = chat_pane_with_temp_root();
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        pane.new_chat_session(&theme);
        pane.seed_messages_for_test(vec![ChatMessage::User("tab-one".to_string())]);
        assert!(pane.activate_chat_session(0, &theme));
        pane.seed_messages_for_test(vec![ChatMessage::User("tab-zero".to_string())]);
        assert!(pane.activate_chat_session(1, &theme));
        assert!(pane.close_chat_session_at(1, &theme));
        assert_eq!(pane.active_session_index(), 0);
        assert!(pane
            .active_session_ref()
            .messages
            .iter()
            .any(|m| matches!(m, ChatMessage::User(s) if s == "tab-zero")));
    }

    #[test]
    fn close_all_chat_sessions_leaves_one_fresh_tab() {
        let mut pane = chat_pane_with_temp_root();
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        pane.new_chat_session(&theme);
        pane.close_all_chat_sessions(&theme);
        assert_eq!(pane.active_session_index(), 0);
        pane.new_chat_session(&theme);
        assert_eq!(pane.active_session_index(), 1);
    }

    #[test]
    fn mention_at_opens_popup_with_matches() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        let (_rt, mut chat, _dir) = chat_with_indexed_project();
        assert!(chat.handle_key_event(&key_char('@'), &theme));
        assert!(chat.mention_popup_open_for_test());
        assert!(chat.mention_match_count_for_test() > 0);
    }

    #[test]
    fn mention_filter_narrows_list() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        let (_rt, mut chat, _dir) = chat_with_indexed_project();
        chat.handle_key_event(&key_char('@'), &theme);
        let full = chat.mention_match_count_for_test();
        for c in "needle".chars() {
            chat.handle_key_event(&key_char(c), &theme);
        }
        assert!(chat.mention_match_count_for_test() <= full);
        assert!(chat
            .mention_selected_label_for_test()
            .is_some_and(|s| s.contains("needle")));
    }

    #[test]
    fn mention_down_past_first_page_advances_scroll() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        let (_rt, mut chat, _dir) = chat_with_indexed_project();
        chat.handle_key_event(&key_char('@'), &theme);
        for c in "f".chars() {
            chat.handle_key_event(&key_char(c), &theme);
        }
        assert!(chat.mention_match_count_for_test() > 8);
        for _ in 0..8 {
            chat.handle_key_event(&key_code(KeyCode::Down), &theme);
        }
        assert!(
            chat.mention_scroll_top_for_test().unwrap_or(0) > 0,
            "expected popup to scroll after many downs"
        );
    }

    #[test]
    fn mention_tab_inserts_file_link() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        let (_rt, mut chat, _dir) = chat_with_indexed_project();
        chat.handle_key_event(&key_char('@'), &theme);
        for c in "needle".chars() {
            chat.handle_key_event(&key_char(c), &theme);
        }
        assert!(chat.handle_key_event(&key_code(KeyCode::Tab), &theme));
        assert!(chat.input_for_test().contains("needle.txt"));
    }

    #[test]
    fn mention_esc_dismisses_popup() {
        let theme = crate::quorp::tui::theme::Theme::antigravity();
        let (_rt, mut chat, _dir) = chat_with_indexed_project();
        chat.handle_key_event(&key_char('@'), &theme);
        assert!(chat.mention_popup_open_for_test());
        assert!(chat.handle_key_event(&key_code(KeyCode::Esc), &theme));
        assert!(!chat.mention_popup_open_for_test());
    }

    #[test]
    fn build_request_expands_file_mention_in_user_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("attached.txt");
        std::fs::write(&f, "secret-body").expect("write");
        let path_index = Arc::new(PathIndex::new(dir.path().to_path_buf()));
        assert!(path_index.blocking_wait_for_ready(dir.path(), Duration::from_secs(8)));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, _rx) = std::sync::mpsc::sync_channel::<crate::quorp::tui::TuiEvent>(8);
        let mut pane = ChatPane::new(
            tx,
            runtime.handle().clone(),
            dir.path().to_path_buf(),
            path_index,
            None,
            None,
        );
        let link = crate::quorp::tui::mention_links::mention_link_for_path(&f, "attached.txt").expect("link");
        pane.seed_messages_for_test(vec![ChatMessage::User(format!("see {link}"))]);
        let rows = pane.request_roles_and_contents_for_test();
        let user = rows.iter().find(|(r, _)| r == "user").map(|(_, c)| c.as_str());
        assert!(
            user.is_some_and(|c| c.contains("secret-body")),
            "{user:?}"
        );
    }
}
