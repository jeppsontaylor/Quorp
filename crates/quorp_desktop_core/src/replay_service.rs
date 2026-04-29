//! Replay a finished run's `events.jsonl` through the same translation
//! pipeline used for live runs.
//!
//! The Tauri shell exposes `replay_run(events_jsonl_path, on_event,
//! pacing)` which calls into [`ReplayService::replay`]. The desktop
//! UI reuses the same timeline reducer for live and replayed runs;
//! the only thing that changes here is the source of events (a file
//! on disk) and the pacing (instant / realtime / Nx / step).

use std::path::{Path, PathBuf};
use std::time::Duration;

use quorp_agent_core::RuntimeEvent;
use quorp_desktop_ipc::{DesktopEvent, RunIdDto, RuntimeEventDto};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

use crate::event_bridge;

/// How fast to walk through a recorded events.jsonl file when replaying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplayPacing {
    /// Emit every event with no delay.
    #[default]
    Instant,
    /// Wait `period` between batches.
    Fixed(Duration),
}

/// Errors returned by the replay service.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("events.jsonl not found at {0}")]
    Missing(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed event at line {line}: {error}")]
    Parse { line: usize, error: String },
    #[error("downstream channel closed")]
    Disconnected,
}

/// Stateless service object that streams a recorded run back through
/// the desktop event bridge.
#[derive(Debug, Default)]
pub struct ReplayService;

impl ReplayService {
    pub fn new() -> Self {
        Self
    }

    /// Replay the events at `events_path` to `sink` as a sequence of
    /// [`DesktopEvent::Runtime`] batches keyed by `run_id`. Batches
    /// hold up to `batch_size` events; after each batch the call
    /// awaits `pacing` if it is `Fixed`.
    ///
    /// Returns the number of events successfully forwarded. A parse
    /// failure on any line aborts the replay; the partial events that
    /// were already sent stay sent (idempotent in the timeline since
    /// `seq` numbers are dense and unique).
    pub async fn replay(
        &self,
        events_path: &Path,
        run_id: RunIdDto,
        sink: UnboundedSender<DesktopEvent>,
        pacing: ReplayPacing,
        batch_size: usize,
    ) -> Result<usize, ReplayError> {
        if !events_path.exists() {
            return Err(ReplayError::Missing(events_path.to_path_buf()));
        }
        let batch_size = batch_size.max(1);
        let file = tokio::fs::File::open(events_path).await?;
        let mut reader = BufReader::new(file).lines();

        let mut sent = 0usize;
        let mut batch_seq = 0u64;
        let mut buffer: Vec<RuntimeEventDto> = Vec::with_capacity(batch_size);
        let mut line_no = 0usize;

        loop {
            let next = reader.next_line().await?;
            match next {
                Some(line) => {
                    line_no += 1;
                    if line.trim().is_empty() {
                        continue;
                    }
                    let event: RuntimeEvent = match serde_json::from_str(&line) {
                        Ok(event) => event,
                        Err(err) => {
                            return Err(ReplayError::Parse {
                                line: line_no,
                                error: err.to_string(),
                            });
                        }
                    };
                    let dto = event_bridge::translate(sent as u64, event);
                    buffer.push(dto);
                    if buffer.len() >= batch_size {
                        flush_batch(&run_id, &mut buffer, &mut batch_seq, &mut sent, &sink)?;
                        if let ReplayPacing::Fixed(d) = pacing
                            && !d.is_zero()
                        {
                            tokio::time::sleep(d).await;
                        }
                    }
                }
                None => {
                    if !buffer.is_empty() {
                        flush_batch(&run_id, &mut buffer, &mut batch_seq, &mut sent, &sink)?;
                    }
                    break;
                }
            }
        }
        Ok(sent)
    }
}

fn flush_batch(
    run_id: &RunIdDto,
    buffer: &mut Vec<RuntimeEventDto>,
    batch_seq: &mut u64,
    sent: &mut usize,
    sink: &UnboundedSender<DesktopEvent>,
) -> Result<(), ReplayError> {
    let seq = *batch_seq;
    *batch_seq += 1;
    let count = buffer.len();
    let event = DesktopEvent::Runtime {
        run_id: run_id.clone(),
        batch: std::mem::take(buffer),
        batch_seq: seq,
    };
    sink.send(event).map_err(|_| ReplayError::Disconnected)?;
    *sent += count;
    Ok(())
}
