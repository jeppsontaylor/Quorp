use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::quorp::tui::agent_context::{
    McpServerConfig, load_agent_config, validation_commands_for_plan,
};
use crate::quorp::tui::agent_protocol::{
    ActionOutcome, AgentAction, PreviewEditPayload, TomlEditOperation,
};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::{ChatUiEvent, TuiEvent};
use quorp_agent_core::{ReadFileRange, stable_content_hash};
use quorp_tools::apply::apply_patch_edit;
use quorp_tools::edit::{
    apply_toml_operations, count_file_lines, list_directory_entries, perform_range_replacement,
    read_file_contents, set_executable_bit, write_full_file,
};
use quorp_tools::patch::{perform_block_replacement, sanitize_project_path};
use quorp_tools::path_index::{
    build_repo_capsule, render_repo_capsule, render_symbol_search_hits, render_text_search_hits,
    search_repo_symbols, search_repo_text,
};
use quorp_tools::preview::{
    load_preview_record, render_preview_edit_result, syntax_preflight_for_preview,
};

#[cfg(test)]
use quorp_tools::edit::write_full_file_allow_create;

const COMMAND_OUTPUT_LIMIT: usize = 16 * 1024;
type SessionShadowFiles = std::collections::HashMap<PathBuf, Option<String>>;
type ShadowWorktree = std::collections::HashMap<usize, SessionShadowFiles>;

static SHADOW_WORKTREE: std::sync::OnceLock<Mutex<ShadowWorktree>> = std::sync::OnceLock::new();

fn get_shadow_worktree() -> &'static Mutex<ShadowWorktree> {
    SHADOW_WORKTREE.get_or_init(|| Mutex::new(ShadowWorktree::new()))
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
                        AgentAction::FindFiles { query, limit } => {
                            spawn_find_files_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                query,
                                limit,
                                responder,
                            );
                        }
                        AgentAction::StructuralSearch {
                            pattern,
                            language,
                            path,
                            limit,
                        } => {
                            spawn_structural_search_task(
                                event_tx.clone(),
                                session_id,
                                StructuralSearchTaskRequest {
                                    cwd,
                                    project_root,
                                    pattern,
                                    language,
                                    path,
                                    limit,
                                    responder,
                                },
                            );
                        }
                        AgentAction::StructuralEditPreview {
                            pattern,
                            rewrite,
                            language,
                            path,
                        } => {
                            spawn_structural_edit_preview_task(
                                event_tx.clone(),
                                session_id,
                                StructuralEditPreviewTaskRequest {
                                    cwd,
                                    project_root,
                                    pattern,
                                    rewrite,
                                    language,
                                    path,
                                    responder,
                                },
                            );
                        }
                        AgentAction::CargoDiagnostics {
                            command,
                            include_clippy,
                        } => {
                            spawn_cargo_diagnostics_task(
                                event_tx.clone(),
                                session_id,
                                cwd,
                                project_root,
                                command,
                                include_clippy,
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

fn spawn_find_files_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    query: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::FindFiles {
            query: query.clone(),
            limit,
        };
        let config = load_agent_config(project_root.as_path());
        let root = effective_project_root(&project_root, &cwd);
        let result = find_files_with_config(&root, &query, limit.max(1), &config);
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "find_files",
            responder,
        );
    });
}

fn find_files_with_config(
    root: &Path,
    query: &str,
    limit: usize,
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> anyhow::Result<String> {
    if !config.agent_tools.enabled || !config.agent_tools.fd.enabled {
        return Err(anyhow::anyhow!(
            "FindFiles is disabled by agent tool settings"
        ));
    }
    let output_limit = config.agent_tools.fd.max_output_bytes.unwrap_or(16 * 1024);
    if quorp_agent_core::command_is_available(&config.agent_tools.fd.command) {
        let command = format!(
            "{} --color never --type f {} .",
            config.agent_tools.fd.command,
            shell_quote(query)
        );
        let captured = run_command_capture(&command, root, output_limit)?;
        if captured.exit_code == 0 {
            let matches = captured
                .output
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(limit)
                .map(|line| line.trim_start_matches("./").to_string())
                .collect::<Vec<_>>();
            return Ok(render_find_files_result(query, "fd", &matches));
        }
    }
    Ok(render_find_files_result(
        query,
        "ignore_walk",
        &find_files_with_ignore_walk(root, query, limit),
    ))
}

fn find_files_with_ignore_walk(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let normalized_query = query.trim().to_ascii_lowercase();
    let mut scored = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .build()
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let rendered = relative.to_string_lossy().replace('\\', "/");
        let haystack = rendered.to_ascii_lowercase();
        if normalized_query.is_empty() || haystack.contains(&normalized_query) {
            let score = if haystack == normalized_query {
                0
            } else if haystack.ends_with(&normalized_query) {
                1
            } else {
                2
            };
            scored.push((score, rendered.len(), rendered));
        }
    }
    scored.sort();
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, path)| path)
        .collect()
}

fn render_find_files_result(query: &str, backend: &str, matches: &[String]) -> String {
    let mut lines = vec![
        "[find_files]".to_string(),
        format!("query: {query}"),
        format!("backend: {backend}"),
        format!("matches: {}", matches.len()),
    ];
    if matches.is_empty() {
        lines.push("[no matches]".to_string());
    } else {
        lines.extend(matches.iter().map(|path| format!("- {path}")));
    }
    lines.join("\n")
}

struct StructuralSearchTaskRequest {
    cwd: PathBuf,
    project_root: PathBuf,
    pattern: String,
    language: Option<String>,
    path: Option<String>,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
}

fn spawn_structural_search_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: StructuralSearchTaskRequest,
) {
    std::thread::spawn(move || {
        let StructuralSearchTaskRequest {
            cwd,
            project_root,
            pattern,
            language,
            path,
            limit,
            responder,
        } = request;
        let action = AgentAction::StructuralSearch {
            pattern: pattern.clone(),
            language: language.clone(),
            path: path.clone(),
            limit,
        };
        let result = run_structural_search(
            &cwd,
            &project_root,
            &pattern,
            language.as_deref(),
            path.as_deref(),
            limit.max(1),
        );
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "structural_search",
            responder,
        );
    });
}

fn run_structural_search(
    cwd: &Path,
    project_root: &Path,
    pattern: &str,
    language: Option<&str>,
    path: Option<&str>,
    limit: usize,
) -> anyhow::Result<String> {
    let config = load_agent_config(project_root);
    let Some(command) = configured_ast_grep_command(&config) else {
        return Err(anyhow::anyhow!(
            "StructuralSearch is unavailable because ast-grep/sg is disabled or not installed"
        ));
    };
    let root = effective_project_root(project_root, cwd);
    let scope = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(".");
    let target = sanitize_project_path(&root, &root, scope)?;
    let target_arg = target
        .strip_prefix(&root)
        .map(|relative| {
            let rendered = relative.to_string_lossy();
            if rendered.is_empty() {
                ".".to_string()
            } else {
                rendered.replace('\\', "/")
            }
        })
        .unwrap_or_else(|_| ".".to_string());
    let language = language.unwrap_or("rust");
    let shell_command = format!(
        "{} --pattern {} --lang {} {}",
        command,
        shell_quote(pattern),
        shell_quote(language),
        shell_quote(&target_arg)
    );
    let output_limit = config
        .agent_tools
        .ast_grep
        .max_output_bytes
        .unwrap_or(32 * 1024);
    let captured = run_command_capture(&shell_command, &root, output_limit)?;
    let mut lines = vec![
        "[structural_search]".to_string(),
        format!("pattern: {pattern}"),
        format!("language: {language}"),
        format!("path: {target_arg}"),
        format!("exit_code: {}", captured.exit_code),
    ];
    let rendered_matches = captured
        .output
        .lines()
        .take(limit.saturating_mul(6))
        .collect::<Vec<_>>()
        .join("\n");
    if rendered_matches.trim().is_empty() {
        lines.push("[no matches]".to_string());
    } else {
        lines.push(rendered_matches);
    }
    Ok(lines.join("\n"))
}

fn configured_ast_grep_command(
    config: &crate::quorp::tui::agent_context::AgentConfig,
) -> Option<String> {
    let tools = &config.agent_tools;
    if !tools.enabled || !tools.ast_grep.enabled {
        return None;
    }
    if quorp_agent_core::command_is_available(&tools.ast_grep.command) {
        return Some(tools.ast_grep.command.clone());
    }
    if tools.ast_grep.command == "ast-grep" && quorp_agent_core::command_is_available("sg") {
        return Some("sg".to_string());
    }
    None
}

struct StructuralEditPreviewTaskRequest {
    cwd: PathBuf,
    project_root: PathBuf,
    pattern: String,
    rewrite: String,
    language: Option<String>,
    path: Option<String>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
}

fn spawn_structural_edit_preview_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    request: StructuralEditPreviewTaskRequest,
) {
    std::thread::spawn(move || {
        let StructuralEditPreviewTaskRequest {
            cwd,
            project_root,
            pattern,
            rewrite,
            language,
            path,
            responder,
        } = request;
        let action = AgentAction::StructuralEditPreview {
            pattern: pattern.clone(),
            rewrite: rewrite.clone(),
            language: language.clone(),
            path: path.clone(),
        };
        let result = render_structural_edit_preview(
            &cwd,
            &project_root,
            &pattern,
            &rewrite,
            language.as_deref(),
            path.as_deref(),
        );
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "structural_edit_preview",
            responder,
        );
    });
}

fn render_structural_edit_preview(
    cwd: &Path,
    project_root: &Path,
    pattern: &str,
    rewrite: &str,
    language: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<String> {
    let config = load_agent_config(project_root);
    if !config.agent_tools.enabled
        || !config.agent_tools.ast_grep.enabled
        || !config.agent_tools.ast_grep.allow_rewrite_preview
    {
        return Err(anyhow::anyhow!(
            "StructuralEditPreview is disabled by agent tool settings"
        ));
    }
    let search = run_structural_search(cwd, project_root, pattern, language, path, 12)?;
    Ok(format!(
        "[structural_edit_preview]\nwould_apply: false\nmutation_performed: false\npattern: {pattern}\nrewrite: {rewrite}\nnext_step: Use PreviewEdit with exact file anchors, then ApplyPreview if accepted.\n\n{search}"
    ))
}

fn spawn_cargo_diagnostics_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    command: Option<String>,
    include_clippy: bool,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::CargoDiagnostics {
            command: command.clone(),
            include_clippy,
        };
        let result = run_cargo_diagnostics(&cwd, &project_root, command.as_deref(), include_clippy);
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "cargo_diagnostics",
            responder,
        );
    });
}

fn run_cargo_diagnostics(
    cwd: &Path,
    project_root: &Path,
    requested_command: Option<&str>,
    include_clippy: bool,
) -> anyhow::Result<String> {
    let config = load_agent_config(project_root);
    let settings = &config.agent_tools.cargo_diagnostics;
    if !config.agent_tools.enabled || !settings.enabled {
        return Err(anyhow::anyhow!(
            "CargoDiagnostics is disabled by agent tool settings"
        ));
    }
    let mut commands = Vec::new();
    let requested = requested_command
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match requested {
        Some(command)
            if command == settings.check_command
                || settings.clippy_command.as_deref() == Some(command) =>
        {
            commands.push(command.to_string());
        }
        Some(command) => {
            return Err(anyhow::anyhow!(
                "CargoDiagnostics command `{command}` is not configured. Allowed commands: `{}`{}",
                settings.check_command,
                settings
                    .clippy_command
                    .as_deref()
                    .map(|command| format!(", `{command}`"))
                    .unwrap_or_default()
            ));
        }
        None => commands.push(settings.check_command.clone()),
    }
    if include_clippy
        && let Some(clippy_command) = settings.clippy_command.as_ref()
        && !commands.iter().any(|command| command == clippy_command)
    {
        commands.push(clippy_command.clone());
    }

    let output_limit = settings.max_output_bytes.unwrap_or(128 * 1024);
    let mut rendered = vec!["[cargo_diagnostics]".to_string()];
    for command in commands {
        if !quorp_agent_core::command_is_available(&command) {
            return Err(anyhow::anyhow!(
                "configured command `{command}` is unavailable"
            ));
        }
        let captured = run_command_capture(&command, cwd, output_limit)?;
        let diagnostics = parse_cargo_json_diagnostics(&captured.output, 20);
        rendered.push(format!("command: {command}"));
        rendered.push(format!("exit_code: {}", captured.exit_code));
        if diagnostics.is_empty() {
            rendered.push("diagnostics: [none parsed]".to_string());
            rendered.push(truncate_diagnostic_text(&captured.output, 1200));
        } else {
            rendered.push("diagnostics:".to_string());
            rendered.extend(diagnostics.into_iter().map(|line| format!("- {line}")));
        }
    }
    Ok(rendered.join("\n"))
}

fn parse_cargo_json_diagnostics(output: &str, limit: usize) -> Vec<String> {
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let level = message
            .get("level")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("diagnostic");
        let code = message
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(|code| format!("[{code}]"))
            .unwrap_or_default();
        let text = message
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let primary_span = message
            .get("spans")
            .and_then(serde_json::Value::as_array)
            .and_then(|spans| {
                spans.iter().find(|span| {
                    span.get("is_primary").and_then(serde_json::Value::as_bool) == Some(true)
                })
            });
        let location = primary_span
            .and_then(|span| {
                let file = span.get("file_name")?.as_str()?;
                let line = span.get("line_start")?.as_u64()?;
                let column = span.get("column_start")?.as_u64()?;
                Some(format!("{file}:{line}:{column}"))
            })
            .unwrap_or_else(|| "<workspace>".to_string());
        diagnostics.push(format!(
            "{}{} {} {}",
            level,
            code,
            location,
            truncate_diagnostic_text(text, 240)
        ));
        if diagnostics.len() >= limit {
            break;
        }
    }
    diagnostics
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
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
    output: String,
    exit_code: i32,
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
        assert!(
            events.iter().any(|event| matches!(
                event,
                TuiEvent::Chat(ChatUiEvent::Error(_, message))
                    if message.contains("Malformed hunk")
            )),
            "{events:#?}"
        );
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
    fn agent_tools_find_files_fallback_uses_ignore_walk() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("src/bin")).expect("dirs");
        std::fs::write(root.path().join("src/lib.rs"), "").expect("lib");
        std::fs::write(root.path().join("src/bin/tool.rs"), "").expect("tool");
        let mut config = crate::quorp::tui::agent_context::AgentConfig::default();
        config.agent_tools.enabled = true;
        config.agent_tools.fd.command = "definitely-missing-fd".to_string();

        let output = find_files_with_config(root.path(), "tool", 10, &config).expect("find");
        assert!(output.contains("backend: ignore_walk"));
        assert!(output.contains("src/bin/tool.rs"));
    }

    #[test]
    fn agent_tools_cargo_diagnostics_parse_json_records() {
        let output = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value `x` in this scope","code":{"code":"E0425"},"spans":[{"file_name":"src/lib.rs","line_start":7,"column_start":13,"is_primary":true}]}}"#;
        let diagnostics = parse_cargo_json_diagnostics(output, 10);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("error[E0425]"));
        assert!(diagnostics[0].contains("src/lib.rs:7:13"));
        assert!(diagnostics[0].contains("cannot find value"));
    }
}
