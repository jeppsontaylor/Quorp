//! Staged verification ladder L0..L4 with content-addressed result cache.
//!
//! Phase 7 ships the API surface and a minimal blake3-style cache key
//! computation so downstream crates can target the type. The cargo /
//! rustfmt / miri executors land in the runtime wire-up phase.

use quorp_ids::VerifyRunId;
pub use quorp_verify_model::*;

/// Compose the canonical cache-key string for hashing.
pub fn cache_key_canonical_string(key: &CacheKey) -> String {
    format!(
        "{git_sha}\0{changed_files_hash}\0{features}\0{target_triple}\0{rustc_version}\0{stage_id}",
        git_sha = key.git_sha,
        changed_files_hash = key.changed_files_hash,
        features = key.features.join(","),
        target_triple = key.target_triple,
        rustc_version = key.rustc_version,
        stage_id = key.stage_id,
    )
}

#[derive(Debug, Clone, Default)]
pub struct VerifierStats {
    pub completed: u32,
    pub cached: u32,
    pub failed: u32,
}

/// Trait implemented by per-stage executors. The `quorp_session` crate
/// supplies the cargo / rustfmt / miri / fuzz runners during wire-up.
pub trait StageExecutor: Send + Sync {
    fn stage_id(&self) -> &str;
    fn level(&self) -> VerifyLevel;
}

/// Plan a run id for a synthetic verify request. Stable enough for tests.
pub fn fresh_run_id() -> VerifyRunId {
    VerifyRunId::new(format!(
        "verify-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_deterministic() {
        let key = CacheKey {
            git_sha: "abc".into(),
            changed_files_hash: "deadbeef".into(),
            features: vec!["default".into()],
            target_triple: "aarch64-apple-darwin".into(),
            rustc_version: "1.93.0".into(),
            stage_id: "L1Check".into(),
        };
        let s1 = cache_key_canonical_string(&key);
        let s2 = cache_key_canonical_string(&key);
        assert_eq!(s1, s2);
        assert!(s1.contains("abc"));
        assert!(s1.contains("L1Check"));
    }

    #[test]
    fn fresh_run_id_is_non_empty() {
        let id = fresh_run_id();
        assert!(id.as_str().starts_with("verify-"));
    }
}
