//! Checkpoint, rollback, diff-apply, and verify-again Tauri commands.
//!
//! These wrap deeper integrations into the agent runtime. The full
//! implementations land alongside their respective inspector views;
//! PR7 ships the typed wire shape so the frontend can light up the
//! corresponding buttons. Callers that haven't finished wiring the
//! backend get a clear `NotImplemented` error instead of a panic.

use std::path::PathBuf;

use quorp_desktop_core::ApplyDiffReceipt as CoreApplyReceipt;
use quorp_desktop_ipc::{IpcError, IpcErrorCode, RunIdDto, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::state::AppHandleState;

/// Wire shape returned to the frontend. Mirrors the receipt the core
/// crate emits 1:1 so callers don't need a translator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyDiffReceipt {
    pub run_id: RunIdDto,
    pub target_workspace_id: WorkspaceId,
    pub applied_files: u32,
    pub skipped_files: u32,
    pub conflict_files: u32,
    pub message: String,
}

impl From<CoreApplyReceipt> for ApplyDiffReceipt {
    fn from(value: CoreApplyReceipt) -> Self {
        Self {
            run_id: value.run_id,
            target_workspace_id: value.target_workspace_id,
            applied_files: value.applied_files,
            skipped_files: value.skipped_files,
            conflict_files: value.conflict_files,
            message: value.message,
        }
    }
}

/// Promote a sandbox run's `final.diff` back into the source
/// workspace. Routes through `quorp_desktop_core::apply_run_diff`,
/// which prefers `git apply` when the workspace is a git repo and
/// falls back to an in-process unified-diff applier otherwise.
///
/// The receipt carries `applied_files`, `skipped_files`, and
/// `conflict_files`. A non-zero `conflict_files` means the apply
/// was rejected entirely; nothing changed on disk.
#[tauri::command]
pub async fn apply_run_diff(
    state: tauri::State<'_, AppHandleState>,
    run_id: RunIdDto,
    target_workspace_id: WorkspaceId,
) -> Result<ApplyDiffReceipt, IpcError> {
    let workspace = state
        .core
        .workspaces
        .get(&target_workspace_id)
        .ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::WorkspaceNotFound,
                format!("workspace not found: {target_workspace_id}"),
            )
        })?;
    let workspace_root = PathBuf::from(&workspace.canonical_path);
    let run_dir = workspace_root
        .join(".quorp")
        .join("runs")
        .join(run_id.as_str());
    let runtime = state.core.runtime.clone();
    let receipt = runtime
        .spawn(async move {
            quorp_desktop_core::apply_run_diff(
                &run_dir,
                &workspace_root,
                run_id,
                target_workspace_id,
            )
            .await
        })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join: {err}")))?
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, err.to_string()))?;
    Ok(ApplyDiffReceipt::from(receipt))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyAgainReceipt {
    pub run_id: RunIdDto,
    /// Identifier of the verification re-run.
    pub verify_run_id: RunIdDto,
}

/// Re-run verification (L0..L4 ladder) for the given run. The real
/// path will spawn a fresh sandboxed agent invocation that reads the
/// run's checkpoint and replays the proof stages.
#[tauri::command]
pub async fn verify_run_again(
    state: tauri::State<'_, AppHandleState>,
    run_id: RunIdDto,
) -> Result<VerifyAgainReceipt, IpcError> {
    let _ = (state, run_id);
    Err(IpcError::not_implemented(
        "verify_run_again: pending PR8 (proof inspector + verification adapter)",
    ))
}

/// Wire shape returned to the frontend. Mirrors
/// `quorp_desktop_core::RollbackReceipt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReceipt {
    pub run_id: RunIdDto,
    pub workspace_id: WorkspaceId,
    pub request_counter: u64,
    pub restored_files: u32,
    pub backup_filename: String,
    pub message: String,
}

impl From<quorp_desktop_core::RollbackReceipt> for RollbackReceipt {
    fn from(value: quorp_desktop_core::RollbackReceipt) -> Self {
        Self {
            run_id: value.run_id,
            workspace_id: value.workspace_id,
            request_counter: value.request_counter,
            restored_files: value.restored_files,
            backup_filename: value.backup_filename,
            message: value.message,
        }
    }
}

/// Roll a run back to the checkpoint with `request_counter`. Reads
/// `<workspace>/.quorp/runs/<run_id>/events.jsonl`, locates the
/// matching `CheckpointSaved` event, backs up the current
/// `checkpoint.json` non-destructively (with an RFC-3339 stamp), and
/// writes the matched checkpoint as the new `checkpoint.json`.
///
/// A subsequent `resume_headless_agent_with_progress(run_dir)` will
/// pick up from the rolled-back state. PR15 wires that resume into
/// a Tauri command; for now the rollback alone is the recovery
/// surface.
#[tauri::command]
pub async fn rollback_to_checkpoint(
    state: tauri::State<'_, AppHandleState>,
    run_id: RunIdDto,
    request_counter: u64,
) -> Result<RollbackReceipt, IpcError> {
    let status = state.core.runs.status(&run_id).ok_or_else(|| {
        IpcError::new(
            IpcErrorCode::RunNotFound,
            format!("run not found: {run_id}"),
        )
    })?;
    let workspace = state
        .core
        .workspaces
        .get(&status.workspace_id)
        .ok_or_else(|| {
            IpcError::new(
                IpcErrorCode::WorkspaceNotFound,
                format!("workspace not found: {}", status.workspace_id),
            )
        })?;
    let workspace_root = std::path::PathBuf::from(&workspace.canonical_path);
    let run_dir = workspace_root
        .join(".quorp")
        .join("runs")
        .join(run_id.as_str());
    let workspace_id = status.workspace_id.clone();
    let runtime = state.core.runtime.clone();
    let receipt = runtime
        .spawn_blocking(move || {
            quorp_desktop_core::rollback_to_checkpoint(
                &run_dir,
                request_counter,
                run_id,
                workspace_id,
            )
        })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join: {err}")))?
        .map_err(map_rollback_error)?;
    Ok(RollbackReceipt::from(receipt))
}

fn map_rollback_error(err: quorp_desktop_core::RollbackError) -> IpcError {
    use quorp_desktop_core::RollbackError;
    let code = match &err {
        RollbackError::RunDirMissing(_) => IpcErrorCode::RunNotFound,
        RollbackError::EventsMissing(_) => IpcErrorCode::FilesystemError,
        RollbackError::CheckpointNotFound(_) => IpcErrorCode::InvalidInput,
        RollbackError::Malformed(_) => IpcErrorCode::Internal,
        RollbackError::Io(_) => IpcErrorCode::FilesystemError,
    };
    IpcError::new(code, err.to_string())
}
