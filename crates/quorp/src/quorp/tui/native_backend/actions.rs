//! Mutating-action handlers and PTY-streaming utilities split out of
//! `native_backend.rs` to keep the parent file under the 2,000 LOC hard
//! cap.
//!
//! Everything here is reachable from the dispatcher in
//! `super::spawn_command_service_loop`. Calls *into* the parent (e.g.
//! `super::stash_file_for_rollback`) use `super::` so the privacy
//! relationship between `native_backend` and this submodule is
//! explicit.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Serialize;
use url::Url;

use super::{COMMAND_OUTPUT_LIMIT, stash_file_for_rollback};
use crate::quorp::tui::agent_context::{
    McpServerConfig, load_agent_config, validation_commands_for_plan,
};
use crate::quorp::tui::agent_protocol::{ActionOutcome, AgentAction, TomlEditOperation};
use crate::quorp::tui::{ChatUiEvent, TuiEvent};
use quorp_agent_core::ToolResultEnvelope;
use quorp_agent_core::{ReadFileRange, stable_content_hash};
use quorp_context::{Anchor, CompileContext, CompileRequest, ContextCompiler, TokenBudget};
use quorp_ids::RuleId;
use quorp_memory::Memory;
use quorp_memory_model::{MemoryQuery, Tier};
use quorp_patch_vm::{
    EditProvenance, FileChange, FileChangeKind, PatchApplyProof, PatchApplyReport, PatchReceipt,
    PatchVm, PatchVmPolicy, hash_bytes,
};
use quorp_rule_forge::{ClusterKey, RuleForge};
use quorp_sandbox::{build_command_plan, default_policy, sandbox_runtime_for_path};
use quorp_tools::apply::apply_patch_edit;
use quorp_tools::edit::{apply_toml_operations, perform_range_replacement, set_executable_bit};
use quorp_tools::patch::{perform_block_replacement, sanitize_project_path};
use quorp_tools::preview::{load_preview_record, syntax_preflight_for_preview};
use quorp_verify::{
    VerifyCommand, VerifyCommandResult, VerifyLevel, VerifyRequest, VerifyStore, VerifyTarget,
    default_verify_plan, execute_verify_request_durable,
};

fn default_patch_vm_policy() -> PatchVmPolicy {
    PatchVmPolicy {
        allow_full_file_rewrite: true,
        max_files: 32,
    }
}

fn record_patch_vm_rollback(session_id: usize, report: &PatchApplyReport) {
    for token in &report.rollback_tokens {
        stash_file_for_rollback(session_id, &token.file);
    }
    for path in &report.touched_paths {
        stash_file_for_rollback(session_id, path);
    }
}

fn render_patch_vm_receipt(receipt: &PatchReceipt) -> String {
    format!(
        "patch_vm_receipt:\npatch_id: {}\nprovenance: {:?}\npreview_id: {}\noutcome: {:?}\ntouched_paths: {}\nrollback_tokens: {}",
        receipt.patch_id,
        receipt.provenance,
        receipt.preview_id,
        receipt.outcome,
        receipt
            .touched_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        receipt.rollback_tokens.len()
    )
}

fn apply_single_file_change(
    session_id: usize,
    path: String,
    target: PathBuf,
    next_content: String,
    provenance: EditProvenance,
) -> anyhow::Result<String> {
    let existing_bytes = std::fs::read(&target).ok();
    let change = FileChange {
        path: target.clone(),
        display_path: path.clone(),
        expected_hash: existing_bytes.as_deref().map(hash_bytes),
        kind: match existing_bytes {
            Some(_) => FileChangeKind::Update {
                content: next_content.into_bytes(),
            },
            None => FileChangeKind::Add {
                content: next_content.into_bytes(),
            },
        },
    };
    let vm = PatchVm::new();
    let patch_id = quorp_ids::PatchId::new(format!(
        "write-{}-{}",
        session_id,
        stable_content_hash(&path)
    ));
    let report = vm.apply_file_changes(
        &patch_id,
        &[change],
        PatchApplyProof::HashesOnly,
        default_patch_vm_policy(),
    )?;
    record_patch_vm_rollback(session_id, &report);
    Ok(render_patch_vm_receipt(&report.receipt(provenance)))
}

fn apply_set_executable_change(
    session_id: usize,
    path: String,
    target: PathBuf,
) -> anyhow::Result<String> {
    let current_bytes = std::fs::read(&target)
        .map_err(|error| anyhow::anyhow!("Failed to read file for set_executable: {error}"))?;
    let change = FileChange {
        path: target.clone(),
        display_path: path.clone(),
        expected_hash: Some(hash_bytes(&current_bytes)),
        kind: FileChangeKind::Update {
            content: current_bytes,
        },
    };
    let vm = PatchVm::new();
    let patch_id = quorp_ids::PatchId::new(format!(
        "chmod-{}-{}",
        session_id,
        stable_content_hash(&path)
    ));
    let report = vm.apply_file_changes(
        &patch_id,
        &[change],
        PatchApplyProof::HashesOnly,
        default_patch_vm_policy(),
    )?;
    record_patch_vm_rollback(session_id, &report);
    set_executable_bit(&target)?;
    Ok(render_patch_vm_receipt(&report.receipt(
        EditProvenance::SetExecutable {
            path: PathBuf::from(path),
        },
    )))
}

fn validation_stage_id(
    plan: &crate::quorp::tui::agent_protocol::ValidationPlan,
    command: &str,
    index: usize,
) -> String {
    if plan.fmt && index == 0 {
        "fmt".to_string()
    } else if plan.clippy && ((plan.fmt && index == 1) || (!plan.fmt && index == 0)) {
        "clippy".to_string()
    } else if plan.workspace_tests && command.contains("test") {
        "workspace_tests".to_string()
    } else if command.contains("cargo test") {
        format!("targeted_test_{index}")
    } else {
        format!("command_{index}")
    }
}

fn validation_plan_to_verify_request(
    cwd: &Path,
    plan: &crate::quorp::tui::agent_protocol::ValidationPlan,
    commands: &[String],
) -> VerifyRequest {
    let mut targets = Vec::new();
    if plan.workspace_tests || plan.fmt || plan.clippy || commands.len() > 1 {
        targets.push(VerifyTarget::Workspace);
    }
    for test in &plan.tests {
        if !test.trim().is_empty() {
            targets.push(VerifyTarget::Test(test.trim().to_string()));
        }
    }
    if targets.is_empty() {
        targets.push(VerifyTarget::Workspace);
    }
    let level = if plan.workspace_tests || plan.clippy {
        VerifyLevel::L3Broad
    } else if plan.fmt || !plan.tests.is_empty() || !plan.custom_commands.is_empty() {
        VerifyLevel::L2Targeted
    } else {
        VerifyLevel::L1Check
    };
    let verify_plan = default_verify_plan(level, targets);
    let joined_commands = commands.join("\n");
    VerifyRequest {
        plan: verify_plan,
        commands: commands
            .iter()
            .enumerate()
            .map(|(index, command)| VerifyCommand {
                stage_id: validation_stage_id(plan, command, index),
                command: command.clone(),
                cwd: cwd.to_path_buf(),
            })
            .collect(),
        git_sha: option_env!("QUORP_COMMIT_SHA")
            .map(str::to_string)
            .unwrap_or_else(|| "workspace".to_string()),
        changed_files_hash: stable_content_hash(&joined_commands),
        features: Vec::new(),
        target_triple: std::env::consts::ARCH.to_string(),
        rustc_version: env!("CARGO_PKG_VERSION").to_string(),
    }
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

fn render_mcp_serialized_result<T: Serialize>(
    server_name: &str,
    tool_name: &str,
    result: &T,
) -> anyhow::Result<String> {
    let rendered = serde_json::to_string_pretty(result)?;
    Ok(format!("MCP {server_name}/{tool_name}\n{rendered}"))
}

fn mcp_workspace_roots(project_root: &Path) -> Vec<crate::quorp::tui::mcp_client::Root> {
    let workspace_root_uri = Url::from_directory_path(project_root)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", project_root.display()));
    vec![crate::quorp::tui::mcp_client::Root {
        uri: workspace_root_uri,
        name: project_root
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string()),
    }]
}

#[allow(clippy::too_many_arguments)]
fn spawn_mcp_serialized_task<F, Fut, T>(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    tool_name: String,
    action: AgentAction,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
    run: F,
) where
    F: FnOnce(crate::quorp::tui::mcp_client::McpBroker) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<T>> + 'static,
    T: Serialize + 'static,
{
    let server_name_for_runtime = server_name.clone();
    let tool_name_for_runtime = tool_name.clone();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<String> {
            let server_config =
                resolve_mcp_server_config(project_root.as_path(), &server_name_for_runtime)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| anyhow::anyhow!("Failed to start MCP runtime: {error}"))?;
            runtime.block_on(async move {
                let client = crate::quorp::tui::mcp_client::McpClient::spawn_transport(
                    crate::quorp::tui::mcp_client::McpTransport::Stdio {
                        command: server_config.command.clone(),
                        args: server_config.args.clone(),
                    },
                    mcp_workspace_roots(project_root.as_path()),
                )
                .await?;
                let broker = crate::quorp::tui::mcp_client::McpBroker::new(
                    client,
                    crate::quorp::tui::mcp_client::McpBrokerPolicy::default(),
                );
                let output = run(broker).await?;
                render_mcp_serialized_result(
                    &server_name_for_runtime,
                    &tool_name_for_runtime,
                    &output,
                )
            })
        })();
        emit_tool_result(&event_tx, session_id, action, result, &tool_name, responder);
    });
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
    let action = AgentAction::McpCallTool {
        server_name: server_name.clone(),
        tool_name: tool_name.clone(),
        arguments: arguments.clone(),
    };
    let server_name_for_runtime = server_name.clone();
    let tool_name_for_runtime = tool_name.clone();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<String> {
            let server_config =
                resolve_mcp_server_config(project_root.as_path(), &server_name_for_runtime)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| anyhow::anyhow!("Failed to start MCP runtime: {error}"))?;
            runtime.block_on(async move {
                let client = crate::quorp::tui::mcp_client::McpClient::spawn_transport(
                    crate::quorp::tui::mcp_client::McpTransport::Stdio {
                        command: server_config.command.clone(),
                        args: server_config.args.clone(),
                    },
                    mcp_workspace_roots(project_root.as_path()),
                )
                .await?;
                let broker = crate::quorp::tui::mcp_client::McpBroker::new(
                    client,
                    crate::quorp::tui::mcp_client::McpBrokerPolicy::default(),
                );
                let result = broker
                    .call_tool(&tool_name_for_runtime, Some(arguments))
                    .await?;
                render_mcp_tool_result(&server_name_for_runtime, &tool_name_for_runtime, &result)
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_mcp_list_tools_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action = AgentAction::McpListTools {
        server_name: server_name.clone(),
    };
    spawn_mcp_serialized_task(
        event_tx,
        session_id,
        project_root,
        server_name,
        "mcp_list_tools".to_string(),
        action,
        responder,
        |broker| async move { broker.list_tools().await },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_mcp_list_resources_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    cursor: Option<String>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action = AgentAction::McpListResources {
        server_name: server_name.clone(),
        cursor: cursor.clone(),
    };
    spawn_mcp_serialized_task(
        event_tx,
        session_id,
        project_root,
        server_name,
        "mcp_list_resources".to_string(),
        action,
        responder,
        move |broker| async move { broker.list_resources(cursor).await },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_mcp_read_resource_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    uri: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action = AgentAction::McpReadResource {
        server_name: server_name.clone(),
        uri: uri.clone(),
    };
    spawn_mcp_serialized_task(
        event_tx,
        session_id,
        project_root,
        server_name,
        "mcp_read_resource".to_string(),
        action,
        responder,
        move |broker| async move { broker.read_resource(&uri).await },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_mcp_list_prompts_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    cursor: Option<String>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action = AgentAction::McpListPrompts {
        server_name: server_name.clone(),
        cursor: cursor.clone(),
    };
    spawn_mcp_serialized_task(
        event_tx,
        session_id,
        project_root,
        server_name,
        "mcp_list_prompts".to_string(),
        action,
        responder,
        move |broker| async move { broker.list_prompts(cursor).await },
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_mcp_get_prompt_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    server_name: String,
    name: String,
    arguments: Option<serde_json::Value>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    let action = AgentAction::McpGetPrompt {
        server_name: server_name.clone(),
        name: name.clone(),
        arguments: arguments.clone(),
    };
    spawn_mcp_serialized_task(
        event_tx,
        session_id,
        project_root,
        server_name,
        "mcp_get_prompt".to_string(),
        action,
        responder,
        move |broker| async move { broker.get_prompt(&name, arguments).await },
    );
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
            let receipt = apply_single_file_change(
                session_id,
                path.clone(),
                candidate,
                content.clone(),
                EditProvenance::WriteFile {
                    path: PathBuf::from(path.clone()),
                },
            )?;
            Ok(format!(
                "Wrote {} bytes to {path}\n{receipt}",
                content.len()
            ))
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
            let receipt = apply_single_file_change(
                session_id,
                path.clone(),
                target,
                new_content,
                EditProvenance::ReplaceBlock {
                    path: PathBuf::from(path.clone()),
                },
            )?;
            Ok(format!("Replaced block in {path}\n{receipt}"))
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
            let receipt = apply_single_file_change(
                session_id,
                path.clone(),
                target,
                updated_content,
                EditProvenance::ReplaceRange {
                    path: PathBuf::from(path.clone()),
                },
            )?;
            Ok(format!(
                "Replaced lines {} in {path}\n{}\n{}",
                range.label(),
                syntax_preflight,
                receipt
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
            let receipt = apply_single_file_change(
                session_id,
                path.clone(),
                target,
                updated_content,
                EditProvenance::ModifyToml {
                    path: PathBuf::from(path.clone()),
                },
            )?;
            Ok(format!(
                "Applied {} TOML operation(s) to {path}\n{}\n{}",
                operations.len(),
                syntax_preflight,
                receipt
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
            let receipt = apply_single_file_change(
                session_id,
                record.path.clone(),
                record.target_path.clone(),
                record.updated_content.clone(),
                EditProvenance::ApplyPreview {
                    preview_id: preview_id.clone(),
                },
            )?;
            Ok(format!(
                "Applied preview {preview_id} to {}\nedit_kind: {}\nsyntax_preflight: {}\n{}",
                record.path, record.edit_kind, record.syntax_status, receipt
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
        let verify_request = validation_plan_to_verify_request(&cwd, &plan, &commands);
        let result =
            run_validation_commands(&event_tx, session_id, &cwd, commands, command_output_limit);

        if let Err(e) = &result {
            let error_text = e.to_string();
            if enable_rollback_on_validation_failure {
                super::rollback_session_worktree(session_id);
                let rolled_back_error = anyhow::anyhow!(
                    "{}\n\n[System] Changes were safely rolled back. Please analyze the error and try applying a corrected fix.",
                    error_text
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
                Err(anyhow::anyhow!(error_text)),
                "run_validation",
                responder,
            );
            return;
        } else {
            super::clear_session_worktree(session_id);
        }

        let rendered_output = result.and_then(|validation_runs| {
            let verify_store = VerifyStore::for_workspace(&cwd);
            let report = execute_verify_request_durable(
                &verify_store,
                &verify_request,
                serde_json::json!({
                    "source": "native_validation",
                    "session_id": session_id,
                    "cwd": cwd,
                }),
                |command| {
                validation_runs
                    .iter()
                    .find(|run| run.command == command.command)
                    .map(|run| VerifyCommandResult {
                        exit_code: run.exit_code,
                        duration_ms: run.duration_ms,
                        output: run.output.clone(),
                        raw_log_path: run.raw_log_path.clone(),
                        tool_version: None,
                        truncated: run.truncated,
                    })
                    .ok_or_else(|| format!("missing validation run for `{}`", command.command))
                },
            )
            .map_err(anyhow::Error::msg)?;
            let mut output = validation_runs
                .iter()
                .map(|run| run.rendered_output.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!(
                "\nVerification report:\noverall: {:?}\ncache_hits: {}\nwall_ms: {}\nstages:\n{}\nproof_packets: {}",
                report.overall,
                report.cache_hits,
                report.wall_ms,
                report
                    .stages
                    .iter()
                    .map(|stage| format!(
                        "- {}: {:?}{} ({})",
                        stage.stage_id,
                        stage.status,
                        if stage.from_cache { ", cached" } else { "" },
                        stage.summary
                    ))
                    .collect::<Vec<_>>()
                    .join("\n"),
                report.proof_packets.len()
            ));
            Ok(output)
        });

        emit_tool_result(
            &event_tx,
            session_id,
            action,
            rendered_output,
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
            let receipt = apply_set_executable_change(session_id, path.clone(), target)?;
            Ok(format!("Enabled executable bit for {path}\n{receipt}"))
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

pub(crate) fn spawn_expand_context_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    handle: String,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ExpandContext {
            handle: handle.clone(),
        };
        let result = (|| -> anyhow::Result<String> {
            let compiler = ContextCompiler::new();
            let query = if handle.trim().is_empty() {
                Anchor::Query("expand context".to_string())
            } else {
                Anchor::Query(handle.clone())
            };
            let request = CompileRequest {
                anchors: vec![query],
                budget: TokenBudget {
                    total: 4_000,
                    per_item_cap: 1_000,
                    reserve_for_output: 600,
                },
            };
            let context = CompileContext {
                git_sha: option_env!("QUORP_COMMIT_SHA").map(str::to_string),
                generated_at_unix: current_unix_time(),
            };
            let pack = compiler.compile_workspace(project_root.as_path(), &request, &context)?;
            Ok(render_context_pack_for_action(&pack))
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "expand_context",
            responder,
        );
    });
}

pub(crate) fn spawn_recall_memory_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    project_root: PathBuf,
    query: String,
    limit: usize,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::RecallMemory {
            query: query.clone(),
            limit,
        };
        let result = (|| -> anyhow::Result<String> {
            let memory = Memory::with_workspace(&project_root)?;
            let hits = memory.recall(&MemoryQuery {
                query_text: Some(query.clone()),
                tier: Some(Tier::Semantic),
                limit: u32::try_from(limit).unwrap_or(u32::MAX),
            })?;
            Ok(render_memory_hits(&query, &hits))
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "recall_memory",
            responder,
        );
    });
}

pub(crate) fn spawn_propose_rule_task(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    statement: String,
    error_code: Option<String>,
    evidence: Option<String>,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
) {
    std::thread::spawn(move || {
        let action = AgentAction::ProposeRule {
            statement: statement.clone(),
            error_code: error_code.clone(),
            evidence: evidence.clone(),
        };
        let result = (|| -> anyhow::Result<String> {
            let forge = RuleForge::new();
            let failure = quorp_verify::Failure {
                code: error_code.clone(),
                message: statement.clone(),
                level: "tool".to_string(),
                file: None,
                line: None,
            };
            let signature = forge.observe_failure(&failure)?;
            let key = ClusterKey::from_failure(&failure);
            let candidate = forge.maybe_emit_candidate(&key, statement.clone())?;
            Ok(render_rule_proposal(
                &statement,
                &signature.signature,
                candidate,
            ))
        })();
        emit_tool_result(
            &event_tx,
            session_id,
            action,
            result,
            "propose_rule",
            responder,
        );
    });
}

fn current_unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn render_context_pack_for_action(pack: &quorp_context::ContextPack) -> String {
    let mut lines = vec![
        "Expanded context pack:".to_string(),
        format!(
            "pack_id={} items={} handles={} budget_used={}",
            pack.pack_id.as_str(),
            pack.items.len(),
            pack.handles.len(),
            pack.budget_used
        ),
    ];
    for item in pack.items.iter().take(6) {
        lines.push(render_context_item_for_action(item));
    }
    for handle in pack.handles.iter().take(4) {
        lines.push(format!(
            "handle {} source={:?} ~{} tokens",
            handle.label, handle.source, handle.estimated_cost_tokens
        ));
    }
    lines.join("\n")
}

fn render_context_item_for_action(item: &quorp_context::ContextItem) -> String {
    match item {
        quorp_context::ContextItem::Excerpt {
            path, range, text, ..
        } => format!(
            "- excerpt {}:{}-{} {}",
            path.display(),
            range.start,
            range.end,
            truncate_action_text(text, 160)
        ),
        quorp_context::ContextItem::SymbolDef {
            path,
            signature,
            body_excerpt,
            ..
        } => format!(
            "- symbol {} :: {} {}",
            path.as_str(),
            truncate_action_text(signature, 96),
            truncate_action_text(body_excerpt, 96)
        ),
        quorp_context::ContextItem::Memory { snippet, .. } => {
            format!("- memory {}", truncate_action_text(snippet, 160))
        }
        quorp_context::ContextItem::Rule {
            rule_id, statement, ..
        } => format!(
            "- rule {} :: {}",
            rule_id.as_str(),
            truncate_action_text(statement, 160)
        ),
        quorp_context::ContextItem::AgentContract { title, body, .. } => format!(
            "- contract {} :: {}",
            title,
            truncate_action_text(body, 160)
        ),
    }
}

fn render_memory_hits(query: &str, hits: &[quorp_memory_model::MemoryHit]) -> String {
    let mut lines = vec![format!("Memory recall for `{query}`:")];
    if hits.is_empty() {
        lines.push("no hits".to_string());
    } else {
        for hit in hits.iter().take(8) {
            lines.push(format!(
                "- {:?} score={:.2} {}",
                hit.tier,
                hit.score,
                truncate_action_text(&hit.snippet, 180)
            ));
        }
    }
    lines.join("\n")
}

fn render_rule_proposal(statement: &str, signature: &str, candidate: Option<RuleId>) -> String {
    let mut lines = vec![
        "Rule proposal:".to_string(),
        format!("statement: {statement}"),
        format!("negative_signature: {signature}"),
    ];
    match candidate {
        Some(rule_id) => lines.push(format!("candidate_rule_id: {}", rule_id.as_str())),
        None => lines.push("candidate_rule_id: pending another matching failure".to_string()),
    }
    lines.join("\n")
}

fn truncate_action_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.to_string();
    truncated.truncate(max_chars);
    truncated.push_str("...");
    truncated
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
            "tool_result": ToolResultEnvelope::from_outcome(outcome.action(), &outcome),
            "output_preview": truncate_diagnostic_text(outcome.output_text(), 240),
        }),
    );
    send_chat_event(
        event_tx,
        ChatUiEvent::CommandOutput(session_id, outcome.output_text().to_string()),
    );
    emit_tool_finished(event_tx, session_id, outcome, responder);
}

pub(crate) struct ValidationCommandRun {
    pub(crate) command: String,
    pub(crate) output: String,
    pub(crate) exit_code: i32,
    pub(crate) duration_ms: u64,
    pub(crate) rendered_output: String,
    pub(crate) raw_log_path: PathBuf,
    pub(crate) truncated: bool,
}

pub(crate) fn run_validation_commands(
    event_tx: &std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    cwd: &Path,
    commands: Vec<String>,
    command_output_limit: usize,
) -> anyhow::Result<Vec<ValidationCommandRun>> {
    if commands.is_empty() {
        return Err(anyhow::anyhow!(
            "run_validation had no resolved commands; check .quorp/agent.toml"
        ));
    }

    let mut combined_runs = Vec::new();
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
        let duration_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let mut rendered_output = format!("$ {command}\n{}", command_output.output);
        if !rendered_output.ends_with('\n') {
            rendered_output.push('\n');
        }
        crate::quorp::tui::diagnostics::log_event(
            "agent.validation_finished",
            serde_json::json!({
                "session_id": session_id,
                "cwd": cwd.display().to_string(),
                "command": command,
                "exit_code": command_output.exit_code,
                "duration_ms": duration_ms,
                "output_preview": truncate_diagnostic_text(&command_output.output, 240),
            }),
        );
        let raw_log_path = cwd
            .join(".quorp")
            .join(format!("verify-{}.log", stable_content_hash(&command)));
        if let Some(parent) = raw_log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                anyhow::anyhow!("Failed to create verification log directory: {error}")
            })?;
        }
        std::fs::write(&raw_log_path, command_output.output.as_bytes())
            .map_err(|error| anyhow::anyhow!("Failed to write verification log: {error}"))?;
        let run = ValidationCommandRun {
            command: command.clone(),
            output: command_output.output.clone(),
            exit_code: command_output.exit_code,
            duration_ms,
            rendered_output,
            raw_log_path,
            truncated: command_output.output.len() >= command_output_limit,
        };
        if command_output.exit_code != 0 {
            let mut combined_output = combined_runs
                .iter()
                .map(|existing: &ValidationCommandRun| existing.rendered_output.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !combined_output.is_empty() && !combined_output.ends_with('\n') {
                combined_output.push('\n');
            }
            combined_output.push_str(run.rendered_output.as_str());
            combined_output.push_str(&format!("[Exit code: {}]", command_output.exit_code));
            return Err(anyhow::anyhow!(combined_output));
        }
        combined_runs.push(run);
    }
    Ok(combined_runs)
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

    let policy = default_policy();
    let runtime = sandbox_runtime_for_path(cwd)?;
    let plan = build_command_plan(quorp_sandbox::SandboxCommandSpec {
        program: std::ffi::OsStr::new(&policy.default_shell),
        args: &[std::ffi::OsStr::new("-lc"), std::ffi::OsStr::new(command)],
        current_dir: cwd,
        runtime: &runtime,
        policy: &policy,
        extra_environment: &[],
        additional_mounts: &[],
        interactive: false,
    })?;
    let mut builder = CommandBuilder::new(plan.program.clone());
    plan.apply_to_command(&mut builder);

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

    let policy = default_policy();
    let runtime = sandbox_runtime_for_path(cwd)?;
    let plan = build_command_plan(quorp_sandbox::SandboxCommandSpec {
        program: std::ffi::OsStr::new(&policy.default_shell),
        args: &[std::ffi::OsStr::new("-lc"), std::ffi::OsStr::new(command)],
        current_dir: cwd,
        runtime: &runtime,
        policy: &policy,
        extra_environment: &[],
        additional_mounts: &[],
        interactive: true,
    })?;
    let mut builder = CommandBuilder::new(plan.program.clone());
    plan.apply_to_command(&mut builder);

    let mut child = pair.slave.spawn_command(builder)?;
    drop(pair.slave);

    #[cfg(unix)]
    let process_group_leader = pair.master.process_group_leader();
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
    let mut cleanup_errors = Vec::new();
    let exit_code = loop {
        if std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            if let Some(leader) = process_group_leader {
                let result = unsafe { libc::killpg(leader, libc::SIGKILL) };
                if result != 0 {
                    cleanup_errors.push(format!(
                        "failed to kill command process group {leader}: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
            #[cfg(not(unix))]
            if let Err(error) = child.kill() {
                cleanup_errors.push(format!("failed to kill timed-out command: {error}"));
            }
            break Some(-1);
        }
        match child.try_wait()? {
            Some(status) => break Some(status.exit_code() as i32),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    if exit_code == Some(-1)
        && let Err(error) = child.wait()
    {
        cleanup_errors.push(format!("failed to wait on timed-out command: {error}"));
    }
    if reader_thread.join().is_err() {
        log::error!("tui: command reader thread panicked");
    }

    let mut final_output = output
        .lock()
        .map(|captured| captured.clone())
        .unwrap_or_default();
    if exit_code == Some(-1) {
        final_output.push_str("\n[Command timed out]");
        append_command_timeout_cleanup_errors(&mut final_output, &cleanup_errors);
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

fn append_command_timeout_cleanup_errors(final_output: &mut String, cleanup_errors: &[String]) {
    if cleanup_errors.is_empty() {
        return;
    }
    final_output.push_str("\n[Command cleanup failed: ");
    final_output.push_str(&cleanup_errors.join("; "));
    final_output.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_cleanup_errors_are_included_in_failure_output() {
        let mut output = String::from("partial output\n[Command timed out]");

        append_command_timeout_cleanup_errors(
            &mut output,
            &[
                "failed to kill command process group 123: no such process".to_string(),
                "failed to wait on timed-out command: child already waited".to_string(),
            ],
        );

        assert!(output.contains("[Command cleanup failed: "));
        assert!(output.contains("failed to kill command process group 123"));
        assert!(output.contains("failed to wait on timed-out command"));
    }

    #[test]
    fn validation_plan_maps_to_verify_request() {
        let plan = crate::quorp::tui::agent_protocol::ValidationPlan {
            fmt: true,
            clippy: true,
            workspace_tests: false,
            tests: vec!["crate::tests::smoke".to_string()],
            custom_commands: vec!["cargo check -p quorp_session".to_string()],
        };
        let commands = vec![
            "cargo fmt --all --check".to_string(),
            "cargo clippy --workspace".to_string(),
            "cargo test crate::tests::smoke".to_string(),
            "cargo check -p quorp_session".to_string(),
        ];

        let request = validation_plan_to_verify_request(Path::new("."), &plan, &commands);

        assert_eq!(request.plan.level, VerifyLevel::L3Broad);
        assert_eq!(request.commands.len(), 4);
        assert_eq!(request.commands[0].stage_id, "fmt");
        assert_eq!(request.commands[1].stage_id, "clippy");
        assert!(request.plan.targets.iter().any(
            |target| matches!(target, VerifyTarget::Test(name) if name == "crate::tests::smoke")
        ));
    }

    #[test]
    fn apply_single_file_change_returns_patch_vm_receipt_and_updates_bytes() {
        let root = tempfile::tempdir().expect("tempdir");
        let target = root.path().join("target.txt");

        let receipt = apply_single_file_change(
            41,
            "target.txt".to_string(),
            target.clone(),
            "hello\n".to_string(),
            EditProvenance::WriteFile {
                path: PathBuf::from("target.txt"),
            },
        )
        .expect("apply");

        assert!(receipt.contains("patch_vm_receipt"));
        assert!(receipt.contains("rollback_tokens: 0"));
        assert_eq!(std::fs::read_to_string(target).expect("read"), "hello\n");
    }
}
