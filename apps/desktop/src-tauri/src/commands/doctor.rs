//! Doctor surface: app/runtime status, wire-version handshake, and
//! a richer multi-probe report.

use serde::Serialize;

use quorp_desktop_core::DoctorReport;
use quorp_desktop_ipc::{DESKTOP_WIRE_VERSION, IpcError};

use crate::state::AppHandleState;

#[derive(Debug, Clone, Serialize)]
pub struct AppStatusDto {
    pub desktop_core_version: &'static str,
    pub desktop_wire_version: u32,
    pub sandbox_exec_present: bool,
    pub has_active_runs: bool,
    pub workspace_count: usize,
    pub pending_permission_count: usize,
    pub provider_has_key: bool,
}

#[tauri::command]
pub fn app_status(state: tauri::State<'_, AppHandleState>) -> Result<AppStatusDto, IpcError> {
    Ok(AppStatusDto {
        desktop_core_version: quorp_desktop_core::DESKTOP_CORE_VERSION,
        desktop_wire_version: DESKTOP_WIRE_VERSION,
        sandbox_exec_present: quorp_sandbox_exec_available(),
        has_active_runs: !state.core.runs.active_handles().is_empty(),
        workspace_count: state.core.workspaces.list().len(),
        pending_permission_count: state.core.permissions.pending_count(),
        provider_has_key: state.core.providers.has_api_key(),
    })
}

#[tauri::command]
pub fn wire_version() -> Result<u32, IpcError> {
    Ok(DESKTOP_WIRE_VERSION)
}

/// Full Doctor report. Runs every probe in
/// [`quorp_desktop_core::doctor`]; cheap probes inline, heavier ones
/// (sandbox-exec smoke, login-shell PATH query) on the desktop core's
/// dedicated tokio runtime.
#[tauri::command]
pub async fn doctor_report(
    state: tauri::State<'_, AppHandleState>,
) -> Result<DoctorReport, IpcError> {
    let core = state.core.clone();
    let runtime = state.core.runtime.clone();
    runtime
        .spawn(async move { quorp_desktop_core::run_doctor(&core) })
        .await
        .map_err(|err| {
            IpcError::new(
                quorp_desktop_ipc::IpcErrorCode::Internal,
                format!("doctor join error: {err}"),
            )
        })
}

fn quorp_sandbox_exec_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::path::Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
