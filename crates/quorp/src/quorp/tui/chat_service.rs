use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::model_registry;
use crate::quorp::tui::ssd_moe_client::{
    SsdMoeChatMessage, SsdMoeChatRequest, SsdMoeClientConfig, SsdMoeStreamEvent,
    build_request_body, chat_completions_url, local_bearer_token, parse_sse_data_line,
    parse_sse_payload,
};
use crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_COALESCE_WINDOW: Duration = Duration::from_millis(35);
const SYSTEM_PROMPT: &str = r#"You are Quorp, a rich terminal coding assistant.

You are working inside a real project checkout. Be concrete and concise.
When the transcript includes command output, use it directly instead of inventing results.
If you want the user to run a command, respond with:
<run_command timeout_ms="30000">your command here</run_command>

Do not emit <run_command> unless you really want the terminal command path to execute."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatServiceRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatServiceMessage {
    pub role: ChatServiceRole,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum ChatServiceRequest {
    SubmitPrompt {
        session_id: usize,
        model_id: String,
        latest_input: String,
        messages: Vec<ChatServiceMessage>,
        project_root: PathBuf,
        base_url_override: Option<String>,
    },
    Cancel {
        session_id: usize,
    },
    SummarizeCommandOutput {
        session_id: usize,
        model_id: String,
        command: String,
        command_output: String,
        messages: Vec<ChatServiceMessage>,
        project_root: PathBuf,
        base_url_override: Option<String>,
    },
}

pub fn spawn_chat_service_loop(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
) -> futures::channel::mpsc::UnboundedSender<ChatServiceRequest> {
    let (request_tx, mut request_rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                    0,
                    format!("Failed to start native chat runtime: {error}"),
                )));
                return;
            }
        };
        let ssd_moe_runtime = SsdMoeRuntimeHandle::shared_handle();
        let active_streams: Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>> =
            Arc::new(Mutex::new(HashMap::new()));

        runtime.block_on(async move {
            while let Some(request) = request_rx.next().await {
                match request {
                    ChatServiceRequest::Cancel { session_id } => {
                        cancel_stream(&active_streams, session_id);
                    }
                    ChatServiceRequest::SubmitPrompt {
                        session_id,
                        model_id,
                        latest_input,
                        messages,
                        project_root,
                        base_url_override,
                    } => {
                        let trimmed = latest_input.trim();
                        if let Some(command) = parse_inline_command(trimmed) {
                            let response = format!(
                                "Command request queued for confirmation.\n<run_command timeout_ms=\"30000\">{command}</run_command>"
                            );
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
                                session_id,
                                response,
                            )));
                            let _ = event_tx
                                .send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
                            continue;
                        }
                        if trimmed.is_empty() {
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                                session_id,
                                "Chat input was empty.".to_string(),
                            )));
                            continue;
                        }
                        if trimmed.contains("<run_command") {
                            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(
                                session_id,
                                "Direct `<run_command>` blocks are no longer accepted in chat input. Use `/run <command>` instead.".to_string(),
                            )));
                            continue;
                        }

                        cancel_stream(&active_streams, session_id);
                        spawn_stream_task(
                            event_tx.clone(),
                            active_streams.clone(),
                            ssd_moe_runtime.clone(),
                            StreamRequest {
                                session_id,
                                model_id,
                                messages,
                                project_root,
                                base_url_override,
                            },
                        );
                    }
                    ChatServiceRequest::SummarizeCommandOutput {
                        session_id,
                        model_id,
                        command,
                        command_output,
                        messages,
                        project_root,
                        base_url_override,
                    } => {
                        let _ = command_output;
                        let _ = command;
                        cancel_stream(&active_streams, session_id);
                        spawn_stream_task(
                            event_tx.clone(),
                            active_streams.clone(),
                            ssd_moe_runtime.clone(),
                            StreamRequest {
                                session_id,
                                model_id,
                                messages,
                                project_root,
                                base_url_override,
                            },
                        );
                    }
                }
            }
        });
    });
    request_tx
}

#[derive(Debug, Clone)]
struct StreamRequest {
    session_id: usize,
    model_id: String,
    messages: Vec<ChatServiceMessage>,
    project_root: PathBuf,
    base_url_override: Option<String>,
}

fn cancel_stream(
    active_streams: &Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    session_id: usize,
) {
    if let Some(handle) = active_streams
        .lock()
        .expect("chat stream map lock")
        .remove(&session_id)
    {
        handle.abort();
    }
}

fn spawn_stream_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    active_streams: Arc<Mutex<HashMap<usize, tokio::task::AbortHandle>>>,
    ssd_moe_runtime: crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: StreamRequest,
) {
    let session_id = request.session_id;
    let task = tokio::spawn(async move {
        if let Err(error) = run_stream_request(event_tx.clone(), ssd_moe_runtime, request).await {
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(session_id, error)));
            let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
        }
    });
    active_streams
        .lock()
        .expect("chat stream map lock")
        .insert(session_id, task.abort_handle());
}

async fn run_stream_request(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    ssd_moe_runtime: crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: StreamRequest,
) -> Result<(), String> {
    let client_config =
        resolve_client_config(&ssd_moe_runtime, &request).map_err(|error| error.to_string())?;
    let request_body = build_request_body(
        &client_config,
        &SsdMoeChatRequest {
            messages: build_request_messages(&request.messages),
            max_tokens: Some(4096),
            reasoning_effort: reasoning_effort_for_model(&request.model_id),
        },
    );
    let url = chat_completions_url(&client_config.base_url).map_err(|error| error.to_string())?;
    let bearer_token =
        local_bearer_token(&client_config.base_url).map_err(|error| error.to_string())?;

    let http_client = reqwest::Client::builder()
        .connect_timeout(client_config.connect_timeout)
        .read_timeout(client_config.read_timeout)
        .build()
        .map_err(|error| format!("Failed to build loopback HTTP client: {error}"))?;

    let response = http_client
        .post(url)
        .bearer_auth(bearer_token)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&request_body)
        .send()
        .await
        .map_err(|error| format!("Failed to connect to SSD-MOE: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<body unavailable>".to_string());
        return Err(format!(
            "SSD-MOE returned {} while starting chat stream: {}",
            status,
            body.trim()
        ));
    }

    stream_response_to_ui(response, request.session_id, &event_tx).await
}

fn resolve_client_config(
    ssd_moe_runtime: &crate::quorp::tui::ssd_moe_tui::SsdMoeRuntimeHandle,
    request: &StreamRequest,
) -> anyhow::Result<SsdMoeClientConfig> {
    let base_url = if let Some(base_url_override) = request.base_url_override.as_ref() {
        crate::quorp::tui::ssd_moe_client::validate_loopback_base_url(base_url_override)
            .map_err(anyhow::Error::msg)?;
        base_url_override.trim().trim_end_matches('/').to_string()
    } else {
        let model = model_registry::local_moe_spec_for_registry_id(&request.model_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown local SSD-MOE model `{}`", request.model_id))?;
        ssd_moe_runtime.ensure_running(&request.project_root, &model);
        ssd_moe_runtime
            .wait_until_ready(SERVER_READY_TIMEOUT)
            .map_err(anyhow::Error::msg)?;
        ssd_moe_runtime.base_url()
    };
    Ok(SsdMoeClientConfig {
        base_url,
        model_id: request.model_id.clone(),
        connect_timeout: CONNECT_TIMEOUT,
        read_timeout: READ_TIMEOUT,
    })
}

fn build_request_messages(messages: &[ChatServiceMessage]) -> Vec<SsdMoeChatMessage> {
    let mut request_messages = vec![SsdMoeChatMessage {
        role: "system",
        content: SYSTEM_PROMPT.to_string(),
    }];
    request_messages.extend(messages.iter().filter_map(|message| {
        if message.content.trim().is_empty() {
            return None;
        }
        Some(SsdMoeChatMessage {
            role: match message.role {
                ChatServiceRole::User => "user",
                ChatServiceRole::Assistant => "assistant",
            },
            content: message.content.clone(),
        })
    }));
    request_messages
}

fn reasoning_effort_for_model(model_id: &str) -> Option<String> {
    model_registry::local_moe_spec_for_registry_id(model_id)
        .filter(|model| model.has_think_tokens)
        .map(|_| "medium".to_string())
}

async fn stream_response_to_ui(
    response: reqwest::Response,
    session_id: usize,
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
) -> Result<(), String> {
    let mut bytes_stream = response.bytes_stream();
    let mut buffered_text = String::new();
    let mut line_buffer = String::new();
    let mut sent_first_delta = false;
    let mut reasoning_header_sent = false;
    let flush_timer = tokio::time::sleep(Duration::from_secs(3600));
    tokio::pin!(flush_timer);
    let mut flush_armed = false;
    let mut stream_finished = false;

    loop {
        tokio::select! {
            _ = &mut flush_timer, if flush_armed => {
                flush_buffered_text(event_tx, session_id, &mut buffered_text);
                flush_armed = false;
            }
            next_chunk = bytes_stream.next() => {
                let Some(next_chunk) = next_chunk else {
                    break;
                };
                let bytes = next_chunk
                    .map_err(|error| format!("SSD-MOE stream error: {error}"))?;
                let chunk_text = String::from_utf8_lossy(&bytes);
                line_buffer.push_str(&chunk_text);

                while let Some(newline_index) = line_buffer.find('\n') {
                    let line = line_buffer[..newline_index].to_string();
                    line_buffer.drain(..=newline_index);
                    let Some(payload) = parse_sse_data_line(&line) else {
                        continue;
                    };
                    let events = parse_sse_payload(payload)?;
                    for event in events {
                        match event {
                            SsdMoeStreamEvent::TextDelta(fragment) => {
                                queue_fragment(
                                    event_tx,
                                    session_id,
                                    &fragment,
                                    &mut buffered_text,
                                    &mut sent_first_delta,
                                    &mut flush_timer,
                                    &mut flush_armed,
                                );
                            }
                            SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                                let reasoning_fragment = if reasoning_header_sent {
                                    fragment
                                } else {
                                    reasoning_header_sent = true;
                                    format!("\n[Reasoning]\n{fragment}")
                                };
                                queue_fragment(
                                    event_tx,
                                    session_id,
                                    &reasoning_fragment,
                                    &mut buffered_text,
                                    &mut sent_first_delta,
                                    &mut flush_timer,
                                    &mut flush_armed,
                                );
                            }
                            SsdMoeStreamEvent::Finished => {
                                stream_finished = true;
                            }
                        }
                    }
                }

                if stream_finished {
                    break;
                }
            }
        }
    }

    if let Some(payload) = parse_sse_data_line(&line_buffer) {
        for event in parse_sse_payload(payload)? {
            match event {
                SsdMoeStreamEvent::TextDelta(fragment) => queue_fragment(
                    event_tx,
                    session_id,
                    &fragment,
                    &mut buffered_text,
                    &mut sent_first_delta,
                    &mut flush_timer,
                    &mut flush_armed,
                ),
                SsdMoeStreamEvent::ReasoningDelta(fragment) => {
                    let reasoning_fragment = if reasoning_header_sent {
                        fragment
                    } else {
                        format!("\n[Reasoning]\n{fragment}")
                    };
                    queue_fragment(
                        event_tx,
                        session_id,
                        &reasoning_fragment,
                        &mut buffered_text,
                        &mut sent_first_delta,
                        &mut flush_timer,
                        &mut flush_armed,
                    );
                }
                SsdMoeStreamEvent::Finished => {}
            }
        }
    }

    flush_buffered_text(event_tx, session_id, &mut buffered_text);
    let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::StreamFinished(session_id)));
    Ok(())
}

fn queue_fragment(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    fragment: &str,
    buffered_text: &mut String,
    sent_first_delta: &mut bool,
    flush_timer: &mut std::pin::Pin<&mut tokio::time::Sleep>,
    flush_armed: &mut bool,
) {
    if fragment.is_empty() {
        return;
    }
    if !*sent_first_delta {
        let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(
            session_id,
            fragment.to_string(),
        )));
        *sent_first_delta = true;
        return;
    }

    buffered_text.push_str(fragment);
    if should_flush_immediately(fragment) {
        flush_buffered_text(event_tx, session_id, buffered_text);
        *flush_armed = false;
        return;
    }

    flush_timer
        .as_mut()
        .reset(tokio::time::Instant::now() + TOKEN_COALESCE_WINDOW);
    *flush_armed = true;
}

fn should_flush_immediately(fragment: &str) -> bool {
    fragment.contains('\n')
        || fragment.ends_with('.')
        || fragment.ends_with('!')
        || fragment.ends_with('?')
        || fragment.ends_with(':')
}

fn flush_buffered_text(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    buffered_text: &mut String,
) {
    if buffered_text.is_empty() {
        return;
    }
    let delta = std::mem::take(buffered_text);
    let _ = event_tx.send(TuiEvent::Chat(ChatUiEvent::AssistantDelta(session_id, delta)));
}

fn parse_inline_command(input: &str) -> Option<String> {
    if let Some(command) = input.strip_prefix("/run ") {
        let trimmed = command.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(command) = input.strip_prefix('!') {
        let trimmed = command.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_mock_sse_server(response_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).expect("read request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            socket.write_all(response.as_bytes()).expect("write response");
        });
        format!("http://127.0.0.1:{}/v1", address.port())
    }

    #[test]
    fn parse_inline_command_supports_run_prefix() {
        assert_eq!(
            parse_inline_command("/run cargo test"),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn parse_inline_command_supports_bang_prefix() {
        assert_eq!(parse_inline_command("!ls -la"), Some("ls -la".to_string()));
    }

    #[test]
    fn submit_prompt_streams_loopback_sse() {
        let base_url = spawn_mock_sse_server(concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(32);
        let request_tx = spawn_chat_service_loop(event_tx);
        request_tx
            .unbounded_send(ChatServiceRequest::SubmitPrompt {
                session_id: 7,
                model_id: "qwen35-35b-a3b".to_string(),
                latest_input: "hello".to_string(),
                messages: vec![ChatServiceMessage {
                    role: ChatServiceRole::User,
                    content: "hello".to_string(),
                }],
                project_root: PathBuf::from("/tmp"),
                base_url_override: Some(base_url),
            })
            .expect("send request");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_delta = false;
        let mut saw_finish = false;
        while std::time::Instant::now() < deadline {
            if let Ok(event) = event_rx.recv_timeout(Duration::from_millis(100)) {
                match event {
                    TuiEvent::Chat(ChatUiEvent::AssistantDelta(7, text))
                        if text.contains("hello") =>
                    {
                        saw_delta = true;
                    }
                    TuiEvent::Chat(ChatUiEvent::StreamFinished(7)) => {
                        saw_finish = true;
                        break;
                    }
                    TuiEvent::Chat(ChatUiEvent::Error(7, error)) => {
                        panic!("unexpected stream error: {error}");
                    }
                    _ => {}
                }
            }
        }

        assert!(saw_delta, "expected assistant delta from local SSE server");
        assert!(saw_finish, "expected stream finished event from local SSE server");
    }

    #[test]
    fn cancel_request_aborts_stream_before_tokens_arrive() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).expect("read request");
            thread::sleep(Duration::from_millis(400));
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late-token\"},\"finish_reason\":null}]}\n\n",
                "data: [DONE]\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).expect("write response");
        });

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(32);
        let request_tx = spawn_chat_service_loop(event_tx);
        request_tx
            .unbounded_send(ChatServiceRequest::SubmitPrompt {
                session_id: 3,
                model_id: "qwen35-35b-a3b".to_string(),
                latest_input: "hello".to_string(),
                messages: vec![ChatServiceMessage {
                    role: ChatServiceRole::User,
                    content: "hello".to_string(),
                }],
                project_root: PathBuf::from("/tmp"),
                base_url_override: Some(format!("http://127.0.0.1:{}/v1", address.port())),
            })
            .expect("send request");
        request_tx
            .unbounded_send(ChatServiceRequest::Cancel { session_id: 3 })
            .expect("cancel request");

        let deadline = std::time::Instant::now() + Duration::from_millis(700);
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(TuiEvent::Chat(ChatUiEvent::AssistantDelta(3, text))) => {
                    panic!("unexpected streamed token after cancel: {text}");
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("event channel error after cancel: {error}"),
            }
        }
    }
}
