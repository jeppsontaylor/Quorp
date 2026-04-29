//! Permission prompts, decisions, and the capability-token taxonomy.
//!
//! `PermissionRequestDto` mirrors the data the agent's permission
//! classifier already produces in `quorp_permissions`. The risk tokens
//! flow through unchanged so the UI can color and label them
//! consistently with the CLI.

use serde::{Deserialize, Serialize};

use crate::run_request::RunIdDto;

/// Identifier for a single permission request. Stable for the lifetime
/// of the request; the broker uses it to resolve the matching oneshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionRequestId(pub String);

impl PermissionRequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PermissionRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Mirrors `quorp_permissions::Mode` on the wire. The desktop UI shows
/// these via the composer's permission chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModeDto {
    ReadOnly,
    Ask,
    AcceptEdits,
    AutoSafe,
    YoloSandbox,
}

/// Capability classifications surfaced to the user. One DTO variant per
/// `quorp_permissions::CapabilityToken`; serde uses `snake_case` so the
/// UI can branch on the wire string directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapabilityTokenDto {
    /// Command runs through a shell with metacharacters that change
    /// argument parsing (e.g. `&&`, `|`, `>`).
    ShellMetacharacters,
    /// Compound shell command — multiple semicolon- or pipe-separated
    /// statements in one call.
    CompoundCommand,
    /// Writes to the filesystem.
    FilesystemWrite,
    /// Deletes a file.
    FilesystemDelete,
    /// Outbound network access.
    Network,
    /// Reads a known-secret-like environment variable or path.
    SecretsRead,
    /// Invokes Docker / Podman / OCI tooling.
    Container,
    /// Runs a binary that was generated during this run.
    GeneratedExecutable,
    /// Mutates a git remote (push, force-push, branch deletion).
    GitRemoteMutation,
    /// Installs a dependency (cargo add, npm install, pip install).
    DependencyInstall,
    /// Calls into the Model Context Protocol (MCP) tool surface.
    Mcp,
    /// Browser automation.
    Browser,
    /// Anything that doesn't match a more specific token. The `label`
    /// here is a free-form string (e.g. `"sysctl"`, `"keychain"`).
    Other { label: String },
}

/// Risk level computed by the classifier; the UI uses this to choose
/// the color of the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// A pending permission request shown in the UI. The agent loop is
/// blocked on the broker's oneshot until the user responds (or 120 s
/// elapses, after which the broker resolves with `Deny`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequestDto {
    pub request_id: PermissionRequestId,
    pub run_id: RunIdDto,
    /// One-line description of the action (e.g. `cargo build --release`).
    pub action_summary: String,
    /// Tool name as reported by the agent (e.g. `shell`, `patch`).
    pub tool: String,
    /// Working directory for the action.
    pub cwd: Option<String>,
    /// Capability tokens passed through verbatim from
    /// `quorp_permissions::CapabilityToken`.
    pub tokens: Vec<CapabilityTokenDto>,
    pub risk: RiskLevel,
    /// Optional human-readable explanation of why the action is risky.
    pub reason: Option<String>,
    /// RFC 3339 timestamp of when the request was generated.
    pub requested_at: String,
}

/// User decision on a permission request. The scope determines whether
/// the broker remembers the answer for future similar actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecisionDto {
    pub decision: PermissionDecisionKind,
    pub scope: PermissionScope,
}

/// Allow vs. Deny. Distinct from scope so the UI can offer "Deny for
/// session" without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionKind {
    Allow,
    Deny,
}

/// How long a decision applies. Project-scope decisions are persisted
/// only when the workspace is `Trusted`; otherwise the broker
/// downgrades to `Session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    /// Just this single action.
    Once,
    /// Until the desktop quits.
    Session,
    /// Until the project's allowlist is cleared (requires Trusted).
    Project,
}
