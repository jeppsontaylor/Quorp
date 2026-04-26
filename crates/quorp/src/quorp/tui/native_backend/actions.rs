//! Mutating-action handlers and PTY-streaming utilities split out of
//! `native_backend.rs` to keep the parent file under the 2,000 LOC hard
//! cap.
//!
//! Everything here is reachable from the dispatcher in
//! `super::spawn_command_service_loop`. Calls *into* the parent (e.g.
//! `super::stash_file_for_rollback`) use `super::` so the privacy
//! relationship between `native_backend` and this submodule is
//! explicit.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::{COMMAND_OUTPUT_LIMIT, stash_file_for_rollback};
use crate::quorp::tui::agent_context::{
    McpServerConfig, load_agent_config, validation_commands_for_plan,
};
use crate::quorp::tui::agent_protocol::{
    ActionOutcome, AgentAction, TomlEditOperation,
};
use crate::quorp::tui::{ChatUiEvent, TuiEvent};
use quorp_agent_core::{ReadFileRange, stable_content_hash};
use quorp_tools::apply::apply_patch_edit;
use quorp_tools::edit::{
    apply_toml_operations, perform_range_replacement, set_executable_bit, write_full_file,
};
use quorp_tools::patch::{perform_block_replacement, sanitize_project_path};
use quorp_tools::preview::{load_preview_record, syntax_preflight_for_preview};
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

pub(crate) fn render_mcp_tool_result(
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
pub(crate) fn spawn_mcp_call_task(
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

pub(crate) fn spawn_write_file_task(
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

pub(crate) fn spawn_apply_patch_task(
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
        let result = apply_patch_edit(&project_root, &cwd, &path, &patch, |touched_path| {
            stash_file_for_rollback(session_id, &touched_path.to_path_buf());
        });
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_replace_block_task(
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
pub(crate) fn spawn_replace_range_task(
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
pub(crate) fn spawn_modify_toml_task(
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

pub(crate) fn spawn_apply_preview_task(
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

pub(crate) fn spawn_run_validation_task(
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
                super::rollback_session_worktree(session_id);
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
            super::clear_session_worktree(session_id);
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

pub(crate) fn emit_tool_error(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    message: String,
) {
    if let Err(error) = event_tx.send(TuiEvent::Chat(ChatUiEvent::Error(session_id, message))) {
        log::error!("tui: tool error channel closed: {error}");
    }
}

pub(crate) fn emit_tool_finished(
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
    send_chat_event(event_tx, ChatUiEvent::CommandFinished(session_id, outcome));
}

pub(crate) fn spawn_set_executable_task(
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

pub(crate) fn emit_tool_result(
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
        ChatUiEvent::CommandOutput(session_id, outcome.output_text().to_string()),
    );
    emit_tool_finished(event_tx, session_id, outcome, responder);
}

pub(crate) fn run_validation_commands(
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
            ChatUiEvent::CommandOutput(session_id, format!("$ {command}")),
        );
        let command_output = run_command_capture(&command, cwd, command_output_limit)
            .map_err(|error| anyhow::anyhow!("failed to run `{command}`: {error}"))?;
        for line in command_output.output.lines() {
            send_chat_event(
                event_tx,
                ChatUiEvent::CommandOutput(session_id, line.to_string()),
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

pub(crate) struct CapturedCommandOutput {
    pub(crate) output: String,
    pub(crate) exit_code: i32,
}

pub(crate) fn run_command_capture(
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

pub(crate) fn send_chat_event(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    event: ChatUiEvent,
) {
    if let Err(error) = event_tx.send(TuiEvent::Chat(event)) {
        log::error!("tui: command event channel closed: {error}");
    }
}

pub(crate) fn truncate_diagnostic_text(text: &str, max_chars: usize) -> String {
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

pub(crate) fn run_command_streaming(
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
                        ChatUiEvent::CommandOutput(session_id, text),
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

