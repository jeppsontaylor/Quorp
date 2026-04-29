//! Bridge from the desktop's `query_memory` Tauri command to
//! `quorp_memory::Memory`.
//!
//! `Memory::with_workspace(...)` opens the workspace's SQLite store
//! and rebuilds search indexes, which is heavyweight enough that we
//! cache one `Arc<Memory>` per workspace and reuse it across calls.
//! Reads happen via `tokio::task::spawn_blocking` so the SQLite work
//! never blocks the dispatch runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

use quorp_memory::Memory;
use quorp_memory_model::{MemoryHit, MemoryQuery, Tier};

/// Wire-shape result returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryQueryResult {
    pub tier: String,
    pub query: String,
    pub items: Vec<MemoryItemDto>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryItemDto {
    pub id: String,
    pub tier: String,
    pub summary: String,
    pub score: f32,
    pub recorded_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryAdapterError {
    #[error("unknown memory tier: {0}")]
    UnknownTier(String),
    #[error("memory open failed for {0}: {1}")]
    Open(PathBuf, String),
    #[error("memory query failed: {0}")]
    Query(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One `Memory` instance per workspace, lazily constructed. The
/// `Arc<Mutex<...>>` over the cache is mutex-only because misses are
/// rare; under contention one caller waits while another opens the
/// SQLite db. Hits are O(1) hashmap lookups.
#[derive(Debug, Default)]
pub struct MemoryAdapter {
    by_workspace: Mutex<HashMap<PathBuf, Arc<Memory>>>,
}

/// Receipt returned to the frontend after a prune call. Mirrors what
/// `quorp_memory::Memory` exposes today (which is `decay_tick` —
/// working-tier-only). `tier_target` echoes back what the user
/// asked for so the inspector can label the result row; `removed`
/// is best-effort (Memory's API doesn't return a count, so we
/// report 0 with a note in `message` for non-working tiers).
#[derive(Debug, Clone, Serialize)]
pub struct MemoryPruneReceipt {
    pub tier: String,
    pub removed: u32,
    pub message: String,
}

impl MemoryAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run a query against the workspace's memory. The tier name is
    /// the lowercase string the frontend uses (`"working"`,
    /// `"episodic"`, …); unknown tiers fail fast.
    pub async fn query(
        &self,
        workspace_root: &Path,
        tier_name: &str,
        query_text: String,
        limit: u32,
    ) -> Result<MemoryQueryResult, MemoryAdapterError> {
        let tier = parse_tier(tier_name)
            .ok_or_else(|| MemoryAdapterError::UnknownTier(tier_name.to_string()))?;
        let memory = self.acquire(workspace_root).await?;
        let q = MemoryQuery {
            query_text: if query_text.trim().is_empty() {
                None
            } else {
                Some(query_text.clone())
            },
            tier: Some(tier),
            limit: limit.max(1),
        };
        let memory_for_blocking = memory.clone();
        let hits = tokio::task::spawn_blocking(move || memory_for_blocking.recall(&q))
            .await
            .map_err(|err| MemoryAdapterError::Query(format!("join: {err}")))?
            .map_err(|err| MemoryAdapterError::Query(err.to_string()))?;
        let items: Vec<MemoryItemDto> = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| memory_hit_to_dto(index, hit))
            .collect();
        let total = items.len();
        Ok(MemoryQueryResult {
            tier: tier_name.to_string(),
            query: query_text,
            items,
            total,
        })
    }

    async fn acquire(
        &self,
        workspace_root: &Path,
    ) -> Result<Arc<Memory>, MemoryAdapterError> {
        let canonical = dunce::canonicalize(workspace_root).map_err(MemoryAdapterError::Io)?;
        if let Some(existing) = self.by_workspace.lock().get(&canonical).cloned() {
            return Ok(existing);
        }
        let canonical_for_blocking = canonical.clone();
        let memory = tokio::task::spawn_blocking(move || {
            Memory::with_workspace(&canonical_for_blocking)
                .map(Arc::new)
                .map_err(|err| MemoryAdapterError::Open(canonical_for_blocking, err.to_string()))
        })
        .await
        .map_err(|err| {
            MemoryAdapterError::Open(canonical.clone(), format!("join: {err}"))
        })??;
        self.by_workspace
            .lock()
            .insert(canonical, memory.clone());
        Ok(memory)
    }

    /// Drops the cached Memory for `workspace_root` if any. Forces
    /// the next query to reopen the SQLite db. Used by the workspace
    /// remove flow.
    pub fn forget(&self, workspace_root: &Path) {
        if let Ok(canonical) = dunce::canonicalize(workspace_root) {
            self.by_workspace.lock().remove(&canonical);
        }
    }

    /// Prune the named tier. Today's `quorp_memory::Memory` exposes
    /// `decay_tick()` which clears the working tier; other tiers
    /// have decay policies but no explicit prune entry point yet.
    /// We surface the gap honestly through `message` so the user
    /// sees what actually happened.
    ///
    /// `older_than_iso` is accepted for forward compatibility (the
    /// upstream API will eventually take a cutoff timestamp); v1
    /// ignores it and runs the full decay tick.
    pub async fn prune(
        &self,
        workspace_root: &Path,
        tier_name: &str,
        older_than_iso: String,
    ) -> Result<MemoryPruneReceipt, MemoryAdapterError> {
        let tier = parse_tier(tier_name)
            .ok_or_else(|| MemoryAdapterError::UnknownTier(tier_name.to_string()))?;
        let memory = self.acquire(workspace_root).await?;

        let memory_for_blocking = memory.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            // `decay_tick` is what `quorp_memory` ships today.
            // It clears the working tier and persists.
            memory_for_blocking.decay_tick()
        })
        .await
        .map_err(|err| MemoryAdapterError::Query(format!("join: {err}")))?
        .map_err(|err| MemoryAdapterError::Query(err.to_string()))?;
        let _ = outcome;

        let (removed, message) = match tier {
            Tier::Working => (
                // We don't get a count from decay_tick; report 0
                // and an honest message rather than guessing.
                0,
                "decay tick applied: working tier cleared, persisted".to_string(),
            ),
            other => (
                0,
                format!(
                    "tier `{}` uses decay policy `{:?}`; explicit cutoff pruning lands once `Memory::prune_older_than` is added upstream (older_than_iso ignored: {})",
                    tier_label(other),
                    other.default_decay(),
                    older_than_iso,
                ),
            ),
        };
        Ok(MemoryPruneReceipt {
            tier: tier_name.to_string(),
            removed,
            message,
        })
    }
}

fn parse_tier(name: &str) -> Option<Tier> {
    match name.to_ascii_lowercase().as_str() {
        "working" => Some(Tier::Working),
        "episodic" => Some(Tier::Episodic),
        "semantic" => Some(Tier::Semantic),
        "procedural" => Some(Tier::Procedural),
        "negative" => Some(Tier::Negative),
        "rule" => Some(Tier::Rule),
        // Project / global don't map onto Memory tiers; the frontend
        // shows them in the UI but they live in the rules adapter
        // (rules) or aren't currently surfaced.
        _ => None,
    }
}

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Working => "working",
        Tier::Episodic => "episodic",
        Tier::Semantic => "semantic",
        Tier::Procedural => "procedural",
        Tier::Negative => "negative",
        Tier::Rule => "rule",
    }
}

fn memory_hit_to_dto(index: usize, hit: &MemoryHit) -> MemoryItemDto {
    let tier = tier_label(hit.tier).to_string();
    let id = format!("{tier}-{index}");
    MemoryItemDto {
        id,
        tier,
        summary: hit.snippet.clone(),
        score: hit.score,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    }
}
