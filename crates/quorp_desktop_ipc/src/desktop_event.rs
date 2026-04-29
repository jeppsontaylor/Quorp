//! `DesktopEvent`: the streaming wire type the run service emits to
//! the frontend over a Tauri channel.
//!
//! Events are batched on the Rust side (16 ms or 128 events, whichever
//! first) and delivered as `DesktopEvent::Runtime { batch }` to amortize
//! IPC overhead. Per-run lifecycle events (`RunStarted`, `RunFinished`,
//! `RunFailed`) are sent unbatched.
//!
//! The translation from `quorp_agent_core::RuntimeEvent` to
//! [`RuntimeEventDto`] lives in `quorp_desktop_core::event_bridge`.
//! Shapes here are intentionally a 1:1 mirror of the runtime variants
//! at `crates/quorp_agent_core/src/runtime.rs:519-674`, with one
//! exception: `Box<AgentCheckpoint>` is flattened to a small summary
//! since the full snapshot lives on disk and is read on demand.

use serde::{Deserialize, Serialize};

use crate::artifact_dto::{ArtifactId, DiffSummary};
use crate::error_dto::IpcError;
use crate::permission_dto::PermissionRequestDto;
use crate::run_request::{RunIdDto, StopReasonDto};

/// Streaming event sent to the frontend. `kind` tags discriminate the
/// payload; serde uses snake_case so the TS side can branch directly on
/// the wire string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DesktopEvent {
    /// Sent immediately after `start_agent_run` returns. The frontend
    /// uses this to switch the timeline to the new run.
    RunStarted {
        run_id: RunIdDto,
        goal: String,
        model_id: String,
        /// RFC 3339 timestamp.
        started_at: String,
    },
    /// Terminal event for a successful or cooperative-cancel run.
    RunFinished {
        run_id: RunIdDto,
        stop_reason: StopReasonDto,
        total_steps: usize,
        total_billed_tokens: u64,
        duration_ms: u64,
    },
    /// Terminal event for a fatal failure. `RunFinished` is NOT sent
    /// when this fires.
    RunFailed {
        run_id: RunIdDto,
        error: String,
        stage: RunFailureStage,
    },
    /// Batch of translated runtime events. Ordered by `seq` within the
    /// batch; `batch_seq` is monotonically increasing across batches.
    Runtime {
        run_id: RunIdDto,
        batch: Vec<RuntimeEventDto>,
        batch_seq: u64,
    },
    /// A permission decision is required. The frontend opens the
    /// modal and calls `respond_to_permission`. The agent loop blocks
    /// on the broker until a decision is recorded (or 120 s elapses,
    /// after which the broker resolves with `Deny`).
    Permission {
        run_id: RunIdDto,
        request: PermissionRequestDto,
    },
    /// A diff was produced. The summary is small enough to embed; the
    /// full hunks are read via `read_diff(diff_id, offset, limit)`.
    Diff {
        run_id: RunIdDto,
        diff_id: ArtifactId,
        summary: DiffSummary,
    },
    /// A validation stage finished (or failed). `proof_receipt_id` is
    /// `Some` when the stage produced a verifiable proof packet.
    Validation {
        run_id: RunIdDto,
        step: usize,
        status: ValidationStatusDto,
        summary: String,
        proof_receipt_id: Option<ArtifactId>,
    },
    /// The agent persisted a checkpoint. The full snapshot stays on
    /// disk; only the addressing metadata travels on the wire.
    Checkpoint {
        run_id: RunIdDto,
        step: usize,
        request_counter: u64,
        /// RFC 3339 timestamp.
        written_at: String,
    },
    /// The runtime compacted its prompt context. Useful for the
    /// Context inspector tab.
    ContextCompaction {
        run_id: RunIdDto,
        before_tokens: u64,
        after_tokens: u64,
        kept_messages: usize,
    },
    /// An error not associated with the runtime per se (e.g. a bridge
    /// failure, a sink disconnect). Run lifecycle continues if possible.
    Error {
        run_id: Option<RunIdDto>,
        error: IpcError,
    },
}

/// Stage at which a run aborted. Used by the UI to color the failure
/// card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureStage {
    SandboxSetup,
    ProviderConnect,
    ToolExecution,
    AgentLoop,
    PostRun,
    Unknown,
}

/// Validation outcome. Mirrors the runtime's free-form `status` string
/// but constrained to known values for the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatusDto {
    Queued,
    Running,
    Passed,
    Failed,
    Skipped,
}

/// Internal event sequence; mirrors the variants of
/// `quorp_agent_core::RuntimeEvent` 1:1 but stays a separate type so
/// the wire is independent of internal runtime evolution.
///
/// The `seq` field is filled in by the desktop bridge as it batches.
/// The variant payloads carry only the fields the desktop UI surfaces;
/// fields used purely for internal control flow are intentionally
/// dropped here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RuntimeEventDto {
    StatusUpdate {
        seq: u64,
        status: String,
    },
    PhaseChanged {
        seq: u64,
        phase: String,
        detail: Option<String>,
    },
    AssistantTurnSummary {
        seq: u64,
        step: usize,
        assistant_message: String,
        actions: Vec<String>,
        wrote_files: bool,
        validation_queued: bool,
        parse_warning_count: usize,
    },
    FatalError {
        seq: u64,
        error: String,
    },
    RunStarted {
        seq: u64,
        goal: String,
        model_id: String,
    },
    ModelRequestStarted {
        seq: u64,
        step: usize,
        request_id: u64,
        message_count: usize,
        prompt_token_estimate: u64,
        completion_token_cap: Option<u32>,
        safety_mode: Option<String>,
    },
    ModelRequestFinished {
        seq: u64,
        step: usize,
        request_id: u64,
        /// Tokens consumed by the model. `None` if the provider didn't
        /// report usage.
        usage: Option<TokenUsageDto>,
        /// Watchdog state at completion: `ok`, `warned`, `killed`, ...
        watchdog: Option<String>,
    },
    ToolCallStarted {
        seq: u64,
        step: usize,
        action: String,
    },
    ToolCallFinished {
        seq: u64,
        step: usize,
        action: String,
        status: String,
        action_kind: String,
        target_path: Option<String>,
        edit_summary: Option<String>,
    },
    ValidationStarted {
        seq: u64,
        step: usize,
        summary: String,
    },
    ValidationFinished {
        seq: u64,
        step: usize,
        summary: String,
        status: String,
    },
    PathResolutionFailed {
        seq: u64,
        step: usize,
        action: String,
        request_path: String,
        suggested_path: Option<String>,
        reason: Option<String>,
        error: String,
    },
    RecoveryTurnQueued {
        seq: u64,
        step: usize,
        action: String,
        suggested_path: Option<String>,
        message: String,
    },
    PolicyDenied {
        seq: u64,
        step: usize,
        action: String,
        reason: String,
    },
    SubscriberBackpressure {
        seq: u64,
        subscriber: String,
        dropped_events: usize,
        capacity: usize,
    },
    CheckpointSaved {
        seq: u64,
        step: usize,
        request_counter: u64,
    },
    RunFinished {
        seq: u64,
        reason: String,
        total_steps: usize,
        total_billed_tokens: u64,
        duration_ms: u64,
    },
    /// Catch-all for runtime variants the wire doesn't yet model. Lets
    /// the bridge keep streaming when the runtime grows new variants
    /// before the wire catches up. The frontend can render a generic
    /// info card.
    Other {
        seq: u64,
        kind: String,
        payload: serde_json::Value,
    },
}

/// Token usage shape mirroring `quorp_agent_core::TokenUsage`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageDto {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Helper container for replays and persisted streams. Not used in the
/// live IPC path (which sends `DesktopEvent::Runtime` directly), but
/// useful for tests and on-disk fixture files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBatch {
    pub run_id: RunIdDto,
    pub batch_seq: u64,
    pub events: Vec<RuntimeEventDto>,
}
