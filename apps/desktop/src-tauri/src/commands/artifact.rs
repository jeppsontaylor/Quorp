//! Artifact-read commands: events.jsonl pagination, summaries,
//! diffs, proof receipts.

use std::path::Path;

use quorp_desktop_ipc::{
    ArtifactKind, ArtifactWindow, IpcError, IpcErrorCode, RunIdDto, WorkspaceId,
};
use serde::Serialize;

use crate::state::AppHandleState;

#[derive(Debug, Clone, Serialize)]
pub struct EventWindowEntry(pub serde_json::Value);

fn workspace_path(
    state: &AppHandleState,
    workspace_id: &WorkspaceId,
) -> Result<std::path::PathBuf, IpcError> {
    state
        .core
        .workspaces
        .get(workspace_id)
        .map(|r| std::path::PathBuf::from(r.canonical_path))
        .ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::WorkspaceNotFound,
                format!("unknown workspace: {workspace_id}"),
            )
        })
}

#[tauri::command]
pub async fn read_artifact(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: WorkspaceId,
    run_id: RunIdDto,
    kind: ArtifactKind,
    offset: u64,
    limit: u64,
) -> Result<ArtifactWindow, IpcError> {
    let root = workspace_path(&state, &workspace_id)?;
    state
        .core
        .artifacts
        .read_kind(&root, &run_id, kind, offset, limit)
        .await
        .map_err(map_artifact_error)
}

#[tauri::command]
pub async fn list_run_artifacts(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: WorkspaceId,
    run_id: RunIdDto,
) -> Result<Vec<ArtifactKind>, IpcError> {
    let root = workspace_path(&state, &workspace_id)?;
    state
        .core
        .artifacts
        .list_kinds(&root, &run_id)
        .await
        .map_err(map_artifact_error)
}

#[tauri::command]
pub async fn read_event_window(
    state: tauri::State<'_, AppHandleState>,
    workspace_id: WorkspaceId,
    run_id: RunIdDto,
    from_seq: u64,
    to_seq: u64,
) -> Result<Vec<serde_json::Value>, IpcError> {
    let root = workspace_path(&state, &workspace_id)?;
    state
        .core
        .artifacts
        .read_event_window(&root, &run_id, from_seq, to_seq)
        .await
        .map_err(map_artifact_error)
}

#[tauri::command]
pub async fn reveal_path(
    state: tauri::State<'_, AppHandleState>,
    path: String,
) -> Result<(), IpcError> {
    let buf = std::path::PathBuf::from(&path);
    // Refuse to reveal paths outside any registered workspace so a
    // malicious frontend can't open arbitrary directories.
    let inside = state
        .core
        .workspaces
        .find_by_path(&buf)
        .map_err(|err| IpcError::new(IpcErrorCode::FilesystemError, err.to_string()))?
        .is_some()
        || state
            .core
            .workspaces
            .list()
            .into_iter()
            .any(|w| Path::new(&path).starts_with(&w.canonical_path));
    if !inside {
        return Err(IpcError::new(
            IpcErrorCode::InvalidInput,
            "reveal_path: path is not inside a registered workspace",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&path)
            .status()
            .map_err(|err| IpcError::new(IpcErrorCode::Internal, err.to_string()))?;
        if !status.success() {
            return Err(IpcError::new(
                IpcErrorCode::Internal,
                format!("open -R exited with status {status}"),
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(IpcError::not_implemented("reveal_path is macOS-only"))
    }
}

fn map_artifact_error(err: quorp_desktop_core::ArtifactError) -> IpcError {
    use quorp_desktop_core::ArtifactError;
    let code = match &err {
        ArtifactError::UnknownWorkspace => IpcErrorCode::WorkspaceNotFound,
        ArtifactError::Missing(_) => IpcErrorCode::FilesystemError,
        ArtifactError::OffsetPastEnd => IpcErrorCode::InvalidInput,
        ArtifactError::Io(_) => IpcErrorCode::FilesystemError,
    };
    IpcError::new(code, err.to_string())
}
