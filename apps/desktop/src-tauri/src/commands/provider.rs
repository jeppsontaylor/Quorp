//! Provider commands. Single provider — NVIDIA NIM Qwen3-Coder.
//!
//! The user pastes their NIM API key in Settings → Provider; the key
//! flows in via `set_nim_api_key`, lives in the macOS Keychain, and
//! never round-trips back to the frontend. Health checks send a
//! 1-token completion to the configured endpoint.

use quorp_desktop_ipc::{IpcError, IpcErrorCode, ProviderHealth, ProviderSummary};

use crate::state::AppHandleState;

#[tauri::command]
pub fn provider_info(
    state: tauri::State<'_, AppHandleState>,
) -> Result<ProviderSummary, IpcError> {
    Ok(state.core.providers.summary())
}

#[tauri::command]
pub fn set_nim_api_key(
    state: tauri::State<'_, AppHandleState>,
    secret: String,
) -> Result<(), IpcError> {
    let result = state
        .core
        .providers
        .set_api_key(&secret)
        .map_err(map_provider_error);
    // Best-effort scrub of the input string. Rust's `String::clear`
    // doesn't zero the backing buffer, but the value drops here and
    // the previous heap region is no longer reachable from JS.
    drop(secret);
    result
}

#[tauri::command]
pub fn clear_nim_api_key(
    state: tauri::State<'_, AppHandleState>,
) -> Result<(), IpcError> {
    state
        .core
        .providers
        .clear_api_key()
        .map_err(map_provider_error)
}

#[tauri::command]
pub async fn validate_nim_provider(
    state: tauri::State<'_, AppHandleState>,
) -> Result<ProviderHealth, IpcError> {
    let providers = state.core.providers.clone();
    // Provider validation does network I/O; run on the desktop core's
    // dedicated tokio runtime so we don't tie up Tauri's dispatch.
    let runtime = state.core.runtime.clone();
    runtime
        .spawn(async move { providers.validate().await })
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::Internal, format!("join error: {err}")))?
        .map_err(map_provider_error)
}

fn map_provider_error(err: quorp_desktop_core::ProviderError) -> IpcError {
    use quorp_desktop_core::ProviderError;
    let (code, message) = match &err {
        ProviderError::MissingKey(_) => (IpcErrorCode::ProviderUnauthorized, err.to_string()),
        ProviderError::Keychain(_) => (IpcErrorCode::KeychainError, err.to_string()),
        ProviderError::Invalid(_) => (IpcErrorCode::InvalidInput, err.to_string()),
        ProviderError::Http(_) | ProviderError::Endpoint { .. } => {
            (IpcErrorCode::ProviderError, err.to_string())
        }
    };
    IpcError::new(code, message)
}
