//! Memory OS — six tiers (Working, Episodic, Semantic, Procedural,
//! Negative, Rule) backed by `quorp_storage` traits.
//!
//! Phase 6 ships an in-memory implementation hung off the
//! `quorp_storage::inmem` backend so the runtime can exercise the API
//! ahead of the sqlite migration work.

use std::sync::RwLock;

use anyhow::Result;
use quorp_memory_model::{
    EpisodicFact, MemoryHit, MemoryQuery, NegativeSignature, ProceduralSkill, RuleEntry,
    SemanticFact, Tier, WorkingFact,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default)]
struct TierStore {
    working: Vec<WorkingFact>,
    episodic: Vec<EpisodicFact>,
    semantic: Vec<SemanticFact>,
    procedural: Vec<ProceduralSkill>,
    negative: Vec<NegativeSignature>,
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
    UpsertRule(RuleEntry),
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, event: MemoryEvent) -> Result<()> {
        let mut inner = self.inner.write().map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        match event {
            MemoryEvent::RecordWorking(f) => inner.working.push(f),
            MemoryEvent::RecordEpisodic(f) => inner.episodic.push(f),
            MemoryEvent::RecordSemantic(f) => inner.semantic.push(f),
            MemoryEvent::RecordProcedural(f) => inner.procedural.push(f),
            MemoryEvent::RecordNegative(f) => inner.negative.push(f),
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
        let inner = self.inner.read().map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let needle = query.query_text.as_deref().map(str::to_ascii_lowercase);
        let mut hits = Vec::new();

        let mut push_if = |tier: Tier, candidate: String| {
            let allow = match needle.as_deref() {
                Some(n) => candidate.to_ascii_lowercase().contains(n),
                None => true,
            };
            if allow && (query.tier.is_none() || query.tier == Some(tier)) {
                hits.push(MemoryHit { tier, snippet: candidate, score: 1.0 });
            }
        };

        for f in &inner.working {
            push_if(Tier::Working, f.body.clone());
        }
        for f in &inner.episodic {
            push_if(Tier::Episodic, format!("{}: {}", f.session, f.summary));
        }
        for f in &inner.semantic {
            push_if(Tier::Semantic, format!("{} {} {}", f.subject, f.predicate, f.object));
        }
        for f in &inner.procedural {
            push_if(Tier::Procedural, format!("{}: {}", f.name, f.trigger_pattern));
        }
        for f in &inner.negative {
            push_if(Tier::Negative, format!("{} ({}x)", f.signature, f.seen_count));
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
        let mut inner = self.inner.write().map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        // Working tier is task-scoped; prune everything older than the tick.
        inner.working.clear();
        Ok(())
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
            .recall(&MemoryQuery { query_text: None, tier: Some(Tier::Working), limit: 8 })
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
            .recall(&MemoryQuery { query_text: Some("borrow".into()), tier: None, limit: 4 })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }
}
