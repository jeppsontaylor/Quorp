//! `DesktopAppState`: the top-level container the Tauri shell holds via
//! `tauri::Builder::manage()`.
//!
//! Each field is `Arc`-wrapped so Tauri command handlers can clone the
//! references cheaply without holding read locks across `.await`s.
//! The state owns its own dedicated tokio runtime; Tauri's command
//! dispatch runtime stays separate to avoid `Handle::current()` mix-ups
//! between worker threads.

use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::artifact_store::ArtifactStore;
use crate::memory_adapter::MemoryAdapter;
use crate::permission_broker::PermissionBroker;
use crate::provider_registry::ProviderRegistry;
use crate::replay_service::ReplayService;
use crate::run_service::RunService;
use crate::secret_keychain::{KeychainSecretStore, SecretStore};
use crate::trust_store::TrustStore;
use crate::workspace_registry::WorkspaceRegistry;

/// Top-level state holder.
#[derive(Debug)]
pub struct DesktopAppState {
    pub workspaces: Arc<WorkspaceRegistry>,
    pub trust_log: Arc<TrustStore>,
    pub runs: Arc<RunService>,
    pub permissions: Arc<PermissionBroker>,
    pub replay: Arc<ReplayService>,
    pub providers: Arc<ProviderRegistry>,
    pub secrets: Arc<dyn SecretStore>,
    pub artifacts: Arc<ArtifactStore>,
    pub memory: Arc<MemoryAdapter>,
    /// Dedicated multi-thread tokio runtime owned by the desktop. All
    /// agent runs and broker waits use this; Tauri's own runtime is
    /// reserved for command dispatch.
    pub runtime: Arc<Runtime>,
}

impl DesktopAppState {
    /// Builds a production state with the OS Keychain backend.
    pub fn new() -> std::io::Result<Self> {
        Self::with_secret_store(KeychainSecretStore::arc())
    }

    /// Builds a state with a caller-supplied secret store. Useful for
    /// tests (in-memory store) and for headless setups.
    pub fn with_secret_store(secrets: Arc<dyn SecretStore>) -> std::io::Result<Self> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_name("quorp-desktop")
                .enable_all()
                .build()?,
        );
        let providers = Arc::new(ProviderRegistry::new(secrets.clone()));
        Ok(Self {
            workspaces: Arc::new(WorkspaceRegistry::new()),
            trust_log: Arc::new(TrustStore::default()),
            runs: Arc::new(RunService::new()),
            permissions: Arc::new(PermissionBroker::default()),
            replay: Arc::new(ReplayService::new()),
            providers,
            secrets,
            artifacts: Arc::new(ArtifactStore::default()),
            memory: Arc::new(MemoryAdapter::new()),
            runtime,
        })
    }
}
