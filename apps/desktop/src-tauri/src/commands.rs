//! Typed Tauri commands. Each submodule maps a domain (workspace,
//! run, permission, etc.) to a small set of `#[tauri::command]`
//! wrappers that delegate to `quorp_desktop_core`.
//!
//! Errors leaving the wrapper are always [`quorp_desktop_ipc::IpcError`]
//! — the frontend never sees a raw `anyhow::Error` chain.

pub mod artifact;
pub mod benchmark;
pub mod checkpoint;
pub mod doctor;
pub mod expansive;
pub mod permission;
pub mod provider;
pub mod replay;
pub mod run;
pub mod workspace;
