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
