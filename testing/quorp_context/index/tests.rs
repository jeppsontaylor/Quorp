use super::*;
use crate::{CompileContext, CompileRequest};
use quorp_context_model::{Anchor, TokenBudget};

#[test]
fn build_and_reopen_index_round_trips_status() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("src")).expect("src");
    fs::write(
        root.path().join("src/lib.rs"),
        "pub fn hello() {}\n#[test]\nfn smoke() {}\n",
    )
    .expect("source");

    let report = build_index(root.path()).expect("build");
    assert_eq!(report.indexed_files, 1);
    let status = index_status(root.path()).expect("status");
    assert!(status.exists);
    assert_eq!(status.stale_files, 0);
    assert!(status.symbol_count >= 1);
}

#[test]
fn build_detects_incremental_invalidation() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("src")).expect("src");
    let source_path = root.path().join("src/lib.rs");
    fs::write(&source_path, "pub fn hello() {}\n").expect("source");
    build_index(root.path()).expect("build");

    fs::write(&source_path, "pub fn hello() {}\npub fn goodbye() {}\n").expect("source");
    let stale = index_status(root.path()).expect("status");
    assert_eq!(stale.stale_files, 1);

    let report = build_index(root.path()).expect("rebuild");
    assert_eq!(report.changed_files, 1);
    assert!(index_status(root.path()).expect("status").stale_files == 0);
}

#[test]
fn explain_symbol_returns_deterministic_definition_order() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("src")).expect("src");
    fs::write(root.path().join("src/a.rs"), "pub fn shared() {}\n").expect("a");
    fs::write(root.path().join("src/b.rs"), "pub fn shared() {}\n").expect("b");
    build_index(root.path()).expect("build");

    let explanation = explain_symbol(root.path(), "shared").expect("explain");
    let paths = explanation
        .definitions
        .iter()
        .map(|definition| definition.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
    );
}

#[test]
fn index_reader_records_context_pack_provenance() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(root.path().join("src")).expect("src");
    fs::write(
        root.path().join("src/lib.rs"),
        "pub fn indexed_symbol() {}\n",
    )
    .expect("source");
    build_index(root.path()).expect("build");

    let compiler = crate::ContextCompiler::new();
    let pack = compiler
        .compile_workspace(
            root.path(),
            &CompileRequest {
                anchors: vec![Anchor::Symbol(SymbolPath::new("indexed_symbol"))],
                budget: TokenBudget {
                    total: 1200,
                    per_item_cap: 600,
                    reserve_for_output: 100,
                },
            },
            &CompileContext {
                git_sha: None,
                generated_at_unix: 42,
            },
        )
        .expect("compile");
    assert!(!pack.items.is_empty());
    assert!(context_pack_provenance_count(root.path()).expect("count") >= 1);
}
