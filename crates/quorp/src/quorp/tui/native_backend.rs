#[cfg(target_os = "macos")]
use std::ffi::{CStr, OsString};
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TrySendError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::agent_context::{
    McpServerConfig, load_agent_config, validation_commands_for_plan,
};
use crate::quorp::tui::agent_protocol::{
    ActionOutcome, AgentAction, PreviewEditPayload, TomlEditOperation,
};
use crate::quorp::tui::bridge::{TerminalFrame, TuiKeystroke, TuiToBackendRequest};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::editor_pane::buffer_snapshot_from_disk;
use crate::quorp::tui::file_tree::{DirectoryListing, read_children};
use crate::quorp::tui::path_index::{
    build_repo_capsule, render_repo_capsule, render_symbol_search_hits, render_text_search_hits,
    search_repo_symbols, search_repo_text,
};
use crate::quorp::tui::terminal_trace::{
    SharedTerminalTraceBuffer, new_shared_terminal_trace, record_trace,
};
use quorp_agent_core::{ReadFileRange, stable_content_hash};

const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;
const TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL: Duration = Duration::from_millis(16);
const TERMINAL_UNFOCUSED_FRAME_PUBLISH_INTERVAL: Duration = Duration::from_millis(64);
const TERMINAL_METADATA_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const TERMINAL_SCROLLBACK_LINES: usize = 5_000;
const FILE_READ_LIMIT_BYTES: usize = 64 * 1024;
const FILE_READ_TRUNCATION_MARKER: &str = "\n[output truncated]";
const DIRECTORY_LIST_LIMIT: usize = 512;
const DIRECTORY_NAME_LIMIT: usize = 80;

type SessionShadowFiles = std::collections::HashMap<PathBuf, Option<String>>;
type ShadowWorktree = std::collections::HashMap<usize, SessionShadowFiles>;
type PreviewCache = std::collections::VecDeque<PreviewRecord>;

static SHADOW_WORKTREE: std::sync::OnceLock<Mutex<ShadowWorktree>> = std::sync::OnceLock::new();
static PREVIEW_CACHE: std::sync::OnceLock<Mutex<PreviewCache>> = std::sync::OnceLock::new();
const PREVIEW_CACHE_LIMIT: usize = 32;

#[derive(Debug, Clone)]
struct PreviewRecord {
    preview_id: String,
    path: String,
    target_path: PathBuf,
    base_hash: String,
    edit_kind: String,
    updated_content: String,
    syntax_status: String,
}

fn get_shadow_worktree() -> &'static Mutex<ShadowWorktree> {
    SHADOW_WORKTREE.get_or_init(|| Mutex::new(ShadowWorktree::new()))
}

fn get_preview_cache() -> &'static Mutex<PreviewCache> {
    PREVIEW_CACHE.get_or_init(|| Mutex::new(PreviewCache::new()))
}

fn store_preview_record(mut record: PreviewRecord) -> anyhow::Result<String> {
    let seed = format!(
        "{}\n{}\n{}\n{}",
        record.path, record.base_hash, record.edit_kind, record.updated_content
    );
    record.preview_id = format!("pv_{}", &stable_content_hash(&seed)[..12]);
    let preview_id = record.preview_id.clone();
    let mut cache = get_preview_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("preview cache lock poisoned"))?;
    cache.retain(|existing| existing.preview_id != preview_id);
    cache.push_back(record);
    while cache.len() > PREVIEW_CACHE_LIMIT {
        cache.pop_front();
    }
    Ok(preview_id)
}

fn load_preview_record(preview_id: &str) -> anyhow::Result<PreviewRecord> {
    let cache = get_preview_cache()
        .lock()
        .map_err(|_| anyhow::anyhow!("preview cache lock poisoned"))?;
    cache
        .iter()
        .find(|record| record.preview_id == preview_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("preview_id `{preview_id}` was not found or has expired"))
}

fn stash_file_for_rollback(session_id: usize, path: &PathBuf) {
    let content = std::fs::read_to_string(path).ok();
    if let Ok(mut tree) = get_shadow_worktree().lock() {
        let session_stash = tree.entry(session_id).or_default();
        if !session_stash.contains_key(path) {
            session_stash.insert(path.clone(), content);
        }
    } else {
        log::error!("tui: failed to lock rollback stash for session {session_id}");
    }
}

fn rollback_session_worktree(session_id: usize) {
    let Ok(mut tree) = get_shadow_worktree().lock() else {
        log::error!("tui: failed to lock rollback stash for session {session_id}");
        return;
    };
    if let Some(session_stash) = tree.remove(&session_id) {
        for (path, content) in session_stash {
            match content {
                Some(data) => {
                    if let Err(error) = std::fs::write(&path, data) {
                        log::error!("tui: failed to restore {}: {error}", path.display());
                    }
                }
                None => {
                    if let Err(error) = std::fs::remove_file(&path) {
                        log::error!(
                            "tui: failed to remove {} during rollback: {error}",
                            path.display()
                        );
                    }
                }
            }
        }
    }
}

fn clear_session_worktree(session_id: usize) {
    match get_shadow_worktree().lock() {
        Ok(mut tree) => {
            tree.remove(&session_id);
        }
        Err(error) => {
            log::error!("tui: failed to clear rollback stash for session {session_id}: {error}");
        }
    }
}

pub fn spawn_native_backend_loop(
    workspace_root: PathBuf,
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    mut request_rx: futures::channel::mpsc::UnboundedReceiver<TuiToBackendRequest>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut terminal = NativeTerminalService::new(event_tx.clone(), workspace_root.clone());
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
                    TuiToBackendRequest::TerminalFocusChanged { focused } => {
                        if let Err(error) = terminal.set_terminal_focused(focused) {
                            log::error!("tui: terminal focus update failed: {error:#}");
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
                    TuiToBackendRequest::TerminalScrollPageUp => {
                        if let Err(error) = terminal.scroll_page_up() {
                            log::error!("tui: terminal scroll page up failed: {error:#}");
                        }
                    }
                    TuiToBackendRequest::TerminalScrollPageDown => {
                        if let Err(error) = terminal.scroll_page_down() {
                            log::error!("tui: terminal scroll page down failed: {error:#}");
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
                    CommandBridgeRequest::ExecuteAction {
                        session_id,
                        action,
                        cwd,
                        project_root,
                        responder,
                        enable_rollback_on_validation_failure,
                    } => match action {
                        AgentAction::RunCommand {
                            command,
                            timeout_ms,
                        } => {
                            spawn_run_command_task(
                                event_tx.clone(),
                                session_id,
                                command,
                                cwd,
                                project_root,
                                Duration::from_millis(timeout_ms),
                                responder,
                            );
                        }
                        AgentAction::ReadFile { path, range } => {
                            spawn_read_file_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                range,
                                responder,
                            );
                        }
                        AgentAction::ListDirectory { path } => {
                            spawn_list_directory_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                responder,
                            );
                        }
                        AgentAction::SearchText { query, limit } => {
                            spawn_search_text_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                query,
                                limit,
                                responder,
                            );
                        }
                        AgentAction::SearchSymbols { query, limit } => {
                            spawn_search_symbols_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                query,
                                limit,
                                responder,
                            );
                        }
                        AgentAction::GetRepoCapsule { query, limit } => {
                            spawn_repo_capsule_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                query,
                                limit,
                                responder,
                            );
                        }
                        AgentAction::ExplainValidationFailure { command, output } => {
                            spawn_explain_validation_failure_task(
                                event_tx.clone(),
                                session_id,
                                command,
                                output,
                                responder,
                            );
                        }
                        AgentAction::SuggestImplementationTargets {
                            command,
                            output,
                            failing_path,
                            failing_line,
                        } => {
                            spawn_suggest_implementation_targets_task(
                                event_tx.clone(),
                                session_id,
                                SuggestImplementationTargetsTaskRequest {
                                    command,
                                    output,
                                    failing_path,
                                    failing_line,
                                    responder,
                                },
                            );
                        }
                        AgentAction::SuggestEditAnchors {
                            path,
                            range,
                            search_hint,
                        } => {
                            spawn_suggest_edit_anchors_task(
                                event_tx.clone(),
                                session_id,
                                SuggestEditAnchorsTaskRequest {
                                    cwd,
                                    project_root,
                                    path,
                                    range,
                                    search_hint,
                                    responder,
                                },
                            );
                        }
                        AgentAction::PreviewEdit { path, edit } => {
                            spawn_preview_edit_task(
                                event_tx.clone(),
                                session_id,
                                PreviewEditTaskRequest {
                                    cwd,
                                    project_root,
                                    path,
                                    edit,
                                    responder,
                                },
                            );
                        }
                        AgentAction::ReplaceRange {
                            path,
                            range,
                            expected_hash,
                            replacement,
                        } => {
                            spawn_replace_range_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                range,
                                expected_hash,
                                replacement,
                                responder,
                            );
                        }
                        AgentAction::ModifyToml {
                            path,
                            expected_hash,
                            operations,
                        } => {
                            spawn_modify_toml_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                expected_hash,
                                operations,
                                responder,
                            );
                        }
                        AgentAction::ApplyPreview { preview_id } => {
                            spawn_apply_preview_task(
                                event_tx.clone(),
                                session_id,
                                preview_id,
                                responder,
                            );
                        }
                        AgentAction::McpCallTool {
                            server_name,
                            tool_name,
                            arguments,
                        } => {
                            spawn_mcp_call_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                server_name,
                                tool_name,
                                arguments,
                                responder,
                            );
                        }
                        AgentAction::WriteFile { path, content } => {
                            spawn_write_file_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                content,
                                responder,
                            );
                        }
                        AgentAction::ApplyPatch { path, patch } => {
                            spawn_apply_patch_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                patch,
                                responder,
                            );
                        }
                        AgentAction::RunValidation { plan } => {
                            spawn_run_validation_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                plan,
                                responder,
                                enable_rollback_on_validation_failure,
                            );
                        }
                        AgentAction::ReplaceBlock {
                            path,
                            search_block,
                            replace_block,
                            range,
                        } => {
                            spawn_replace_block_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                search_block,
                                replace_block,
                                range,
                                responder,
                            );
                        }
                        AgentAction::SetExecutable { path } => {
                            spawn_set_executable_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                path,
                                responder,
                            );
                        }
                    },
                }
            }
        });
    })
}

fn spawn_run_command_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    command: String,
    cwd: PathBuf,
    project_root: PathBuf,
    timeout: Duration,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let config = load_agent_config(project_root.as_path());
        let command_output_limit = config
            .policy
            .limits
            .max_command_output_bytes
            .unwrap_or(COMMAND_OUTPUT_LIMIT);
        let action = AgentAction::RunCommand {
            command: command.clone(),
            timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
        };
        #[allow(clippy::disallowed_methods)]
        let result = run_command_streaming(
            &event_tx,
            session_id,
            &command,
            &cwd,
            timeout,
            command_output_limit,
        );
        match result {
            Ok(output) => {
                emit_tool_finished(
                    &event_tx,
                    session_id,
                    ActionOutcome::Success { action, output },
                    responder,
                );
            }
            Err(error) => {
                let message = format!("Command failed: {error:#}");
                emit_tool_error(&event_tx, session_id, message.clone());
                emit_tool_finished(
                    &event_tx,
                    session_id,
                    ActionOutcome::Failure {
                        action,
                        error: message,
                    },
                    responder,
                );
            }
        }
    });
}

fn effective_project_root(project_root: &Path, cwd: &Path) -> PathBuf {
    if project_root.as_os_str().is_empty() {
        cwd.to_path_buf()
    } else {
        project_root.to_path_buf()
    }
}

fn spawn_read_file_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    range: Option<ReadFileRange>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ReadFile {
            path: path.clone(),
            range,
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|candidate| {
            if !candidate.exists() {
                return Err(read_only_path_failure(
                    &project_root,
                    &cwd,
                    &path,
                    "Path does not exist",
                ));
            }
            let output = read_file_contents(&candidate, range.and_then(ReadFileRange::normalized))?;
            let mut lines = vec!["[read_file]".to_string(), format!("path: {path}")];
            lines.push(format!("content_hash: {}", stable_content_hash(&output)));
            if let Some(range) = range.and_then(ReadFileRange::normalized) {
                let total_lines = count_file_lines(&candidate)?.max(1);
                let honored_start = range.start_line.min(total_lines);
                let honored_end = range.end_line.min(total_lines).max(honored_start);
                lines.push(format!("requested_range: {}", range.label()));
                lines.push(format!(
                    "honored_range: {}-{} of {}",
                    honored_start, honored_end, total_lines
                ));
            }
            lines.push(output);
            Ok(lines.join("\n"))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "read_file",
            responder,
        );
    });
}

fn spawn_list_directory_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ListDirectory { path: path.clone() };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|candidate| {
            if !candidate.exists() {
                return Err(read_only_path_failure(
                    &project_root,
                    &cwd,
                    &path,
                    "Path does not exist",
                ));
            }
            let entries = list_directory_entries(&candidate)?;
            let mut lines = vec![format!("Directory listing for {path}:")];
            if entries.is_empty() {
                lines.push("[empty]".to_string());
            }
            lines.extend(entries);
            Ok(lines.join("\n"))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "list_directory",
            responder,
        );
    });
}

fn read_only_path_failure(
    project_root: &Path,
    cwd: &Path,
    request_path: &str,
    base_error: &str,
) -> anyhow::Error {
    let suggested_path = diagnose_redundant_workspace_prefix(project_root, request_path);
    let reason = suggested_path
        .as_ref()
        .map(|_| "redundant_workspace_prefix".to_string());
    crate::quorp::tui::diagnostics::log_event(
        "agent.path_resolution_failed",
        serde_json::json!({
            "project_root": project_root.display().to_string(),
            "cwd": cwd.display().to_string(),
            "request_path": request_path,
            "suggested_path": suggested_path,
            "reason": reason,
            "error": base_error,
        }),
    );
    let mut message = format!("{base_error}\nrequest_path: {request_path}");
    if let Some(suggested_path) = diagnose_redundant_workspace_prefix(project_root, request_path) {
        message.push_str(&format!("\nsuggested_path: {suggested_path}"));
        message.push_str("\nreason: redundant_workspace_prefix");
    }
    anyhow::anyhow!(message)
}

fn diagnose_redundant_workspace_prefix(project_root: &Path, request_path: &str) -> Option<String> {
    let stripped = request_path.strip_prefix("workspace/")?;
    let candidate = project_root.join(stripped);
    candidate.exists().then(|| stripped.to_string())
}

fn spawn_search_text_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    query: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::SearchText {
            query: query.clone(),
            limit,
        };
        let root = effective_project_root(&project_root, &cwd);
        let result = Ok(render_text_search_hits(
            &query,
            &search_repo_text(&root, &query, limit.max(1)),
        ));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "search_text",
            responder,
        );
    });
}

fn spawn_search_symbols_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    query: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::SearchSymbols {
            query: query.clone(),
            limit,
        };
        let root = effective_project_root(&project_root, &cwd);
        let result = Ok(render_symbol_search_hits(
            &query,
            &search_repo_symbols(&root, &query, limit.max(1)),
        ));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "search_symbols",
            responder,
        );
    });
}

fn spawn_repo_capsule_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    query: Option<String>,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::GetRepoCapsule {
            query: query.clone(),
            limit,
        };
        let root = effective_project_root(&project_root, &cwd);
        let capsule = build_repo_capsule(&root, query.as_deref(), limit.max(1));
        let result = Ok(render_repo_capsule(query.as_deref(), &capsule));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "get_repo_capsule",
            responder,
        );
    });
}

fn spawn_explain_validation_failure_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    command: String,
    output: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ExplainValidationFailure {
            command: command.clone(),
            output: output.clone(),
        };
        let result = Ok(render_validation_failure_explanation(&command, &output));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "explain_validation_failure",
            responder,
        );
    });
}

struct SuggestImplementationTargetsTaskRequest {
    command: String,
    output: String,
    failing_path: Option<String>,
    failing_line: Option<usize>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
}

fn spawn_suggest_implementation_targets_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: SuggestImplementationTargetsTaskRequest,
) {
    std::thread::spawn(move || {
        let SuggestImplementationTargetsTaskRequest {
            command,
            output,
            failing_path,
            failing_line,
            responder,
        } = request;
        let action = AgentAction::SuggestImplementationTargets {
            command: command.clone(),
            output: output.clone(),
            failing_path: failing_path.clone(),
            failing_line,
        };
        let result = Ok(render_implementation_target_suggestions(
            &command,
            &output,
            failing_path.as_deref(),
            failing_line,
        ));
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "suggest_implementation_targets",
            responder,
        );
    });
}

struct SuggestEditAnchorsTaskRequest {
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    range: Option<ReadFileRange>,
    search_hint: Option<String>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
}

fn spawn_suggest_edit_anchors_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: SuggestEditAnchorsTaskRequest,
) {
    std::thread::spawn(move || {
        let SuggestEditAnchorsTaskRequest {
            cwd,
            project_root,
            path,
            range,
            search_hint,
            responder,
        } = request;
        let action = AgentAction::SuggestEditAnchors {
            path: path.clone(),
            range,
            search_hint: search_hint.clone(),
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|candidate| {
            if !candidate.exists() {
                return Err(read_only_path_failure(
                    &project_root,
                    &cwd,
                    &path,
                    "Path does not exist",
                ));
            }
            let full_text = read_file_contents(&candidate, None)?;
            Ok(render_edit_anchor_suggestions(
                &path,
                &full_text,
                range.and_then(ReadFileRange::normalized),
                search_hint.as_deref(),
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "suggest_edit_anchors",
            responder,
        );
    });
}

struct PreviewEditTaskRequest {
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    edit: PreviewEditPayload,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
}

fn spawn_preview_edit_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: PreviewEditTaskRequest,
) {
    std::thread::spawn(move || {
        let PreviewEditTaskRequest {
            cwd,
            project_root,
            path,
            edit,
            responder,
        } = request;
        let action = AgentAction::PreviewEdit {
            path: path.clone(),
            edit: edit.clone(),
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|target| {
            render_preview_edit_result(&project_root, &cwd, &path, &target, &edit)
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "preview_edit",
            responder,
        );
    });
}

fn render_preview_edit_result(
    project_root: &Path,
    cwd: &Path,
    path: &str,
    target: &Path,
    edit: &PreviewEditPayload,
) -> anyhow::Result<String> {
    match edit {
        PreviewEditPayload::ApplyPatch { patch } => Ok(
            match preview_apply_patch_edit(project_root, cwd, path, patch) {
                Ok(summary) => format!(
                    "[preview_edit]\npath: {path}\nedit_kind: apply_patch\nwould_apply: true\nsyntax_preflight: unavailable\nsyntax_diagnostic: apply_patch preview validated target resolution but did not materialize a complete scratch file\n{summary}"
                ),
                Err(error) => format!(
                    "[preview_edit]\npath: {path}\nedit_kind: apply_patch\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Use a unified diff with unique context, SEARCH/REPLACE blocks, or PreviewEdit a smaller ReplaceBlock before writing."
                ),
            },
        ),
        PreviewEditPayload::ReplaceBlock {
            search_block,
            replace_block,
            range,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match perform_block_replacement(
                    &current_content,
                    search_block,
                    replace_block,
                    *range,
                ) {
                    Ok(updated_content) => {
                        let matches = preview_block_replacement_matches(
                            &current_content,
                            search_block,
                            *range,
                        );
                        let matching_lines = format_preview_match_lines(&matches);
                        let syntax_preflight = syntax_preflight_for_preview(path, &updated_content);
                        let range_note = (*range)
                        .and_then(ReadFileRange::normalized)
                        .map(|range| format!("\nnormalized_suggestion: Use ReplaceBlock with range {{\"start_line\":{},\"end_line\":{}}} or ApplyPatch with the same unique context.", range.start_line, range.end_line))
                        .unwrap_or_else(|| {
                            matches.as_slice().first().map(|candidate| {
                                format!(
                                    "\nnormalized_suggestion: Use ReplaceBlock with range {{\"start_line\":{},\"end_line\":{}}} to keep the write anchored.",
                                    candidate.start_line,
                                    candidate.end_line
                                )
                            }).unwrap_or_default()
                        });
                        format!(
                            "[preview_edit]\npath: {path}\nedit_kind: replace_block\nwould_apply: true\nmatching_line_numbers: {matching_lines}\n{syntax_preflight}{range_note}"
                        )
                    }
                    Err(error) => {
                        let matches = preview_block_replacement_matches(
                            &current_content,
                            search_block,
                            *range,
                        );
                        let matching_lines = format_preview_match_lines(&matches);
                        format!(
                            "[preview_edit]\npath: {path}\nedit_kind: replace_block\nwould_apply: false\nmatching_line_numbers: {matching_lines}\ndiagnostic: {error}\nnormalized_suggestion: Ask for SuggestEditAnchors on the focused range, then use ApplyPatch or ranged ReplaceBlock with exact visible context."
                        )
                    }
                },
            )
        }
        PreviewEditPayload::ReplaceRange {
            range,
            expected_hash,
            replacement,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match perform_range_replacement(
                    &current_content,
                    *range,
                    expected_hash,
                    replacement,
                ) {
                    Ok(updated_content) => render_successful_preview(
                        path,
                        target,
                        "replace_range",
                        &current_content,
                        updated_content,
                    )?,
                    Err(error) => format!(
                        "[preview_edit]\npath: {path}\nedit_kind: replace_range\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Reread the exact range, copy its content_hash, then retry PreviewEdit with replace_range or ReplaceRange."
                    ),
                },
            )
        }
        PreviewEditPayload::ModifyToml {
            expected_hash,
            operations,
        } => {
            let current_content = std::fs::read_to_string(target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            Ok(
                match apply_toml_operations(&current_content, expected_hash, operations) {
                    Ok(updated_content) => render_successful_preview(
                        path,
                        target,
                        "modify_toml",
                        &current_content,
                        updated_content,
                    )?,
                    Err(error) => format!(
                        "[preview_edit]\npath: {path}\nedit_kind: modify_toml\nwould_apply: false\ndiagnostic: {error}\nnormalized_suggestion: Read the full manifest first, then use ModifyToml with that full-file content_hash."
                    ),
                },
            )
        }
    }
}

fn render_successful_preview(
    path: &str,
    target: &Path,
    edit_kind: &str,
    current_content: &str,
    updated_content: String,
) -> anyhow::Result<String> {
    let syntax_preflight = syntax_preflight_for_preview(path, &updated_content);
    let syntax_status = syntax_preflight
        .lines()
        .find_map(|line| line.strip_prefix("syntax_preflight:"))
        .map(str::trim)
        .unwrap_or("unavailable")
        .to_string();
    let base_hash = stable_content_hash(current_content);
    let preview_id = store_preview_record(PreviewRecord {
        preview_id: String::new(),
        path: path.to_string(),
        target_path: target.to_path_buf(),
        base_hash: base_hash.clone(),
        edit_kind: edit_kind.to_string(),
        updated_content,
        syntax_status,
    })?;
    let apply_preview_example = serde_json::json!({
        "actions": [
            {
                "ApplyPreview": {
                    "preview_id": preview_id
                }
            }
        ],
        "assistant_message": "Applying the clean preview."
    });
    Ok(format!(
        "[preview_edit]\npath: {path}\nedit_kind: {edit_kind}\nwould_apply: true\npreview_id: {preview_id}\nbase_hash: {base_hash}\n{syntax_preflight}\napply_preview: {apply_preview_example}"
    ))
}

#[allow(clippy::disallowed_methods)]
fn syntax_preflight_for_preview(path: &str, updated_content: &str) -> String {
    if path.ends_with(".toml") {
        return match updated_content.parse::<toml_edit::DocumentMut>() {
            Ok(_) => {
                "syntax_preflight: passed\nsyntax_diagnostic: TOML parser accepted scratch content"
                    .to_string()
            }
            Err(error) => format!(
                "syntax_preflight: failed\nsyntax_diagnostic: {}",
                truncate_anchor_line(&error.to_string(), 320)
            ),
        };
    }
    if !path.ends_with(".rs") {
        return "syntax_preflight: unavailable\nsyntax_diagnostic: no cheap syntax preflight registered for this file type".to_string();
    }
    let scratch_path = std::env::temp_dir().join(format!(
        "quorp-preview-{}-{}.rs",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    if let Err(error) = std::fs::write(&scratch_path, updated_content) {
        return format!(
            "syntax_preflight: unavailable\nsyntax_diagnostic: failed to write scratch file: {error}"
        );
    }
    let result = match std::process::Command::new("rustfmt")
        .arg("--check")
        .arg(&scratch_path)
        .output()
    {
        Ok(output) if output.status.success() => {
            "syntax_preflight: passed\nsyntax_diagnostic: rustfmt accepted scratch file".to_string()
        }
        Ok(output) => {
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let diagnostic = if diagnostic.trim().is_empty() {
                String::from_utf8_lossy(&output.stdout).to_string()
            } else {
                diagnostic.to_string()
            };
            format!(
                "syntax_preflight: failed\nsyntax_diagnostic: {}",
                truncate_anchor_line(diagnostic.trim(), 320)
            )
        }
        Err(error) => format!(
            "syntax_preflight: unavailable\nsyntax_diagnostic: rustfmt unavailable: {error}"
        ),
    };
    if let Err(error) = std::fs::remove_file(&scratch_path) {
        return format!("{result}\ncleanup_diagnostic: failed to remove scratch file: {error}");
    }
    result
}

fn preview_apply_patch_edit(
    project_root: &Path,
    cwd: &Path,
    path: &str,
    patch: &str,
) -> anyhow::Result<String> {
    let target = sanitize_project_path(project_root, cwd, path)?;
    if let Some(blocks) = try_parse_search_replace_blocks(patch) {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let mut line_notes = Vec::new();
        for (search, replace) in blocks {
            let matches = preview_block_replacement_matches(&current_content, &search, None);
            line_notes.push(format_preview_match_lines(&matches));
            current_content = perform_block_replacement(&current_content, &search, &replace, None)?;
        }
        return Ok(format!(
            "patch_form: search_replace_blocks\nmatching_line_numbers: {}",
            line_notes.join("; ")
        ));
    }

    if let Some(line_replacement) = try_parse_line_replacement_shorthand(patch)? {
        let mut current_content = std::fs::read_to_string(&target)
            .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
        let line_number = perform_line_replacement_shorthand(
            &mut current_content,
            &line_replacement.search,
            &line_replacement.replace,
        )?;
        return Ok(format!(
            "patch_form: line_replacement_shorthand\nmatching_line_numbers: {line_number}"
        ));
    }

    let (patch_input, normalized_single_file_hunk) = normalize_single_file_hunk_patch(path, patch)?;
    let file_patches = parse_multi_file_patch(patch_input.as_deref().unwrap_or(patch))?;
    if file_patches.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch expects a unified diff patch or SEARCH/REPLACE blocks"
        ));
    }
    let resolved = resolve_file_patches(project_root, cwd, &file_patches)?;
    let summary = resolved
        .iter()
        .map(|patch| match &patch.operation {
            PatchOperation::Add => format!("A {}", patch.display_path),
            PatchOperation::Update => format!("M {}", patch.display_path),
            PatchOperation::Delete => format!("D {}", patch.display_path),
            PatchOperation::Move { move_path } => {
                format!("R {} -> {}", patch.display_path, move_path)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let patch_form = if normalized_single_file_hunk {
        "single_file_hunk"
    } else {
        "unified_diff"
    };
    Ok(format!(
        "patch_form: {patch_form}\nresolved_files: {summary}"
    ))
}

fn preview_block_replacement_matches(
    current_content: &str,
    search_block: &str,
    range: Option<ReadFileRange>,
) -> Vec<BlockReplacementMatch> {
    let normalized_range = range.and_then(ReadFileRange::normalized);
    let exact_matches = filter_matches_by_range(
        &exact_block_matches(current_content, search_block),
        normalized_range,
    );
    if !exact_matches.is_empty() {
        return exact_matches;
    }

    let search_lines = search_block.lines().collect::<Vec<_>>();
    let content_lines = current_content.lines().collect::<Vec<_>>();
    if search_lines.is_empty() || search_lines.len() > content_lines.len() {
        return Vec::new();
    }

    let line_spans = line_spans(current_content);
    let mut fuzzy_matches = Vec::new();
    for index in 0..=(content_lines.len() - search_lines.len()) {
        let matched = search_lines
            .iter()
            .enumerate()
            .all(|(search_index, line)| content_lines[index + search_index].trim() == line.trim());
        if matched {
            let end_index = index + search_lines.len().saturating_sub(1);
            if let (Some(start), Some(end)) = (line_spans.get(index), line_spans.get(end_index)) {
                fuzzy_matches.push(BlockReplacementMatch {
                    start_byte: start.0,
                    end_byte: end.1,
                    start_line: index + 1,
                    end_line: end_index + 1,
                });
            }
        }
    }
    filter_matches_by_range(&fuzzy_matches, normalized_range)
}

fn format_preview_match_lines(matches: &[BlockReplacementMatch]) -> String {
    if matches.is_empty() {
        return "none".to_string();
    }
    matches
        .iter()
        .map(|candidate| {
            if candidate.start_line == candidate.end_line {
                candidate.start_line.to_string()
            } else {
                format!("{}-{}", candidate.start_line, candidate.end_line)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RustDiagnosticSummary {
    diagnostic_class: String,
    target_class: String,
    primary_anchor: Option<String>,
    real_error_seen: bool,
    error_excerpts: Vec<String>,
}

fn classify_rust_validation_output(output: &str) -> RustDiagnosticSummary {
    let mut primary_anchor = None;
    let mut error_excerpts = Vec::new();
    let mut real_error_seen = false;
    let lower_output = output.to_ascii_lowercase();
    let diagnostic_class = if lower_output.contains("error[e0432]")
        || lower_output.contains("error[e0433]")
        || lower_output.contains("unresolved import")
        || lower_output.contains("unresolved crate")
        || lower_output.contains("use of unresolved module or unlinked crate")
    {
        real_error_seen = true;
        "manifest_dependency_error"
    } else if lower_output.contains("expected one of")
        || lower_output.contains("mismatched closing delimiter")
        || lower_output.contains("unclosed delimiter")
        || lower_output.contains("unexpected closing delimiter")
        || lower_output.contains("could not compile")
            && lower_output.contains("previous error")
            && lower_output.contains("error:")
    {
        real_error_seen = true;
        "rust_parse_error"
    } else if lower_output.contains("error[") || lower_output.contains("\nerror:") {
        real_error_seen = true;
        "rust_compile_error"
    } else if lower_output.contains("panicked at")
        || lower_output.contains("assertion `")
        || lower_output.contains("test result: failed")
    {
        "test_assertion_failure"
    } else {
        "unknown"
    }
    .to_string();

    let mut inside_warning = false;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("warning:") {
            inside_warning = true;
            continue;
        }
        if lower.starts_with("error") || lower.starts_with("---- ") {
            inside_warning = false;
        }
        if real_error_seen && inside_warning {
            continue;
        }
        if primary_anchor.is_none()
            && (line.starts_with("-->") || looks_like_source_anchor(line))
            && (!real_error_seen || !lower.contains("warning:"))
        {
            primary_anchor = Some(line.to_string());
        }
        if error_excerpts.len() < 8
            && (lower.starts_with("error")
                || lower.contains("unresolved import")
                || lower.contains("panicked at")
                || lower.contains("assertion `")
                || lower.contains("expected one of"))
        {
            error_excerpts.push(truncate_anchor_line(line, 180));
        }
    }

    let target_class = if diagnostic_class == "manifest_dependency_error" {
        "manifest"
    } else if primary_anchor
        .as_deref()
        .is_some_and(|anchor| anchor.contains("/tests/") || anchor.contains("tests/"))
    {
        "test_evidence"
    } else if primary_anchor.is_some() {
        "implementation"
    } else {
        "unknown"
    }
    .to_string();

    RustDiagnosticSummary {
        diagnostic_class,
        target_class,
        primary_anchor,
        real_error_seen,
        error_excerpts,
    }
}

fn render_validation_failure_explanation(command: &str, output: &str) -> String {
    let diagnostic = classify_rust_validation_output(output);
    let mut failing_tests = Vec::new();
    let mut anchors = Vec::new();
    let mut excerpts = Vec::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(test_name) = line
            .strip_prefix("---- ")
            .and_then(|tail| tail.strip_suffix(" stdout ----"))
            .or_else(|| {
                line.strip_prefix("test ")
                    .and_then(|tail| tail.split_whitespace().next())
            })
            .or_else(|| {
                line.strip_prefix("failures:")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
            && failing_tests.len() < 8
            && !failing_tests.iter().any(|existing| existing == test_name)
        {
            failing_tests.push(test_name.to_string());
        }
        let lower = line.to_ascii_lowercase();
        if diagnostic.real_error_seen && lower.contains("warning:") {
            continue;
        }
        if looks_like_source_anchor(line)
            && anchors.len() < 8
            && !anchors.iter().any(|existing| existing == line)
        {
            anchors.push(line.to_string());
        }
        if (line.contains("assertion")
            || line.contains("panicked at")
            || line.contains("error:")
            || line.contains("FAILED")
            || line.contains("expected")
            || line.contains("actual"))
            && excerpts.len() < 8
        {
            excerpts.push(truncate_anchor_line(line, 180));
        }
    }
    let mut lines = vec![
        "[explain_validation_failure]".to_string(),
        format!("command: {}", command.trim()),
        format!("diagnostic_class: {}", diagnostic.diagnostic_class),
        format!("target_class: {}", diagnostic.target_class),
    ];
    if let Some(anchor) = diagnostic.primary_anchor.as_ref() {
        lines.push(format!("primary_anchor: {anchor}"));
    }
    if !failing_tests.is_empty() {
        lines.push(format!("failing_tests: {}", failing_tests.join(", ")));
    }
    if !anchors.is_empty() {
        lines.push("file_line_anchors:".to_string());
        lines.extend(anchors.iter().map(|anchor| format!("- {anchor}")));
    }
    if !excerpts.is_empty() {
        lines.push("assertion_excerpts:".to_string());
        lines.extend(excerpts.iter().map(|excerpt| format!("- {excerpt}")));
    }
    if !diagnostic.error_excerpts.is_empty() {
        lines.push("diagnostic_excerpts:".to_string());
        lines.extend(
            diagnostic
                .error_excerpts
                .iter()
                .map(|excerpt| format!("- {excerpt}")),
        );
    }
    lines.push(
        "next_step: use suggest_implementation_targets or the benchmark packet target lease, then request anchors or preview a patch on the implementation target; this tool does not infer a patch."
            .to_string(),
    );
    lines.join("\n")
}

fn render_implementation_target_suggestions(
    command: &str,
    output: &str,
    failing_path: Option<&str>,
    failing_line: Option<usize>,
) -> String {
    let diagnostic = classify_rust_validation_output(output);
    let mut ranked = Vec::new();
    if diagnostic.diagnostic_class == "manifest_dependency_error" {
        ranked.push(("Cargo.toml".to_string(), "manifest_dependency_error"));
    }
    if let Some(path) = failing_path.map(str::trim).filter(|path| !path.is_empty()) {
        let reason = if is_obvious_test_evidence_path(path) {
            "test_evidence_only"
        } else {
            "failing_implementation_anchor"
        };
        ranked.push((path.to_string(), reason));
    }
    if let Some(anchor) = diagnostic.primary_anchor.as_deref()
        && let Some(path) = extract_path_from_anchor(anchor)
    {
        let reason = if is_obvious_test_evidence_path(&path) {
            "test_evidence_only"
        } else {
            "diagnostic_implementation_anchor"
        };
        ranked.push((path, reason));
    }
    if ranked.is_empty() {
        ranked.push((
            "benchmark_packet_expected_touch_target".to_string(),
            "expected_touch_target",
        ));
    }

    let required_next_target = ranked
        .iter()
        .find(|(_, reason)| *reason != "test_evidence_only")
        .map(|(path, _)| path.as_str())
        .unwrap_or("benchmark_packet_expected_touch_target");
    let mut lines = vec![
        "[suggest_implementation_targets]".to_string(),
        format!("command: {}", command.trim()),
        format!("diagnostic_class: {}", diagnostic.diagnostic_class),
        format!("target_class: {}", diagnostic.target_class),
        format!("required_next_target: {required_next_target}"),
    ];
    if let Some(line) = failing_line {
        lines.push(format!("failing_line: {line}"));
    }
    lines.push("ranked_targets:".to_string());
    let mut seen = std::collections::BTreeSet::new();
    for (path, reason) in ranked {
        if seen.insert(path.clone()) {
            lines.push(format!("- path: {path} reason: {reason}"));
        }
    }
    lines.push(
        "next_step: request SuggestEditAnchors or PreviewEdit on the required implementation target; do not preview or write test evidence files unless they are explicit touch targets."
            .to_string(),
    );
    lines.join("\n")
}

fn extract_path_from_anchor(anchor: &str) -> Option<String> {
    let trimmed = anchor.trim().trim_start_matches("-->").trim();
    let before_colon = trimmed.split(':').next()?.trim();
    (!before_colon.is_empty()).then(|| before_colon.to_string())
}

fn is_obvious_test_evidence_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains("/tests/")
        || normalized.starts_with("tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_tests.rs")
}

fn looks_like_source_anchor(line: &str) -> bool {
    line.contains(".rs:")
        || line.contains(".py:")
        || line.contains(".ts:")
        || line.contains(".tsx:")
        || line.contains(".js:")
        || line.contains(".jsx:")
        || line.contains("src/")
        || line.contains("tests/")
}

fn truncate_anchor_line(line: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, character) in line.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(character);
    }
    truncated
}

fn render_edit_anchor_suggestions(
    path: &str,
    full_text: &str,
    range: Option<ReadFileRange>,
    search_hint: Option<&str>,
) -> String {
    let all_lines = full_text.lines().collect::<Vec<_>>();
    let total_lines = all_lines.len().max(1);
    let effective_range = range.unwrap_or(ReadFileRange {
        start_line: 1,
        end_line: total_lines.min(80),
    });
    let start_line = effective_range.start_line.max(1).min(total_lines);
    let end_line = effective_range.end_line.max(start_line).min(total_lines);
    let start_index = start_line.saturating_sub(1);
    let end_index = end_line.min(all_lines.len());
    let focused_lines = if start_index < end_index {
        &all_lines[start_index..end_index]
    } else {
        &[][..]
    };
    let mut anchors = unique_anchor_lines(full_text, focused_lines, start_line);
    if anchors.is_empty() {
        anchors.push(format!(
            "lines {}-{}: no unique non-blank single-line anchors found; use ApplyPatch with broader unique context.",
            start_line, end_line
        ));
    }
    let repeated_warning = search_hint
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(|hint| {
            let count = full_text.matches(hint).count();
            format!("search_hint_occurrences: {count}")
        });
    let mut lines = vec![
        "[suggest_edit_anchors]".to_string(),
        format!("path: {path}"),
        format!("focused_range: {}-{}", start_line, end_line),
        "unique_surrounding_anchors:".to_string(),
    ];
    lines.extend(anchors.iter().take(6).map(|anchor| format!("- {anchor}")));
    if let Some(warning) = repeated_warning {
        lines.push(warning);
    }
    lines.push("safe_edit_forms:".to_string());
    lines.push(format!(
        "- ApplyPatch with context from the focused range: @@ ... unique anchor near line {} ... @@",
        start_line
    ));
    lines.push(format!(
        "- ReplaceBlock with range {{\"start_line\":{},\"end_line\":{}}} and an exact search_block from the focused slice.",
        start_line, end_line
    ));
    lines.push(
        "guardrail: if the same snippet appears multiple times, do not use bare ReplaceBlock."
            .to_string(),
    );
    lines.join("\n")
}

fn unique_anchor_lines(
    full_text: &str,
    focused_lines: &[&str],
    first_line_number: usize,
) -> Vec<String> {
    focused_lines
        .iter()
        .enumerate()
        .filter_map(|(offset, line)| {
            let trimmed = line.trim();
            if trimmed.len() < 12 || trimmed.starts_with("//") || trimmed.starts_with('#') {
                return None;
            }
            if full_text.matches(trimmed).count() != 1 {
                return None;
            }
            Some(format!(
                "line {}: {}",
                first_line_number + offset,
                truncate_anchor_line(trimmed, 140)
            ))
        })
        .collect()
}

fn resolve_mcp_server_config(
    project_root: &Path,
    server_name: &str,
) -> anyhow::Result<McpServerConfig> {
    let config = load_agent_config(project_root);
    let available = config
        .mcp_servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    config
        .mcp_servers
        .into_iter()
        .find(|server| server.name == server_name)
        .ok_or_else(|| {
            if available.is_empty() {
                anyhow::anyhow!(
                    "MCP server `{server_name}` is not configured in .quorp/agent.toml"
                )
            } else {
                anyhow::anyhow!(
                    "MCP server `{server_name}` is not configured in .quorp/agent.toml. Available servers: {available}"
                )
            }
        })
}

fn render_mcp_tool_result(
    server_name: &str,
    tool_name: &str,
    result: &crate::quorp::tui::mcp_client::CallToolResult,
) -> anyhow::Result<String> {
    let mut sections = Vec::new();
    for content in &result.content {
        match content {
            crate::quorp::tui::mcp_client::CallToolResultContent::Text { text } => {
                sections.push(text.clone());
            }
            crate::quorp::tui::mcp_client::CallToolResultContent::Image { mime_type, data } => {
                sections.push(format!(
                    "[image result]\nmime_type: {mime_type}\nbase64_bytes: {}",
                    data.len()
                ));
            }
            crate::quorp::tui::mcp_client::CallToolResultContent::Resource { resource } => {
                let rendered =
                    serde_json::to_string_pretty(resource).unwrap_or_else(|_| resource.to_string());
                sections.push(format!("[resource result]\n{rendered}"));
            }
        }
    }

    let body = if sections.is_empty() {
        "[no MCP content returned]".to_string()
    } else {
        sections.join("\n\n")
    };
    if result.is_error.unwrap_or(false) {
        Err(anyhow::anyhow!(
            "MCP {server_name}/{tool_name} returned an error:\n{body}"
        ))
    } else {
        Ok(format!("MCP {server_name}/{tool_name}\n{body}"))
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_mcp_call_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    _cwd: PathBuf,
    project_root: PathBuf,
    server_name: String,
    tool_name: String,
    arguments: serde_json::Value,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::McpCallTool {
            server_name: server_name.clone(),
            tool_name: tool_name.clone(),
            arguments: arguments.clone(),
        };
        let result = (|| -> anyhow::Result<String> {
            let server_config = resolve_mcp_server_config(project_root.as_path(), &server_name)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| anyhow::anyhow!("Failed to start MCP runtime: {error}"))?;
            runtime.block_on(async move {
                let args = server_config
                    .args
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let client =
                    crate::quorp::tui::mcp_client::McpClient::spawn(&server_config.command, &args)
                        .await?;
                let result = client.call_tool(&tool_name, Some(arguments)).await?;
                render_mcp_tool_result(&server_name, &tool_name, &result)
            })
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "mcp_call_tool",
            responder,
        );
    });
}

fn spawn_write_file_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    content: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::WriteFile {
            path: path.clone(),
            content: content.clone(),
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|candidate| {
            stash_file_for_rollback(session_id, &candidate);
            write_full_file(&candidate, &content)?;
            Ok(format!("Wrote {} bytes to {path}", content.len()))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Write file: {path}\n{output}")),
            "write_file",
            responder,
        );
    });
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum PatchOperation {
    Add,
    Update,
    Delete,
    Move { move_path: String },
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct FilePatch {
    path: String,
    operation: PatchOperation,
    hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<PatchLine>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ResolvedFilePatch {
    source_path: PathBuf,
    target_path: PathBuf,
    display_path: String,
    operation: PatchOperation,
    new_content: Option<String>,
}

fn spawn_apply_patch_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    patch: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ApplyPatch {
            path: path.clone(),
            patch: patch.clone(),
        };
        let result = (|| -> anyhow::Result<String> {
            let target = sanitize_project_path(&project_root, &cwd, &path)?;
            if let Some(blocks) = try_parse_search_replace_blocks(&patch) {
                let mut current_content = std::fs::read_to_string(&target)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                stash_file_for_rollback(session_id, &target);
                for (search, replace) in blocks {
                    current_content =
                        perform_block_replacement(&current_content, &search, &replace, None)?;
                }
                write_full_file(&target, &current_content)?;
                return Ok(format!("Applied search/replace blocks to {path}"));
            }

            if let Some(line_replacement) = try_parse_line_replacement_shorthand(&patch)? {
                let mut current_content = std::fs::read_to_string(&target)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                let line_number = perform_line_replacement_shorthand(
                    &mut current_content,
                    &line_replacement.search,
                    &line_replacement.replace,
                )?;
                stash_file_for_rollback(session_id, &target);
                write_full_file(&target, &current_content)?;
                return Ok(format!(
                    "Applied single-line replacement shorthand to {path}: line {line_number}"
                ));
            }

            let (patch_input, normalized_single_file_hunk) =
                normalize_single_file_hunk_patch(&path, &patch)?;
            let file_patches = parse_multi_file_patch(patch_input.as_deref().unwrap_or(&patch))?;
            if file_patches.is_empty() {
                return Err(anyhow::anyhow!(
                    "apply_patch expects a unified diff patch or SEARCH/REPLACE blocks"
                ));
            }

            let resolved = resolve_file_patches(&project_root, &cwd, &file_patches)?;
            for patch in &resolved {
                stash_file_for_rollback(session_id, &patch.source_path);
                if patch.target_path != patch.source_path {
                    stash_file_for_rollback(session_id, &patch.target_path);
                }
            }
            apply_resolved_file_patches(&resolved)?;
            let summary = resolved
                .iter()
                .map(|patch| match &patch.operation {
                    PatchOperation::Add => format!("A {}", patch.display_path),
                    PatchOperation::Update => format!("M {}", patch.display_path),
                    PatchOperation::Delete => format!("D {}", patch.display_path),
                    PatchOperation::Move { move_path } => {
                        format!("R {} -> {}", patch.display_path, move_path)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if normalized_single_file_hunk {
                Ok(format!("Applied single-file hunk patch:\n{summary}"))
            } else {
                Ok(format!("Applied unified diff patch:\n{summary}"))
            }
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Apply patch: {path}\n{output}")),
            "apply_patch",
            responder,
        );
    });
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct LineReplacementShorthand {
    search: String,
    replace: String,
}

fn try_parse_line_replacement_shorthand(
    patch_text: &str,
) -> anyhow::Result<Option<LineReplacementShorthand>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let meaningful = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let [search_line, replace_line] = meaningful.as_slice() else {
        return Ok(None);
    };
    let search_line = search_line.trim_start();
    let replace_line = replace_line.trim_start();
    let Some(search) = search_line.strip_prefix('/') else {
        return Ok(None);
    };
    let Some(replace) = replace_line.strip_prefix('+') else {
        return Ok(None);
    };
    let search = search.trim();
    if search.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand requires a non-empty search string"
        ));
    }

    Ok(Some(LineReplacementShorthand {
        search: search.to_string(),
        replace: replace.to_string(),
    }))
}

fn perform_line_replacement_shorthand(
    content: &mut String,
    search: &str,
    replace: &str,
) -> anyhow::Result<usize> {
    let mut matches = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let occurrences = line.matches(search).count();
        if occurrences > 0 {
            matches.push((index + 1, occurrences));
        }
    }

    let [(line_number, occurrences)] = matches.as_slice() else {
        if matches.is_empty() {
            return Err(anyhow::anyhow!(
                "apply_patch line replacement shorthand found no lines containing `{search}`. Use a unified diff hunk, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
            ));
        }
        let line_numbers = matches
            .iter()
            .map(|(line_number, _)| line_number.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand is ambiguous; `{search}` appears on lines {line_numbers}. Use a unified diff hunk with context, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
        ));
    };
    if *occurrences > 1 {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand is ambiguous; `{search}` appears more than once on line {line_number}. Use a unified diff hunk with context, SEARCH/REPLACE block, ReplaceBlock with range, or SuggestEditAnchors."
        ));
    }

    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let Some(line) = lines.get_mut(line_number.saturating_sub(1)) else {
        return Err(anyhow::anyhow!(
            "apply_patch line replacement shorthand resolved outside file"
        ));
    };
    if line.trim() == search && replace.chars().next().is_some_and(char::is_whitespace) {
        *line = replace.to_string();
    } else {
        *line = line.replacen(search, replace, 1);
    }
    let had_trailing_newline = content.ends_with('\n');
    *content = lines.join("\n");
    if had_trailing_newline {
        content.push('\n');
    }
    Ok(*line_number)
}

fn normalize_single_file_hunk_patch(
    request_path: &str,
    patch_text: &str,
) -> anyhow::Result<(Option<String>, bool)> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let hunk_body = normalized.trim_start();
    if !hunk_body.starts_with("@@") {
        return Ok((None, false));
    }

    let path = request_path.trim();
    if path.is_empty() {
        return Err(anyhow::anyhow!(
            "apply_patch hunk-only input requires an explicit path"
        ));
    }
    if path.contains('\n') || path.contains('\r') {
        return Err(anyhow::anyhow!(
            "apply_patch hunk-only path cannot contain newlines"
        ));
    }

    Ok((
        Some(format!("--- a/{path}\n+++ b/{path}\n{hunk_body}")),
        true,
    ))
}

#[allow(clippy::too_many_arguments)]
fn spawn_replace_block_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    search_block: String,
    replace_block: String,
    range: Option<ReadFileRange>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ReplaceBlock {
            path: path.clone(),
            search_block: search_block.clone(),
            replace_block: replace_block.clone(),
            range,
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|target| {
            let current_content = std::fs::read_to_string(&target)
                .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;
            let new_content =
                perform_block_replacement(&current_content, &search_block, &replace_block, range)?;
            stash_file_for_rollback(session_id, &target);
            write_full_file(&target, &new_content)?;
            Ok(format!("Replaced block in {path}"))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Replace block: {path}\n{output}")),
            "replace_block",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_replace_range_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    range: ReadFileRange,
    expected_hash: String,
    replacement: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ReplaceRange {
            path: path.clone(),
            range,
            expected_hash: expected_hash.clone(),
            replacement: replacement.clone(),
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|target| {
            let current_content = std::fs::read_to_string(&target)
                .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
            let updated_content =
                perform_range_replacement(&current_content, range, &expected_hash, &replacement)?;
            let syntax_preflight = syntax_preflight_for_preview(&path, &updated_content);
            if syntax_preflight.contains("syntax_preflight: failed") {
                return Err(anyhow::anyhow!(
                    "replace_range syntax preflight failed:\n{syntax_preflight}"
                ));
            }
            stash_file_for_rollback(session_id, &target);
            write_full_file(&target, &updated_content)?;
            Ok(format!(
                "Replaced lines {} in {path}\n{}",
                range.label(),
                syntax_preflight
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Replace range: {path}\n{output}")),
            "replace_range",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_modify_toml_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    expected_hash: String,
    operations: Vec<TomlEditOperation>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ModifyToml {
            path: path.clone(),
            expected_hash: expected_hash.clone(),
            operations: operations.clone(),
        };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|target| {
            let current_content = std::fs::read_to_string(&target)
                .map_err(|error| anyhow::anyhow!("Failed to read TOML file: {error}"))?;
            let updated_content =
                apply_toml_operations(&current_content, &expected_hash, &operations)?;
            let syntax_preflight = syntax_preflight_for_preview(&path, &updated_content);
            if syntax_preflight.contains("syntax_preflight: failed") {
                return Err(anyhow::anyhow!(
                    "modify_toml syntax preflight failed:\n{syntax_preflight}"
                ));
            }
            stash_file_for_rollback(session_id, &target);
            write_full_file(&target, &updated_content)?;
            Ok(format!(
                "Applied {} TOML operation(s) to {path}\n{}",
                operations.len(),
                syntax_preflight
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Modify TOML: {path}\n{output}")),
            "modify_toml",
            responder,
        );
    });
}

fn spawn_apply_preview_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    preview_id: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ApplyPreview {
            preview_id: preview_id.clone(),
        };
        let result = (|| -> anyhow::Result<String> {
            let record = load_preview_record(&preview_id)?;
            let current_content = std::fs::read_to_string(&record.target_path)
                .map_err(|error| anyhow::anyhow!("Failed to read preview target: {error}"))?;
            let current_hash = stable_content_hash(&current_content);
            if current_hash != record.base_hash {
                return Err(anyhow::anyhow!(
                    "preview_apply_mismatch: preview expected base_hash={} but current content_hash={current_hash}. Reread the target and preview again.",
                    record.base_hash
                ));
            }
            stash_file_for_rollback(session_id, &record.target_path);
            write_full_file(&record.target_path, &record.updated_content)?;
            Ok(format!(
                "Applied preview {preview_id} to {}\nedit_kind: {}\nsyntax_preflight: {}",
                record.path, record.edit_kind, record.syntax_status
            ))
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result.map(|output| format!("Apply preview: {preview_id}\n{output}")),
            "apply_preview",
            responder,
        );
    });
}

fn spawn_run_validation_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    plan: crate::quorp::tui::agent_protocol::ValidationPlan,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
    enable_rollback_on_validation_failure: bool,
) {
    std::thread::spawn(move || {
        let action = AgentAction::RunValidation { plan: plan.clone() };
        let config = load_agent_config(project_root.as_path());
        let commands = validation_commands_for_plan(&config, &plan);
        let command_output_limit = config
            .policy
            .limits
            .max_command_output_bytes
            .unwrap_or(COMMAND_OUTPUT_LIMIT);
        let result =
            run_validation_commands(&event_tx, session_id, &cwd, commands, command_output_limit);

        if let Err(ref e) = result {
            if enable_rollback_on_validation_failure {
                rollback_session_worktree(session_id);
                let rolled_back_error = anyhow::anyhow!(
                    "{}\n\n[System] Changes were safely rolled back. Please analyze the error and try applying a corrected fix.",
                    e
                );
                emit_tool_result(
                    &event_tx,
                    session_id,
                    action,
                    Err(rolled_back_error),
                    "run_validation",
                    responder,
                );
                return;
            }
            emit_tool_result(
                &event_tx,
                session_id,
                action,
                result,
                "run_validation",
                responder,
            );
            return;
        } else {
            clear_session_worktree(session_id);
        }

        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "run_validation",
            responder,
        );
    });
}

fn emit_tool_error(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    message: String,
) {
    if let Err(error) = event_tx.send(TuiEvent::Chat(crate::quorp::tui::chat::ChatUiEvent::Error(
        session_id, message,
    ))) {
        log::error!("tui: tool error channel closed: {error}");
    }
}

fn emit_tool_finished(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    outcome: ActionOutcome,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    if let Some(responder) = responder
        && responder.send(outcome.clone()).is_err()
    {
        log::warn!("tui: tool responder was dropped before completion for session {session_id}");
    }
    send_chat_event(
        event_tx,
        crate::quorp::tui::chat::ChatUiEvent::CommandFinished(session_id, outcome),
    );
}

fn spawn_set_executable_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::SetExecutable { path: path.clone() };
        let result = sanitize_project_path(&project_root, &cwd, &path).and_then(|target| {
            stash_file_for_rollback(session_id, &target);
            set_executable_bit(&target)?;
            Ok(format!("Enabled executable bit for {path}"))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "set_executable",
            responder,
        );
    });
}

fn emit_tool_result(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    action: AgentAction,
    result: anyhow::Result<String>,
    action_label: &str,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action_text = format!("{action:?}");
    let outcome = match result {
        Ok(message) => ActionOutcome::Success {
            action,
            output: message,
        },
        Err(error) => {
            let error_text = format!("{action_label}: {error}");
            emit_tool_error(event_tx, session_id, error_text.clone());
            ActionOutcome::Failure {
                action,
                error: error_text,
            }
        }
    };
    crate::quorp::tui::diagnostics::log_event(
        "agent.tool_finished",
        serde_json::json!({
            "session_id": session_id,
            "action": action_text,
            "status": if matches!(outcome, ActionOutcome::Success { .. }) { "success" } else { "failure" },
            "output_preview": truncate_diagnostic_text(outcome.output_text(), 240),
        }),
    );
    send_chat_event(
        event_tx,
        crate::quorp::tui::chat::ChatUiEvent::CommandOutput(
            session_id,
            outcome.output_text().to_string(),
        ),
    );
    emit_tool_finished(event_tx, session_id, outcome, responder);
}

fn run_validation_commands(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: &Path,
    commands: Vec<String>,
    command_output_limit: usize,
) -> anyhow::Result<String> {
    if commands.is_empty() {
        return Err(anyhow::anyhow!(
            "run_validation had no resolved commands; check .quorp/agent.toml"
        ));
    }

    let mut combined_output = String::new();
    for command in commands {
        let started_at = Instant::now();
        crate::quorp::tui::diagnostics::log_event(
            "agent.validation_started",
            serde_json::json!({
                "session_id": session_id,
                "cwd": cwd.display().to_string(),
                "command": command,
            }),
        );
        send_chat_event(
            event_tx,
            crate::quorp::tui::chat::ChatUiEvent::CommandOutput(session_id, format!("$ {command}")),
        );
        let command_output = run_command_capture(&command, cwd, command_output_limit)
            .map_err(|error| anyhow::anyhow!("failed to run `{command}`: {error}"))?;
        for line in command_output.output.lines() {
            send_chat_event(
                event_tx,
                crate::quorp::tui::chat::ChatUiEvent::CommandOutput(session_id, line.to_string()),
            );
        }
        combined_output.push_str(&format!("$ {command}\n"));
        combined_output.push_str(command_output.output.as_str());
        crate::quorp::tui::diagnostics::log_event(
            "agent.validation_finished",
            serde_json::json!({
                "session_id": session_id,
                "cwd": cwd.display().to_string(),
                "command": command,
                "exit_code": command_output.exit_code,
                "duration_ms": started_at.elapsed().as_millis() as u64,
                "output_preview": truncate_diagnostic_text(&command_output.output, 240),
            }),
        );
        if command_output.exit_code != 0 {
            if !combined_output.ends_with('\n') {
                combined_output.push('\n');
            }
            combined_output.push_str(&format!("[Exit code: {}]", command_output.exit_code));
            return Err(anyhow::anyhow!(combined_output));
        }
        if !combined_output.ends_with('\n') {
            combined_output.push('\n');
        }
    }
    combined_output.push_str("[Validation finished]");
    Ok(combined_output)
}

struct CapturedCommandOutput {
    output: String,
    exit_code: i32,
}

fn run_command_capture(
    command: &str,
    cwd: &Path,
    command_output_limit: usize,
) -> anyhow::Result<CapturedCommandOutput> {
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

    let reader_thread = std::thread::spawn(move || -> anyhow::Result<String> {
        let mut chunk = [0u8; 4096];
        let mut captured = String::new();
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read_len) => {
                    let text = String::from_utf8_lossy(&chunk[..read_len]);
                    captured.push_str(text.as_ref());
                    if captured.len() > command_output_limit {
                        captured.truncate(command_output_limit);
                    }
                }
                Err(error) => return Err(anyhow::anyhow!("failed reading PTY output: {error}")),
            }
        }
        Ok(captured)
    });

    let status = child.wait()?;
    let exit_code = status.exit_code() as i32;
    let output = match reader_thread.join() {
        Ok(result) => result?,
        Err(panic_payload) => std::panic::resume_unwind(panic_payload),
    };

    Ok(CapturedCommandOutput { output, exit_code })
}

fn send_chat_event(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    event: crate::quorp::tui::chat::ChatUiEvent,
) {
    if let Err(error) = event_tx.send(TuiEvent::Chat(event)) {
        log::error!("tui: command event channel closed: {error}");
    }
}

fn read_file_contents(path: &Path, range: Option<ReadFileRange>) -> anyhow::Result<String> {
    use std::io::Read;
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("Path is not a regular file"));
    }
    if metadata.len() > FILE_READ_LIMIT_BYTES as u64 {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::with_capacity(FILE_READ_LIMIT_BYTES);
        file.take(FILE_READ_LIMIT_BYTES as u64)
            .read_to_end(&mut bytes)?;
        let mut text = String::from_utf8(bytes)
            .map_err(|error| anyhow::anyhow!("File is not valid UTF-8: {error}"))?;
        text.push_str(FILE_READ_TRUNCATION_MARKER);
        return slice_text_by_range(&text, range);
    }
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes)
        .map_err(|error| anyhow::anyhow!("File is not valid UTF-8: {error}"))?;
    slice_text_by_range(&text, range)
}

fn slice_text_by_range(text: &str, range: Option<ReadFileRange>) -> anyhow::Result<String> {
    let Some(range) = range.and_then(ReadFileRange::normalized) else {
        return Ok(text.to_string());
    };
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let start_index = range
        .start_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let end_index = range.end_line.max(range.start_line).min(lines.len());
    Ok(lines[start_index..end_index].join("\n"))
}

fn perform_range_replacement(
    current_content: &str,
    range: ReadFileRange,
    expected_hash: &str,
    replacement: &str,
) -> anyhow::Result<String> {
    let range = range
        .normalized()
        .ok_or_else(|| anyhow::anyhow!("replace_range requires a valid 1-based line range"))?;
    let mut lines = current_content
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(anyhow::anyhow!(
            "replace_range cannot target an empty file; use WriteFile if full-file creation is intended"
        ));
    }
    let start_index = range
        .start_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let end_index = range.end_line.min(lines.len()).max(start_index + 1);
    let current_range_content = lines[start_index..end_index].join("\n");
    let current_hash = stable_content_hash(&current_range_content);
    if current_hash != expected_hash.trim() {
        return Err(anyhow::anyhow!(
            "replace_range hash mismatch for lines {}: expected_hash={} current content_hash={current_hash}. Reread the exact range before editing.",
            range.label(),
            expected_hash.trim()
        ));
    }
    let replacement_lines = replacement.lines().map(str::to_string).collect::<Vec<_>>();
    lines.splice(start_index..end_index, replacement_lines);
    let mut updated = lines.join("\n");
    if current_content.ends_with('\n') {
        updated.push('\n');
    }
    Ok(updated)
}

fn apply_toml_operations(
    current_content: &str,
    expected_hash: &str,
    operations: &[TomlEditOperation],
) -> anyhow::Result<String> {
    let current_hash = stable_content_hash(current_content);
    if current_hash != expected_hash.trim() {
        return Err(anyhow::anyhow!(
            "modify_toml hash mismatch: expected_hash={} current full-file content_hash={current_hash}. Read the full manifest first; partial range hashes are not accepted for TOML edits.",
            expected_hash.trim()
        ));
    }
    let mut document = current_content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| anyhow::anyhow!("current TOML did not parse: {error}"))?;
    for operation in operations {
        apply_toml_operation(&mut document, current_content, operation)?;
    }
    let updated = document.to_string();
    updated
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| anyhow::anyhow!("updated TOML did not parse: {error}"))?;
    Ok(updated)
}

fn apply_toml_operation(
    document: &mut toml_edit::DocumentMut,
    original_content: &str,
    operation: &TomlEditOperation,
) -> anyhow::Result<()> {
    match operation {
        TomlEditOperation::SetDependency {
            table,
            name,
            version,
            features,
            default_features,
            optional,
            package,
            path,
        } => {
            validate_dependency_table(document, original_content, table)?;
            ensure_dependency_table(document, table)?;
            let table_item = document
                .as_table_mut()
                .get_mut(table)
                .ok_or_else(|| anyhow::anyhow!("TOML table `{table}` was not available"))?;
            let table = table_item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("TOML item `{table}` is not a table"))?;
            table.insert(
                name,
                dependency_item(
                    version,
                    features,
                    *default_features,
                    *optional,
                    package,
                    path,
                ),
            );
            Ok(())
        }
        TomlEditOperation::RemoveDependency { table, name } => {
            validate_dependency_table(document, original_content, table)?;
            if let Some(table_item) = document.as_table_mut().get_mut(table)
                && let Some(table) = table_item.as_table_mut()
            {
                table.remove(name);
            }
            Ok(())
        }
    }
}

fn validate_dependency_table(
    document: &toml_edit::DocumentMut,
    original_content: &str,
    table: &str,
) -> anyhow::Result<()> {
    match table {
        "dependencies" | "dev-dependencies" | "build-dependencies" => Ok(()),
        value if value.starts_with("target.") => {
            if toml_header_exists(original_content, value)
                && document.as_table().get(value).is_some()
            {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "target-specific dependency table `{value}` must already exist as a directly editable table; use ReplaceRange for complex dotted/quoted target tables"
                ))
            }
        }
        other => Err(anyhow::anyhow!(
            "unsupported dependency table `{other}`. Use dependencies, dev-dependencies, build-dependencies, or an already-present target-specific table."
        )),
    }
}

fn toml_header_exists(content: &str, table: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .is_some_and(|header| header.trim() == table)
    })
}

fn ensure_dependency_table(
    document: &mut toml_edit::DocumentMut,
    table: &str,
) -> anyhow::Result<()> {
    if document.as_table().get(table).is_none() {
        document
            .as_table_mut()
            .insert(table, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    document
        .as_table_mut()
        .get_mut(table)
        .and_then(toml_edit::Item::as_table_mut)
        .map(|_| ())
        .ok_or_else(|| anyhow::anyhow!("TOML item `{table}` is not a table"))
}

fn dependency_item(
    version: &Option<String>,
    features: &[String],
    default_features: Option<bool>,
    optional: Option<bool>,
    package: &Option<String>,
    path: &Option<String>,
) -> toml_edit::Item {
    let needs_inline_table = !features.is_empty()
        || default_features.is_some()
        || optional.is_some()
        || package.is_some()
        || path.is_some();
    if !needs_inline_table {
        return toml_edit::value(version.as_deref().unwrap_or("*"));
    }

    let mut table = toml_edit::InlineTable::new();
    if let Some(version) = version.as_deref() {
        table.insert("version", toml_edit::Value::from(version));
    }
    if let Some(path) = path.as_deref() {
        table.insert("path", toml_edit::Value::from(path));
    }
    if let Some(package) = package.as_deref() {
        table.insert("package", toml_edit::Value::from(package));
    }
    if !features.is_empty() {
        let mut array = toml_edit::Array::new();
        for feature in features {
            array.push(feature.as_str());
        }
        table.insert("features", toml_edit::Value::Array(array));
    }
    if let Some(default_features) = default_features {
        table.insert("default-features", toml_edit::Value::from(default_features));
    }
    if let Some(optional) = optional {
        table.insert("optional", toml_edit::Value::from(optional));
    }
    table.fmt();
    toml_edit::Item::Value(toml_edit::Value::InlineTable(table))
}

fn count_file_lines(path: &Path) -> anyhow::Result<usize> {
    let text = read_file_contents(path, None)?;
    Ok(text.lines().count())
}

fn list_directory_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    if !path.exists() {
        return Err(anyhow::anyhow!("Path does not exist"));
    }
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(anyhow::anyhow!("Path is not a directory"));
    }
    let mut names = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            let metadata = entry.metadata().ok()?;
            let mut name = file_name;
            if metadata.is_dir() {
                name.push('/');
            }
            Some(name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.truncate(DIRECTORY_LIST_LIMIT);
    let mut output = Vec::new();
    for name in names {
        let mut line = name.clone();
        if name.len() > DIRECTORY_NAME_LIMIT {
            let truncated = DIRECTORY_NAME_LIMIT.saturating_sub(3);
            if truncated > 0 {
                if name.ends_with('/') {
                    line = format!(
                        "{}...",
                        &name[..truncated.min(name.len().saturating_sub(1))]
                    );
                    if !line.ends_with('/') {
                        line.push('/');
                    }
                } else {
                    line = format!("{}...", &name[..truncated]);
                }
            }
        }
        output.push(line);
    }
    Ok(output)
}

fn write_full_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(anyhow::anyhow!(
                "Parent directory does not exist: {parent:?}"
            ));
        }
        if !parent.is_dir() {
            return Err(anyhow::anyhow!("Parent path is not a directory"));
        }
    } else {
        return Err(anyhow::anyhow!("Invalid file path"));
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Invalid target path"))?
        .to_string_lossy()
        .replace(['/', '\\'], "_");
    let tmp = path.with_file_name(format!(".{filename}.tmp.{nanos}"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn write_full_file_allow_create(path: &Path, content: &str) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| anyhow::anyhow!("Failed to create parent directory: {error}"))?;
    write_full_file(path, content)
}

#[cfg(unix)]
fn set_executable_bit(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("Path is not a regular file"));
    }
    let mut permissions = metadata.permissions();
    let mode = permissions.mode();
    permissions.set_mode(mode | 0o111);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_bit(_path: &Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "set_executable is only supported on unix-like systems"
    ))
}

fn parse_multi_file_patch(patch_text: &str) -> anyhow::Result<Vec<FilePatch>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().is_empty() {
        return Ok(Vec::new());
    }
    if normalized.trim().starts_with("*** Begin Patch") {
        return parse_model_patch(&normalized);
    }

    let file_header = regex::Regex::new(r"^---\s+(?:a/)?(.+?)(?:\s+\d{4}-\d{2}-\d{2}.*)?$")
        .map_err(|error| anyhow::anyhow!("Failed to compile file header regex: {error}"))?;
    let new_file_header = regex::Regex::new(r"^\+\+\+\s+(?:b/)?(.+?)(?:\s+\d{4}-\d{2}-\d{2}.*)?$")
        .map_err(|error| anyhow::anyhow!("Failed to compile new file header regex: {error}"))?;
    let hunk_header = regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
        .map_err(|error| anyhow::anyhow!("Failed to compile hunk header regex: {error}"))?;
    let rename_from_re = regex::Regex::new(r"^rename from (.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile rename-from regex: {error}"))?;
    let rename_to_re = regex::Regex::new(r"^rename to (.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile rename-to regex: {error}"))?;

    let mut file_patches = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_path: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;

    for line in normalized.lines() {
        if line.starts_with("diff --git ") || line.starts_with("diff -") {
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
                    file_patches.push(file);
                }
            }
            old_path = None;
            rename_from = None;
            rename_to = None;
            continue;
        }

        if let Some(caps) = rename_from_re.captures(line) {
            rename_from = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }
        if let Some(caps) = rename_to_re.captures(line) {
            rename_to = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }

        if let Some(caps) = file_header.captures(line) {
            if let Some(file) = current_file.as_mut()
                && let Some(hunk) = current_hunk.take()
            {
                file.hunks.push(hunk);
            }
            old_path = caps.get(1).map(|value| value.as_str().to_string());
            continue;
        }

        if let Some(caps) = new_file_header.captures(line) {
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
                    file_patches.push(file);
                }
            }

            let new_path = caps
                .get(1)
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            let (path, operation) = if let (Some(rename_from), Some(rename_to)) =
                (rename_from.clone(), rename_to.clone())
            {
                (
                    rename_from,
                    PatchOperation::Move {
                        move_path: rename_to,
                    },
                )
            } else if old_path.as_deref() == Some("/dev/null") {
                (new_path.clone(), PatchOperation::Add)
            } else if new_path == "/dev/null" {
                (old_path.clone().unwrap_or_default(), PatchOperation::Delete)
            } else {
                (
                    old_path.clone().unwrap_or_else(|| new_path.clone()),
                    PatchOperation::Update,
                )
            };

            if path.is_empty() || path == "/dev/null" {
                continue;
            }

            current_file = Some(FilePatch {
                path,
                operation,
                hunks: Vec::new(),
            });
            continue;
        }

        if let Some(caps) = hunk_header.captures(line) {
            let Some(file) = current_file.as_mut() else {
                return Err(anyhow::anyhow!(
                    "Found a hunk header before a file header in apply_patch input"
                ));
            };
            if let Some(hunk) = current_hunk.take() {
                file.hunks.push(hunk);
            }
            let old_start = caps
                .get(1)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let old_count = caps
                .get(2)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let new_start = caps
                .get(3)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let new_count = caps
                .get(4)
                .and_then(|value| value.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            current_hunk = Some(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            if let Some(stripped) = line.strip_prefix(' ') {
                hunk.lines.push(PatchLine::Context(stripped.to_string()));
            } else if let Some(stripped) = (!line.starts_with("--- "))
                .then(|| line.strip_prefix('-'))
                .flatten()
            {
                hunk.lines.push(PatchLine::Remove(stripped.to_string()));
            } else if let Some(stripped) = (!line.starts_with("+++ "))
                .then(|| line.strip_prefix('+'))
                .flatten()
            {
                hunk.lines.push(PatchLine::Add(stripped.to_string()));
            } else if line.starts_with('\\') {
                continue;
            }
        }
    }

    if let Some(mut file) = current_file {
        if let Some(hunk) = current_hunk {
            file.hunks.push(hunk);
        }
        if !file.hunks.is_empty() || matches!(file.operation, PatchOperation::Delete) {
            file_patches.push(file);
        }
    }

    Ok(file_patches)
}

fn parse_model_patch(patch_text: &str) -> anyhow::Result<Vec<FilePatch>> {
    let begin_file = regex::Regex::new(r"^\*\*\*\s+Begin\s+File:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile model patch regex: {error}"))?;
    let delete_file = regex::Regex::new(r"^\*\*\*\s+Delete\s+File:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile delete regex: {error}"))?;
    let end_patch = regex::Regex::new(r"^\*\*\*\s+End\s+Patch$")
        .map_err(|error| anyhow::anyhow!("Failed to compile end regex: {error}"))?;
    let move_to = regex::Regex::new(r"^\*\*\*\s+Move\s+To:\s+(.+)$")
        .map_err(|error| anyhow::anyhow!("Failed to compile move regex: {error}"))?;

    let mut file_patches = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut content_lines = Vec::new();

    for line in patch_text.lines() {
        if line == "*** Begin Patch" {
            continue;
        }
        if let Some(caps) = delete_file.captures(line) {
            let path = caps
                .get(1)
                .map(|value| value.as_str().trim().to_string())
                .unwrap_or_default();
            if !path.is_empty() {
                file_patches.push(FilePatch {
                    path,
                    operation: PatchOperation::Delete,
                    hunks: Vec::new(),
                });
            }
            current_file = None;
            content_lines.clear();
            continue;
        }
        if let Some(caps) = move_to.captures(line) {
            if let Some(file) = current_file.as_mut()
                && let Some(target) = caps.get(1).map(|value| value.as_str().trim())
            {
                file.operation = PatchOperation::Move {
                    move_path: target.to_string(),
                };
            }
            continue;
        }
        if let Some(caps) = begin_file.captures(line) {
            current_file = Some(FilePatch {
                path: caps
                    .get(1)
                    .map(|value| value.as_str().trim().to_string())
                    .unwrap_or_default(),
                operation: PatchOperation::Add,
                hunks: Vec::new(),
            });
            content_lines.clear();
            continue;
        }
        if line == "*** End File" {
            if let Some(mut file) = current_file.take() {
                let hunk = Hunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 1,
                    new_count: content_lines.len(),
                    lines: content_lines.drain(..).map(PatchLine::Add).collect(),
                };
                file.hunks.push(hunk);
                file_patches.push(file);
            }
            continue;
        }
        if end_patch.is_match(line) {
            break;
        }
        if current_file.is_some() {
            content_lines.push(line.to_string());
        }
    }

    Ok(file_patches)
}

fn resolve_file_patches(
    project_root: &Path,
    cwd: &Path,
    file_patches: &[FilePatch],
) -> anyhow::Result<Vec<ResolvedFilePatch>> {
    let mut resolved = Vec::with_capacity(file_patches.len());
    for file_patch in file_patches {
        let source_path = sanitize_project_path(project_root, cwd, &file_patch.path)?;
        let target_path = match &file_patch.operation {
            PatchOperation::Move { move_path } => {
                sanitize_project_path(project_root, cwd, move_path)?
            }
            _ => source_path.clone(),
        };
        let new_content = match &file_patch.operation {
            PatchOperation::Add => Some(render_added_file_content(&file_patch.hunks)),
            PatchOperation::Update | PatchOperation::Move { .. } => {
                let current_content = std::fs::read_to_string(&source_path)
                    .map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
                Some(apply_hunks(&current_content, &file_patch.hunks)?)
            }
            PatchOperation::Delete => None,
        };
        resolved.push(ResolvedFilePatch {
            source_path,
            target_path,
            display_path: file_patch.path.clone(),
            operation: file_patch.operation.clone(),
            new_content,
        });
    }
    Ok(resolved)
}

fn render_added_file_content(hunks: &[Hunk]) -> String {
    let lines = hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| match line {
            PatchLine::Add(text) => Some(text.clone()),
            PatchLine::Context(_) | PatchLine::Remove(_) => None,
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        let mut content = lines.join("\n");
        content.push('\n');
        content
    }
}

fn apply_resolved_file_patches(resolved: &[ResolvedFilePatch]) -> anyhow::Result<()> {
    for patch in resolved {
        match &patch.operation {
            PatchOperation::Add | PatchOperation::Update => {
                let content = patch
                    .new_content
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("apply_patch resolved without new content"))?;
                write_full_file_allow_create(&patch.target_path, content)?;
            }
            PatchOperation::Delete => {
                if patch.source_path.exists() {
                    std::fs::remove_file(&patch.source_path)
                        .map_err(|error| anyhow::anyhow!("Failed to delete file: {error}"))?;
                }
            }
            PatchOperation::Move { .. } => {
                let content = patch.new_content.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("apply_patch resolved move without new content")
                })?;
                write_full_file_allow_create(&patch.target_path, content)?;
                if patch.source_path.exists() {
                    std::fs::remove_file(&patch.source_path).map_err(|error| {
                        anyhow::anyhow!("Failed to remove moved source file: {error}")
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn apply_hunks(original: &str, hunks: &[Hunk]) -> anyhow::Result<String> {
    let had_trailing_newline = original.ends_with('\n');
    let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
    let mut offset = 0isize;

    for hunk in hunks {
        let expected_old = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchLine::Context(text) | PatchLine::Remove(text) => Some(text.clone()),
                PatchLine::Add(_) => None,
            })
            .collect::<Vec<_>>();
        let replacement = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchLine::Context(text) | PatchLine::Add(text) => Some(text.clone()),
                PatchLine::Remove(_) => None,
            })
            .collect::<Vec<_>>();

        let expected_old_count = hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Remove(_)))
            .count();
        let expected_new_count = hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Add(_)))
            .count();
        if hunk.old_count > 0 && expected_old_count != hunk.old_count {
            return Err(anyhow::anyhow!(
                "Malformed hunk for line {}: expected {} old lines but found {}",
                hunk.old_start,
                hunk.old_count,
                expected_old_count
            ));
        }
        if hunk.new_count > 0 && expected_new_count != hunk.new_count {
            return Err(anyhow::anyhow!(
                "Malformed hunk for line {}: expected {} new lines but found {}",
                hunk.new_start,
                hunk.new_count,
                expected_new_count
            ));
        }

        let preferred_index = (hunk.old_start as isize + offset - 1).max(0) as usize;
        let start_index = if exact_line_match(&lines, preferred_index, &expected_old) {
            preferred_index
        } else if expected_old.is_empty() {
            preferred_index.min(lines.len())
        } else {
            let matches = find_exact_hunk_matches(&lines, &expected_old);
            match matches.as_slice() {
                [index] => *index,
                [] => {
                    return Err(anyhow::anyhow!(
                        "Could not locate hunk context for {}",
                        hunk.old_start
                    ));
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Patch hunk is ambiguous for {}",
                        hunk.old_start
                    ));
                }
            }
        };

        let range_end = start_index
            .saturating_add(expected_old.len())
            .min(lines.len());
        lines.splice(start_index..range_end, replacement.clone());
        offset += replacement.len() as isize - expected_old.len() as isize;
    }

    let mut rendered = lines.join("\n");
    if had_trailing_newline && !rendered.is_empty() {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn exact_line_match(lines: &[String], start: usize, expected: &[String]) -> bool {
    if start > lines.len() || start.saturating_add(expected.len()) > lines.len() {
        return false;
    }
    lines[start..start + expected.len()]
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual == expected)
}

fn find_exact_hunk_matches(lines: &[String], expected: &[String]) -> Vec<usize> {
    if expected.is_empty() {
        return vec![lines.len()];
    }
    if expected.len() > lines.len() {
        return Vec::new();
    }
    (0..=lines.len() - expected.len())
        .filter(|index| exact_line_match(lines, *index, expected))
        .collect()
}

fn try_parse_search_replace_blocks(patch: &str) -> Option<Vec<(String, String)>> {
    let mut blocks = Vec::new();
    let mut current_search = String::new();
    let mut current_replace = String::new();
    let mut state = 0; // 0=outside, 1=inside search, 2=inside replace

    for line in patch.split_inclusive('\n') {
        if line.starts_with("<<<<") {
            state = 1;
            current_search.clear();
            current_replace.clear();
            continue;
        } else if line.starts_with("====") && state == 1 {
            state = 2;
            continue;
        } else if line.starts_with(">>>>") && state == 2 {
            // Trim leading/trailing newlines specifically caused by markers
            let search_trim = current_search
                .strip_suffix('\n')
                .unwrap_or(&current_search)
                .to_string();
            let replace_trim = current_replace
                .strip_suffix('\n')
                .unwrap_or(&current_replace)
                .to_string();
            blocks.push((search_trim, replace_trim));
            state = 0;
            continue;
        }

        if state == 1 {
            current_search.push_str(line);
        } else if state == 2 {
            current_replace.push_str(line);
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

fn perform_block_replacement(
    current_content: &str,
    search_block: &str,
    replace_block: &str,
    range: Option<ReadFileRange>,
) -> anyhow::Result<String> {
    let first_result =
        perform_block_replacement_inner(current_content, search_block, replace_block, range);
    if first_result.is_ok() || !should_retry_literal_newline_block(search_block) {
        return first_result;
    }

    let normalized_search_block = search_block.replace("\\n", "\n");
    let normalized_replace_block = if should_retry_literal_newline_block(replace_block) {
        replace_block.replace("\\n", "\n")
    } else {
        replace_block.to_string()
    };
    perform_block_replacement_inner(
        current_content,
        &normalized_search_block,
        &normalized_replace_block,
        range,
    )
    .map_err(|normalized_error| {
        anyhow::anyhow!(
            "Could not apply ReplaceBlock as written, and retrying literal \\n as line breaks also failed: {normalized_error}"
        )
    })
}

fn perform_block_replacement_inner(
    current_content: &str,
    search_block: &str,
    replace_block: &str,
    range: Option<ReadFileRange>,
) -> anyhow::Result<String> {
    if current_content.is_empty() {
        if search_block.trim().is_empty() {
            return Ok(replace_block.to_string());
        }
        return Err(anyhow::anyhow!("File is empty but search block is not"));
    }

    let normalized_range = range.and_then(ReadFileRange::normalized);
    let exact_matches = exact_block_matches(current_content, search_block);
    let exact_candidates = filter_matches_by_range(&exact_matches, normalized_range);
    if let Some(candidate) = unique_replacement_candidate(
        "Search block",
        &exact_matches,
        &exact_candidates,
        normalized_range,
    )? {
        return Ok(replace_block_match(
            current_content,
            candidate,
            replace_block,
        ));
    }

    let search_lines: Vec<&str> = search_block.lines().collect();
    let content_lines: Vec<&str> = current_content.lines().collect();

    if search_lines.is_empty() {
        return Err(anyhow::anyhow!("Search block is empty"));
    }

    let mut fuzzy_matches = Vec::new();

    if search_lines.len() <= content_lines.len() {
        let line_spans = line_spans(current_content);
        for index in 0..=(content_lines.len() - search_lines.len()) {
            let mut matched = true;
            for search_index in 0..search_lines.len() {
                if content_lines[index + search_index].trim() != search_lines[search_index].trim() {
                    matched = false;
                    break;
                }
            }
            if matched {
                let end_index = index + search_lines.len().saturating_sub(1);
                if let (Some(start), Some(end)) = (line_spans.get(index), line_spans.get(end_index))
                {
                    fuzzy_matches.push(BlockReplacementMatch {
                        start_byte: start.0,
                        end_byte: end.1,
                        start_line: index + 1,
                        end_line: end_index + 1,
                    });
                }
            }
        }
    }
    let fuzzy_candidates = filter_matches_by_range(&fuzzy_matches, normalized_range);

    if let Some(candidate) = unique_replacement_candidate(
        "Search block",
        &fuzzy_matches,
        &fuzzy_candidates,
        normalized_range,
    )? {
        return Ok(replace_block_match(
            current_content,
            candidate,
            replace_block,
        ));
    }

    if fuzzy_matches.is_empty() {
        return Err(anyhow::anyhow!(
            "Could not find the search block in the file (even ignoring whitespace)"
        ));
    }
    Err(anyhow::anyhow!(
        "Search block is ambiguous; found {} matches at lines {}. Include enough surrounding context to make the search block unique, add a `range`, or use ApplyPatch/WriteFile.",
        fuzzy_matches.len(),
        format_match_lines(&fuzzy_matches)
    ))
}

fn should_retry_literal_newline_block(block: &str) -> bool {
    block.contains("\\n") && !block.contains('\n')
}

#[derive(Debug, Clone, Copy)]
struct BlockReplacementMatch {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
}

fn exact_block_matches(current_content: &str, search_block: &str) -> Vec<BlockReplacementMatch> {
    if search_block.is_empty() {
        return Vec::new();
    }
    let line_count = search_block.lines().count().max(1);
    current_content
        .match_indices(search_block)
        .map(|(start_byte, matched)| {
            let start_line = byte_line_number(current_content, start_byte);
            BlockReplacementMatch {
                start_byte,
                end_byte: start_byte + matched.len(),
                start_line,
                end_line: start_line + line_count.saturating_sub(1),
            }
        })
        .collect()
}

fn filter_matches_by_range(
    matches: &[BlockReplacementMatch],
    range: Option<ReadFileRange>,
) -> Vec<BlockReplacementMatch> {
    let Some(range) = range else {
        return matches.to_vec();
    };
    matches
        .iter()
        .copied()
        .filter(|candidate| {
            candidate.start_line >= range.start_line && candidate.end_line <= range.end_line
        })
        .collect()
}

fn unique_replacement_candidate(
    label: &str,
    all_matches: &[BlockReplacementMatch],
    candidates: &[BlockReplacementMatch],
    range: Option<ReadFileRange>,
) -> anyhow::Result<Option<BlockReplacementMatch>> {
    match candidates {
        [] => {
            if let Some(range) = range
                && !all_matches.is_empty()
            {
                return Err(anyhow::anyhow!(
                    "{label} has {} matches at lines {}, but none are fully inside requested range {}. Reread the file or provide a fresh range before patching.",
                    all_matches.len(),
                    format_match_lines(all_matches),
                    range.label()
                ));
            }
            Ok(None)
        }
        [candidate] => Ok(Some(*candidate)),
        _ => {
            let range_note = range
                .map(|range| format!(" inside requested range {}", range.label()))
                .unwrap_or_default();
            Err(anyhow::anyhow!(
                "{label} is ambiguous; found {} matches{} at lines {}. Include enough surrounding context to make the search block unique, use a narrower `range`, or use ApplyPatch/WriteFile.",
                candidates.len(),
                range_note,
                format_match_lines(candidates)
            ))
        }
    }
}

fn replace_block_match(
    current_content: &str,
    candidate: BlockReplacementMatch,
    replace_block: &str,
) -> String {
    let mut out = String::with_capacity(
        current_content
            .len()
            .saturating_sub(candidate.end_byte.saturating_sub(candidate.start_byte))
            .saturating_add(replace_block.len())
            .saturating_add(1),
    );
    out.push_str(&current_content[..candidate.start_byte]);
    out.push_str(replace_block);
    if !replace_block.ends_with('\n') && !replace_block.is_empty() {
        out.push('\n');
    }
    out.push_str(&current_content[candidate.end_byte..]);
    out
}

fn byte_line_number(text: &str, byte_index: usize) -> usize {
    text[..byte_index.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        spans.push((start, end));
        start = end;
    }
    if !text.ends_with('\n') && start < text.len() {
        spans.push((start, text.len()));
    }
    spans
}

fn format_match_lines(matches: &[BlockReplacementMatch]) -> String {
    let mut rendered = matches
        .iter()
        .take(8)
        .map(|candidate| candidate.start_line.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if matches.len() > 8 {
        rendered.push_str(", ...");
    }
    rendered
}

fn sanitize_project_path(
    project_root: &Path,
    cwd: &Path,
    request_path: &str,
) -> anyhow::Result<PathBuf> {
    if request_path.trim().is_empty() {
        crate::quorp::tui::diagnostics::log_event(
            "agent.path_resolution_failed",
            serde_json::json!({
                "project_root": project_root.display().to_string(),
                "cwd": cwd.display().to_string(),
                "request_path": request_path,
                "error": "Path cannot be empty",
            }),
        );
        return Err(anyhow::anyhow!("Path cannot be empty"));
    }

    let project_root = if project_root.as_os_str().is_empty() {
        cwd
    } else {
        project_root
    };
    let requested = Path::new(request_path);
    let request_relative = if requested.is_absolute() {
        if let Ok(stripped) = requested.strip_prefix(project_root) {
            stripped.to_path_buf()
        } else {
            let canonical_root = project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.to_path_buf());
            let canonical_requested_parent = requested
                .parent()
                .and_then(|parent| parent.canonicalize().ok());
            if let Some(canonical_parent) = canonical_requested_parent {
                if canonical_parent.starts_with(&canonical_root) {
                    requested
                        .strip_prefix(&canonical_root)
                        .map(Path::to_path_buf)
                        .map_err(|_| anyhow::anyhow!("Absolute paths are not allowed"))?
                } else {
                    let error_text = "Absolute paths are not allowed";
                    crate::quorp::tui::diagnostics::log_event(
                        "agent.path_resolution_failed",
                        serde_json::json!({
                            "project_root": project_root.display().to_string(),
                            "cwd": cwd.display().to_string(),
                            "request_path": request_path,
                            "error": error_text,
                        }),
                    );
                    return Err(anyhow::anyhow!(error_text));
                }
            } else {
                let error_text = "Absolute paths are not allowed";
                crate::quorp::tui::diagnostics::log_event(
                    "agent.path_resolution_failed",
                    serde_json::json!({
                        "project_root": project_root.display().to_string(),
                        "cwd": cwd.display().to_string(),
                        "request_path": request_path,
                        "error": error_text,
                    }),
                );
                return Err(anyhow::anyhow!(error_text));
            }
        }
    } else {
        requested.to_path_buf()
    };
    let mut candidate = PathBuf::new();
    for component in request_relative.components() {
        use std::path::Component;
        match component {
            Component::Normal(part) => candidate.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                let error_text = "Parent directory traversal is not allowed";
                crate::quorp::tui::diagnostics::log_event(
                    "agent.path_resolution_failed",
                    serde_json::json!({
                        "project_root": project_root.display().to_string(),
                        "cwd": cwd.display().to_string(),
                        "request_path": request_path,
                        "error": error_text,
                    }),
                );
                return Err(anyhow::anyhow!(error_text));
            }
            Component::RootDir | Component::Prefix(_) => {
                let error_text = "Absolute-like paths are not allowed";
                crate::quorp::tui::diagnostics::log_event(
                    "agent.path_resolution_failed",
                    serde_json::json!({
                        "project_root": project_root.display().to_string(),
                        "cwd": cwd.display().to_string(),
                        "request_path": request_path,
                        "error": error_text,
                    }),
                );
                return Err(anyhow::anyhow!(error_text));
            }
        }
    }

    let resolved = project_root.join(candidate);
    if !resolved.starts_with(project_root) {
        let error_text = "Path resolved outside project root";
        crate::quorp::tui::diagnostics::log_event(
            "agent.path_resolution_failed",
            serde_json::json!({
                "project_root": project_root.display().to_string(),
                "cwd": cwd.display().to_string(),
                "request_path": request_path,
                "error": error_text,
            }),
        );
        return Err(anyhow::anyhow!(error_text));
    }

    if !resolved.exists() {
        crate::quorp::tui::diagnostics::log_event(
            "agent.path_resolved",
            serde_json::json!({
                "project_root": project_root.display().to_string(),
                "cwd": cwd.display().to_string(),
                "request_path": request_path,
                "resolved_path": resolved.display().to_string(),
                "exists": false,
            }),
        );
        return Ok(resolved);
    }

    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_target = resolved
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("Failed to resolve target path: {error}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        let error_text = "Path resolved outside project root";
        crate::quorp::tui::diagnostics::log_event(
            "agent.path_resolution_failed",
            serde_json::json!({
                "project_root": project_root.display().to_string(),
                "cwd": cwd.display().to_string(),
                "request_path": request_path,
                "error": error_text,
            }),
        );
        return Err(anyhow::anyhow!(error_text));
    }
    crate::quorp::tui::diagnostics::log_event(
        "agent.path_resolved",
        serde_json::json!({
            "project_root": project_root.display().to_string(),
            "cwd": cwd.display().to_string(),
            "request_path": request_path,
            "resolved_path": canonical_target.display().to_string(),
            "exists": true,
        }),
    );
    Ok(canonical_target)
}

fn truncate_diagnostic_text(text: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            break;
        }
        truncated.push(ch);
    }
    truncated
}

fn run_command_streaming(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    command: &str,
    cwd: &Path,
    timeout: Duration,
    command_output_limit: usize,
) -> anyhow::Result<String> {
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
                    if let Ok(mut full_output) = output_for_reader.lock()
                        && full_output.len() < command_output_limit
                    {
                        full_output.push_str(&text);
                        if full_output.len() > command_output_limit {
                            full_output.truncate(command_output_limit);
                        }
                    }
                    send_chat_event(
                        &event_tx_for_reader,
                        crate::quorp::tui::chat::ChatUiEvent::CommandOutput(session_id, text),
                    );
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

    if reader_thread.join().is_err() {
        log::error!("tui: command reader thread panicked");
    }

    let mut final_output = output
        .lock()
        .map(|captured| captured.clone())
        .unwrap_or_default();
    if exit_code == Some(-1) {
        final_output.push_str("\n[Command timed out]");
        anyhow::bail!(final_output);
    }
    let exit_code = exit_code.unwrap_or_default();
    final_output.push_str(&format!("\n[Exit code: {exit_code}]"));
    if exit_code == 0 {
        Ok(final_output)
    } else {
        anyhow::bail!(final_output);
    }
}

struct NativeTerminalService {
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    workspace_root: PathBuf,
    session: Option<TerminalSession>,
    terminal_focused: bool,
}

impl NativeTerminalService {
    fn new(event_tx: std::sync::mpsc::SyncSender<TuiEvent>, workspace_root: PathBuf) -> Self {
        Self {
            event_tx,
            workspace_root,
            session: None,
            terminal_focused: false,
        }
    }

    fn ensure_session(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(session) = self.session.as_mut() {
            session.resize(cols, rows)?;
            return Ok(());
        }
        self.session = Some(TerminalSession::spawn(
            self.event_tx.clone(),
            &self.workspace_root,
            cols,
            rows,
            self.terminal_focused,
        )?);
        Ok(())
    }

    fn set_terminal_focused(&mut self, focused: bool) -> anyhow::Result<()> {
        self.terminal_focused = focused;
        if let Some(session) = self.session.as_mut() {
            session.set_focused(focused)?;
        }
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

    fn scroll_page_up(&mut self) -> anyhow::Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session.scroll_page_up()
    }

    fn scroll_page_down(&mut self) -> anyhow::Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session.scroll_page_down()
    }
}

struct TerminalSession {
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    state: Arc<Mutex<TerminalState>>,
    current_cwd: Arc<Mutex<Option<PathBuf>>>,
    shell_label: String,
    shutdown: Arc<AtomicBool>,
    trace: Option<SharedTerminalTraceBuffer>,
    _reader_thread: std::thread::JoinHandle<()>,
    _metadata_thread: std::thread::JoinHandle<()>,
}

struct TerminalState {
    parser: vt100::Parser,
    rows: u16,
    focused: bool,
}

impl TerminalSession {
    fn spawn(
        event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
        workspace_root: &Path,
        cols: u16,
        rows: u16,
        focused: bool,
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
        let shell_label = shell_label_from_program(&shell);
        let state = Arc::new(Mutex::new(TerminalState {
            parser: crate::quorp::tui::terminal_surface::new_parser(
                rows,
                cols,
                TERMINAL_SCROLLBACK_LINES,
            ),
            rows,
            focused,
        }));
        let current_cwd = Arc::new(Mutex::new(Some(workspace_root.to_path_buf())));
        let shutdown = Arc::new(AtomicBool::new(false));
        let trace = if cfg!(any(test, debug_assertions)) {
            Some(new_shared_terminal_trace())
        } else {
            None
        };
        record_trace(
            trace.as_ref(),
            format!("spawn cols={cols} rows={rows} focused={focused}"),
        );
        let mut builder = CommandBuilder::new(shell);
        builder.arg("-i");
        builder.cwd(workspace_root);
        let child = pair.slave.spawn_command(builder)?;
        let child_pid = child.process_id();
        drop(pair.slave);
        let state_for_reader = Arc::clone(&state);
        let cwd_for_reader = Arc::clone(&current_cwd);
        let shell_label_for_reader = shell_label.clone();
        let event_tx_for_reader = event_tx.clone();
        let shutdown_for_reader = Arc::clone(&shutdown);
        let trace_for_reader = trace.clone();

        let reader_thread = std::thread::spawn(move || {
            let _child = child;
            let mut chunk = [0u8; 4096];
            let mut last_publish_at = Instant::now()
                .checked_sub(TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL)
                .unwrap_or_else(Instant::now);
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        shutdown_for_reader.store(true, Ordering::Relaxed);
                        emit_terminal_close(
                            &event_tx_for_reader,
                            &state_for_reader,
                            &cwd_for_reader,
                            &shell_label_for_reader,
                            trace_for_reader.as_ref(),
                            "reader-eof",
                        );
                        break;
                    }
                    Ok(read_len) => {
                        record_trace(
                            trace_for_reader.as_ref(),
                            format!(
                                "pty-read len={} bytes={}",
                                read_len,
                                debug_bytes(&chunk[..read_len])
                            ),
                        );
                        {
                            let mut state = lock_or_recover(&state_for_reader);
                            state.parser.process(&chunk[..read_len]);
                        }
                        let now = Instant::now();
                        if !should_publish_terminal_frame(&state_for_reader, last_publish_at, now) {
                            continue;
                        }
                        publish_terminal_frame(
                            &event_tx_for_reader,
                            &state_for_reader,
                            &cwd_for_reader,
                            &shell_label_for_reader,
                            trace_for_reader.as_ref(),
                            "pty-read",
                        );
                        last_publish_at = now;
                    }
                    Err(_) => {
                        shutdown_for_reader.store(true, Ordering::Relaxed);
                        emit_terminal_close(
                            &event_tx_for_reader,
                            &state_for_reader,
                            &cwd_for_reader,
                            &shell_label_for_reader,
                            trace_for_reader.as_ref(),
                            "reader-error",
                        );
                        break;
                    }
                }
            }
        });

        let state_for_metadata = Arc::clone(&state);
        let cwd_for_metadata = Arc::clone(&current_cwd);
        let shell_label_for_metadata = shell_label.clone();
        let event_tx_for_metadata = event_tx.clone();
        let shutdown_for_metadata = Arc::clone(&shutdown);
        let trace_for_metadata = trace.clone();
        let metadata_thread = std::thread::spawn(move || {
            let mut last_known_cwd = lock_or_recover(&cwd_for_metadata).clone();
            while !shutdown_for_metadata.load(Ordering::Relaxed) {
                std::thread::sleep(TERMINAL_METADATA_REFRESH_INTERVAL);
                if shutdown_for_metadata.load(Ordering::Relaxed) {
                    break;
                }
                let next_cwd = child_pid.and_then(terminal_session_cwd);
                if next_cwd.is_some() && next_cwd != last_known_cwd {
                    *lock_or_recover(&cwd_for_metadata) = next_cwd.clone();
                    last_known_cwd = next_cwd;
                    record_trace(
                        trace_for_metadata.as_ref(),
                        format!("metadata-refresh cwd={:?}", last_known_cwd),
                    );
                    publish_terminal_frame(
                        &event_tx_for_metadata,
                        &state_for_metadata,
                        &cwd_for_metadata,
                        &shell_label_for_metadata,
                        trace_for_metadata.as_ref(),
                        "metadata-refresh",
                    );
                }
            }
        });

        Ok(Self {
            event_tx,
            master,
            writer,
            state,
            current_cwd,
            shell_label,
            shutdown,
            trace,
            _reader_thread: reader_thread,
            _metadata_thread: metadata_thread,
        })
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        {
            let mut state = lock_or_recover(&self.state);
            state.rows = rows;
            state.parser.set_size(rows, cols);
        }
        record_trace(
            self.trace.as_ref(),
            format!("resize cols={cols} rows={rows}"),
        );
        self.publish_snapshot("resize");
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        record_trace(
            self.trace.as_ref(),
            format!(
                "write-bytes len={} bytes={}",
                bytes.len(),
                debug_bytes(bytes)
            ),
        );
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn set_focused(&mut self, focused: bool) -> anyhow::Result<()> {
        {
            let mut state = lock_or_recover(&self.state);
            state.focused = focused;
        }
        record_trace(self.trace.as_ref(), format!("focused={focused}"));
        self.publish_snapshot("focus-change");
        Ok(())
    }

    fn scroll_page_up(&mut self) -> anyhow::Result<()> {
        {
            let mut state = lock_or_recover(&self.state);
            let page_rows = usize::from(state.rows.saturating_sub(1).max(1));
            let next_scrollback = state.parser.screen().scrollback().saturating_add(page_rows);
            state.parser.set_scrollback(next_scrollback);
        }
        record_trace(self.trace.as_ref(), "scroll-page-up");
        self.publish_snapshot("scroll-page-up");
        Ok(())
    }

    fn scroll_page_down(&mut self) -> anyhow::Result<()> {
        {
            let mut state = lock_or_recover(&self.state);
            let page_rows = usize::from(state.rows.saturating_sub(1).max(1));
            let next_scrollback = state.parser.screen().scrollback().saturating_sub(page_rows);
            state.parser.set_scrollback(next_scrollback);
        }
        record_trace(self.trace.as_ref(), "scroll-page-down");
        self.publish_snapshot("scroll-page-down");
        Ok(())
    }

    fn publish_snapshot(&self, reason: &str) {
        publish_terminal_frame(
            &self.event_tx,
            &self.state,
            &self.current_cwd,
            &self.shell_label,
            self.trace.as_ref(),
            reason,
        );
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

fn publish_terminal_frame(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    state: &Arc<Mutex<TerminalState>>,
    current_cwd: &Arc<Mutex<Option<PathBuf>>>,
    shell_label: &str,
    trace: Option<&SharedTerminalTraceBuffer>,
    reason: &str,
) {
    let state = lock_or_recover(state);
    let snapshot =
        crate::quorp::tui::terminal_surface::TerminalSnapshot::from_screen(state.parser.screen());
    let window_title = crate::quorp::tui::terminal_surface::terminal_window_title(&state.parser);
    let focused = state.focused;
    drop(state);

    let cwd = lock_or_recover(current_cwd).clone();
    let frame = TerminalFrame {
        snapshot,
        cwd,
        shell_label: Some(shell_label.to_string()),
        window_title,
    };
    record_trace(
        trace,
        format!(
            "publish reason={reason} focused={focused} cwd={:?} alt={} scrollback={} cursor={:?}",
            frame.cwd,
            frame.snapshot.alternate_screen,
            frame.snapshot.scrollback,
            frame.snapshot.cursor
        ),
    );
    match event_tx.try_send(TuiEvent::TerminalFrame(frame)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            record_trace(trace, format!("publish-dropped reason={reason}"));
        }
        Err(TrySendError::Disconnected(_)) => {
            record_trace(trace, format!("publish-disconnected reason={reason}"));
        }
    }
}

fn emit_terminal_close(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    state: &Arc<Mutex<TerminalState>>,
    current_cwd: &Arc<Mutex<Option<PathBuf>>>,
    shell_label: &str,
    trace: Option<&SharedTerminalTraceBuffer>,
    reason: &str,
) {
    publish_terminal_frame(event_tx, state, current_cwd, shell_label, trace, reason);
    record_trace(trace, format!("closed reason={reason}"));
    if event_tx.send(TuiEvent::TerminalClosed).is_err() {
        record_trace(trace, "close-disconnected");
    }
}

fn should_publish_terminal_frame(
    state: &Arc<Mutex<TerminalState>>,
    last_publish_at: Instant,
    now: Instant,
) -> bool {
    let focused = lock_or_recover(state).focused;
    now.duration_since(last_publish_at) >= terminal_publish_interval(focused)
}

fn terminal_publish_interval(focused: bool) -> Duration {
    if focused {
        TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL
    } else {
        TERMINAL_UNFOCUSED_FRAME_PUBLISH_INTERVAL
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn shell_label_from_program(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|label| !label.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "shell".to_string())
}

fn debug_bytes(bytes: &[u8]) -> String {
    const LIMIT: usize = 48;
    let mut rendered = String::new();
    for byte in bytes.iter().take(LIMIT) {
        match byte {
            b'\x1b' => rendered.push_str("\\e"),
            b'\r' => rendered.push_str("\\r"),
            b'\n' => rendered.push_str("\\n"),
            b'\t' => rendered.push_str("\\t"),
            0x20..=0x7e => rendered.push(*byte as char),
            _ => rendered.push_str(&format!("\\x{byte:02x}")),
        }
    }
    if bytes.len() > LIMIT {
        rendered.push_str("...");
    }
    rendered
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod terminal_certification_tests {
    use super::*;

    fn sample_terminal_state(text: &[u8], focused: bool) -> Arc<Mutex<TerminalState>> {
        let mut parser = crate::quorp::tui::terminal_surface::new_parser(12, 40, 128);
        parser.process(text);
        Arc::new(Mutex::new(TerminalState {
            parser,
            rows: 12,
            focused,
        }))
    }

    #[test]
    fn terminal_certification_modified_keys_match_xterm_sequences() {
        let ctrl_shift_up = TuiKeystroke {
            key: "up".to_string(),
            modifiers: crate::quorp::tui::bridge::TuiKeyModifiers {
                control: true,
                shift: true,
                alt: false,
            },
        };
        assert_eq!(keystroke_to_bytes(&ctrl_shift_up), b"\x1b[1;6A");

        let alt_ctrl_a = TuiKeystroke {
            key: "a".to_string(),
            modifiers: crate::quorp::tui::bridge::TuiKeyModifiers {
                control: true,
                shift: false,
                alt: true,
            },
        };
        assert_eq!(keystroke_to_bytes(&alt_ctrl_a), vec![0x1b, 0x01]);

        let shift_f5 = TuiKeystroke {
            key: "f5".to_string(),
            modifiers: crate::quorp::tui::bridge::TuiKeyModifiers {
                control: false,
                shift: true,
                alt: false,
            },
        };
        assert_eq!(keystroke_to_bytes(&shift_f5), b"\x1b[15;2~");

        let alt_page_up = TuiKeystroke {
            key: "pageup".to_string(),
            modifiers: crate::quorp::tui::bridge::TuiKeyModifiers {
                control: false,
                shift: false,
                alt: true,
            },
        };
        assert_eq!(keystroke_to_bytes(&alt_page_up), b"\x1b[5;3~");
    }

    #[test]
    fn terminal_certification_focus_controls_publish_interval() {
        assert_eq!(
            terminal_publish_interval(true),
            TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL
        );
        assert_eq!(
            terminal_publish_interval(false),
            TERMINAL_UNFOCUSED_FRAME_PUBLISH_INTERVAL
        );
        let now = Instant::now();
        let focused = sample_terminal_state(b"focus", true);
        let unfocused = sample_terminal_state(b"blur", false);
        assert!(!should_publish_terminal_frame(
            &focused,
            now,
            now + TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL / 2
        ));
        assert!(should_publish_terminal_frame(
            &focused,
            now,
            now + TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL
        ));
        assert!(!should_publish_terminal_frame(
            &unfocused,
            now,
            now + TERMINAL_FOCUSED_FRAME_PUBLISH_INTERVAL
        ));
        assert!(should_publish_terminal_frame(
            &unfocused,
            now,
            now + TERMINAL_UNFOCUSED_FRAME_PUBLISH_INTERVAL
        ));
    }

    #[test]
    fn terminal_certification_close_emits_final_frame_before_closed() {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
        let state = sample_terminal_state(b"final prompt", true);
        let cwd = Arc::new(Mutex::new(Some(PathBuf::from("/tmp/quorp"))));
        emit_terminal_close(&event_tx, &state, &cwd, "zsh", None, "unit-test");

        let first = event_rx.recv().expect("final frame");
        let second = event_rx.recv().expect("closed event");
        match first {
            TuiEvent::TerminalFrame(frame) => {
                assert_eq!(
                    frame.snapshot.row_strings(1),
                    vec!["final prompt".to_string()]
                );
                assert_eq!(frame.cwd, Some(PathBuf::from("/tmp/quorp")));
            }
            other => panic!("expected TerminalFrame, got {other:?}"),
        }
        assert!(matches!(second, TuiEvent::TerminalClosed));
    }

    #[test]
    fn terminal_certification_publish_frame_uses_latest_cwd_metadata() {
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(8);
        let state = sample_terminal_state(b"metadata", true);
        let cwd = Arc::new(Mutex::new(Some(PathBuf::from("/tmp/quorp-next"))));
        publish_terminal_frame(&event_tx, &state, &cwd, "zsh", None, "metadata-test");
        match event_rx.recv().expect("metadata frame") {
            TuiEvent::TerminalFrame(frame) => {
                assert_eq!(frame.cwd, Some(PathBuf::from("/tmp/quorp-next")));
            }
            other => panic!("expected metadata frame, got {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_certification_title_update_arrives_over_real_pty() {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut builder = CommandBuilder::new("/bin/zsh");
        builder.arg("-lc");
        builder.arg("printf '\\e]0;phase2-title\\a'; printf 'READY\\n'");
        let mut child = pair.slave.spawn_command(builder).expect("spawn zsh");
        drop(pair.slave);
        drop(pair.master);

        let mut parser = crate::quorp::tui::terminal_surface::new_parser(24, 80, 128);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut chunk = [0u8; 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read_len) => parser.process(&chunk[..read_len]),
                Err(error) => panic!("pty read failed: {error}"),
            }
            if Instant::now() >= deadline {
                break;
            }
            if let Some(status) = child.try_wait().expect("wait zsh")
                && status.exit_code() == 0
            {
                break;
            }
        }

        let snapshot =
            crate::quorp::tui::terminal_surface::TerminalSnapshot::from_screen(parser.screen());
        assert_eq!(
            crate::quorp::tui::terminal_surface::terminal_window_title(&parser).as_deref(),
            Some("phase2-title")
        );
        assert!(
            snapshot
                .row_strings(24)
                .iter()
                .any(|row| row.contains("READY")),
            "expected READY output in zsh PTY snapshot, got {:?}",
            snapshot.row_strings(24)
        );
    }
}

#[cfg(target_os = "linux")]
fn terminal_session_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "macos")]
fn terminal_session_cwd(pid: u32) -> Option<PathBuf> {
    let mut path_info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let expected_size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let result = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            path_info.as_mut_ptr().cast(),
            expected_size,
        )
    };
    if result != expected_size {
        return None;
    }

    let path_info = unsafe { path_info.assume_init() };
    let path_bytes =
        unsafe { CStr::from_ptr(path_info.pvi_cdir.vip_path.as_ptr().cast::<libc::c_char>()) }
            .to_bytes();
    if path_bytes.is_empty() {
        return None;
    }

    Some(PathBuf::from(OsString::from_vec(path_bytes.to_vec())))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn terminal_session_cwd(_pid: u32) -> Option<PathBuf> {
    None
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
        "up" => encode_modified_arrow("A", &keystroke.modifiers),
        "down" => encode_modified_arrow("B", &keystroke.modifiers),
        "right" => encode_modified_arrow("C", &keystroke.modifiers),
        "left" => encode_modified_arrow("D", &keystroke.modifiers),
        "home" => encode_modified_home_end("H", &keystroke.modifiers),
        "end" => encode_modified_home_end("F", &keystroke.modifiers),
        "pageup" => encode_modified_tilde_key(5, &keystroke.modifiers),
        "pagedown" => encode_modified_tilde_key(6, &keystroke.modifiers),
        "delete" => encode_modified_tilde_key(3, &keystroke.modifiers),
        "insert" => encode_modified_tilde_key(2, &keystroke.modifiers),
        "space" => vec![b' '],
        key if key.len() == 1 => {
            let byte = key.as_bytes()[0];
            encode_modified_char(byte, &keystroke.modifiers)
        }
        function if function.starts_with('f') => {
            encode_function_key(function, &keystroke.modifiers)
        }
        _ => Vec::new(),
    }
}

fn encode_modified_char(
    byte: u8,
    modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers,
) -> Vec<u8> {
    let mut bytes = if modifiers.control && byte.is_ascii_alphabetic() {
        vec![byte.to_ascii_lowercase() - b'a' + 1]
    } else {
        vec![byte]
    };
    if modifiers.alt && !bytes.is_empty() {
        let mut prefixed = vec![0x1b];
        prefixed.append(&mut bytes);
        prefixed
    } else {
        bytes
    }
}

fn encode_modified_arrow(
    suffix: &str,
    modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers,
) -> Vec<u8> {
    if let Some(parameter) = xterm_modifier_parameter(modifiers) {
        format!("\x1b[1;{parameter}{suffix}").into_bytes()
    } else {
        format!("\x1b[{suffix}").into_bytes()
    }
}

fn encode_modified_home_end(
    suffix: &str,
    modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers,
) -> Vec<u8> {
    if let Some(parameter) = xterm_modifier_parameter(modifiers) {
        format!("\x1b[1;{parameter}{suffix}").into_bytes()
    } else {
        format!("\x1b[{suffix}").into_bytes()
    }
}

fn encode_modified_tilde_key(
    code: u8,
    modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers,
) -> Vec<u8> {
    if let Some(parameter) = xterm_modifier_parameter(modifiers) {
        format!("\x1b[{code};{parameter}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn encode_function_key(
    function: &str,
    modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers,
) -> Vec<u8> {
    let Some(number) = function
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
    else {
        return Vec::new();
    };
    match number {
        1..=4 => {
            let suffix = match number {
                1 => "P",
                2 => "Q",
                3 => "R",
                4 => "S",
                _ => unreachable!(),
            };
            if let Some(parameter) = xterm_modifier_parameter(modifiers) {
                format!("\x1b[1;{parameter}{suffix}").into_bytes()
            } else {
                format!("\x1bO{suffix}").into_bytes()
            }
        }
        5 => encode_modified_tilde_key(15, modifiers),
        6 => encode_modified_tilde_key(17, modifiers),
        7 => encode_modified_tilde_key(18, modifiers),
        8 => encode_modified_tilde_key(19, modifiers),
        9 => encode_modified_tilde_key(20, modifiers),
        10 => encode_modified_tilde_key(21, modifiers),
        11 => encode_modified_tilde_key(23, modifiers),
        12 => encode_modified_tilde_key(24, modifiers),
        _ => Vec::new(),
    }
}

fn xterm_modifier_parameter(modifiers: &crate::quorp::tui::bridge::TuiKeyModifiers) -> Option<u8> {
    let mut parameter = 1u8;
    if modifiers.shift {
        parameter = parameter.saturating_add(1);
    }
    if modifiers.alt {
        parameter = parameter.saturating_add(2);
    }
    if modifiers.control {
        parameter = parameter.saturating_add(4);
    }
    (parameter > 1).then_some(parameter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorp::tui::ChatUiEvent;
    use crate::quorp::tui::TuiEvent;
    use serde_json::json;
    use tempfile::tempdir;

    fn capture_tool_events(
        output: String,
        event_rx: std::sync::mpsc::Receiver<TuiEvent>,
    ) -> Vec<TuiEvent> {
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match event_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(event) => {
                    let is_finished =
                        matches!(
                            event,
                            TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _))
                                if output.is_empty()
                        ) || matches!(event, TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _)));
                    events.push(event);
                    if is_finished {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }
        events
    }

    #[cfg(unix)]
    fn write_test_script(path: &Path, content: &str) {
        write_full_file(path, content).expect("write script");
        set_executable_bit(path).expect("chmod script");
    }

    #[cfg(unix)]
    fn write_mcp_config(root: &Path, server_name: &str, command: &Path) {
        let config_dir = root.join(".quorp");
        std::fs::create_dir_all(&config_dir).expect("mkdir config");
        std::fs::write(
            config_dir.join("agent.toml"),
            format!(
                "[[mcp_servers]]\nname = \"{server_name}\"\ncommand = \"{}\"\n",
                command.display()
            ),
        )
        .expect("write config");
    }

    #[test]
    fn validation_failure_explanation_extracts_anchors_without_patch_advice() {
        let output = "---- round::tests::chrono stdout ----\nthread panicked at src/round.rs:42:5\nassertion failed: expected left == right";

        let rendered = render_validation_failure_explanation("cargo test chrono", output);

        assert!(rendered.contains("[explain_validation_failure]"));
        assert!(rendered.contains("round::tests::chrono"));
        assert!(rendered.contains("src/round.rs:42:5"));
        assert!(!rendered.contains("replace with"));
    }

    #[test]
    fn validation_failure_explanation_prioritizes_errors_over_warning_anchors() {
        let output = "warning: unexpected `cfg` condition value: `bench`\n --> tests/noise.rs:10:1\nerror[E0432]: unresolved import `serde`\n --> Cargo.toml:1:1\n";

        let rendered = render_validation_failure_explanation("cargo test", output);

        assert!(rendered.contains("diagnostic_class: manifest_dependency_error"));
        assert!(rendered.contains("target_class: manifest"));
        assert!(rendered.contains("primary_anchor: --> Cargo.toml:1:1"));
        assert!(!rendered.contains("primary_anchor: --> tests/noise.rs:10:1"));
    }

    #[test]
    fn implementation_target_suggestions_rank_manifest_dependency_errors() {
        let output = "error[E0432]: unresolved import `serde`\n --> src/lib.rs:2:5\n";

        let rendered = render_implementation_target_suggestions(
            "cargo test",
            output,
            Some("tests/issues/issue_474.rs"),
            Some(12),
        );

        assert!(rendered.contains("[suggest_implementation_targets]"));
        assert!(rendered.contains("diagnostic_class: manifest_dependency_error"));
        assert!(rendered.contains("required_next_target: Cargo.toml"));
        assert!(rendered.contains("reason: test_evidence_only"));
    }

    #[test]
    fn edit_anchor_suggestions_warn_about_repeated_hints() {
        let source =
            "fn alpha() {}\nlet repeated = 1;\nlet unique_anchor_value = 2;\nlet repeated = 3;\n";

        let rendered = render_edit_anchor_suggestions(
            "src/lib.rs",
            source,
            Some(ReadFileRange {
                start_line: 1,
                end_line: 4,
            }),
            Some("let repeated"),
        );

        assert!(rendered.contains("[suggest_edit_anchors]"));
        assert!(rendered.contains("line 3: let unique_anchor_value = 2;"));
        assert!(rendered.contains("search_hint_occurrences: 2"));
        assert!(rendered.contains("ReplaceBlock with range"));
    }

    #[test]
    fn sanitize_project_path_rejects_traversal_and_external_absolute() {
        let root = tempdir().expect("tempdir");
        let outside = tempdir().expect("outside");
        let file = outside.path().join("secret");
        std::fs::write(&file, "x").expect("write");

        assert!(sanitize_project_path(root.path(), root.path(), "../outside").is_err());
        assert!(sanitize_project_path(root.path(), root.path(), &file.to_string_lossy()).is_err());
    }

    #[test]
    fn sanitize_project_path_allows_relative_in_root() {
        let root = tempdir().expect("tempdir");
        let candidate =
            sanitize_project_path(root.path(), root.path(), "src/main.rs").expect("sanitized");
        assert_eq!(candidate, root.path().join("src/main.rs"));
    }

    #[test]
    fn sanitize_project_path_allows_absolute_paths_inside_root() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("src").join("main.rs");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, "fn main() {}\n").expect("write");

        let candidate = sanitize_project_path(root.path(), root.path(), &file.to_string_lossy())
            .expect("sanitized");
        assert_eq!(candidate, file.canonicalize().expect("canonical"));
    }

    #[test]
    fn diagnose_redundant_workspace_prefix_suggests_workspace_relative_path() {
        let root = tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("crates").join("reconciliation-core"))
            .expect("mkdir");

        let suggested = diagnose_redundant_workspace_prefix(
            root.path(),
            "workspace/crates/reconciliation-core",
        );

        assert_eq!(suggested.as_deref(), Some("crates/reconciliation-core"));
    }

    #[test]
    fn read_file_contents_rejects_binary_and_truncates() {
        let root = tempdir().expect("tempdir");
        let huge = root.path().join("huge.txt");
        let bytes = vec![b'a'; FILE_READ_LIMIT_BYTES + 123];
        std::fs::write(&huge, &bytes).expect("write");
        let output = read_file_contents(&huge, None).expect("read");
        assert!(output.ends_with(FILE_READ_TRUNCATION_MARKER));
        assert_eq!(
            output.len(),
            FILE_READ_LIMIT_BYTES + FILE_READ_TRUNCATION_MARKER.len()
        );

        let binary = root.path().join("binary.bin");
        std::fs::write(&binary, [0xff, 0x00]).expect("write");
        assert!(read_file_contents(&binary, None).is_err());
    }

    #[test]
    fn read_file_contents_honors_requested_range() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("sample.txt");
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").expect("write");

        let output = read_file_contents(
            &file,
            Some(ReadFileRange {
                start_line: 2,
                end_line: 3,
            }),
        )
        .expect("read");

        assert_eq!(output, "two\nthree");
    }

    #[test]
    fn list_directory_entries_orders_and_truncates() {
        let root = tempdir().expect("tempdir");
        for i in 0..(DIRECTORY_LIST_LIMIT + 20) {
            let path = root.path().join(format!("file-{i:04}.txt"));
            std::fs::write(path, b"x").expect("write");
        }
        let entries = list_directory_entries(root.path()).expect("list");
        assert_eq!(entries.len(), DIRECTORY_LIST_LIMIT);
        assert!(entries.windows(2).all(|window| window[0] <= window[1]));

        let long_file = root.path().join("a".repeat(DIRECTORY_NAME_LIMIT + 20));
        std::fs::write(&long_file, b"x").expect("write long");
        let entries = list_directory_entries(root.path()).expect("list");
        assert!(
            entries
                .iter()
                .any(|entry| entry.len() <= DIRECTORY_NAME_LIMIT)
        );
    }

    #[test]
    fn write_full_file_replaces_content_and_requires_parent_dir() {
        let root = tempdir().expect("tempdir");
        let path = root.path().join("nested").join("file.txt");
        assert!(write_full_file(&path, "new").is_err());

        let file = root.path().join("existing.txt");
        write_full_file(&file, "before").expect("write");
        write_full_file(&file, "after").expect("rewrite");
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "after");
    }

    #[cfg(unix)]
    #[test]
    fn set_executable_bit_marks_regular_file_executable() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("tempdir");
        let file = root.path().join("script.sh");
        write_full_file(&file, "#!/bin/sh\necho hi\n").expect("write");
        set_executable_bit(&file).expect("chmod");
        let mode = std::fs::metadata(&file)
            .expect("metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }

    #[test]
    fn apply_patch_task_applies_unified_diff_update() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "old\nkeep\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        let request_path = file
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_string();
        spawn_apply_patch_task(
            event_tx,
            0,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            request_path,
            "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n keep\n".to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "new\nkeep\n");
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("Applied unified diff patch")
        )));
    }

    #[test]
    fn apply_patch_task_accepts_hunk_only_patch_for_explicit_path() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "one\ntwo\nthree\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            0,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n".to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "one\nTWO\nthree\n");
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("Applied single-file hunk patch")
                    && line.contains("M target.txt")
        )));
    }

    #[test]
    fn apply_patch_hunk_only_normalization_rejects_newline_paths() {
        let error = normalize_single_file_hunk_patch(
            "src/lib.rs\n+++ b/other.rs",
            "@@ -1 +1 @@\n-old\n+new\n",
        )
        .expect_err("reject newline path");

        assert!(error.to_string().contains("cannot contain newlines"));
    }

    #[test]
    fn apply_patch_task_accepts_unique_line_replacement_shorthand() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "alpha\n    beta\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            0,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            "/beta\n+    gamma\n".to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "alpha\n    gamma\n");
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("Applied single-line replacement shorthand")
                    && line.contains("line 2")
        )));
    }

    #[test]
    fn apply_patch_task_rejects_ambiguous_line_replacement_shorthand() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "alpha\nbeta\nbeta\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            0,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            "/beta\n+gamma\n".to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "alpha\nbeta\nbeta\n");
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("line replacement shorthand is ambiguous")
                    && message.contains("lines 2, 3")
        )));
    }

    #[test]
    fn preview_edit_replace_block_reports_unique_match_without_mutating() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "alpha\nbeta\ngamma\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_preview_edit_task(
            event_tx,
            0,
            PreviewEditTaskRequest {
                cwd: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                path: "target.txt".to_string(),
                edit: PreviewEditPayload::ReplaceBlock {
                    search_block: "beta".to_string(),
                    replace_block: "BETA".to_string(),
                    range: None,
                },
                responder: None,
            },
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "alpha\nbeta\ngamma\n"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("would_apply: true")
                    && line.contains("matching_line_numbers: 2")
        )));
    }

    #[test]
    fn preview_edit_replace_block_reports_rust_syntax_preflight_without_mutating() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("src").join("lib.rs");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        write_full_file(&file, "fn alpha() {\n    let value = 1;\n}\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_preview_edit_task(
            event_tx,
            0,
            PreviewEditTaskRequest {
                cwd: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                path: "src/lib.rs".to_string(),
                edit: PreviewEditPayload::ReplaceBlock {
                    search_block: "let value = 1;".to_string(),
                    replace_block: "let value = ;".to_string(),
                    range: Some(ReadFileRange {
                        start_line: 1,
                        end_line: 3,
                    }),
                },
                responder: None,
            },
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "fn alpha() {\n    let value = 1;\n}\n"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("would_apply: true")
                    && line.contains("syntax_preflight: failed")
        )));
    }

    #[test]
    fn preview_edit_replace_block_reports_ambiguity_without_mutating() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "alpha\nbeta\nbeta\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_preview_edit_task(
            event_tx,
            0,
            PreviewEditTaskRequest {
                cwd: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                path: "target.txt".to_string(),
                edit: PreviewEditPayload::ReplaceBlock {
                    search_block: "beta".to_string(),
                    replace_block: "BETA".to_string(),
                    range: None,
                },
                responder: None,
            },
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "alpha\nbeta\nbeta\n"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("would_apply: false")
                    && line.contains("matching_line_numbers: 2,3")
        )));
    }

    #[test]
    fn preview_edit_apply_patch_dry_runs_without_mutating() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "one\ntwo\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_preview_edit_task(
            event_tx,
            0,
            PreviewEditTaskRequest {
                cwd: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                path: "target.txt".to_string(),
                edit: PreviewEditPayload::ApplyPatch {
                    patch: "@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n".to_string(),
                },
                responder: None,
            },
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "one\ntwo\n");
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("would_apply: true")
                    && line.contains("patch_form: single_file_hunk")
        )));
    }

    #[test]
    fn replace_range_uses_stable_hash_and_preserves_surrounding_content() {
        let current = "one\ntwo\nthree\n";
        let range = ReadFileRange {
            start_line: 2,
            end_line: 2,
        };
        let expected_hash = stable_content_hash("two");
        let updated =
            perform_range_replacement(current, range, &expected_hash, "TWO").expect("replace");
        assert_eq!(updated, "one\nTWO\nthree\n");
        let stale = perform_range_replacement(current, range, "0000000000000000", "TWO")
            .expect_err("stale hash");
        assert!(stale.to_string().contains("hash mismatch"));
    }

    #[test]
    fn modify_toml_sets_and_removes_dependency_with_full_file_hash() {
        let current = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
        let expected_hash = stable_content_hash(current);
        let updated = apply_toml_operations(
            current,
            &expected_hash,
            &[TomlEditOperation::SetDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
                version: Some("0.4".to_string()),
                features: vec!["clock".to_string()],
                default_features: Some(false),
                optional: None,
                package: None,
                path: None,
            }],
        )
        .expect("set dependency");
        assert!(updated.contains("[dependencies]"));
        assert!(updated.contains("chrono"));
        assert!(updated.parse::<toml_edit::DocumentMut>().is_ok());

        let updated_hash = stable_content_hash(&updated);
        let removed = apply_toml_operations(
            &updated,
            &updated_hash,
            &[TomlEditOperation::RemoveDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
            }],
        )
        .expect("remove dependency");
        assert!(!removed.contains("chrono"));
    }

    #[test]
    fn preview_replace_range_returns_apply_preview_id_without_mutating() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "one\ntwo\nthree\n").expect("bootstrap");
        let hash = stable_content_hash("two");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_preview_edit_task(
            event_tx,
            0,
            PreviewEditTaskRequest {
                cwd: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                path: "target.txt".to_string(),
                edit: PreviewEditPayload::ReplaceRange {
                    range: ReadFileRange {
                        start_line: 2,
                        end_line: 2,
                    },
                    expected_hash: hash,
                    replacement: "TWO".to_string(),
                },
                responder: None,
            },
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "one\ntwo\nthree\n"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("would_apply: true")
                    && line.contains("preview_id: pv_")
                    && line.contains("ApplyPreview")
        )));
    }

    #[test]
    fn apply_patch_task_supports_add_delete_and_move() {
        let root = tempdir().expect("tempdir");
        let moved_source = root.path().join("move_source.txt");
        let deleted = root.path().join("delete.txt");
        let untouched = root.path().join("untouched.txt");
        write_full_file(&moved_source, "before").expect("bootstrap move");
        write_full_file(&deleted, "delete me").expect("bootstrap delete");
        write_full_file(&untouched, "stay put").expect("bootstrap untouched");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        let request_path = moved_source
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_string();
        spawn_apply_patch_task(
            event_tx,
            1,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            request_path,
            concat!(
                "--- /dev/null\n",
                "+++ b/added.txt\n",
                "@@ -0,0 +1,2 @@\n",
                "+hello\n",
                "+world\n",
                "--- a/delete.txt\n",
                "+++ /dev/null\n",
                "@@ -1 +0,0 @@\n",
                "-delete me\n",
                "rename from move_source.txt\n",
                "rename to moved.txt\n",
                "--- a/move_source.txt\n",
                "+++ b/moved.txt\n",
                "@@ -1 +1 @@\n",
                "-before\n",
                "+after\n"
            )
            .to_string(),
            None,
        );
        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(root.path().join("added.txt")).expect("read added"),
            "hello\nworld\n"
        );
        assert!(!deleted.exists());
        assert!(!moved_source.exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("moved.txt")).expect("read moved"),
            "after"
        );
        assert_eq!(
            std::fs::read_to_string(&untouched).expect("read untouched"),
            "stay put"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("A added.txt")
                    && line.contains("D delete.txt")
                    && line.contains("R move_source.txt -> moved.txt")
        )));
    }

    #[test]
    fn apply_patch_task_rejects_malformed_hunks() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "old").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            2,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,1 @@\n-old\n".to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("Malformed hunk")
        )));
    }

    #[test]
    fn apply_patch_task_supports_multiple_hunks_in_one_file() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file(&file, "one\ntwo\nthree\nfour\n").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            3,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            concat!(
                "--- a/target.txt\n",
                "+++ b/target.txt\n",
                "@@ -1,2 +1,2 @@\n",
                " one\n",
                "-two\n",
                "+TWO\n",
                "@@ -3,2 +3,2 @@\n",
                " three\n",
                "-four\n",
                "+FOUR\n",
            )
            .to_string(),
            None,
        );

        let _events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "one\nTWO\nthree\nFOUR\n");
    }

    #[test]
    fn apply_patch_task_preserves_missing_trailing_newline() {
        let root = tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        write_full_file_allow_create(&file, "one\ntwo").expect("bootstrap");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            4,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "target.txt".to_string(),
            concat!(
                "--- a/target.txt\n",
                "+++ b/target.txt\n",
                "@@ -1,2 +1,2 @@\n",
                " one\n",
                "-two\n",
                "+three\n",
                "\\ No newline at end of file\n",
            )
            .to_string(),
            None,
        );

        let _events = capture_tool_events(String::new(), event_rx);
        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "one\nthree");
    }

    #[test]
    fn apply_patch_task_accepts_placeholder_path_for_multi_file_diff() {
        let root = tempdir().expect("tempdir");
        let source = root.path().join("move_source.txt");
        let deleted = root.path().join("delete.txt");
        write_full_file(&source, "before\n").expect("bootstrap source");
        write_full_file(&deleted, "remove\n").expect("bootstrap deleted");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_apply_patch_task(
            event_tx,
            5,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "placeholder.txt".to_string(),
            concat!(
                "--- /dev/null\n",
                "+++ b/added.txt\n",
                "@@ -0,0 +1 @@\n",
                "+hello\n",
                "--- a/delete.txt\n",
                "+++ /dev/null\n",
                "@@ -1 +0,0 @@\n",
                "-remove\n",
                "rename from move_source.txt\n",
                "rename to moved.txt\n",
                "--- a/move_source.txt\n",
                "+++ b/moved.txt\n",
                "@@ -1 +1 @@\n",
                "-before\n",
                "+after\n",
            )
            .to_string(),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert_eq!(
            std::fs::read_to_string(root.path().join("added.txt")).expect("read added"),
            "hello\n"
        );
        assert!(!deleted.exists());
        assert!(!source.exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("moved.txt")).expect("read moved"),
            "after\n"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("A added.txt")
                    && line.contains("D delete.txt")
                    && line.contains("R move_source.txt -> moved.txt")
        )));
    }

    #[test]
    fn resolve_file_patches_rejects_out_of_root_targets() {
        let root = tempdir().expect("tempdir");
        let file_patches = vec![FilePatch {
            path: "../escape.txt".to_string(),
            operation: PatchOperation::Update,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: vec![PatchLine::Context("safe".to_string())],
            }],
        }];

        let error =
            resolve_file_patches(root.path(), root.path(), &file_patches).expect_err("reject");
        assert!(
            error
                .to_string()
                .contains("Parent directory traversal is not allowed")
        );
    }

    #[test]
    fn apply_hunks_rejects_ambiguous_matches() {
        let error = apply_hunks(
            "same\nline\nsame\nline",
            &[Hunk {
                old_start: 99,
                old_count: 2,
                new_start: 99,
                new_count: 2,
                lines: vec![
                    PatchLine::Context("same".to_string()),
                    PatchLine::Remove("line".to_string()),
                    PatchLine::Add("updated".to_string()),
                ],
            }],
        )
        .expect_err("ambiguous hunk");

        assert!(error.to_string().contains("Patch hunk is ambiguous"));
    }

    #[test]
    fn rollback_session_worktree_restores_all_touched_files() {
        let root = tempdir().expect("tempdir");
        let existing = root.path().join("existing.txt");
        let created = root.path().join("created.txt");
        write_full_file(&existing, "before").expect("bootstrap");
        stash_file_for_rollback(77, &existing);
        stash_file_for_rollback(77, &created);

        write_full_file(&existing, "after").expect("mutate existing");
        write_full_file_allow_create(&created, "new").expect("create new");

        rollback_session_worktree(77);

        assert_eq!(
            std::fs::read_to_string(&existing).expect("read existing"),
            "before"
        );
        assert!(!created.exists());
    }

    #[test]
    fn render_mcp_tool_result_formats_structured_content() {
        let result = crate::quorp::tui::mcp_client::CallToolResult {
            content: vec![
                crate::quorp::tui::mcp_client::CallToolResultContent::Text {
                    text: "hello".to_string(),
                },
                crate::quorp::tui::mcp_client::CallToolResultContent::Image {
                    data: "aGVsbG8=".to_string(),
                    mime_type: "image/png".to_string(),
                },
                crate::quorp::tui::mcp_client::CallToolResultContent::Resource {
                    resource: json!({"uri":"file:///tmp/demo","kind":"resource"}),
                },
            ],
            is_error: Some(false),
        };

        let rendered = render_mcp_tool_result("demo", "inspect", &result).expect("render");
        assert!(rendered.contains("MCP demo/inspect"));
        assert!(rendered.contains("hello"));
        assert!(rendered.contains("[image result]"));
        assert!(rendered.contains("[resource result]"));
    }

    #[test]
    fn render_mcp_tool_result_surfaces_tool_errors() {
        let result = crate::quorp::tui::mcp_client::CallToolResult {
            content: vec![crate::quorp::tui::mcp_client::CallToolResultContent::Text {
                text: "boom".to_string(),
            }],
            is_error: Some(true),
        };

        let error = render_mcp_tool_result("demo", "inspect", &result).expect_err("tool error");
        assert!(error.to_string().contains("returned an error"));
    }

    #[cfg(unix)]
    #[test]
    fn mcp_call_task_reports_missing_server_configuration() {
        let root = tempdir().expect("tempdir");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_mcp_call_task(
            event_tx,
            10,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "missing".to_string(),
            "echo".to_string(),
            json!({"value":"hi"}),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert!(events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("not configured")
        )));
    }

    #[cfg(unix)]
    #[test]
    fn mcp_call_task_executes_stdio_server_tool() {
        let root = tempdir().expect("tempdir");
        let script = root.path().join("fake-mcp.sh");
        write_test_script(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"1.0.0"}}}'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"tool ok"}]}}'
      exit 0
      ;;
  esac
done
"#,
        );
        write_mcp_config(root.path(), "fake", &script);

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_mcp_call_task(
            event_tx,
            11,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "fake".to_string(),
            "echo".to_string(),
            json!({"value":"hi"}),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert!(
            events.iter().any(|event| matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                    if line.contains("MCP fake/echo") && line.contains("tool ok")
            )),
            "events: {events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn mcp_call_task_surfaces_server_errors() {
        let root = tempdir().expect("tempdir");
        let script = root.path().join("fake-mcp-error.sh");
        write_test_script(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"1.0.0"}}}'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}'
      exit 0
      ;;
  esac
done
"#,
        );
        write_mcp_config(root.path(), "fake", &script);

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_mcp_call_task(
            event_tx,
            12,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "fake".to_string(),
            "echo".to_string(),
            json!({"value":"hi"}),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert!(
            events.iter().any(|event| matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::Error(_, message))
                    if message.contains("boom")
            )),
            "events: {events:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn mcp_call_task_handles_malformed_json_rpc_responses() {
        let root = tempdir().expect("tempdir");
        let script = root.path().join("fake-mcp-malformed.sh");
        write_test_script(
            &script,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' 'not json'
      exit 0
      ;;
  esac
done
"#,
        );
        write_mcp_config(root.path(), "fake", &script);

        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_mcp_call_task(
            event_tx,
            13,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "fake".to_string(),
            "echo".to_string(),
            json!({"value":"hi"}),
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        assert!(
            events.iter().any(|event| matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::Error(_, message))
                    if message.contains("mcp_call_tool")
            )),
            "events: {events:?}"
        );
    }

    #[test]
    fn search_text_task_returns_ranked_matches() {
        let root = tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
        std::fs::write(
            root.path().join("src/lib.rs"),
            "fn render_agent_turn_text() {}\n",
        )
        .expect("write");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_search_text_task(
            event_tx,
            7,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "render_agent_turn_text".to_string(),
            4,
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let saw_match = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                    if line.contains("src/lib.rs:1")
                        && line.contains("render_agent_turn_text")
            )
        });
        assert!(saw_match, "expected formatted search hit in {events:?}");
    }

    #[test]
    fn search_symbols_task_returns_symbol_hits() {
        let root = tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
        std::fs::write(
            root.path().join("src/lib.rs"),
            "pub struct RepoCapsule;\npub fn render_repo_capsule() {}\n",
        )
        .expect("write");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_search_symbols_task(
            event_tx,
            8,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "RepoCapsule".to_string(),
            4,
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let saw_symbol = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                    if line.contains("struct RepoCapsule")
            )
        });
        assert!(saw_symbol, "expected formatted symbol hit in {events:?}");
    }

    #[test]
    fn repo_capsule_task_reports_workspace_members_and_focus_files() {
        let root = tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/quorp"]

[package]
name = "quorp"
"#,
        )
        .expect("write cargo");
        std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
        std::fs::write(
            root.path().join("src/lib.rs"),
            "pub fn render_agent_turn_text() {}\n",
        )
        .expect("write lib");
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
        spawn_repo_capsule_task(
            event_tx,
            9,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            Some("render_agent_turn_text".to_string()),
            4,
            None,
        );

        let events = capture_tool_events(String::new(), event_rx);
        let saw_capsule = events.iter().any(|event| {
            matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                    if line.contains("members: crates/quorp")
                        || line.contains("focus files:")
                        || line.contains("focus symbols:")
            )
        });
        assert!(saw_capsule, "expected repo capsule output in {events:?}");
    }

    #[test]
    fn test_perform_block_replacement_exact_match() {
        let current = "line 1\nline 2\nline 3\nline 4\n";
        let search = "line 2\nline 3\n";
        let replace = "line 2 modified\nline 3 modified\n";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(result, "line 1\nline 2 modified\nline 3 modified\nline 4\n");
    }

    #[test]
    fn test_perform_block_replacement_fuzzy_trailing_whitespace() {
        let current = "fn foo() {\n    let x = 1; \n    let y = 2;\n}\n";
        let search = "    let x = 1;\n    let y = 2;";
        let replace = "    let x = 100;\n    let y = 200;";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(
            result,
            "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
        );
    }

    #[test]
    fn test_perform_block_replacement_ambiguous() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(current, search, replace, None).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert!(err.to_string().contains("lines 2, 4"));
    }

    #[test]
    fn test_perform_block_replacement_not_found() {
        let current = "a\nb\nc\n";
        let search = "d\n";
        let replace = "x\n";
        let err = perform_block_replacement(current, search, replace, None).unwrap_err();
        assert!(err.to_string().contains("Could not find"));
    }

    #[test]
    fn test_try_parse_search_replace_blocks() {
        let patch = "\
Here is my patch!
<<<<
fn foo() {
====
fn foo(bar: i32) {
>>>>
Done.";
        let blocks = super::try_parse_search_replace_blocks(patch).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "fn foo() {");
        assert_eq!(blocks[0].1, "fn foo(bar: i32) {");
    }

    #[test]
    fn test_perform_block_replacement_fuzzy_leading_whitespace() {
        let current = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
        let search = "let x = 1;\nlet y = 2;";
        let replace = "    let x = 100;\n    let y = 200;";
        let result = perform_block_replacement(current, search, replace, None).unwrap();
        assert_eq!(
            result,
            "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
        );
    }

    #[test]
    fn test_perform_block_replacement_ranged_disambiguates() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let result = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 4,
                end_line: 4,
            }),
        )
        .unwrap();
        assert_eq!(result, "a\nb\nc\nx\nd\n");
    }

    #[test]
    fn test_perform_block_replacement_ranged_stale_range_fails() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 5,
                end_line: 5,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("none are fully inside"));
        assert!(err.to_string().contains("lines 2, 4"));
    }

    #[test]
    fn test_perform_block_replacement_ranged_still_ambiguous_fails() {
        let current = "a\nb\nc\nb\nd\n";
        let search = "b\n";
        let replace = "x\n";
        let err = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 1,
                end_line: 5,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert!(err.to_string().contains("requested range 1-5"));
    }

    #[test]
    fn test_perform_block_replacement_accepts_literal_newline_escape_fallback() {
        let current = "pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n    if delayed_change {\n        \"immediate\"\n    } else {\n        \"immediate\"\n    }\n}\n";
        let search = "if delayed_change {\\n        \"immediate\"\\n    } else {\\n        \"immediate\"\\n    }";
        let replace = "if delayed_change {\\n        \"scheduled_at_period_end\"\\n    } else {\\n        \"immediate\"\\n    }";
        let result = perform_block_replacement(
            current,
            search,
            replace,
            Some(ReadFileRange {
                start_line: 1,
                end_line: 7,
            }),
        )
        .unwrap();

        assert!(result.contains("\"scheduled_at_period_end\""));
        assert!(!result.contains("\\n"));
    }
}
