use std::path::{Path, PathBuf};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::agent_context::load_agent_config;
use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction};
use quorp_agent_core::command_is_available;
use quorp_lsp::WorkspaceSemanticIndex;

use super::emit_tool_result;

fn build_semantic_index(cwd: &Path, project_root: &Path) -> anyhow::Result<WorkspaceSemanticIndex> {
    let root = super::effective_project_root(project_root, cwd);
    let config = load_agent_config(&root);
    if config.agent_tools.enabled
        && config.agent_tools.rust_analyzer.enabled
        && command_is_available(&config.agent_tools.rust_analyzer.command)
    {
        match WorkspaceSemanticIndex::build_with_rust_language_server(
            &root,
            Some(&config.agent_tools.rust_analyzer.command),
        ) {
            Ok(index) => return Ok(index),
            Err(error) => {
                log::warn!(
                    "failed to start rust language server for {}: {error:#}",
                    root.display()
                );
            }
        }
    }
    WorkspaceSemanticIndex::build(&root)
}

pub(crate) fn spawn_lsp_diagnostics_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspDiagnostics { path: path.clone() };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let diagnostics = index.diagnostics(&path);
            let mut lines = vec!["[lsp_diagnostics]".to_string(), format!("path: {path}")];
            lines.push(index.render_diagnostics(&diagnostics));
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_diagnostics",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lsp_definition_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    symbol: String,
    line: Option<usize>,
    character: Option<usize>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspDefinition {
            path: path.clone(),
            symbol: symbol.clone(),
            line,
            character,
        };
        let result = build_semantic_index(&cwd, &project_root).and_then(|index| {
            let definition = index
                .definition_at(Some(&path), &symbol, line, character)
                .ok_or_else(|| anyhow::anyhow!("No definition found for `{symbol}`"))?;
            Ok(format!(
                "[lsp_definition]\npath: {path}\nsymbol: {symbol}\n{}",
                index.render_symbols(&[definition])
            ))
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_definition",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lsp_references_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: Option<String>,
    symbol: String,
    line: Option<usize>,
    character: Option<usize>,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspReferences {
            path: path.clone(),
            symbol: symbol.clone(),
            line,
            character,
            limit,
        };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let references =
                index.references_at(path.as_deref(), &symbol, line, character, limit.max(1));
            let mut lines = vec!["[lsp_references]".to_string(), format!("symbol: {symbol}")];
            if let Some(path) = path.as_deref() {
                lines.push(format!("path: {path}"));
            }
            if let (Some(line), Some(character)) = (line, character) {
                lines.push(format!("cursor: {line}:{character}"));
            }
            lines.push(index.render_locations(&references));
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_references",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lsp_hover_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    line: usize,
    character: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspHover {
            path: path.clone(),
            line,
            character,
        };
        let result = build_semantic_index(&cwd, &project_root).and_then(|index| {
            index
                .hover(&path, line, character)
                .map(|hover| index.render_hover(&hover))
                .ok_or_else(|| {
                    anyhow::anyhow!("No hover information available at {path}:{line}:{character}")
                })
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_hover",
            responder,
        );
    });
}

pub(crate) fn spawn_lsp_workspace_symbols_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    query: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspWorkspaceSymbols {
            query: query.clone(),
            limit,
        };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let symbols = index.workspace_symbols(&query, limit.max(1));
            let mut lines = vec![
                "[lsp_workspace_symbols]".to_string(),
                format!("query: {query}"),
            ];
            lines.push(index.render_symbols(&symbols));
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_workspace_symbols",
            responder,
        );
    });
}

pub(crate) fn spawn_lsp_document_symbols_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspDocumentSymbols { path: path.clone() };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let symbols = index.document_symbols(&path);
            let mut lines = vec![
                "[lsp_document_symbols]".to_string(),
                format!("path: {path}"),
            ];
            if symbols.is_empty() {
                lines.push("[no matches]".to_string());
            } else {
                lines.extend(symbols.into_iter().map(|symbol| {
                    format!(
                        "- {}:{}:{} {} {}",
                        symbol.location.path,
                        symbol.location.line,
                        symbol.location.column,
                        symbol.kind,
                        symbol.name
                    )
                }));
            }
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_document_symbols",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lsp_code_actions_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    line: usize,
    character: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspCodeActions {
            path: path.clone(),
            line,
            character,
        };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let code_actions = index.code_actions(&path, line, character);
            let mut lines = vec![
                "[lsp_code_actions]".to_string(),
                format!("path: {path}"),
                format!("cursor: {line}:{character}"),
            ];
            if code_actions.is_empty() {
                lines.push("[no actions]".to_string());
            } else {
                lines.extend(code_actions.into_iter().map(|code_action| {
                    format!(
                        "- {} [{}] {}",
                        code_action.title, code_action.kind, code_action.detail
                    )
                }));
            }
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_code_actions",
            responder,
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_lsp_rename_preview_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: PathBuf,
    project_root: PathBuf,
    path: String,
    old_name: String,
    new_name: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::LspRenamePreview {
            path: path.clone(),
            old_name: old_name.clone(),
            new_name: new_name.clone(),
            limit,
        };
        let result = build_semantic_index(&cwd, &project_root).map(|index| {
            let preview = index.rename_preview(&old_name, &new_name, limit.max(1));
            let mut lines = vec![
                "[lsp_rename_preview]".to_string(),
                format!("path: {path}"),
                format!("old_name: {old_name}"),
                format!("new_name: {new_name}"),
            ];
            lines.push(index.render_rename_preview(&preview));
            lines.join("\n")
        });
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "lsp_rename_preview",
            responder,
        );
    });
}
