//! In-memory workspace registry with deterministic id generation and
//! macOS path canonicalization.
//!
//! Persistence (`~/Library/Application Support/Quorp/workspaces.json`)
//! lands in PR4. PR2 ships an in-memory store sufficient to wire the
//! Add-Folder flow end-to-end.

use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};

use quorp_desktop_ipc::{TrustDecision, TrustReceipt, WorkspaceId, WorkspaceSummary};

/// Errors returned by the workspace registry.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceRegistryError {
    #[error("workspace path does not exist: {0}")]
    PathMissing(String),
    #[error("workspace path is not a directory: {0}")]
    NotADirectory(String),
    #[error("failed to canonicalize workspace path: {0}")]
    Canonicalize(#[from] std::io::Error),
    #[error("workspace not found: {0}")]
    NotFound(WorkspaceId),
}

/// In-memory representation of a registered workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub canonical_path: String,
    pub display_name: String,
    pub trust: TrustDecision,
    pub last_opened_at: Option<String>,
    pub pinned: bool,
    pub run_count: u64,
}

impl WorkspaceRecord {
    pub fn to_summary(&self) -> WorkspaceSummary {
        WorkspaceSummary {
            id: self.id.clone(),
            canonical_path: self.canonical_path.clone(),
            display_name: self.display_name.clone(),
            trust: self.trust,
            last_opened_at: self.last_opened_at.clone(),
            pinned: self.pinned,
            run_count: self.run_count,
        }
    }
}

/// In-memory workspace registry. Thread-safe via `RwLock`.
#[derive(Debug, Default)]
pub struct WorkspaceRegistry {
    inner: RwLock<HashMap<WorkspaceId, WorkspaceRecord>>,
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a workspace by absolute path. Path is canonicalized
    /// via `dunce` so `/tmp` and `/private/tmp` collapse to one entry.
    /// Re-adding an existing workspace returns the original record.
    pub fn add(&self, path: impl AsRef<Path>) -> Result<WorkspaceRecord, WorkspaceRegistryError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(WorkspaceRegistryError::PathMissing(
                path.display().to_string(),
            ));
        }
        if !path.is_dir() {
            return Err(WorkspaceRegistryError::NotADirectory(
                path.display().to_string(),
            ));
        }
        let canonical = dunce::canonicalize(path).map_err(WorkspaceRegistryError::Canonicalize)?;
        let canonical_str = canonical.to_string_lossy().into_owned();
        let id = derive_workspace_id(&canonical_str);
        let display_name = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| canonical_str.clone());

        let mut guard = self.inner.write();
        if let Some(existing) = guard.get(&id) {
            return Ok(existing.clone());
        }
        let record = WorkspaceRecord {
            id: id.clone(),
            canonical_path: canonical_str,
            display_name,
            trust: TrustDecision::Untrusted,
            last_opened_at: None,
            pinned: false,
            run_count: 0,
        };
        guard.insert(id, record.clone());
        Ok(record)
    }

    pub fn list(&self) -> Vec<WorkspaceRecord> {
        let guard = self.inner.read();
        let mut records: Vec<_> = guard.values().cloned().collect();
        records.sort_by(|a, b| {
            // Pinned first; then by display name.
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        records
    }

    pub fn get(&self, id: &WorkspaceId) -> Option<WorkspaceRecord> {
        self.inner.read().get(id).cloned()
    }

    pub fn find_by_path(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Option<WorkspaceRecord>, WorkspaceRegistryError> {
        let canonical =
            dunce::canonicalize(path.as_ref()).map_err(WorkspaceRegistryError::Canonicalize)?;
        let canonical_str = canonical.to_string_lossy().into_owned();
        let guard = self.inner.read();
        Ok(guard
            .values()
            .find(|r| r.canonical_path == canonical_str)
            .cloned())
    }

    pub fn remove(&self, id: &WorkspaceId) -> Result<(), WorkspaceRegistryError> {
        let mut guard = self.inner.write();
        match guard.remove(id) {
            Some(_) => Ok(()),
            None => Err(WorkspaceRegistryError::NotFound(id.clone())),
        }
    }

    /// Updates trust state for a workspace and returns a [`TrustReceipt`]
    /// summarizing the transition. The desktop core uses this receipt
    /// to seed the audit log.
    pub fn set_trust(
        &self,
        id: &WorkspaceId,
        new_trust: TrustDecision,
    ) -> Result<TrustReceipt, WorkspaceRegistryError> {
        let mut guard = self.inner.write();
        let record = guard
            .get_mut(id)
            .ok_or_else(|| WorkspaceRegistryError::NotFound(id.clone()))?;
        let previous = record.trust;
        record.trust = new_trust;
        Ok(TrustReceipt {
            workspace_id: id.clone(),
            previous,
            current: new_trust,
            decided_at: Utc::now().to_rfc3339(),
        })
    }

    pub fn touch_opened(&self, id: &WorkspaceId) -> Result<(), WorkspaceRegistryError> {
        let mut guard = self.inner.write();
        let record = guard
            .get_mut(id)
            .ok_or_else(|| WorkspaceRegistryError::NotFound(id.clone()))?;
        record.last_opened_at = Some(Utc::now().to_rfc3339());
        Ok(())
    }
}

/// Deterministic workspace id from the canonical path. Stable across
/// runs and machines; the id is `ws-` + the first 16 hex digits of the
/// SHA-256 hash. Stable enough for the lifetime of the workspace
/// registry on a given host.
fn derive_workspace_id(canonical_path: &str) -> WorkspaceId {
    let mut hasher = Sha256::new();
    hasher.update(canonical_path.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    WorkspaceId::new(format!("ws-{hex}"))
}
