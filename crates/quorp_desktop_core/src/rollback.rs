//! Materialize a saved checkpoint as the run's active
//! `checkpoint.json`, with a non-destructive backup of whatever was
//! there before.
//!
//! The agent runtime persists every `RuntimeEvent::CheckpointSaved`
//! into `events.jsonl`. The run dir also carries a single
//! `checkpoint.json` that the resume path
//! (`resume_headless_agent_with_progress`) reads. To "roll back" to
//! an earlier checkpoint we don't need to mutate the resume API at
//! all — we just rewrite `checkpoint.json` to the snapshot at the
//! requested `request_counter` and let the existing resume code
//! handle the rest.
//!
//! Backups are tagged with an RFC-3339 timestamp so multiple
//! rollbacks against the same run don't overwrite each other. The
//! receipt's `restored_files` count includes the new
//! `checkpoint.json` plus the backup file that was created.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use quorp_desktop_ipc::{RunIdDto, WorkspaceId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReceipt {
    pub run_id: RunIdDto,
    pub workspace_id: WorkspaceId,
    pub request_counter: u64,
    pub restored_files: u32,
    /// Filename of the backup `checkpoint.json` so the user can
    /// undo the rollback by copying it back. Empty when there was
    /// no prior `checkpoint.json` (first rollback).
    pub backup_filename: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    #[error("run dir missing on disk: {0}")]
    RunDirMissing(PathBuf),
    #[error("events.jsonl missing in run dir: {0}")]
    EventsMissing(PathBuf),
    #[error("no checkpoint with request_counter={0} found in events.jsonl")]
    CheckpointNotFound(u64),
    #[error("malformed checkpoint payload: {0}")]
    Malformed(String),
    #[error("io error during rollback: {0}")]
    Io(#[from] std::io::Error),
}

/// Roll back the run at `run_dir` to the checkpoint with
/// `request_counter`. Atomic in the sense that the new
/// `checkpoint.json` is only written after the prior file (if any)
/// was successfully backed up; a half-applied rollback is
/// observable but never silently discards data.
pub fn rollback_to_checkpoint(
    run_dir: &Path,
    request_counter: u64,
    run_id: RunIdDto,
    workspace_id: WorkspaceId,
) -> Result<RollbackReceipt, RollbackError> {
    if !run_dir.is_dir() {
        return Err(RollbackError::RunDirMissing(run_dir.to_path_buf()));
    }
    let events_path = run_dir.join("events.jsonl");
    if !events_path.is_file() {
        return Err(RollbackError::EventsMissing(events_path));
    }
    let checkpoint_payload = find_checkpoint(&events_path, request_counter)?;
    let checkpoint_path = run_dir.join("checkpoint.json");

    // Back up the existing checkpoint (if any) before overwriting.
    let mut restored_files = 0u32;
    let mut backup_filename = String::new();
    if checkpoint_path.exists() {
        let stamp = chrono::Utc::now()
            .format("%Y%m%dT%H%M%S%3fZ")
            .to_string();
        let backup_name = format!("checkpoint-pre-rollback-{stamp}.json");
        let backup_path = run_dir.join(&backup_name);
        std::fs::copy(&checkpoint_path, &backup_path)?;
        backup_filename = backup_name;
        restored_files += 1;
    }

    // Write the new checkpoint.json.
    let pretty = serde_json::to_vec_pretty(&checkpoint_payload)
        .map_err(|err| RollbackError::Malformed(err.to_string()))?;
    std::fs::write(&checkpoint_path, &pretty)?;
    restored_files += 1;

    Ok(RollbackReceipt {
        run_id,
        workspace_id,
        request_counter,
        restored_files,
        backup_filename,
        message: format!(
            "rolled back to request_counter={request_counter}; \
             {restored_files} file(s) restored"
        ),
    })
}

/// Walk `events.jsonl` line by line, find the
/// `CheckpointSaved.checkpoint` payload that matches
/// `request_counter`, and return its JSON value. Multiple matches
/// (rare — checkpoints are issued at distinct counters) return the
/// last seen, which is the most recent re-save of the same
/// counter.
fn find_checkpoint(
    events_path: &Path,
    request_counter: u64,
) -> Result<serde_json::Value, RollbackError> {
    let body = std::fs::read_to_string(events_path)?;
    let mut last_match: Option<serde_json::Value> = None;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let event_kind = value
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if event_kind != "checkpoint_saved" && event_kind != "CheckpointSaved" {
            continue;
        }
        let checkpoint = match value.get("checkpoint") {
            Some(c) => c,
            None => continue,
        };
        let counter = checkpoint
            .get("request_counter")
            .and_then(|v| v.as_u64())
            .unwrap_or_default();
        if counter == request_counter {
            last_match = Some(checkpoint.clone());
        }
    }
    last_match.ok_or(RollbackError::CheckpointNotFound(request_counter))
}
