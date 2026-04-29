//! Paged reader for run artifacts under `<workspace>/.quorp/runs/<run-id>/`.
//!
//! The Tauri shell exposes `read_diff(...)`, `read_proof(...)`,
//! `read_event_window(...)`, and `list_run_artifacts(...)`, all of
//! which call into [`ArtifactStore`]. The store never loads a whole
//! file into memory; every read takes an `(offset, limit)` window so
//! the UI can paginate freely.
//!
//! A small LRU keeps recent windows hot. Cache keys include the
//! file's `(modified_at, size)` so stale windows after a rotation
//! miss instead of returning stale data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use quorp_desktop_ipc::{ArtifactKind, ArtifactWindow, RunIdDto};

/// Maximum bytes returned in a single window. The frontend can call
/// repeatedly with monotonically increasing `offset` values to read
/// the rest of the file. 1 MiB is enough for one CodeMirror chunk
/// and short enough to keep the IPC payload responsive.
pub const DEFAULT_WINDOW_LIMIT: u64 = 1024 * 1024;

/// Maximum total number of cached windows across all runs.
pub const DEFAULT_LRU_CAPACITY: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("workspace not registered or path resolution failed")]
    UnknownWorkspace,
    #[error("artifact missing on disk: {0}")]
    Missing(PathBuf),
    #[error("requested window is past end of file")]
    OffsetPastEnd,
    #[error("io error reading artifact: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    run_id: String,
    kind: ArtifactKind,
    path_suffix: String,
    offset: u64,
    limit: u64,
    modified_unix_ms: u64,
    size: u64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    window: ArtifactWindow,
    inserted_at: SystemTime,
}

/// Store of recent artifact windows. Thread-safe; one instance per
/// `DesktopAppState`. Operations short-circuit cache hits to avoid
/// re-hashing large windows.
#[derive(Debug)]
pub struct ArtifactStore {
    capacity: usize,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_LRU_CAPACITY)
    }
}

impl ArtifactStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Read a window from a known artifact kind under a workspace's
    /// `.quorp/runs/<run-id>/` directory. The artifact's path on disk
    /// is derived from `kind` (e.g. `events.jsonl`, `summary.json`).
    pub async fn read_kind(
        &self,
        workspace_root: &Path,
        run_id: &RunIdDto,
        kind: ArtifactKind,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactWindow, ArtifactError> {
        let path = artifact_path(workspace_root, run_id, kind);
        self.read_path_typed(run_id, kind, &path, offset, limit)
            .await
    }

    /// Read a window from an arbitrary file inside a run's directory.
    /// Used by the diff inspector to load per-hunk slices.
    pub async fn read_path(
        &self,
        run_id: &RunIdDto,
        path: &Path,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactWindow, ArtifactError> {
        self.read_path_typed(run_id, ArtifactKind::Other, path, offset, limit)
            .await
    }

    /// Read a slice of an `events.jsonl` file, returning the lines that
    /// fall within `[from_seq, to_seq)`. The file is parsed line by
    /// line; each line is expected to be a JSON object with a `seq`
    /// field (RuntimeEvent emit order).
    pub async fn read_event_window(
        &self,
        workspace_root: &Path,
        run_id: &RunIdDto,
        from_seq: u64,
        to_seq: u64,
    ) -> Result<Vec<serde_json::Value>, ArtifactError> {
        let path = artifact_path(workspace_root, run_id, ArtifactKind::EventsJsonl);
        if !path.exists() {
            return Err(ArtifactError::Missing(path));
        }
        let body = tokio::fs::read_to_string(&path).await?;
        let mut out = Vec::new();
        for (idx, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Use `seq` if present; otherwise fall back to the line
            // index so the caller can still page deterministically.
            let seq = value
                .get("seq")
                .and_then(|s| s.as_u64())
                .unwrap_or(idx as u64);
            if seq >= from_seq && seq < to_seq {
                out.push(value);
            }
            if seq >= to_seq {
                break;
            }
        }
        Ok(out)
    }

    /// List which artifact kinds are present on disk for `run_id`.
    /// Useful for the inspector to know whether the Diff or Proof
    /// tabs have anything to show.
    pub async fn list_kinds(
        &self,
        workspace_root: &Path,
        run_id: &RunIdDto,
    ) -> Result<Vec<ArtifactKind>, ArtifactError> {
        let dir = run_dir(workspace_root, run_id);
        if !dir.is_dir() {
            return Err(ArtifactError::Missing(dir));
        }
        let mut found = Vec::new();
        for kind in [
            ArtifactKind::EventsJsonl,
            ArtifactKind::Summary,
            ArtifactKind::Transcript,
            ArtifactKind::Checkpoint,
            ArtifactKind::FinalDiff,
            ArtifactKind::ProofReceipt,
            ArtifactKind::Request,
            ArtifactKind::Metadata,
            ArtifactKind::RoutingSummary,
        ] {
            if artifact_path(workspace_root, run_id, kind).exists() {
                found.push(kind);
            }
        }
        Ok(found)
    }

    async fn read_path_typed(
        &self,
        run_id: &RunIdDto,
        kind: ArtifactKind,
        path: &Path,
        offset: u64,
        requested_limit: u64,
    ) -> Result<ArtifactWindow, ArtifactError> {
        if !path.exists() {
            return Err(ArtifactError::Missing(path.to_path_buf()));
        }
        let metadata = tokio::fs::metadata(path).await?;
        let total_size = metadata.len();
        if offset >= total_size && total_size > 0 {
            return Err(ArtifactError::OffsetPastEnd);
        }
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let limit = requested_limit.clamp(1, DEFAULT_WINDOW_LIMIT);
        let path_suffix = path
            .strip_prefix(run_dir_root(path))
            .ok()
            .map(|rel| rel.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        let key = CacheKey {
            run_id: run_id.as_str().to_string(),
            kind,
            path_suffix: path_suffix.clone(),
            offset,
            limit,
            modified_unix_ms,
            size: total_size,
        };
        if let Some(hit) = self.cache_get(&key) {
            return Ok(hit);
        }

        let to_read = limit.min(total_size.saturating_sub(offset));
        let bytes = read_at(path, offset, to_read).await?;
        let is_binary = is_likely_binary(&bytes);
        let (content, binary_encoded) = if is_binary {
            use base64::Engine as _;
            (
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                true,
            )
        } else {
            match String::from_utf8(bytes.clone()) {
                Ok(text) => (text, false),
                Err(_) => {
                    use base64::Engine as _;
                    (
                        base64::engine::general_purpose::STANDARD.encode(&bytes),
                        true,
                    )
                }
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let content_hash = hex_short(&hasher.finalize());
        let is_truncated = offset + to_read < total_size;

        let window = ArtifactWindow {
            run_id: run_id.clone(),
            kind,
            offset,
            limit: to_read,
            total_size,
            content_hash,
            binary_encoded,
            content,
            is_truncated,
        };
        self.cache_put(key, window.clone());
        Ok(window)
    }

    fn cache_get(&self, key: &CacheKey) -> Option<ArtifactWindow> {
        self.cache.lock().get(key).map(|entry| entry.window.clone())
    }

    fn cache_put(&self, key: CacheKey, window: ArtifactWindow) {
        let mut guard = self.cache.lock();
        if guard.len() >= self.capacity {
            // Cheap LRU approximation: drop the oldest by `inserted_at`.
            // Plenty fast for our N ≤ 64 cap.
            if let Some(oldest_key) = guard
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest_key);
            }
        }
        guard.insert(
            key,
            CacheEntry {
                window,
                inserted_at: SystemTime::now(),
            },
        );
    }
}

async fn read_at(path: &Path, offset: u64, limit: u64) -> Result<Vec<u8>, ArtifactError> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    let mut buffer = vec![0u8; limit as usize];
    let mut total = 0usize;
    while total < buffer.len() {
        let n = file.read(&mut buffer[total..]).await?;
        if n == 0 {
            break;
        }
        total += n;
    }
    buffer.truncate(total);
    Ok(buffer)
}

/// Best-effort heuristic: bytes that contain a NUL or have a high
/// proportion of bytes outside printable ASCII are likely binary.
fn is_likely_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0u8) {
        return true;
    }
    if bytes.is_empty() {
        return false;
    }
    let non_print = bytes
        .iter()
        .filter(|b| {
            !(**b == b'\n' || **b == b'\r' || **b == b'\t' || (b'\x20'..=b'\x7e').contains(*b))
        })
        .count();
    non_print * 100 / bytes.len() > 30
}

fn artifact_path(workspace_root: &Path, run_id: &RunIdDto, kind: ArtifactKind) -> PathBuf {
    let dir = run_dir(workspace_root, run_id);
    let leaf = match kind {
        ArtifactKind::EventsJsonl => "events.jsonl",
        ArtifactKind::Summary => "summary.json",
        ArtifactKind::Transcript => "transcript.json",
        ArtifactKind::Checkpoint => "checkpoint.json",
        ArtifactKind::FinalDiff => "final.diff",
        ArtifactKind::ProofReceipt => "proof-receipt.json",
        ArtifactKind::Request => "request.json",
        ArtifactKind::Metadata => "metadata.json",
        ArtifactKind::RoutingSummary => "routing-summary.json",
        ArtifactKind::Other => return dir,
    };
    dir.join(leaf)
}

fn run_dir(workspace_root: &Path, run_id: &RunIdDto) -> PathBuf {
    workspace_root
        .join(".quorp")
        .join("runs")
        .join(run_id.as_str())
}

fn run_dir_root(file: &Path) -> &Path {
    file.parent().unwrap_or(file)
}

fn hex_short(digest: &impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}
