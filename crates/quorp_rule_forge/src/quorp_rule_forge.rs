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
use quorp_memory_model::{FailureFingerprint, NegativeSignature};
use quorp_rule_model::{Rule, RulePattern, RuleState, Scope, Trigger};
use quorp_verify_model::{Failure, ProofPacket};

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
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        let count = state
            .clusters
            .entry(key.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        Ok(NegativeSignature {
            signature: format!(
                "{}:{}",
                key.error_code.clone().unwrap_or_default(),
                key.message_skeleton
            ),
            failure_kind: key.error_code.clone().unwrap_or_else(|| "unknown".into()),
            seen_count: *count,
        })
    }

    /// Emit a candidate rule when a cluster crosses the threshold.
    pub fn maybe_emit_candidate(
        &self,
        key: &ClusterKey,
        statement: String,
    ) -> Result<Option<RuleId>> {
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("forge poisoned"))?;
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

    pub fn observe_packet_failure(
        &self,
        packet: &ProofPacket,
        attempted_fix_hash: impl Into<String>,
        owner: Option<String>,
    ) -> Result<Option<FailureFingerprint>> {
        let Some(failure) = first_packet_failure(packet) else {
            return Ok(None);
        };
        let negative = self.observe_failure(&failure)?;
        Ok(Some(FailureFingerprint {
            signature: negative.signature,
            failure_kind: negative.failure_kind,
            owner,
            attempted_fix_hash: attempted_fix_hash.into(),
            evidence_hash: packet.raw_log_ref.sha256.clone(),
        }))
    }

    pub fn record_shadow_result(
        &self,
        id: &RuleId,
        prevented_failure: bool,
    ) -> Result<Option<RuleState>> {
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        let Some(rule) = state.rules.iter_mut().find(|rule| &rule.id == id) else {
            return Ok(None);
        };
        if prevented_failure {
            rule.verified_for_runs = rule.verified_for_runs.saturating_add(1);
            rule.confidence = (rule.confidence + 0.1).min(1.0);
            if matches!(rule.state, RuleState::Draft | RuleState::Candidate)
                && rule.verified_for_runs >= 2
                && rule.confidence >= rule.pattern.min_confidence
            {
                rule.state = RuleState::Verified;
            }
            if matches!(rule.state, RuleState::Verified) && rule.verified_for_runs >= 3 {
                rule.state = RuleState::Active;
            }
        } else {
            rule.false_positive_runs = rule.false_positive_runs.saturating_add(1);
            rule.confidence = (rule.confidence - 0.2).max(0.0);
            rule.state = if rule.false_positive_runs >= 2 {
                RuleState::Rejected
            } else {
                RuleState::Challenged
            };
        }
        Ok(Some(rule.state))
    }

    /// Drive a rule through the lifecycle. Returns the new state.
    pub fn promote(&self, id: &RuleId) -> Result<Option<RuleState>> {
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("forge poisoned"))?;
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
        let state = self
            .state
            .read()
            .map_err(|_| anyhow::anyhow!("forge poisoned"))?;
        Ok(state
            .rules
            .iter()
            .filter(|r| r.scope == scope && matches!(r.state, RuleState::Active))
            .cloned()
            .collect())
    }
}

fn first_packet_failure(packet: &ProofPacket) -> Option<Failure> {
    if let Some(diagnostic) = packet.diagnostics.first() {
        return Some(Failure {
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
            level: diagnostic.level.clone(),
            file: diagnostic
                .primary_span
                .as_ref()
                .map(|span| span.file.clone()),
            line: diagnostic.primary_span.as_ref().map(|span| span.line),
        });
    }
    if let Some(test) = packet.failing_tests.first() {
        return Some(Failure {
            code: None,
            message: test
                .panic
                .clone()
                .unwrap_or_else(|| "test failed".to_string()),
            level: "test".to_string(),
            file: None,
            line: None,
        });
    }
    if let Some(finding) = packet.security_findings.first() {
        return Some(Failure {
            code: finding.advisory_id.clone(),
            message: finding.message.clone(),
            level: finding
                .severity
                .clone()
                .unwrap_or_else(|| "security".to_string()),
            file: finding.path.clone(),
            line: None,
        });
    }
    None
}
#[cfg(test)]
#[path = "../../../testing/quorp_rule_forge/quorp_rule_forge/tests.rs"]
mod tests;
