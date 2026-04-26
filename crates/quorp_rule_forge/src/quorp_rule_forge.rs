//! Dynamic rule forge — mines failure signals into draft rules,
//! validates them in shadow mode, promotes verified rules to active.
//!
//! Phase 6 ships the signature/cluster machinery and lifecycle state
//! transitions. The shadow-validation harness lands once `quorp_verify`
//! and `quorp_session` are wired together.

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::Result;
use quorp_ids::RuleId;
use quorp_memory_model::NegativeSignature;
use quorp_rule_model::{Rule, RulePattern, RuleState, Scope, Trigger};
use quorp_verify_model::Failure;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClusterKey {
    pub error_code: Option<String>,
    pub message_skeleton: String,
}

impl ClusterKey {
    pub fn from_failure(failure: &Failure) -> Self {
        Self {
            error_code: failure.code.clone(),
            message_skeleton: normalize_message(&failure.message),
        }
    }
}

fn normalize_message(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    for ch in msg.chars() {
        if ch.is_ascii_digit() {
            out.push('#');
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Debug, Default)]
struct ForgeState {
    clusters: HashMap<ClusterKey, u32>,
    rules: Vec<Rule>,
}

#[derive(Debug, Default)]
pub struct RuleForge {
    state: RwLock<ForgeState>,
}

impl RuleForge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe a verifier failure and return the signature recorded in
    /// the negative-memory tier.
    pub fn observe_failure(&self, failure: &Failure) -> Result<NegativeSignature> {
        let key = ClusterKey::from_failure(failure);
        let mut state = self.state.write().map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        let count = state.clusters.entry(key.clone()).and_modify(|c| *c += 1).or_insert(1);
        Ok(NegativeSignature {
            signature: format!("{}:{}", key.error_code.clone().unwrap_or_default(), key.message_skeleton),
            failure_kind: key.error_code.clone().unwrap_or_else(|| "unknown".into()),
            seen_count: *count,
        })
    }

    /// Emit a candidate rule when a cluster crosses the threshold.
    pub fn maybe_emit_candidate(&self, key: &ClusterKey, statement: String) -> Result<Option<RuleId>> {
        let mut state = self.state.write().map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        let count = *state.clusters.get(key).unwrap_or(&0);
        if count < 2 {
            return Ok(None);
        }
        let rule_id = RuleId::new(format!("rule-{}", state.rules.len() + 1));
        let rule = Rule {
            id: rule_id.clone(),
            state: RuleState::Candidate,
            scope: Scope::Repo,
            statement,
            effect: quorp_rule_model::RuleEffect::PromptRule,
            pattern: RulePattern {
                trigger: Trigger {
                    error_code: key.error_code.clone(),
                    symbol_path_prefix: None,
                    message_skeleton: Some(key.message_skeleton.clone()),
                    ast_kind: None,
                },
                min_cluster_count: 2,
                min_confidence: 0.6,
            },
            confidence: 0.6,
            created_at_unix: 0,
            updated_at_unix: 0,
            verified_for_runs: 0,
            false_positive_runs: 0,
        };
        state.rules.push(rule);
        Ok(Some(rule_id))
    }

    /// Drive a rule through the lifecycle. Returns the new state.
    pub fn promote(&self, id: &RuleId) -> Result<Option<RuleState>> {
        let mut state = self.state.write().map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        if let Some(rule) = state.rules.iter_mut().find(|r| &r.id == id) {
            rule.state = match rule.state {
                RuleState::Candidate => RuleState::Draft,
                RuleState::Draft => RuleState::Verified,
                RuleState::Verified => RuleState::Active,
                RuleState::Challenged => RuleState::Active,
                RuleState::Active => RuleState::Active,
                other => other,
            };
            return Ok(Some(rule.state));
        }
        Ok(None)
    }

    /// Snapshot of currently-active rules for the prompt assembler.
    pub fn active_rules(&self, scope: Scope) -> Result<Vec<Rule>> {
        let state = self.state.read().map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        Ok(state
            .rules
            .iter()
            .filter(|r| r.scope == scope && matches!(r.state, RuleState::Active))
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(code: &str, msg: &str) -> Failure {
        Failure {
            code: Some(code.into()),
            message: msg.into(),
            level: "error".into(),
            file: None,
            line: None,
        }
    }

    #[test]
    fn cluster_increments_then_emits_candidate() {
        let forge = RuleForge::new();
        let f1 = fail("E0382", "value moved here at line 12");
        let f2 = fail("E0382", "value moved here at line 27");
        let _ = forge.observe_failure(&f1).unwrap();
        let _ = forge.observe_failure(&f2).unwrap();
        let key = ClusterKey::from_failure(&f1);
        let id = forge.maybe_emit_candidate(&key, "do not move owned vec across loop body".into()).unwrap();
        assert!(id.is_some());
    }

    #[test]
    fn promote_walks_states() {
        let forge = RuleForge::new();
        let f = fail("E0382", "borrow of moved value");
        let _ = forge.observe_failure(&f).unwrap();
        let _ = forge.observe_failure(&f).unwrap();
        let key = ClusterKey::from_failure(&f);
        let id = forge.maybe_emit_candidate(&key, "x".into()).unwrap().unwrap();
        let s1 = forge.promote(&id).unwrap();
        let s2 = forge.promote(&id).unwrap();
        let s3 = forge.promote(&id).unwrap();
        assert_eq!(s1, Some(RuleState::Draft));
        assert_eq!(s2, Some(RuleState::Verified));
        assert_eq!(s3, Some(RuleState::Active));
    }
}
