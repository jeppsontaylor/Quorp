//! Memory OS — six tiers (Working, Episodic, Semantic, Procedural,
//! Negative, Rule) backed by `quorp_storage` traits.
//!
//! Phase 6 ships an in-memory implementation hung off the
//! `quorp_storage::inmem` backend so the runtime can exercise the API
//! ahead of the sqlite migration work.

use std::sync::RwLock;

use anyhow::Result;
use quorp_memory_model::{
    EpisodicFact, FailedAttemptRecord, FailureFingerprint, MemoryHit, MemoryQuery,
    NegativeSignature, ProceduralSkill, RetryDecision, RuleEntry, SemanticFact, Tier, WorkingFact,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default)]
struct TierStore {
    working: Vec<WorkingFact>,
    episodic: Vec<EpisodicFact>,
    semantic: Vec<SemanticFact>,
    procedural: Vec<ProceduralSkill>,
    negative: Vec<NegativeSignature>,
    failed_attempts: Vec<FailedAttemptRecord>,
    rule: Vec<RuleEntry>,
}

#[derive(Debug, Default)]
pub struct Memory {
    inner: RwLock<TierStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryEvent {
    RecordWorking(WorkingFact),
    RecordEpisodic(EpisodicFact),
    RecordSemantic(SemanticFact),
    RecordProcedural(ProceduralSkill),
    RecordNegative(NegativeSignature),
    RecordFailedAttempt(FailedAttemptRecord),
    UpsertRule(RuleEntry),
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, event: MemoryEvent) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        match event {
            MemoryEvent::RecordWorking(f) => inner.working.push(f),
            MemoryEvent::RecordEpisodic(f) => inner.episodic.push(f),
            MemoryEvent::RecordSemantic(f) => inner.semantic.push(f),
            MemoryEvent::RecordProcedural(f) => inner.procedural.push(f),
            MemoryEvent::RecordNegative(f) => inner.negative.push(f),
            MemoryEvent::RecordFailedAttempt(record) => {
                if let Some(existing) = inner
                    .failed_attempts
                    .iter_mut()
                    .find(|existing| existing.fingerprint == record.fingerprint)
                {
                    existing.seen_count = existing.seen_count.saturating_add(record.seen_count);
                    existing.last_seen_unix = existing.last_seen_unix.max(record.last_seen_unix);
                    existing.run_id = record.run_id.or_else(|| existing.run_id.clone());
                } else {
                    inner.failed_attempts.push(record);
                }
            }
            MemoryEvent::UpsertRule(rule) => {
                if let Some(existing) = inner.rule.iter_mut().find(|r| r.id == rule.id) {
                    *existing = rule;
                } else {
                    inner.rule.push(rule);
                }
            }
        }
        Ok(())
    }

    pub fn recall(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let needle = query.query_text.as_deref().map(str::to_ascii_lowercase);
        let mut hits = Vec::new();

        let mut push_if = |tier: Tier, candidate: String| {
            let allow = match needle.as_deref() {
                Some(n) => candidate.to_ascii_lowercase().contains(n),
                None => true,
            };
            if allow && (query.tier.is_none() || query.tier == Some(tier)) {
                hits.push(MemoryHit {
                    tier,
                    snippet: candidate,
                    score: 1.0,
                });
            }
        };

        for f in &inner.working {
            push_if(Tier::Working, f.body.clone());
        }
        for f in &inner.episodic {
            push_if(Tier::Episodic, format!("{}: {}", f.session, f.summary));
        }
        for f in &inner.semantic {
            push_if(
                Tier::Semantic,
                format!("{} {} {}", f.subject, f.predicate, f.object),
            );
        }
        for f in &inner.procedural {
            push_if(
                Tier::Procedural,
                format!("{}: {}", f.name, f.trigger_pattern),
            );
        }
        for f in &inner.negative {
            push_if(
                Tier::Negative,
                format!("{} ({}x)", f.signature, f.seen_count),
            );
        }
        for f in &inner.failed_attempts {
            push_if(
                Tier::Negative,
                format!(
                    "{} fix={} evidence={} ({}x)",
                    f.fingerprint.signature,
                    f.fingerprint.attempted_fix_hash,
                    f.fingerprint.evidence_hash,
                    f.seen_count
                ),
            );
        }
        for f in &inner.rule {
            push_if(Tier::Rule, f.statement.clone());
        }

        hits.truncate(query.limit as usize);
        Ok(hits)
    }

    /// Decay tick: shrinks counters and prunes the working tier. Real
    /// implementation will compute exponential decay and persist.
    pub fn decay_tick(&self) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        // Working tier is task-scoped; prune everything older than the tick.
        inner.working.clear();
        Ok(())
    }

    pub fn record_failed_attempt(
        &self,
        fingerprint: FailureFingerprint,
        run_id: Option<quorp_ids::VerifyRunId>,
        now_unix: i64,
    ) -> Result<FailedAttemptRecord> {
        let record = FailedAttemptRecord {
            fingerprint,
            run_id,
            seen_count: 1,
            last_seen_unix: now_unix,
        };
        self.record(MemoryEvent::RecordFailedAttempt(record.clone()))?;
        Ok(record)
    }

    pub fn retry_decision(&self, fingerprint: &FailureFingerprint) -> Result<RetryDecision> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let same_fix_seen = inner
            .failed_attempts
            .iter()
            .find(|record| record.fingerprint == *fingerprint);
        if let Some(record) = same_fix_seen {
            return Ok(RetryDecision::Block {
                reason: format!(
                    "same failed fix already observed for {} ({}x); gather new evidence or change the patch",
                    record.fingerprint.signature, record.seen_count
                ),
            });
        }
        Ok(RetryDecision::Allow)
    }

    pub fn failed_attempt_count(&self) -> Result<usize> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        Ok(inner.failed_attempts.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quorp_ids::SessionId;

    #[test]
    fn record_and_recall_semantic_fact() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordSemantic(SemanticFact {
            subject: "crate:quorp_agent_core".into(),
            predicate: "forbids".into(),
            object: "let _ = on fallible".into(),
            confidence: 0.95,
        }))
        .unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: Some("forbids".into()),
                tier: Some(Tier::Semantic),
                limit: 8,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("forbids"));
    }

    #[test]
    fn decay_tick_clears_working_tier() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordWorking(WorkingFact {
            task: quorp_ids::TurnId::new("t-1"),
            kind: "scratch".into(),
            body: "thinking".into(),
            tokens: 10,
        }))
        .unwrap();
        mem.decay_tick().unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: None,
                tier: Some(Tier::Working),
                limit: 8,
            })
            .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn record_episodic_uses_session_id() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordEpisodic(EpisodicFact {
            session: SessionId::new("s-001"),
            summary: "fix borrow checker error in widget".into(),
            outcome: "merged".into(),
        }))
        .unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: Some("borrow".into()),
                tier: None,
                limit: 4,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn failed_attempt_blocks_same_fix_without_new_evidence() {
        let mem = Memory::new();
        let fingerprint = FailureFingerprint {
            signature: "E0308:mismatched types".to_string(),
            failure_kind: "E0308".to_string(),
            owner: Some("domain".to_string()),
            attempted_fix_hash: "patch-a".to_string(),
            evidence_hash: "log-a".to_string(),
        };

        assert!(matches!(
            mem.retry_decision(&fingerprint).unwrap(),
            RetryDecision::Allow
        ));
        mem.record_failed_attempt(fingerprint.clone(), None, 10)
            .unwrap();
        assert_eq!(mem.failed_attempt_count().unwrap(), 1);
        assert!(matches!(
            mem.retry_decision(&fingerprint).unwrap(),
            RetryDecision::Block { .. }
        ));
    }

    #[test]
    fn failed_attempt_allows_changed_patch_or_evidence() {
        let mem = Memory::new();
        let original = FailureFingerprint {
            signature: "E0308:mismatched types".to_string(),
            failure_kind: "E0308".to_string(),
            owner: None,
            attempted_fix_hash: "patch-a".to_string(),
            evidence_hash: "log-a".to_string(),
        };
        let changed_evidence = FailureFingerprint {
            evidence_hash: "log-b".to_string(),
            ..original.clone()
        };
        mem.record_failed_attempt(original, None, 10).unwrap();

        assert!(matches!(
            mem.retry_decision(&changed_evidence).unwrap(),
            RetryDecision::Allow
        ));
    }
}
