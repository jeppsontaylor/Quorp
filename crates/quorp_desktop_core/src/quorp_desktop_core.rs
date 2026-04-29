//! Service layer that powers the Quorp desktop app.
//!
//! `quorp_desktop_core` sits between the Tauri shell at
//! `apps/desktop/src-tauri` and the runtime crates (`quorp_agent_core`,
//! `quorp_session`, `quorp_sandbox`, `quorp_permissions`, `quorp_config`,
//! `quorp_storage`, `quorp_provider`).
//!
//! This crate intentionally has no Tauri dependency. Every public type
//! is testable from a plain `cargo test` run, and the same logic is
//! reusable from any future shell (web app, IDE plugin, automation).
//!
//! # Wire compatibility
//!
//! The crate consumes [`quorp_desktop_ipc`] for all wire types. The
//! shared [`quorp_desktop_ipc::DESKTOP_WIRE_VERSION`] constant must
//! match between client (frontend) and server (this crate); the Tauri
//! shell rejects mismatches at startup.

#![allow(dead_code)]

pub mod app_state;
pub mod artifact_store;
pub mod diff_applier;
pub mod doctor;
pub mod memory_adapter;
pub mod rollback;
pub mod rules_adapter;
pub mod event_bridge;
pub mod permission_broker;
pub mod provider_registry;
pub mod replay_service;
pub mod run_service;
pub mod secret_keychain;
pub mod trust_store;
pub mod workspace_registry;

pub use app_state::DesktopAppState;
pub use artifact_store::{
    ArtifactError, ArtifactStore, DEFAULT_LRU_CAPACITY, DEFAULT_WINDOW_LIMIT,
};
pub use diff_applier::{ApplyDiffError, ApplyDiffReceipt, apply_run_diff};
pub use doctor::{DoctorCheck, DoctorReport, DoctorStatus, run_doctor};
pub use memory_adapter::{
    MemoryAdapter, MemoryAdapterError, MemoryItemDto, MemoryPruneReceipt, MemoryQueryResult,
};
pub use rollback::{RollbackError, RollbackReceipt, rollback_to_checkpoint};
pub use rules_adapter::{RuleSummaryDto, RulesAdapterError, list_rules, update_lifecycle};
pub use event_bridge::DesktopRuntimeSink;
pub use permission_broker::{
    BrokerOutcome, DEFAULT_PERMISSION_TIMEOUT, PendingAction, PermissionBroker,
    PermissionBrokerError,
};
pub use provider_registry::{NIM_KEYCHAIN_ACCOUNT, ProviderError, ProviderRegistry};
pub use replay_service::{ReplayError, ReplayPacing, ReplayService};
pub use run_service::{RunError, RunService};
pub use secret_keychain::{
    InMemorySecretStore, KEYCHAIN_SERVICE, KeychainError, KeychainSecretStore, SecretStore,
};
pub use trust_store::TrustStore;
pub use workspace_registry::{WorkspaceRecord, WorkspaceRegistry, WorkspaceRegistryError};

pub use quorp_desktop_ipc::DESKTOP_WIRE_VERSION;

/// Crate version, surfaced to the frontend in `app_status` so the UI
/// can display a build-id and detect upgrades. Generated from
/// `CARGO_PKG_VERSION` at compile time.
pub const DESKTOP_CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
#[path = "../../../testing/quorp_desktop_core/quorp_desktop_core/tests.rs"]
mod tests;
