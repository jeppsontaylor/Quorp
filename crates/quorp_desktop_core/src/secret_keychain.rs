//! macOS Keychain access for the desktop's single API key.
//!
//! The desktop ships with a single provider (NVIDIA NIM Qwen3-Coder).
//! The user supplies their NIM API key once via Settings → Provider;
//! it lives only in the macOS Keychain. The Tauri shell never sees it
//! after the initial `set_nim_api_key` call returns. Health checks and
//! actual model requests run inside `quorp_desktop_core`, where the
//! key is read from Keychain on demand.
//!
//! On non-macOS hosts the `keyring` crate uses the OS-native fallback
//! (Secret Service on Linux, Credential Manager on Windows). We
//! abstract behind a trait so tests can swap in an in-memory store.

use std::sync::Arc;

use keyring::Entry;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Service identifier under which Quorp stores its secrets in the
/// platform credential vault. Matches the bundle identifier of the
/// desktop app so the Keychain UI groups them with the .app.
pub const KEYCHAIN_SERVICE: &str = "ai.veox.quorp.desktop";

/// Errors returned by the keychain layer.
#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("keychain access denied or service missing: {0}")]
    Backend(String),
    #[error("no entry found for `{0}`")]
    NotFound(String),
}

impl From<keyring::Error> for KeychainError {
    fn from(value: keyring::Error) -> Self {
        match value {
            keyring::Error::NoEntry => KeychainError::NotFound(String::from("(unspecified)")),
            other => KeychainError::Backend(format!("{other:?}")),
        }
    }
}

/// Trait for storing and reading per-provider secrets. The production
/// impl is [`KeychainSecretStore`]; tests use [`InMemorySecretStore`].
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    fn set(&self, account: &str, secret: &str) -> Result<(), KeychainError>;
    fn get(&self, account: &str) -> Result<Option<String>, KeychainError>;
    fn clear(&self, account: &str) -> Result<(), KeychainError>;
    fn has(&self, account: &str) -> bool;
}

/// Production secret store backed by the OS keychain.
#[derive(Debug, Default)]
pub struct KeychainSecretStore;

impl KeychainSecretStore {
    pub fn new() -> Self {
        Self
    }

    /// Convenience helper to wrap into an `Arc<dyn SecretStore>` for
    /// injection into [`crate::DesktopAppState`].
    pub fn arc() -> Arc<dyn SecretStore> {
        Arc::new(Self)
    }
}

impl SecretStore for KeychainSecretStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), KeychainError> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)?;
        entry.set_password(secret)?;
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, KeychainError> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(other) => Err(KeychainError::Backend(format!("{other:?}"))),
        }
    }

    fn clear(&self, account: &str) -> Result<(), KeychainError> {
        let entry = Entry::new(KEYCHAIN_SERVICE, account)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(other) => Err(KeychainError::Backend(format!("{other:?}"))),
        }
    }

    fn has(&self, account: &str) -> bool {
        matches!(self.get(account), Ok(Some(_)))
    }
}

/// In-memory secret store for tests and headless setups where the
/// platform keychain is unavailable. Never used in production.
#[derive(Debug, Default)]
pub struct InMemorySecretStore {
    inner: RwLock<HashMap<String, String>>,
}

impl InMemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arc() -> Arc<dyn SecretStore> {
        Arc::new(Self::new())
    }
}

impl SecretStore for InMemorySecretStore {
    fn set(&self, account: &str, secret: &str) -> Result<(), KeychainError> {
        self.inner
            .write()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<String>, KeychainError> {
        Ok(self.inner.read().get(account).cloned())
    }

    fn clear(&self, account: &str) -> Result<(), KeychainError> {
        self.inner.write().remove(account);
        Ok(())
    }

    fn has(&self, account: &str) -> bool {
        self.inner.read().contains_key(account)
    }
}
