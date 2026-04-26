//! Context Compiler — produces token-budgeted ContextPacks for the agent
//! turn loop. Multi-source retrieval + knapsack selection + progressive
//! disclosure via handles.
//!
//! Phase 7 ships the public API and a deterministic mock-source selector
//! used by tests. Real graph/lexical/vector retrieval lands when
//! `quorp_storage` and `quorp_repo_scan` are wired into the runtime.

use anyhow::Result;
pub use quorp_context_model::*;
use quorp_ids::{ChunkId, PackId};
use quorp_memory::Memory;
use quorp_memory_model::{MemoryQuery, Tier};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileContext {
    pub git_sha: Option<String>,
    pub generated_at_unix: i64,
}

/// Inputs to one compile call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileRequest {
    pub anchors: Vec<Anchor>,
    pub budget: TokenBudget,
}

#[derive(Debug)]
pub struct ContextCompiler<'a> {
    pub memory: Option<&'a Memory>,
}

impl<'a> ContextCompiler<'a> {
    pub fn new() -> Self {
        Self { memory: None }
    }

    pub fn with_memory(memory: &'a Memory) -> Self {
        Self { memory: Some(memory) }
    }

    pub fn compile(
        &self,
        request: &CompileRequest,
        ctx: &CompileContext,
    ) -> Result<ContextPack> {
        let mut items = Vec::new();
        let mut handles = Vec::new();
        let mut budget_used = 0u32;

        // Memory recall: pull a handful of related semantic facts.
        if let Some(memory) = self.memory {
            let query = derive_memory_query(&request.anchors);
            for hit in memory.recall(&query)? {
                let cost = estimate_tokens(&hit.snippet);
                if budget_used + cost <= request.budget.total {
                    items.push(ContextItem::Memory {
                        chunk: ChunkId::new(format!("mem-{}", items.len())),
                        snippet: hit.snippet,
                        meta: ItemMeta {
                            relevance: hit.score,
                            freshness_secs: 0,
                            trust: Trust::Recalled,
                            cost_tokens: cost,
                            source: Source::Memory,
                        },
                    });
                    budget_used += cost;
                } else {
                    handles.push(Handle {
                        chunk: ChunkId::new(format!("mem-{}-handle", handles.len())),
                        source: Source::Memory,
                        estimated_cost_tokens: cost,
                        label: format!("memory hit ({:?})", hit.tier),
                    });
                }
            }
        }

        // Anchors translate one-to-one into stub items so callers can see
        // the shape of the pack pre wire-up.
        for (idx, anchor) in request.anchors.iter().enumerate() {
            let label = anchor_label(anchor);
            handles.push(Handle {
                chunk: ChunkId::new(format!("anchor-{idx}")),
                source: Source::Symbol,
                estimated_cost_tokens: 200,
                label,
            });
        }

        Ok(ContextPack {
            pack_id: PackId::new(format!("pack-{}", ctx.generated_at_unix)),
            items,
            handles,
            budget_used,
            metadata: PackMetadata {
                git_sha: ctx.git_sha.clone(),
                generated_at_unix: ctx.generated_at_unix,
                compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })
    }
}

impl<'a> Default for ContextCompiler<'a> {
    fn default() -> Self {
        Self::new()
    }
}

fn derive_memory_query(anchors: &[Anchor]) -> MemoryQuery {
    let query_text = anchors.iter().find_map(|a| match a {
        Anchor::Query(s) => Some(s.clone()),
        Anchor::Symbol(p) => Some(p.as_str().to_string()),
        _ => None,
    });
    MemoryQuery {
        query_text,
        tier: Some(Tier::Semantic),
        limit: 4,
    }
}

fn anchor_label(anchor: &Anchor) -> String {
    match anchor {
        Anchor::Symbol(p) => format!("symbol {}", p.as_str()),
        Anchor::File(p) => format!("file {}", p.display()),
        Anchor::Range(p, r) => format!("range {}:{}-{}", p.display(), r.start, r.end),
        Anchor::Query(q) => format!("query \"{q}\""),
    }
}

/// Heuristic token estimate: ~4 bytes per token, but at least 8 to keep
/// trivial snippets countable.
pub fn estimate_tokens(text: &str) -> u32 {
    let raw = (text.len() / 4).max(8);
    raw.try_into().unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quorp_repo_graph::SymbolPath;

    #[test]
    fn compile_with_no_memory_returns_handle_per_anchor() {
        let compiler = ContextCompiler::new();
        let req = CompileRequest {
            anchors: vec![
                Anchor::Symbol(SymbolPath::new("crate::main")),
                Anchor::Query("borrow checker".into()),
            ],
            budget: TokenBudget { total: 8000, per_item_cap: 2000, reserve_for_output: 1000 },
        };
        let ctx = CompileContext { git_sha: Some("abc".into()), generated_at_unix: 0 };
        let pack = compiler.compile(&req, &ctx).unwrap();
        assert!(pack.items.is_empty());
        assert_eq!(pack.handles.len(), 2);
    }

    #[test]
    fn compile_with_memory_inlines_recalled_snippet() {
        let memory = Memory::new();
        memory
            .record(quorp_memory::MemoryEvent::RecordSemantic(
                quorp_memory_model::SemanticFact {
                    subject: "crate:quorp_agent_core".into(),
                    predicate: "forbids".into(),
                    object: "let _ = on fallible".into(),
                    confidence: 0.95,
                },
            ))
            .unwrap();

        let compiler = ContextCompiler::with_memory(&memory);
        let req = CompileRequest {
            anchors: vec![Anchor::Query("forbids let _".into())],
            budget: TokenBudget { total: 8000, per_item_cap: 2000, reserve_for_output: 1000 },
        };
        let ctx = CompileContext { git_sha: None, generated_at_unix: 1 };
        let pack = compiler.compile(&req, &ctx).unwrap();
        assert!(pack.items.iter().any(|item| matches!(item, ContextItem::Memory { .. })));
    }

    #[test]
    fn token_estimate_at_least_eight() {
        assert!(estimate_tokens("hi") >= 8);
        assert!(estimate_tokens(&"x".repeat(120)) >= 30);
    }
}
