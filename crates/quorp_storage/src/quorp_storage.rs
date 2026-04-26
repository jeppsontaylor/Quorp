//! Storage adapter facade for Quorp: SQLite (rusqlite WAL), lexical index
//! (tantivy), vector index (usearch), embedder (fastembed-rs).
//!
//! Phase 6 ships the trait surface and an in-memory placeholder
//! implementation so consumers can compile and unit-test against the
//! traits without spinning up real backends. The on-disk implementations
//! land when the runtime wire-up needs them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRoot {
    pub workspace: PathBuf,
    pub sqlite_path: PathBuf,
    pub tantivy_dir: PathBuf,
    pub vectors_path: PathBuf,
}

impl StorageRoot {
    pub fn under(workspace: PathBuf) -> Self {
        let dot_quorp = workspace.join(".quorp");
        Self {
            workspace,
            sqlite_path: dot_quorp.join("memory.sqlite"),
            tantivy_dir: dot_quorp.join("index").join("tantivy"),
            vectors_path: dot_quorp.join("index").join("vectors.usearch"),
        }
    }
}

/// Append-only event log shared by the memory + rule-forge subscribers.
pub trait EventLog: Send + Sync {
    fn append(&self, event: serde_json::Value) -> Result<u64>;
    fn count(&self) -> Result<u64>;
}

/// BM25-style lexical search over symbols, file text, and git notes.
pub trait LexicalIndex: Send + Sync {
    fn upsert(&self, doc_id: &str, fields: BTreeMap<String, String>) -> Result<()>;
    fn query(&self, q: &str, limit: usize) -> Result<Vec<LexicalHit>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexicalHit {
    pub doc_id: String,
    pub score: f32,
    pub snippet: String,
}

/// Vector index for semantic recall.
pub trait VectorIndex: Send + Sync {
    fn upsert(&self, doc_id: &str, embedding: &[f32]) -> Result<()>;
    fn query(&self, embedding: &[f32], k: usize) -> Result<Vec<VectorHit>>;
    fn dimension(&self) -> usize;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorHit {
    pub doc_id: String,
    pub score: f32,
}

/// Embedding model wrapper. Concrete impl loads BGE-small-en-v1.5 ONNX.
pub trait Embedder: Send + Sync {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimension(&self) -> usize;
}

/// Top-level storage facade — what most callers consume.
pub trait Storage: Send + Sync {
    fn root(&self) -> &StorageRoot;
    fn events(&self) -> &dyn EventLog;
    fn lexical(&self) -> &dyn LexicalIndex;
    fn vectors(&self) -> &dyn VectorIndex;
    fn embedder(&self) -> &dyn Embedder;
}

pub mod inmem {
    //! Minimal in-memory implementations used by tests and skeletons.
    use std::sync::{Mutex, RwLock};

    use super::*;

    #[derive(Debug, Default)]
    pub struct InMemoryEventLog {
        events: Mutex<Vec<serde_json::Value>>,
    }

    impl EventLog for InMemoryEventLog {
        fn append(&self, event: serde_json::Value) -> Result<u64> {
            let mut events = self.events.lock().map_err(|_| anyhow::anyhow!("event log poisoned"))?;
            events.push(event);
            Ok(events.len() as u64)
        }

        fn count(&self) -> Result<u64> {
            let events = self.events.lock().map_err(|_| anyhow::anyhow!("event log poisoned"))?;
            Ok(events.len() as u64)
        }
    }

    #[derive(Debug, Default)]
    pub struct InMemoryLexicalIndex {
        docs: RwLock<BTreeMap<String, BTreeMap<String, String>>>,
    }

    impl LexicalIndex for InMemoryLexicalIndex {
        fn upsert(&self, doc_id: &str, fields: BTreeMap<String, String>) -> Result<()> {
            let mut docs = self.docs.write().map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
            docs.insert(doc_id.to_string(), fields);
            Ok(())
        }

        fn query(&self, q: &str, limit: usize) -> Result<Vec<LexicalHit>> {
            let docs = self.docs.read().map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
            let q_lower = q.to_ascii_lowercase();
            let mut hits: Vec<LexicalHit> = docs
                .iter()
                .filter_map(|(id, fields)| {
                    let total = fields.values().map(String::as_str).collect::<Vec<_>>().join("\n");
                    if total.to_ascii_lowercase().contains(&q_lower) {
                        Some(LexicalHit {
                            doc_id: id.clone(),
                            score: 1.0,
                            snippet: total.chars().take(160).collect(),
                        })
                    } else {
                        None
                    }
                })
                .collect();
            hits.truncate(limit);
            Ok(hits)
        }
    }

    #[derive(Debug)]
    pub struct InMemoryVectorIndex {
        dim: usize,
        rows: RwLock<BTreeMap<String, Vec<f32>>>,
    }

    impl InMemoryVectorIndex {
        pub fn new(dim: usize) -> Self {
            Self { dim, rows: RwLock::new(BTreeMap::new()) }
        }
    }

    impl VectorIndex for InMemoryVectorIndex {
        fn upsert(&self, doc_id: &str, embedding: &[f32]) -> Result<()> {
            anyhow::ensure!(embedding.len() == self.dim, "wrong vector dim");
            let mut rows = self.rows.write().map_err(|_| anyhow::anyhow!("vec poisoned"))?;
            rows.insert(doc_id.to_string(), embedding.to_vec());
            Ok(())
        }

        fn query(&self, embedding: &[f32], k: usize) -> Result<Vec<VectorHit>> {
            let rows = self.rows.read().map_err(|_| anyhow::anyhow!("vec poisoned"))?;
            let mut scored: Vec<VectorHit> = rows
                .iter()
                .map(|(id, v)| VectorHit { doc_id: id.clone(), score: cosine(embedding, v) })
                .collect();
            scored.sort_by(|a, b| b.score.total_cmp(&a.score));
            scored.truncate(k);
            Ok(scored)
        }

        fn dimension(&self) -> usize {
            self.dim
        }
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
    }

    /// Deterministic stand-in embedder: hashes input text into a small
    /// sparse vector. Good enough for unit tests; real embeddings ship
    /// with the BGE-small ONNX backend.
    #[derive(Debug)]
    pub struct StubEmbedder {
        dim: usize,
    }

    impl Default for StubEmbedder {
        fn default() -> Self {
            Self { dim: 384 }
        }
    }

    impl StubEmbedder {
        pub fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    impl Embedder for StubEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0_f32; self.dim];
                    for (i, byte) in t.bytes().enumerate() {
                        let slot = (byte as usize + i) % self.dim;
                        v[slot] += 1.0;
                    }
                    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                    if norm > 0.0 {
                        for x in v.iter_mut() {
                            *x /= norm;
                        }
                    }
                    v
                })
                .collect())
        }

        fn dimension(&self) -> usize {
            self.dim
        }
    }

    #[derive(Debug)]
    pub struct InMemoryStorage {
        pub root: StorageRoot,
        pub events: InMemoryEventLog,
        pub lexical: InMemoryLexicalIndex,
        pub vectors: InMemoryVectorIndex,
        pub embedder: StubEmbedder,
    }

    impl InMemoryStorage {
        pub fn new(workspace: PathBuf) -> Self {
            let root = StorageRoot::under(workspace);
            Self {
                root,
                events: InMemoryEventLog::default(),
                lexical: InMemoryLexicalIndex::default(),
                vectors: InMemoryVectorIndex::new(384),
                embedder: StubEmbedder::default(),
            }
        }
    }

    impl Storage for InMemoryStorage {
        fn root(&self) -> &StorageRoot { &self.root }
        fn events(&self) -> &dyn EventLog { &self.events }
        fn lexical(&self) -> &dyn LexicalIndex { &self.lexical }
        fn vectors(&self) -> &dyn VectorIndex { &self.vectors }
        fn embedder(&self) -> &dyn Embedder { &self.embedder }
    }
}

#[cfg(test)]
mod tests {
    use super::inmem::*;
    use super::*;

    #[test]
    fn lexical_finds_substring() {
        let idx = InMemoryLexicalIndex::default();
        let mut fields = BTreeMap::new();
        fields.insert("path".into(), "src/main.rs".into());
        fields.insert("text".into(), "fn main() { println!(\"hello quorp\"); }".into());
        idx.upsert("doc-1", fields).unwrap();
        let hits = idx.query("quorp", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "doc-1");
    }

    #[test]
    fn vector_upsert_and_query() {
        let idx = InMemoryVectorIndex::new(3);
        idx.upsert("a", &[1.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0]).unwrap();
        let hits = idx.query(&[1.0, 0.1, 0.0], 2).unwrap();
        assert_eq!(hits[0].doc_id, "a");
    }

    #[test]
    fn stub_embedder_returns_unit_vectors() {
        let e = StubEmbedder::default();
        let vs = e.embed_batch(&["hello", "world"]).unwrap();
        assert_eq!(vs.len(), 2);
        assert_eq!(vs[0].len(), 384);
        let norm: f32 = vs[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }

    #[test]
    fn event_log_appends() {
        let log = InMemoryEventLog::default();
        let id = log.append(serde_json::json!({"kind": "test"})).unwrap();
        assert_eq!(id, 1);
        assert_eq!(log.count().unwrap(), 1);
    }
}
