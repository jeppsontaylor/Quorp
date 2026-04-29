//! Workspace identifiers and trust state.

use serde::{Deserialize, Serialize};

/// Identifier for a registered desktop workspace. Stable across runs;
/// generated when a workspace is first added.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Summary view of a workspace shown in the sidebar list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    /// Canonicalized absolute path on disk. Always passes through
    /// `dunce::canonicalize` on the Rust side.
    pub canonical_path: String,
    /// Display name (last path component by default; user-editable later).
    pub display_name: String,
    /// Trust state. Untrusted workspaces cannot escalate to FullAuto+Host
    /// nor load `.quorp/settings.json` overrides.
    pub trust: TrustDecision,
    /// RFC 3339 timestamp of last open from the desktop, or `None` if never.
    pub last_opened_at: Option<String>,
    /// Whether the user has pinned this workspace to the top of the list.
    pub pinned: bool,
    /// Number of recorded runs under `<workspace>/.quorp/runs/`. Hint for
    /// retention sweeps; informational only.
    pub run_count: u64,
}

/// Trust decision for a workspace. The desktop core enforces this in
/// Rust; the frontend can only display it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustDecision {
    /// Default for newly added workspaces.
    Untrusted,
    /// Read access is trusted (suitable for "Open in CLI" surfaces).
    TrustedForReadOnly,
    /// Full trust: enables FullAuto+Host and project-scope allowlists.
    Trusted,
}

/// Receipt returned after a trust change. Holds the new state plus an
/// audit-friendly timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustReceipt {
    pub workspace_id: WorkspaceId,
    pub previous: TrustDecision,
    pub current: TrustDecision,
    /// RFC 3339 timestamp.
    pub decided_at: String,
}
