use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;

use crate::quorp::tui::agent_context::load_agent_config;
use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction, PreviewEditPayload};
use crate::quorp::tui::command_bridge::CommandBridgeRequest;
use crate::quorp::tui::TuiEvent;
use quorp_agent_core::{ReadFileRange, stable_content_hash};
use quorp_tools::edit::{count_file_lines, list_directory_entries, read_file_contents};
use quorp_tools::patch::sanitize_project_path;
use quorp_tools::path_index::{
    build_repo_capsule, render_repo_capsule, render_symbol_search_hits, render_text_search_hits,
    search_repo_symbols, search_repo_text,
};
use quorp_tools::preview::render_preview_edit_result;

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

mod actions;
#[allow(unused_imports)]
pub(crate) use actions::{
    emit_tool_error, emit_tool_finished, emit_tool_result, render_mcp_tool_result,
    run_command_capture, run_command_streaming, spawn_apply_patch_task,
    spawn_apply_preview_task, spawn_mcp_call_task, spawn_modify_toml_task,
    spawn_replace_block_task, spawn_replace_range_task, spawn_run_validation_task,
    spawn_set_executable_task, spawn_write_file_task, truncate_diagnostic_text,
};

#[cfg(test)]
mod tests;

