//! Artifact identifiers and paged read windows.
//!
//! Run artifacts (events.jsonl, summary.json, transcript.json,
//! final.diff, proof receipts) are read in fixed-size windows from
//! disk. The Rust side caches windows by `(run_id, kind, offset, limit)`
//! so the UI can paginate freely without re-reading.

use serde::{Deserialize, Serialize};

use crate::run_request::RunIdDto;

/// Identifier for an artifact within a run. Used by the UI to lazy-load
/// diffs and proof packets without loading the entire blob.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Kinds of run artifacts the desktop knows how to surface. Each maps
/// to a file under `<workspace>/.quorp/runs/<run_id>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// `events.jsonl`
    EventsJsonl,
    /// `summary.json`
    Summary,
    /// `transcript.json`
    Transcript,
    /// `checkpoint.json`
    Checkpoint,
    /// `final.diff`
    FinalDiff,
    /// `proof-receipt.json` if present.
    ProofReceipt,
    /// `request.json`
    Request,
    /// `metadata.json`
    Metadata,
    /// `routing-summary.json`
    RoutingSummary,
    /// Any other file. The path is carried in the [`ArtifactWindow`].
    Other,
}

/// A window of bytes read from an artifact. Content is UTF-8 if the
/// artifact is textual; binary artifacts are base64-encoded by the
/// reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactWindow {
    pub run_id: RunIdDto,
    pub kind: ArtifactKind,
    /// Offset into the file in bytes.
    pub offset: u64,
    /// Number of bytes the caller requested. May exceed the actual
    /// returned size when the file is shorter than `offset + limit`.
    pub limit: u64,
    /// Total file size in bytes at read time.
    pub total_size: u64,
    /// SHA-256 of the returned bytes. Lets the UI cache aggressively
    /// and detect rotation.
    pub content_hash: String,
    /// `true` when content is base64-encoded (binary or non-UTF-8).
    pub binary_encoded: bool,
    /// The window contents. UTF-8 by default, base64 when
    /// `binary_encoded`.
    pub content: String,
    /// `true` when more bytes follow this window.
    pub is_truncated: bool,
}

/// Summary of a single diff produced during a run. The full diff body
/// is read separately via [`ArtifactKind::FinalDiff`] or per-hunk
/// pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSummary {
    pub diff_id: ArtifactId,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
    /// Top-level paths the diff touches; capped at a small number for
    /// the timeline card. Full list lives in the artifact body.
    pub sample_paths: Vec<String>,
}

/// A verification proof packet surfaced in the Proof inspector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofReceiptDto {
    pub receipt_id: ArtifactId,
    /// L0..L4 ladder stage that produced this packet.
    pub stage: String,
    /// Pass / fail / queued.
    pub status: String,
    /// Optional canonical cache key the receipt was indexed under.
    pub cache_key: Option<String>,
    /// Path on disk to the raw JSON receipt, relative to the run dir.
    pub receipt_path: String,
    /// RFC 3339 timestamp.
    pub recorded_at: String,
}
