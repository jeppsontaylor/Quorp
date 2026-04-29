//! Per-run request, handle, and status DTOs.

use serde::{Deserialize, Serialize};

use crate::permission_dto::PermissionModeDto;
use crate::workspace_dto::WorkspaceId;

/// Identifier for a desktop-driven run. Distinct from any internal run
/// id used by the agent runtime; the desktop core maps the two when
/// it spawns a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunIdDto(pub String);

impl RunIdDto {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunIdDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Sandbox surface chosen for a run. Mirrors `quorp_sandbox::SandboxBackend`
/// (plus the new `MacAppleSandbox` variant introduced in PR3) on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxModeDto {
    /// Run on the host with no confinement. Trust + explicit confirm
    /// required; the run service downgrades this on untrusted workspaces.
    Host,
    /// Detached git worktree of the source workspace.
    GitWorktree,
    /// Plain `cp -c -R` clone of the workspace into `/tmp/quorp/<run-id>/work/`.
    /// Available without macOS-specific tooling.
    TmpCopy,
    /// macOS sandbox-exec profile + clonefile lifecycle. Default for
    /// benchmark runs and recommended for any user run on macOS.
    MacAppleSandbox,
}

/// A request to start a run. The desktop core validates the request
/// against the workspace's trust state before spawning anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunRequest {
    pub workspace_id: WorkspaceId,
    /// User-supplied prompt or task description.
    pub goal: String,
    pub permission_mode: PermissionModeDto,
    pub sandbox_mode: SandboxModeDto,
    /// Optional model id override; defaults to Quorp's single supported
    /// model when `None`.
    pub model_id: Option<String>,
    /// Wall-clock budget in seconds. `None` falls back to the configured
    /// default in `DesktopSettingsDto`.
    pub wall_clock_budget_seconds: Option<u64>,
}

/// Options specific to a benchmark run. Composes a [`StartRunRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkOptions {
    pub fixture_id: String,
    pub permission_mode: PermissionModeDto,
    pub sandbox_mode: SandboxModeDto,
    pub model_id: Option<String>,
    pub wall_clock_budget_seconds: Option<u64>,
}

/// Lightweight handle returned immediately when a run starts. Events
/// stream on the caller's `Channel<DesktopEvent>`; status is queried
/// via [`RunStatusDto`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHandle {
    pub run_id: RunIdDto,
    /// RFC 3339 timestamp.
    pub started_at: String,
}

/// Snapshot of a run's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStatusDto {
    pub run_id: RunIdDto,
    pub workspace_id: WorkspaceId,
    /// Phase of the run: starting / running / cancelling / finished / failed.
    pub phase: RunPhaseDto,
    pub permission_mode: PermissionModeDto,
    pub sandbox_mode: SandboxModeDto,
    pub model_id: String,
    pub current_step: usize,
    pub total_billed_tokens: u64,
    /// Context utilization as a fraction in `[0.0, 1.0]`. `None` if not
    /// yet measured.
    pub context_pressure: Option<f64>,
    /// RFC 3339 timestamp.
    pub started_at: String,
    /// RFC 3339 timestamp set when the run terminates.
    pub finished_at: Option<String>,
    pub stop_reason: Option<StopReasonDto>,
}

/// Coarse-grained phase of a run for the mission bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhaseDto {
    Starting,
    Running,
    Cancelling,
    Finished,
    Failed,
}

/// Why a run stopped. Values mirror `quorp_agent_core::StopReason` plus
/// the desktop-only `Cancelled` and `Timeout` cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReasonDto {
    Completed,
    Cancelled,
    Timeout,
    BudgetExhausted,
    ToolFailure,
    PolicyDenied,
    FatalError,
    UnknownError,
}
