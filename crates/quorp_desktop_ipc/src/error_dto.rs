//! Error envelope crossing the IPC boundary.
//!
//! `IpcError` is the only error type that ever reaches the frontend.
//! Internal `anyhow::Error` chains are flattened into a stable error
//! code plus a redacted message; secret-bearing fragments
//! (`Authorization`, `*_KEY`, `*_TOKEN`, etc.) are stripped before the
//! error leaves the desktop core.

use serde::{Deserialize, Serialize};

/// Stable error codes the frontend can branch on. Codes are part of the
/// public IPC contract; once published, they may only be deprecated,
/// never repurposed. New variants append at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    /// Unknown / unstructured failure. Avoid.
    Internal,
    /// The requested feature isn't implemented in this build.
    NotImplemented,
    /// The IPC payload failed validation (missing fields, bad enum, etc.).
    InvalidInput,
    /// The referenced workspace doesn't exist in the registry.
    WorkspaceNotFound,
    /// The action requires a Trusted workspace; the workspace is not
    /// trusted.
    TrustRequired,
    /// The referenced run doesn't exist or has finished.
    RunNotFound,
    /// The referenced permission request has expired or already been
    /// resolved.
    Stale,
    /// The agent runtime returned a fatal error.
    RuntimeError,
    /// A sandbox setup step failed (profile rendering, /tmp creation,
    /// clone failure, sandbox-exec missing).
    SandboxError,
    /// Provider call failed (network, auth, rate limit).
    ProviderError,
    /// The provider API key is missing or invalid.
    ProviderUnauthorized,
    /// macOS Keychain access denied or service missing.
    KeychainError,
    /// Filesystem error on an artifact read or workspace operation.
    FilesystemError,
    /// Operation was cancelled cooperatively.
    Cancelled,
    /// Operation exceeded its wall-clock budget.
    Timeout,
    /// The wire version of the caller and callee don't match.
    WireVersionMismatch,
}

/// Error envelope crossing the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct IpcError {
    pub code: IpcErrorCode,
    /// User-facing message. Already redacted of secrets.
    pub message: String,
    /// Optional context string for diagnostic surfaces (Doctor, log
    /// viewer). Also redacted.
    pub cause: Option<String>,
}

impl IpcError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::InvalidInput, message)
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::NotImplemented, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Internal, message)
    }
}
