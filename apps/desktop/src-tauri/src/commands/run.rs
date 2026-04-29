//! Run lifecycle commands.

use quorp_desktop_core::run_service::RealRunOptions;
use quorp_desktop_ipc::{
    DEFAULT_MODEL_ID, DesktopEvent, IpcError, IpcErrorCode, RunHandle, RunIdDto, RunStatusDto,
    StartRunRequest,
};
use tokio::sync::mpsc;

use crate::state::AppHandleState;

/// Start a desktop-driven agent run. Returns immediately with a
/// [`RunHandle`]; events stream through the Tauri channel `on_event`.
///
/// Dispatch policy:
/// - When `provider_registry.has_api_key()` is `true`, the run drives
///   `run_headless_agent_with_hooks` against the live NVIDIA NIM
///   endpoint (real model traffic, real RuntimeEvents).
/// - Otherwise we fall back to `start_demo_run` so the rest of the
///   desktop can be exercised without a configured key. The demo
///   path emits a synthetic stream so the timeline reducer can be
///   exercised.
#[tauri::command]
pub async fn start_agent_run(
    state: tauri::State<'_, AppHandleState>,
    request: StartRunRequest,
    on_event: tauri::ipc::Channel<DesktopEvent>,
) -> Result<RunHandle, IpcError> {
    let sanitized = state
        .core
        .runs
        .sanitize_request(&state.core.workspaces, request)
        .map_err(IpcError::from)?;
    let workspace = state
        .core
        .workspaces
        .get(&sanitized.workspace_id)
        .ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::WorkspaceNotFound,
                format!("workspace not found: {}", sanitized.workspace_id),
            )
        })?;

    let (forward_tx, mut forward_rx) = mpsc::unbounded_channel::<DesktopEvent>();
    let handle = if state.core.providers.has_api_key() {
        let options = RealRunOptions::defaults_for(
            std::path::PathBuf::from(&workspace.canonical_path),
            sanitized.goal.clone(),
            sanitized
                .model_id
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            sanitized.permission_mode,
            sanitized.sandbox_mode,
        );
        state
            .core
            .runs
            .start_real_run(
                options,
                state.core.providers.clone(),
                state.core.secrets.clone(),
                state.core.runtime.clone(),
                forward_tx,
            )
            .map_err(IpcError::from)?
    } else {
        state
            .core
            .runs
            .start_demo_run(sanitized, forward_tx)
            .map_err(IpcError::from)?
    };

    // Drainer task: forward every DesktopEvent into the Tauri channel.
    let runtime = state.core.runtime.clone();
    runtime.spawn(async move {
        while let Some(event) = forward_rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    Ok(handle)
}

#[tauri::command]
pub fn cancel_run(
    state: tauri::State<'_, AppHandleState>,
    run_id: RunIdDto,
) -> Result<(), IpcError> {
    state
        .core
        .runs
        .cancel_run(&run_id)
        .map_err(IpcError::from)?;
    state.core.permissions.cancel_all();
    Ok(())
}

#[tauri::command]
pub fn get_run_status(
    state: tauri::State<'_, AppHandleState>,
    run_id: RunIdDto,
) -> Result<RunStatusDto, IpcError> {
    state
        .core
        .runs
        .status(&run_id)
        .ok_or_else(|| IpcError::new(IpcErrorCode::RunNotFound, format!("run not found: {run_id}")))
}

#[tauri::command]
pub fn list_active_runs(
    state: tauri::State<'_, AppHandleState>,
) -> Result<Vec<RunHandle>, IpcError> {
    Ok(state.core.runs.active_handles())
}
