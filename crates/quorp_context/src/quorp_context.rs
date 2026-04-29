//! Context Compiler — produces token-budgeted ContextPacks for the agent
//! turn loop. Multi-source retrieval + knapsack selection + progressive
//! disclosure via handles.
//!
//! Phase 7 ships the public API and a deterministic mock-source selector
//! used by tests. Real graph/lexical/vector retrieval lands when
//! `quorp_storage` and `quorp_repo_scan` are wired into the runtime.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
pub use quorp_context_model::*;
use quorp_ids::{ChunkId, PackId};
use quorp_memory::Memory;
use quorp_memory_model::{MemoryQuery, Tier};
use quorp_repo_graph::LineRange;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

mod compaction;
mod handles;
mod index;
mod pressure;
mod prompt_frame;
mod provenance;
pub use compaction::{ContextCompactionReport, compact_prompt_frame};
pub use handles::{HandleStore, HandleSummary, ResultHandle, ToolSynopsis};
pub use index::*;
pub use pressure::{ContextPressureReport, measure_context_pressure};
pub use prompt_frame::PromptFrame;
pub use provenance::PromptProvenance;

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
        Self {
            memory: Some(memory),
        }
    }

    pub fn compile(&self, request: &CompileRequest, ctx: &CompileContext) -> Result<ContextPack> {
        let mut items = Vec::new();
        let mut handles = Vec::new();
        let mut budget_used = 0u32;
        let mut seen = BTreeSet::new();

        // Memory recall: pull a handful of related semantic facts.
        if let Some(memory) = self.memory {
            let query = derive_memory_query(&request.anchors);
            for hit in memory.recall(&query)? {
                let cost = estimate_tokens(&hit.snippet);
                if seen.insert(format!("memory:{}", hit.snippet))
                    && try_reserve(&mut budget_used, request.budget, cost)
                {
                    items.push(ContextItem::Memory {
                        chunk: ChunkId::new(format!("mem-{}", items.len())),
                        snippet: hit.snippet,
                        meta: ItemMeta {
                            relevance: hit.score,
                            freshness_secs: 0,
                            trust: Trust::Recalled,
                            cost_tokens: cost,
                            source: Source::Memory,
                            source_hash: None,
                            selection_reason: Some("memory recall".to_string()),
                        },
                    });
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

    pub fn compile_workspace(
        &self,
        workspace_root: &Path,
        request: &CompileRequest,
        ctx: &CompileContext,
    ) -> Result<ContextPack> {
        let mut pack = self.compile(request, ctx)?;
        let mut seen = pack_seen_keys(&pack);
        let mut budget_used = pack.budget_used;
        let agent_contracts = load_agent_contracts(workspace_root)?;
        let index_reader = IndexReader::open_if_fresh(workspace_root)?;
        let lexical_limit = 4usize;

        for anchor in &request.anchors {
            if let Some(index_reader) = index_reader.as_ref() {
                match anchor {
                    Anchor::Symbol(symbol) => {
                        for excerpt in index_reader.symbol_excerpts(symbol)? {
                            push_budgeted_item(
                                &mut pack,
                                &mut seen,
                                &mut budget_used,
                                request.budget,
                                excerpt,
                            );
                        }
                    }
                    Anchor::Query(query) => {
                        for excerpt in index_reader.lexical_excerpts(query, lexical_limit)? {
                            push_budgeted_item(
                                &mut pack,
                                &mut seen,
                                &mut budget_used,
                                request.budget,
                                excerpt,
                            );
                        }
                    }
                    Anchor::File(_) | Anchor::Range(_, _) => {}
                }
            }
            for excerpt in anchor_excerpts(workspace_root, anchor)? {
                push_budgeted_item(
                    &mut pack,
                    &mut seen,
                    &mut budget_used,
                    request.budget,
                    excerpt,
                );
            }
            for excerpt in lexical_excerpts(workspace_root, anchor)? {
                push_budgeted_item(
                    &mut pack,
                    &mut seen,
                    &mut budget_used,
                    request.budget,
                    excerpt,
                );
            }

            let anchor_path = anchor_file_path(anchor);
            if let Some(anchor_path) = anchor_path.as_deref() {
                for contract in agent_contracts.items_for_path(anchor_path) {
                    push_budgeted_item(
                        &mut pack,
                        &mut seen,
                        &mut budget_used,
                        request.budget,
                        contract,
                    );
                }
            }
        }

        pack.budget_used = budget_used;
        pack.handles
            .sort_by(|left, right| left.label.cmp(&right.label));
        if let Some(index_reader) = index_reader.as_ref() {
            index_reader.record_context_pack(&pack.pack_id, &pack.items)?;
        }
        Ok(pack)
    }
}

impl<'a> Default for ContextCompiler<'a> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn packet_content_hash(packet_json: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(packet_json.as_bytes());
    format!("{:x}", hasher.finalize())
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

#[derive(Debug, Default)]
struct AgentContracts {
    owners: Vec<OwnerEntry>,
    test_lanes: Vec<TestLaneEntry>,
    proof_lanes: Vec<ProofLaneEntry>,
    generated_zones: Vec<GeneratedZoneEntry>,
}

impl AgentContracts {
    fn items_for_path(&self, path: &Path) -> Vec<ContextItem> {
        let mut items = Vec::new();
        for owner in self.owners.iter().filter(|owner| owner.matches(path)) {
            items.push(contract_item(
                Source::OwnerMap,
                format!("owner:{}", owner.id),
                format!("owner {}\n{}", owner.id, owner.responsibility),
                0.98,
            ));
        }
        for lane in self.test_lanes.iter().filter(|lane| lane.matches(path)) {
            items.push(contract_item(
                Source::TestMap,
                format!("test-lane:{}", lane.id),
                format!("test lane {}\n{}", lane.id, lane.commands.join("\n")),
                0.92,
            ));
        }
        for lane in &self.proof_lanes {
            items.push(contract_item(
                Source::ProofLane,
                format!("proof-lane:{}", lane.id),
                format!(
                    "proof lane {}\n{}\n{}",
                    lane.id,
                    lane.description,
                    lane.commands.join("\n")
                ),
                proof_lane_relevance(&lane.id),
            ));
        }
        for zone in self
            .generated_zones
            .iter()
            .filter(|zone| zone.matches(path))
        {
            items.push(contract_item(
                Source::GeneratedZone,
                format!("generated-zone:{}", zone.path),
                format!(
                    "generated zone {}\npolicy: {}\n{}",
                    zone.path, zone.policy, zone.reason
                ),
                0.99,
            ));
        }
        items
    }
}

#[derive(Debug)]
struct OwnerEntry {
    id: String,
    matchers: PathMatchers,
    responsibility: String,
}

impl OwnerEntry {
    fn matches(&self, path: &Path) -> bool {
        self.matchers.matches(path)
    }
}

#[derive(Debug)]
struct TestLaneEntry {
    id: String,
    matchers: PathMatchers,
    commands: Vec<String>,
}

impl TestLaneEntry {
    fn matches(&self, path: &Path) -> bool {
        self.matchers.matches(path)
    }
}

#[derive(Debug)]
struct ProofLaneEntry {
    id: String,
    description: String,
    commands: Vec<String>,
}

#[derive(Debug)]
struct GeneratedZoneEntry {
    path: String,
    matchers: PathMatchers,
    policy: String,
    reason: String,
}

impl GeneratedZoneEntry {
    fn matches(&self, path: &Path) -> bool {
        self.matchers.matches(path)
    }
}

#[derive(Debug)]
struct PathMatchers {
    globset: GlobSet,
}

impl PathMatchers {
    fn new(patterns: &[String]) -> Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        Ok(Self {
            globset: builder.build()?,
        })
    }

    fn matches(&self, path: &Path) -> bool {
        self.globset.is_match(path)
    }
}

#[derive(Debug, Deserialize)]
struct OwnerMapFile {
    owners: Vec<OwnerMapEntry>,
}

#[derive(Debug, Deserialize)]
struct OwnerMapEntry {
    id: String,
    paths: Vec<String>,
    responsibility: String,
}

#[derive(Debug, Deserialize)]
struct TestMapFile {
    lanes: Vec<TestMapEntry>,
}

#[derive(Debug, Deserialize)]
struct TestMapEntry {
    id: String,
    paths: Vec<String>,
    commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProofLanesFile {
    lanes: std::collections::BTreeMap<String, ProofLaneToml>,
}

#[derive(Debug, Deserialize)]
struct ProofLaneToml {
    description: Option<String>,
    commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GeneratedZonesFile {
    zones: Vec<GeneratedZoneToml>,
}

#[derive(Debug, Deserialize)]
struct GeneratedZoneToml {
    path: String,
    policy: String,
    reason: String,
}

fn load_agent_contracts(workspace_root: &Path) -> Result<AgentContracts> {
    let agent_dir = workspace_root.join("agent");
    Ok(AgentContracts {
        owners: load_owner_map(&agent_dir.join("owner-map.json"))?,
        test_lanes: load_test_map(&agent_dir.join("test-map.json"))?,
        proof_lanes: load_proof_lanes(&agent_dir.join("proof-lanes.toml"))?,
        generated_zones: load_generated_zones(&agent_dir.join("generated-zones.toml"))?,
    })
}

fn load_owner_map(path: &Path) -> Result<Vec<OwnerEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let parsed: OwnerMapFile = serde_json::from_str(&fs::read_to_string(path)?)?;
    parsed
        .owners
        .into_iter()
        .map(|entry| {
            Ok(OwnerEntry {
                id: entry.id,
                matchers: PathMatchers::new(&entry.paths)?,
                responsibility: entry.responsibility,
            })
        })
        .collect()
}

fn load_test_map(path: &Path) -> Result<Vec<TestLaneEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let parsed: TestMapFile = serde_json::from_str(&fs::read_to_string(path)?)?;
    parsed
        .lanes
        .into_iter()
        .map(|entry| {
            Ok(TestLaneEntry {
                id: entry.id,
                matchers: PathMatchers::new(&entry.paths)?,
                commands: entry.commands,
            })
        })
        .collect()
}

fn load_proof_lanes(path: &Path) -> Result<Vec<ProofLaneEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let parsed: ProofLanesFile = toml::from_str(&fs::read_to_string(path)?)?;
    Ok(parsed
        .lanes
        .into_iter()
        .map(|(id, lane)| ProofLaneEntry {
            id,
            description: lane.description.unwrap_or_default(),
            commands: lane.commands,
        })
        .collect())
}

fn load_generated_zones(path: &Path) -> Result<Vec<GeneratedZoneEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let parsed: GeneratedZonesFile = toml::from_str(&fs::read_to_string(path)?)?;
    parsed
        .zones
        .into_iter()
        .map(|zone| {
            Ok(GeneratedZoneEntry {
                matchers: PathMatchers::new(std::slice::from_ref(&zone.path))?,
                path: zone.path,
                policy: zone.policy,
                reason: zone.reason,
            })
        })
        .collect()
}

fn anchor_excerpts(workspace_root: &Path, anchor: &Anchor) -> Result<Vec<ContextItem>> {
    let Some(path) = anchor_file_path(anchor) else {
        return Ok(Vec::new());
    };
    let absolute = workspace_root.join(&path);
    let text = match fs::read_to_string(&absolute) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let range = match anchor {
        Anchor::Range(_, range) => *range,
        _ => LineRange {
            start: 1,
            end: text.lines().count().min(80).try_into().unwrap_or(u32::MAX),
        },
    };
    let excerpt = slice_lines(&text, range);
    let cost = estimate_tokens(&excerpt);
    Ok(vec![ContextItem::Excerpt {
        chunk: ChunkId::new(format!("file:{}", path.display())),
        path,
        range,
        text: excerpt,
        meta: ItemMeta {
            relevance: 1.0,
            freshness_secs: 0,
            trust: Trust::Source,
            cost_tokens: cost,
            source: Source::File,
            source_hash: Some(stable_text_hash(text.as_bytes())),
            selection_reason: Some("anchored file excerpt".to_string()),
        },
    }])
}

fn lexical_excerpts(workspace_root: &Path, anchor: &Anchor) -> Result<Vec<ContextItem>> {
    let Anchor::Query(query) = anchor else {
        return Ok(Vec::new());
    };
    let terms = query_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    collect_lexical_hits(workspace_root, workspace_root, &terms, &mut hits)?;
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.range.start.cmp(&right.range.start))
    });
    Ok(hits
        .into_iter()
        .take(4)
        .map(|hit| {
            let source_hash = stable_text_hash(hit.text.as_bytes());
            let cost = estimate_tokens(&hit.text);
            ContextItem::Excerpt {
                chunk: ChunkId::new(format!(
                    "lexical:{}:{}-{}",
                    hit.path.display(),
                    hit.range.start,
                    hit.range.end
                )),
                path: hit.path,
                range: hit.range,
                text: hit.text,
                meta: ItemMeta {
                    relevance: (0.55 + (hit.score as f32 * 0.05)).min(0.85),
                    freshness_secs: 0,
                    trust: Trust::Source,
                    cost_tokens: cost,
                    source: Source::Lexical,
                    source_hash: Some(source_hash),
                    selection_reason: Some(format!("lexical fallback hit for query `{query}`")),
                },
            }
        })
        .collect())
}

#[derive(Debug)]
struct LexicalHit {
    path: PathBuf,
    range: LineRange,
    text: String,
    score: usize,
}

fn collect_lexical_hits(
    root: &Path,
    directory: &Path,
    terms: &[String],
    hits: &mut Vec<LexicalHit>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if matches!(
            file_name.as_ref(),
            ".git" | "target" | ".quorp" | "node_modules"
        ) {
            continue;
        }
        if path.is_dir() {
            collect_lexical_hits(root, &path, terms, hits)?;
            continue;
        }
        if !is_context_source_file(&path) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(hit) = lexical_hit_for_file(root, &path, &text, terms) {
            hits.push(hit);
        }
    }
    Ok(())
}

fn lexical_hit_for_file(
    root: &Path,
    path: &Path,
    text: &str,
    terms: &[String],
) -> Option<LexicalHit> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut best: Option<(usize, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let score = terms
            .iter()
            .filter(|term| lower.contains(term.as_str()))
            .count();
        if score > 0 && best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((index, score));
        }
    }
    let (index, score) = best?;
    let start_index = index.saturating_sub(2);
    let end_index = (index + 3).min(lines.len());
    let range = LineRange {
        start: u32::try_from(start_index + 1).ok()?,
        end: u32::try_from(end_index).ok()?,
    };
    let relative = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    Some(LexicalHit {
        path: relative,
        range,
        text: lines[start_index..end_index].join("\n"),
        score,
    })
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter_map(|term| {
            let term = term.trim().to_ascii_lowercase();
            (term.len() >= 3).then_some(term)
        })
        .take(8)
        .collect()
}

fn is_context_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "rs" | "toml" | "json" | "md" | "ts" | "tsx" | "py" | "go"
            )
        })
}

fn anchor_file_path(anchor: &Anchor) -> Option<PathBuf> {
    match anchor {
        Anchor::File(path) | Anchor::Range(path, _) => Some(path.clone()),
        Anchor::Symbol(_) | Anchor::Query(_) => None,
    }
}

fn slice_lines(text: &str, range: LineRange) -> String {
    let start = range.start.max(1);
    let end = range.end.max(start);
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_number = u32::try_from(index + 1).ok()?;
            (line_number >= start && line_number <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn contract_item(source: Source, title: String, body: String, relevance: f32) -> ContextItem {
    let source_hash = stable_text_hash(body.as_bytes());
    let cost = estimate_tokens(&body);
    ContextItem::AgentContract {
        chunk: ChunkId::new(format!("{source:?}:{title}")),
        title,
        body,
        meta: ItemMeta {
            relevance,
            freshness_secs: 0,
            trust: Trust::Derived,
            cost_tokens: cost,
            source,
            source_hash: Some(source_hash),
            selection_reason: Some("agent contract".to_string()),
        },
    }
}

fn stable_text_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn proof_lane_relevance(id: &str) -> f32 {
    match id {
        "fast" => 0.95,
        "medium" => 0.9,
        "security" => 0.72,
        "loc-check" => 0.68,
        _ => 0.5,
    }
}

fn push_budgeted_item(
    pack: &mut ContextPack,
    seen: &mut BTreeSet<String>,
    budget_used: &mut u32,
    budget: TokenBudget,
    item: ContextItem,
) {
    let key = item_key(&item);
    if !seen.insert(key) {
        return;
    }
    let cost = item_cost(&item);
    if cost > budget.per_item_cap || !try_reserve(budget_used, budget, cost) {
        pack.handles.push(Handle {
            chunk: item_chunk(&item),
            source: item_source(&item),
            estimated_cost_tokens: cost,
            label: item_label(&item),
        });
        return;
    }
    pack.items.push(item);
}

fn try_reserve(used: &mut u32, budget: TokenBudget, cost: u32) -> bool {
    let available = budget.total.saturating_sub(budget.reserve_for_output);
    if cost > budget.per_item_cap || used.saturating_add(cost) > available {
        return false;
    }
    *used += cost;
    true
}

fn pack_seen_keys(pack: &ContextPack) -> BTreeSet<String> {
    pack.items.iter().map(item_key).collect()
}

fn item_key(item: &ContextItem) -> String {
    match item {
        ContextItem::Excerpt { path, range, .. } => {
            format!("file:{}:{}-{}", path.display(), range.start, range.end)
        }
        ContextItem::SymbolDef { path, .. } => format!("symbol:{}", path.as_str()),
        ContextItem::Memory { snippet, .. } => format!("memory:{snippet}"),
        ContextItem::Rule { rule_id, .. } => format!("rule:{}", rule_id.as_str()),
        ContextItem::AgentContract { title, .. } => format!("contract:{title}"),
    }
}

fn item_cost(item: &ContextItem) -> u32 {
    match item {
        ContextItem::Excerpt { meta, .. }
        | ContextItem::SymbolDef { meta, .. }
        | ContextItem::Memory { meta, .. }
        | ContextItem::Rule { meta, .. }
        | ContextItem::AgentContract { meta, .. } => meta.cost_tokens,
    }
}

fn item_chunk(item: &ContextItem) -> ChunkId {
    match item {
        ContextItem::Excerpt { chunk, .. }
        | ContextItem::SymbolDef { chunk, .. }
        | ContextItem::Memory { chunk, .. }
        | ContextItem::Rule { chunk, .. }
        | ContextItem::AgentContract { chunk, .. } => chunk.clone(),
    }
}

fn item_source(item: &ContextItem) -> Source {
    match item {
        ContextItem::Excerpt { meta, .. }
        | ContextItem::SymbolDef { meta, .. }
        | ContextItem::Memory { meta, .. }
        | ContextItem::Rule { meta, .. }
        | ContextItem::AgentContract { meta, .. } => meta.source,
    }
}

fn item_label(item: &ContextItem) -> String {
    match item {
        ContextItem::Excerpt { path, range, .. } => {
            format!("{}:{}-{}", path.display(), range.start, range.end)
        }
        ContextItem::SymbolDef { path, .. } => format!("symbol {}", path.as_str()),
        ContextItem::Memory { .. } => "memory hit".to_string(),
        ContextItem::Rule { rule_id, .. } => format!("rule {}", rule_id.as_str()),
        ContextItem::AgentContract { title, .. } => title.clone(),
    }
}
#[cfg(test)]
#[path = "../../../testing/quorp_context/quorp_context/tests.rs"]
mod tests;
