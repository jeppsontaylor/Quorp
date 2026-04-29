//! Permission decision routing.

use quorp_desktop_ipc::{IpcError, IpcErrorCode, PermissionDecisionDto, PermissionRequestId};

use crate::state::AppHandleState;

#[tauri::command]
pub fn respond_to_permission(
    state: tauri::State<'_, AppHandleState>,
    request_id: PermissionRequestId,
    decision: PermissionDecisionDto,
) -> Result<(), IpcError> {
    state
        .core
        .permissions
        .resolve(&request_id, decision)
        .map_err(|err| IpcError::new(IpcErrorCode::Stale, err.to_string()))
}

/// Snapshot of the broker's pending-count. The full pending request
/// list is not exposed via this command — the live `Permission`
/// channel events are the source of truth for the UI's queue, which
/// avoids the broker having to materialize and clone every pending
/// `PermissionRequestDto` on each poll.
#[tauri::command]
pub fn pending_permission_count(
    state: tauri::State<'_, AppHandleState>,
) -> Result<usize, IpcError> {
    Ok(state.core.permissions.pending_count())
}

/// Cancel every in-flight permission request. Useful when shutting
/// down a run that has stuck modals.
#[tauri::command]
pub fn cancel_all_permissions(
    state: tauri::State<'_, AppHandleState>,
) -> Result<(), IpcError> {
    state.core.permissions.cancel_all();
    Ok(())
}
