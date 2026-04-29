//! Replay a recorded `events.jsonl` file through the same channel
//! pipeline as live runs.

use std::path::PathBuf;
use std::time::Duration;

use quorp_desktop_core::ReplayPacing;
use quorp_desktop_ipc::{DesktopEvent, IpcError, IpcErrorCode, RunIdDto};
use tokio::sync::mpsc;

use crate::state::AppHandleState;

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacingDto {
    Instant,
    Realtime,
    /// Multiplier (1.0 = realtime, 2.0 = 2× faster, …). Falls back to
    /// `Instant` when ≤ 0.
    Multiplied(f32),
    /// Wait `ms` between batches.
    Fixed { ms: u64 },
}

impl Default for PacingDto {
    fn default() -> Self {
        Self::Instant
    }
}

impl PacingDto {
    fn into_pacing(self) -> ReplayPacing {
        match self {
            PacingDto::Instant | PacingDto::Realtime => ReplayPacing::Instant,
            PacingDto::Fixed { ms } => ReplayPacing::Fixed(Duration::from_millis(ms)),
            PacingDto::Multiplied(factor) if factor > 0.0 => {
                let scaled = (50.0 / factor.max(0.01)).round() as u64;
                ReplayPacing::Fixed(Duration::from_millis(scaled))
            }
            PacingDto::Multiplied(_) => ReplayPacing::Instant,
        }
    }
}

#[tauri::command]
pub async fn replay_run(
    state: tauri::State<'_, AppHandleState>,
    events_path: String,
    run_id: RunIdDto,
    pacing: Option<PacingDto>,
    on_event: tauri::ipc::Channel<DesktopEvent>,
) -> Result<usize, IpcError> {
    let path = PathBuf::from(events_path);
    let pacing = pacing.unwrap_or_default().into_pacing();

    let (sink, mut rx) = mpsc::unbounded_channel::<DesktopEvent>();
    let runtime = state.core.runtime.clone();
    runtime.spawn(async move {
        while let Some(event) = rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    let replay = state.core.replay.clone();
    replay
        .replay(&path, run_id, sink, pacing, 64)
        .await
        .map_err(|err| IpcError::new(IpcErrorCode::FilesystemError, err.to_string()))
}
