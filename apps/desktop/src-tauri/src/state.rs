//! Wrapper around `quorp_desktop_core::DesktopAppState` that Tauri
//! commands take by `tauri::State` reference.
//!
//! Holding the wrapper here (instead of using
//! `quorp_desktop_core::DesktopAppState` directly via `manage()`)
//! gives the shell a single place to layer Tauri-specific state
//! later (per-window settings, menu router channels, etc.) without
//! touching the core crate.

use std::sync::Arc;

use quorp_desktop_core::DesktopAppState;

/// State container `manage()`d by Tauri. Holds an `Arc` to the core
/// state; Tauri commands clone it cheaply for closure capture.
pub struct AppHandleState {
    pub core: Arc<DesktopAppState>,
}

impl AppHandleState {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            core: Arc::new(DesktopAppState::new()?),
        })
    }
}
