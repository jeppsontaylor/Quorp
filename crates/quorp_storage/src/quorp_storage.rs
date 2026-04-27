//! Storage adapter facade for Quorp: SQLite-backed event log, lexical
//! index, vector index, and embedder abstraction.
//!
//! The crate keeps an in-memory implementation for tests, but the
//! primary on-disk implementation now persists state in SQLite so the
//! rest of the runtime can rely on durable storage.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::params;
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

#[derive(Debug)]
pub struct SqliteStorage {
    root: StorageRoot,
    events: SqliteEventLog,
    lexical: SqliteLexicalIndex,
    vectors: SqliteVectorIndex,
    embedder: inmem::StubEmbedder,
}

impl SqliteStorage {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let root = StorageRoot::under(workspace);
        ensure_storage_directories(&root)?;
        let events = SqliteEventLog::new(root.sqlite_path.clone())?;
        let lexical = SqliteLexicalIndex::new(root.sqlite_path.clone())?;
        let vectors = SqliteVectorIndex::new(root.sqlite_path.clone(), 384)?;
        let embedder = inmem::StubEmbedder::default();
        Ok(Self {
            root,
            events,
            lexical,
            vectors,
            embedder,
        })
    }
}

impl Storage for SqliteStorage {
    fn root(&self) -> &StorageRoot {
        &self.root
    }

    fn events(&self) -> &dyn EventLog {
        &self.events
    }

    fn lexical(&self) -> &dyn LexicalIndex {
        &self.lexical
    }

    fn vectors(&self) -> &dyn VectorIndex {
        &self.vectors
    }

    fn embedder(&self) -> &dyn Embedder {
        &self.embedder
    }
}

#[derive(Debug)]
struct StorageDatabase {
    sqlite_path: PathBuf,
}

impl StorageDatabase {
    fn new(sqlite_path: PathBuf) -> Result<Self> {
        if let Some(parent) = sqlite_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let database = Self { sqlite_path };
        database.initialize()?;
        Ok(database)
    }

    fn open(&self) -> Result<rusqlite::Connection> {
        let connection = rusqlite::Connection::open(&self.sqlite_path)?;
        connection.execute_batch(
            r#"
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;
            "#,
        )?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<()> {
        let connection = self.open()?;
        connection.execute_batch(
            r#"
                CREATE TABLE IF NOT EXISTS event_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    event_json TEXT NOT NULL,
                    created_unix INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS lexical_documents (
                    doc_id TEXT PRIMARY KEY,
                    fields_json TEXT NOT NULL,
                    content_text TEXT NOT NULL,
                    updated_unix INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS vector_documents (
                    doc_id TEXT PRIMARY KEY,
                    embedding_json TEXT NOT NULL,
                    updated_unix INTEGER NOT NULL
                );
            "#,
        )?;
        Ok(())
    }
}

fn ensure_storage_directories(root: &StorageRoot) -> Result<()> {
    if let Some(parent) = root.sqlite_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = root.tantivy_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&root.tantivy_dir)?;
    if let Some(parent) = root.vectors_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct SqliteEventLog {
    database: Mutex<StorageDatabase>,
}

impl SqliteEventLog {
    pub fn new(sqlite_path: PathBuf) -> Result<Self> {
        Ok(Self {
            database: Mutex::new(StorageDatabase::new(sqlite_path)?),
        })
    }
}

impl EventLog for SqliteEventLog {
    fn append(&self, event: serde_json::Value) -> Result<u64> {
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("event log poisoned"))?;
        let connection = database.open()?;
        let created_unix = current_unix_seconds();
        connection.execute(
            "INSERT INTO event_log (event_json, created_unix) VALUES (?1, ?2)",
            params![serde_json::to_string(&event)?, created_unix],
        )?;
        Ok(connection.last_insert_rowid() as u64)
    }

    fn count(&self) -> Result<u64> {
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("event log poisoned"))?;
        let connection = database.open()?;
        let count: u64 =
            connection.query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))?;
        Ok(count)
    }
}

#[derive(Debug)]
pub struct SqliteLexicalIndex {
    database: Mutex<StorageDatabase>,
}

impl SqliteLexicalIndex {
    pub fn new(sqlite_path: PathBuf) -> Result<Self> {
        Ok(Self {
            database: Mutex::new(StorageDatabase::new(sqlite_path)?),
        })
    }
}

impl LexicalIndex for SqliteLexicalIndex {
    fn upsert(&self, doc_id: &str, fields: BTreeMap<String, String>) -> Result<()> {
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
        let connection = database.open()?;
        let content_text = fields
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        connection.execute(
            r#"
                INSERT INTO lexical_documents (doc_id, fields_json, content_text, updated_unix)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(doc_id) DO UPDATE SET
                    fields_json = excluded.fields_json,
                    content_text = excluded.content_text,
                    updated_unix = excluded.updated_unix
            "#,
            params![
                doc_id,
                serde_json::to_string(&fields)?,
                content_text,
                current_unix_seconds()
            ],
        )?;
        Ok(())
    }

    fn query(&self, q: &str, limit: usize) -> Result<Vec<LexicalHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
        let connection = database.open()?;
        let mut statement = connection.prepare(
            "SELECT doc_id, content_text FROM lexical_documents ORDER BY updated_unix DESC, doc_id ASC",
        )?;
        let q_lower = q.to_ascii_lowercase();
        let mut hits = Vec::new();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let doc_id: String = row.get(0)?;
            let content_text: String = row.get(1)?;
            if content_text.to_ascii_lowercase().contains(&q_lower) {
                hits.push(LexicalHit {
                    doc_id,
                    score: 1.0,
                    snippet: content_text.chars().take(160).collect(),
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }
        Ok(hits)
    }
}

#[derive(Debug)]
pub struct SqliteVectorIndex {
    database: Mutex<StorageDatabase>,
    dimension: usize,
}

impl SqliteVectorIndex {
    pub fn new(sqlite_path: PathBuf, dimension: usize) -> Result<Self> {
        Ok(Self {
            database: Mutex::new(StorageDatabase::new(sqlite_path)?),
            dimension,
        })
    }
}

impl VectorIndex for SqliteVectorIndex {
    fn upsert(&self, doc_id: &str, embedding: &[f32]) -> Result<()> {
        anyhow::ensure!(embedding.len() == self.dimension, "wrong vector dim");
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("vector poisoned"))?;
        let connection = database.open()?;
        connection.execute(
            r#"
                INSERT INTO vector_documents (doc_id, embedding_json, updated_unix)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(doc_id) DO UPDATE SET
                    embedding_json = excluded.embedding_json,
                    updated_unix = excluded.updated_unix
            "#,
            params![
                doc_id,
                serde_json::to_string(embedding)?,
                current_unix_seconds()
            ],
        )?;
        Ok(())
    }

    fn query(&self, embedding: &[f32], k: usize) -> Result<Vec<VectorHit>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        anyhow::ensure!(embedding.len() == self.dimension, "wrong vector dim");
        let database = self
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("vector poisoned"))?;
        let connection = database.open()?;
        let mut statement =
            connection.prepare("SELECT doc_id, embedding_json FROM vector_documents")?;
        let mut rows = statement.query([])?;
        let mut scored = Vec::new();
        while let Some(row) = rows.next()? {
            let doc_id: String = row.get(0)?;
            let stored: String = row.get(1)?;
            let stored_embedding: Vec<f32> = serde_json::from_str(&stored)?;
            if stored_embedding.len() != self.dimension {
                continue;
            }
            scored.push(VectorHit {
                doc_id,
                score: cosine(embedding, &stored_embedding),
            });
        }
        scored.sort_by(|left, right| right.score.total_cmp(&left.score));
        scored.truncate(k);
        Ok(scored)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
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
            let mut events = self
                .events
                .lock()
                .map_err(|_| anyhow::anyhow!("event log poisoned"))?;
            events.push(event);
            Ok(events.len() as u64)
        }

        fn count(&self) -> Result<u64> {
            let events = self
                .events
                .lock()
                .map_err(|_| anyhow::anyhow!("event log poisoned"))?;
            Ok(events.len() as u64)
        }
    }

    #[derive(Debug, Default)]
    pub struct InMemoryLexicalIndex {
        docs: RwLock<BTreeMap<String, BTreeMap<String, String>>>,
    }

    impl LexicalIndex for InMemoryLexicalIndex {
        fn upsert(&self, doc_id: &str, fields: BTreeMap<String, String>) -> Result<()> {
            let mut docs = self
                .docs
                .write()
                .map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
            docs.insert(doc_id.to_string(), fields);
            Ok(())
        }

        fn query(&self, q: &str, limit: usize) -> Result<Vec<LexicalHit>> {
            let docs = self
                .docs
                .read()
                .map_err(|_| anyhow::anyhow!("lexical poisoned"))?;
            let q_lower = q.to_ascii_lowercase();
            let mut hits: Vec<LexicalHit> = docs
                .iter()
                .filter_map(|(id, fields)| {
                    let total = fields
                        .values()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\n");
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
            Self {
                dim,
                rows: RwLock::new(BTreeMap::new()),
            }
        }
    }

    impl VectorIndex for InMemoryVectorIndex {
        fn upsert(&self, doc_id: &str, embedding: &[f32]) -> Result<()> {
            anyhow::ensure!(embedding.len() == self.dim, "wrong vector dim");
            let mut rows = self
                .rows
                .write()
                .map_err(|_| anyhow::anyhow!("vec poisoned"))?;
            rows.insert(doc_id.to_string(), embedding.to_vec());
            Ok(())
        }

        fn query(&self, embedding: &[f32], k: usize) -> Result<Vec<VectorHit>> {
            let rows = self
                .rows
                .read()
                .map_err(|_| anyhow::anyhow!("vec poisoned"))?;
            let mut scored: Vec<VectorHit> = rows
                .iter()
                .map(|(id, v)| VectorHit {
                    doc_id: id.clone(),
                    score: cosine(embedding, v),
                })
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
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
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
        fn root(&self) -> &StorageRoot {
            &self.root
        }
        fn events(&self) -> &dyn EventLog {
            &self.events
        }
        fn lexical(&self) -> &dyn LexicalIndex {
            &self.lexical
        }
        fn vectors(&self) -> &dyn VectorIndex {
            &self.vectors
        }
        fn embedder(&self) -> &dyn Embedder {
            &self.embedder
        }
    }
}

#[cfg(test)]
mod tests {
    use super::inmem::*;
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lexical_finds_substring() {
        let idx = InMemoryLexicalIndex::default();
        let mut fields = BTreeMap::new();
        fields.insert("path".into(), "src/main.rs".into());
        fields.insert(
            "text".into(),
            "fn main() { println!(\"hello quorp\"); }".into(),
        );
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

    #[test]
    fn sqlite_storage_persists_event_lexical_and_vector_data() {
        let workspace = TempDir::new().expect("tempdir");
        let storage = SqliteStorage::new(workspace.path().to_path_buf()).expect("storage");
        assert_eq!(storage.vectors().dimension(), 384);

        let event_id = storage
            .events()
            .append(serde_json::json!({"kind": "memory", "value": 1}))
            .expect("append");
        assert_eq!(event_id, 1);

        let mut lexical_fields = BTreeMap::new();
        lexical_fields.insert("path".into(), "src/main.rs".into());
        lexical_fields.insert("text".into(), "hello durable storage".into());
        storage
            .lexical()
            .upsert("doc-1", lexical_fields)
            .expect("lexical upsert");

        let mut vector = vec![0.0_f32; 384];
        vector[0] = 1.0;
        storage
            .vectors()
            .upsert("doc-1", &vector)
            .expect("vector upsert");

        drop(storage);

        let reopened = SqliteStorage::new(workspace.path().to_path_buf()).expect("reopen");
        assert_eq!(reopened.events().count().expect("count"), 1);
        let hits = reopened.lexical().query("durable", 10).expect("query");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "doc-1");
        let vector_hits = reopened.vectors().query(&vector, 1).expect("vector query");
        assert_eq!(vector_hits.len(), 1);
        assert_eq!(vector_hits[0].doc_id, "doc-1");
    }

    #[test]
    fn storage_root_under_places_files_inside_quorp_directory() {
        let workspace = std::path::Path::new("/tmp/workspace-example").to_path_buf();
        let root = StorageRoot::under(workspace.clone());
        assert!(root.sqlite_path.ends_with(".quorp/memory.sqlite"));
        assert!(root.tantivy_dir.ends_with(".quorp/index/tantivy"));
        assert!(root.vectors_path.ends_with(".quorp/index/vectors.usearch"));
        assert_eq!(root.workspace, workspace);
    }
}
