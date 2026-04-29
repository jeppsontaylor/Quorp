//! Workspace registry commands. Exposed as `quorp:*` invocations.
//!
//! Add Folder is the only fs-touching frontend surface and routes
//! through `tauri-plugin-dialog` on the JS side; the path it returns
//! is canonicalized in Rust before any work happens.

use std::path::PathBuf;

use quorp_desktop_ipc::{IpcError, IpcErrorCode, TrustDecision, TrustReceipt, WorkspaceId, WorkspaceSummary};

use crate::state::AppHandleState;

#[tauri::command]
pub fn add_workspace(
    state: tauri::State<'_, AppHandleState>,
    path: String,
) -> Result<WorkspaceSummary, IpcError> {
    let path = PathBuf::from(&path);
    let record = state
        .core
        .workspaces
        .add(&path)
        .map_err(|err| IpcError::new(IpcErrorCode::InvalidInput, err.to_string()))?;
    Ok(record.to_summary())
}

#[tauri::command]
pub fn list_workspaces(
    state: tauri::State<'_, AppHandleState>,
) -> Result<Vec<WorkspaceSummary>, IpcError> {
    Ok(state
        .core
        .workspaces
        .list()
        .into_iter()
        .map(|r| r.to_summary())
        .collect())
}

#[tauri::command]
pub fn trust_workspace(
    state: tauri::State<'_, AppHandleState>,
    id: WorkspaceId,
    decision: TrustDecision,
) -> Result<TrustReceipt, IpcError> {
    let receipt = state
        .core
        .workspaces
        .set_trust(&id, decision)
        .map_err(|err| IpcError::new(IpcErrorCode::WorkspaceNotFound, err.to_string()))?;
    state.core.trust_log.record(receipt.clone());
    Ok(receipt)
}

#[tauri::command]
pub fn remove_workspace(
    state: tauri::State<'_, AppHandleState>,
    id: WorkspaceId,
) -> Result<(), IpcError> {
    state
        .core
        .workspaces
        .remove(&id)
        .map_err(|err| IpcError::new(IpcErrorCode::WorkspaceNotFound, err.to_string()))
}

/// Open the system Terminal at `path`. macOS-only; on other platforms
/// returns `NotImplemented`. The path must canonically resolve inside
/// a registered workspace; otherwise we refuse so a malicious frontend
/// cannot prompt the user to type into a Terminal at an arbitrary
/// directory.
#[tauri::command]
pub fn open_terminal_at(
    state: tauri::State<'_, AppHandleState>,
    path: String,
) -> Result<(), IpcError> {
    let buf = PathBuf::from(&path);
    let known = state
        .core
        .workspaces
        .find_by_path(&buf)
        .map_err(|err| IpcError::new(IpcErrorCode::FilesystemError, err.to_string()))?;
    if known.is_none() {
        return Err(IpcError::new(
            IpcErrorCode::InvalidInput,
            "open_terminal_at: path is not a registered workspace",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\" to do script \"cd {}\"",
            shell_quote(&path)
        );
        match std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .status()
        {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(IpcError::new(
                IpcErrorCode::Internal,
                format!("osascript exited with status {status}"),
            )),
            Err(err) => Err(IpcError::new(IpcErrorCode::Internal, err.to_string())),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(IpcError::not_implemented(
            "open_terminal_at is macOS-only in v1",
        ))
    }
}

#[cfg(target_os = "macos")]
fn shell_quote(path: &str) -> String {
    // Escape backslashes and quotes for the AppleScript double-quoted
    // string. `osascript -e 'tell ... do script "..."'` evaluates the
    // string in two layers (sh + AppleScript); we keep the input
    // simple by accepting only the escapes that survive both.
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!("'{escaped}'")
}
