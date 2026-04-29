//! Wire types shared between `quorp_desktop_core` and the Tauri shell.
//!
//! Pure-Rust DTOs: no async, no Tauri, no business logic. Every type
//! derives `Serialize` and `Deserialize`. Timestamps are RFC 3339 strings
//! so the wire stays language-agnostic. Identifiers are simple newtype
//! wrappers around `String` for domain typing.
//!
//! Wire compatibility is governed by [`DESKTOP_WIRE_VERSION`]. Any
//! breaking change to a payload shape must bump this constant; the
//! desktop core refuses to attach a newer-than-known version.

#![allow(dead_code)]

pub mod artifact_dto;
pub mod desktop_event;
pub mod error_dto;
pub mod permission_dto;
pub mod run_request;
pub mod settings_dto;
pub mod workspace_dto;

pub use artifact_dto::{ArtifactId, ArtifactKind, ArtifactWindow, DiffSummary, ProofReceiptDto};
pub use desktop_event::{
    DesktopEvent, EventBatch, RunFailureStage, RuntimeEventDto, TokenUsageDto, ValidationStatusDto,
};
pub use error_dto::{IpcError, IpcErrorCode};
pub use permission_dto::{
    CapabilityTokenDto, PermissionDecisionDto, PermissionDecisionKind, PermissionModeDto,
    PermissionRequestDto, PermissionRequestId, PermissionScope, RiskLevel,
};
pub use run_request::{
    BenchmarkOptions, RunHandle, RunIdDto, RunPhaseDto, RunStatusDto, SandboxModeDto,
    StartRunRequest, StopReasonDto,
};
pub use settings_dto::{
    BenchmarkFixture, DesktopSettingsDto, GeneralSettingsDto, NetworkAllowanceDto, ProviderHealth,
    ProviderSummary, RunRetentionDto, SandboxSettingsDto, ThemeDto,
};
pub use workspace_dto::{TrustDecision, TrustReceipt, WorkspaceId, WorkspaceSummary};

/// Wire-protocol version. Bump on any breaking shape change.
pub const DESKTOP_WIRE_VERSION: u32 = 1;

/// Single, fixed model identifier the desktop ships with. Quorp uses
/// NVIDIA NIM Qwen3-Coder exclusively.
pub const DEFAULT_MODEL_ID: &str = "qwen/qwen3-coder-480b-a35b-instruct";

/// Single, fixed provider identifier. Quorp ships with one provider.
pub const DEFAULT_PROVIDER_NAME: &str = "nvidia-nim";

#[cfg(test)]
#[path = "../../../testing/quorp_desktop_ipc/quorp_desktop_ipc/tests.rs"]
mod tests;
