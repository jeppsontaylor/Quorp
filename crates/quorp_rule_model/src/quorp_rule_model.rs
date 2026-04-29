//! Domain types for the dynamic rule forge — pattern, anchor, trigger,
//! lifecycle state, effect.

#![allow(dead_code)]

use quorp_ids::RuleId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleState {
    Candidate,
    Draft,
    Verified,
    Active,
    Challenged,
    Retired,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Global,
    Org,
    Repo,
    Branch,
    Task,
}

/// What kind of artifact a rule compiles into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleEffect {
    PromptRule,
    Lint,
    Test,
    PatchGuard,
}

/// Trigger conditions a rule attaches to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub error_code: Option<String>,
    pub symbol_path_prefix: Option<String>,
    pub message_skeleton: Option<String>,
    pub ast_kind: Option<String>,
}

/// A pattern that matches a class of failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulePattern {
    pub trigger: Trigger,
    pub min_cluster_count: u32,
    pub min_confidence: f32,
}

/// A rule as stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub state: RuleState,
    pub scope: Scope,
    pub statement: String,
    pub effect: RuleEffect,
    pub pattern: RulePattern,
    pub confidence: f32,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    pub verified_for_runs: u32,
    pub false_positive_runs: u32,
}
#[cfg(test)]
#[path = "../../../testing/quorp_rule_model/quorp_rule_model/tests.rs"]
mod tests;
