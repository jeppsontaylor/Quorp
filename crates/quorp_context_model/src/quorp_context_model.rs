//! Domain types for context capsules — chunks, citations, item metadata,
//! token budgets.

#![allow(dead_code)]

use std::path::PathBuf;

use quorp_ids::{ChunkId, PackId, RuleId};
use quorp_repo_graph::{LineRange, SymbolPath};
use serde::{Deserialize, Serialize};

/// Token-budget envelope for one compile call.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TokenBudget {
    pub total: u32,
    pub per_item_cap: u32,
    pub reserve_for_output: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trust {
    /// Sourced directly from on-disk files.
    Source,
    /// Derived from a parser / static analyser.
    Derived,
    /// Sourced from prior memory or rules.
    Recalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    File,
    Symbol,
    Diagnostic,
    Memory,
    Rule,
    GitHistory,
    Vector,
    Lexical,
}

/// Per-item metadata used by the compiler's knapsack-style selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemMeta {
    pub relevance: f32,
    pub freshness_secs: u64,
    pub trust: Trust,
    pub cost_tokens: u32,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackMetadata {
    pub git_sha: Option<String>,
    pub generated_at_unix: i64,
    pub compiler_version: String,
}

/// Anchor for a compile request: what the agent told the compiler to
/// gather context around.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Anchor {
    Symbol(SymbolPath),
    File(PathBuf),
    Range(PathBuf, LineRange),
    Query(String),
}

/// One inlined item in a context pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextItem {
    Excerpt {
        chunk: ChunkId,
        path: PathBuf,
        range: LineRange,
        text: String,
        meta: ItemMeta,
    },
    SymbolDef {
        chunk: ChunkId,
        path: SymbolPath,
        signature: String,
        body_excerpt: String,
        meta: ItemMeta,
    },
    Memory {
        chunk: ChunkId,
        snippet: String,
        meta: ItemMeta,
    },
    Rule {
        chunk: ChunkId,
        rule_id: RuleId,
        statement: String,
        meta: ItemMeta,
    },
}

/// Handle to an item the compiler didn't inline (cost too high) but kept
/// available for `expand_context`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handle {
    pub chunk: ChunkId,
    pub source: Source,
    pub estimated_cost_tokens: u32,
    pub label: String,
}

/// Output of one compile call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub pack_id: PackId,
    pub items: Vec<ContextItem>,
    pub handles: Vec<Handle>,
    pub budget_used: u32,
    pub metadata: PackMetadata,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_round_trips() {
        let b = TokenBudget {
            total: 64_000,
            per_item_cap: 8_000,
            reserve_for_output: 4_000,
        };
        let json = serde_json::to_string(&b).unwrap();
        let _back: TokenBudget = serde_json::from_str(&json).unwrap();
    }
}
