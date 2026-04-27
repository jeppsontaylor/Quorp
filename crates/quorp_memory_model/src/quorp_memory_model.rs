//! Memory-OS domain types — tier shapes and decay-policy enums.

#![allow(dead_code)]

use quorp_ids::{RuleId, SessionId, TurnId, VerifyRunId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    Working,
    Episodic,
    Semantic,
    Procedural,
    Negative,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecayPolicy {
    Fast,
    Medium,
    Slow,
    Never,
}

impl Tier {
    pub fn default_decay(self) -> DecayPolicy {
        match self {
            Tier::Working => DecayPolicy::Fast,
            Tier::Episodic => DecayPolicy::Medium,
            Tier::Semantic => DecayPolicy::Slow,
            Tier::Procedural => DecayPolicy::Never,
            Tier::Negative => DecayPolicy::Medium,
            Tier::Rule => DecayPolicy::Never,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingFact {
    pub task: TurnId,
    pub kind: String,
    pub body: String,
    pub tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicFact {
    pub session: SessionId,
    pub summary: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProceduralSkill {
    pub name: String,
    pub trigger_pattern: String,
    pub steps_yaml: String,
    pub success_count: u32,
    pub failure_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegativeSignature {
    pub signature: String,
    pub failure_kind: String,
    pub seen_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureFingerprint {
    pub signature: String,
    pub failure_kind: String,
    pub owner: Option<String>,
    pub attempted_fix_hash: String,
    pub evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedAttemptRecord {
    pub fingerprint: FailureFingerprint,
    pub run_id: Option<VerifyRunId>,
    pub seen_count: u32,
    pub last_seen_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryDecision {
    Allow,
    Block { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEntry {
    pub id: RuleId,
    pub state: String,
    pub scope: String,
    pub statement: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub query_text: Option<String>,
    pub tier: Option<Tier>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    pub tier: Tier,
    pub snippet: String,
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_decay_per_tier() {
        assert_eq!(Tier::Working.default_decay(), DecayPolicy::Fast);
        assert_eq!(Tier::Procedural.default_decay(), DecayPolicy::Never);
    }
}
