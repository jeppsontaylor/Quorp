//! Tauri commands for the Expansive surfaces (PR10).
//!
//! Each command here is the wire-level entry point; the substantive
//! Rust-side implementations live in their respective modules in
//! `quorp_desktop_core`. Several of the deeper integrations
//! (multi-window state-shape, terminal PTY, auto-updater feed,
//! memory/rules CRUD) are intentionally typed stubs that return
//! `IpcError::not_implemented(...)` so the frontend can render the
//! surface with a clear "coming in vN.M" message rather than crashing
//! against a panic.

use std::path::PathBuf;

use quorp_desktop_ipc::{IpcError, IpcErrorCode, WorkspaceId};
use serde::Serialize;

use crate::state::AppHandleState;

#[derive(Debug, Clone, Serialize)]
pub struct UpdaterStatus {
    pub current_version: String,
    pub latest_known: String,
    pub update_available: bool,
    pub channel: String,
}

#[tauri::command]
pub fn check_for_updates(
    state: tauri::State<'_, AppHandleState>,
) -> Result<UpdaterStatus, IpcError> {
    let _ = state;
    let current = env!("CARGO_PKG_VERSION").to_string();
    Ok(UpdaterStatus {
        current_version: current.clone(),
        latest_known: current,
        update_available: false,
        channel: "manual".to_string(),
    })
}

#[tauri::command]
pub fn apply_update(
    state: tauri::State<'_, AppHandleState>,
) -> Result<(), IpcError> {
    let _ = state;
    Err(IpcError::not_implemented(
        "apply_update: ships once the Sparkle/Tauri-updater feed is configured",
    ))
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    pub label: String,
    pub workspace_id: Option<WorkspaceId>,
}

/// Open a new desktop window scoped to the named workspace. The
/// real implementation negotiates a fresh per-window slice of
/// `DesktopAppState`; today we surface a clear NotImplemented so
/// the menu item is honest about its readiness.
#[tauri::command]
pub fn new_window(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: Option<WorkspaceId>,
) -> Result<WindowInfo, IpcError> {
    let _ = state;
    Err(IpcError::not_implemented(format!(
        "new_window(workspace_id={workspace_id:?}): multi-window lands once the per-window state-shape is split",
    )))
}

/// Wire shape returned by `query_memory`. Mirrors
/// `quorp_desktop_core::MemoryQueryResult` 1:1 so the wrapper can
/// pass it through transparently.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryQueryResult {
    pub tier: String,
    pub query: String,
    pub items: Vec<MemoryItemDto>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryItemDto {
    pub id: String,
    pub tier: String,
    pub summary: String,
    pub score: f32,
    pub recorded_at: String,
}

impl From<quorp_desktop_core::MemoryQueryResult> for MemoryQueryResult {
    fn from(value: quorp_desktop_core::MemoryQueryResult) -> Self {
        Self {
            tier: value.tier,
            query: value.query,
            items: value.items.into_iter().map(MemoryItemDto::from).collect(),
            total: value.total,
        }
    }
}

impl From<quorp_desktop_core::MemoryItemDto> for MemoryItemDto {
    fn from(value: quorp_desktop_core::MemoryItemDto) -> Self {
        Self {
            id: value.id,
            tier: value.tier,
            summary: value.summary,
            score: value.score,
            recorded_at: value.recorded_at,
        }
    }
}

#[tauri::command]
pub async fn query_memory(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: WorkspaceId,
    tier: String,
    query: String,
    limit: u32,
) -> Result<MemoryQueryResult, IpcError> {
    if tier.is_empty() {
        return Err(IpcError::invalid_input("tier must not be empty"));
    }
    let workspace = state.core.workspaces.get(&workspace_id).ok_or_else(|| {
        IpcError::new(
            IpcErrorCode::WorkspaceNotFound,
            format!("workspace not found: {workspace_id}"),
        )
    })?;
    let workspace_root = PathBuf::from(&workspace.canonical_path);
    let memory = state.core.memory.clone();
    let runtime = state.core.runtime.clone();
    let result = runtime
        .spawn(async move {
            memory
                .query(&workspace_root, &tier, query, limit)
                .await
        })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join: {err}")))?
        .map_err(map_memory_error)?;
    Ok(MemoryQueryResult::from(result))
}

fn map_memory_error(err: quorp_desktop_core::MemoryAdapterError) -> IpcError {
    use quorp_desktop_core::MemoryAdapterError;
    let code = match &err {
        MemoryAdapterError::UnknownTier(_) => IpcErrorCode::InvalidInput,
        MemoryAdapterError::Open(..) => IpcErrorCode::FilesystemError,
        MemoryAdapterError::Query(_) => IpcErrorCode::Internal,
        MemoryAdapterError::Io(_) => IpcErrorCode::FilesystemError,
    };
    IpcError::new(code, err.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryPruneReceipt {
    pub tier: String,
    pub removed: u32,
}

#[tauri::command]
pub async fn prune_memory(
    state: tauri::State<'_, AppHandleState>,
    tier: String,
    older_than_iso: String,
) -> Result<MemoryPruneReceipt, IpcError> {
    let _ = (state, older_than_iso);
    Err(IpcError::not_implemented(format!(
        "prune_memory(tier={tier}): lands once the memory adapter is wired"
    )))
}

/// Wire shape mirroring `quorp_desktop_core::RuleSummaryDto`.
#[derive(Debug, Clone, Serialize)]
pub struct RuleSummaryDto {
    pub id: String,
    pub display_name: String,
    pub source_path: String,
    pub lifecycle: String,
    pub evidence_count: u32,
}

impl From<quorp_desktop_core::RuleSummaryDto> for RuleSummaryDto {
    fn from(value: quorp_desktop_core::RuleSummaryDto) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            source_path: value.source_path,
            lifecycle: value.lifecycle,
            evidence_count: value.evidence_count,
        }
    }
}

#[tauri::command]
pub async fn list_rules(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: Option<WorkspaceId>,
) -> Result<Vec<RuleSummaryDto>, IpcError> {
    let workspace_root = match workspace_id.as_ref() {
        Some(id) => state.core.workspaces.get(id).map(|w| {
            PathBuf::from(w.canonical_path)
        }),
        None => None,
    };
    let runtime = state.core.runtime.clone();
    let rules = runtime
        .spawn_blocking(move || {
            let root = workspace_root.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            });
            quorp_desktop_core::list_rules(&root)
        })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join: {err}")))?
        .map_err(map_rules_error)?;
    Ok(rules.into_iter().map(RuleSummaryDto::from).collect())
}

#[tauri::command]
pub async fn update_rule_lifecycle(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: WorkspaceId,
    rule_id: String,
    lifecycle: String,
) -> Result<String, IpcError> {
    let workspace = state.core.workspaces.get(&workspace_id).ok_or_else(|| {
        IpcError::new(
            IpcErrorCode::WorkspaceNotFound,
            format!("workspace not found: {workspace_id}"),
        )
    })?;
    let workspace_root = PathBuf::from(&workspace.canonical_path);
    let runtime = state.core.runtime.clone();
    let new_lifecycle = runtime
        .spawn_blocking(move || {
            quorp_desktop_core::update_lifecycle(&workspace_root, &rule_id, &lifecycle)
        })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join: {err}")))?
        .map_err(map_rules_error)?;
    Ok(new_lifecycle)
}

fn map_rules_error(err: quorp_desktop_core::RulesAdapterError) -> IpcError {
    use quorp_desktop_core::RulesAdapterError;
    let code = match &err {
        RulesAdapterError::Io(_) => IpcErrorCode::FilesystemError,
        RulesAdapterError::Ledger(_) => IpcErrorCode::InvalidInput,
    };
    IpcError::new(code, err.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentTimelineFilterDto {
    pub agent: String,
    pub matched_event_kinds: Vec<String>,
}

/// Returns the list of agent identities the current run has emitted
/// events for. Today this is always `["main"]`; once the runtime
/// produces decomposed verifier / patch-reviewer streams, the run
/// service will track them per `agent_id` and we'll surface the
/// distinct list here.
#[tauri::command]
pub fn list_agents_in_run(
    state: tauri::State<'_, AppHandleState>,
    run_id: quorp_desktop_ipc::RunIdDto,
) -> Result<Vec<String>, IpcError> {
    if state.core.runs.status(&run_id).is_none() {
        return Err(IpcError::new(
            IpcErrorCode::RunNotFound,
            format!("run not found: {run_id}"),
        ));
    }
    Ok(vec!["main".to_string()])
}
