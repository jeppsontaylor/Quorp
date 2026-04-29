use super::*;
use std::fs;
use std::sync::{Arc, RwLock};

fn entry(rel: &str, is_dir: bool) -> PathEntry {
    let lowercase_rel = rel.to_lowercase();
    PathEntry {
        relative_display: rel.to_string(),
        lowercase_rel,
        ascii_char_mask: ascii_char_mask_from_str(&rel.to_lowercase()),
        is_directory: is_dir,
        abs_path: PathBuf::from("/tmp").join(rel),
    }
}

#[test]
fn rank_empty_query_returns_all_sorted_by_display() {
    let entries = vec![entry("b", false), entry("a", false), entry(".", true)];
    let ranked = rank_path_entries(&entries, "", 10);
    assert_eq!(
        ranked
            .iter()
            .map(|e| e.relative_display.as_str())
            .collect::<Vec<_>>(),
        vec![".", "a", "b"]
    );
}

#[test]
fn rank_substring_beats_subsequence() {
    let entries = vec![entry("src/lib.rs", false), entry("other.txt", false)];
    let ranked = rank_path_entries(&entries, "lib", 10);
    assert_eq!(ranked[0].relative_display, "src/lib.rs");
}

#[test]
fn rank_limit_truncates() {
    let entries: Vec<PathEntry> = (0..20).map(|i| entry(&format!("f{i}"), false)).collect();
    assert_eq!(rank_path_entries(&entries, "", 5).len(), 5);
}

#[test]
fn ascii_mask_prefilter_matches_brute_force_small() {
    let entries: Vec<PathEntry> = (0..50)
        .map(|i| entry(&format!("file_{i}_x.txt"), false))
        .collect();
    for q in ["x", "file", "25", "no_such"] {
        let brute: Vec<_> = entries
            .iter()
            .filter(|e| score_match(&e.lowercase_rel, q).is_some())
            .map(|e| e.relative_display.clone())
            .collect();
        let fast: Vec<_> = rank_path_entries(&entries, q, 200)
            .into_iter()
            .map(|e| e.relative_display)
            .collect();
        let mut b = brute;
        b.sort();
        let mut f = fast;
        f.sort();
        assert_eq!(f, b, "query {q:?}");
    }
}

#[test]
fn walk_project_lists_file_under_temp_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("hello.txt"), "x").expect("write");
    let walked = walk_project(dir.path(), || {});
    let names: Vec<_> = walked
        .iter()
        .filter(|e| e.relative_display != ".")
        .map(|e| e.relative_display.as_str())
        .collect();
    assert!(
        names.contains(&"hello.txt"),
        "expected hello.txt in {:?}",
        names
    );
}

#[test]
fn path_index_eventually_indexes_temp_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("indexed.rs"), "").expect("write");
    let index = PathIndex::new(dir.path().to_path_buf());
    assert!(
        index.blocking_wait_for_ready(dir.path(), Duration::from_secs(5)),
        "index should populate"
    );
    let matches = index.match_query("indexed", 10);
    assert!(
        matches.iter().any(|e| e.relative_display == "indexed.rs"),
        "matches: {:?}",
        matches
    );
}

#[test]
fn progress_goes_scanning_then_ready_on_temp_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("p.txt"), "").expect("write");
    let index = PathIndex::new(dir.path().to_path_buf());
    let _ = index.blocking_wait_for_ready(dir.path(), Duration::from_secs(5));
    let p = index.snapshot_progress();
    assert_eq!(p.phase, PathIndexPhase::Ready);
    assert!(p.entry_count >= 2);
    assert!(p.files_seen >= 2);
}

#[test]
fn only_dot_entry_counts_as_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let index = PathIndex::new(dir.path().to_path_buf());
    assert!(
        index.blocking_wait_for_ready(dir.path(), Duration::from_secs(5)),
        "empty tree still has '.'"
    );
    let m = index.match_query("", 10);
    assert!(m.iter().any(|e| e.relative_display == "."), "{m:?}");
}

#[test]
fn project_backed_apply_snapshot_updates_match_query() {
    let watch = Arc::new(RwLock::new(PathBuf::from("/proj")));
    let index = PathIndex::new_project_backed(PathBuf::from("/proj"), Arc::clone(&watch));
    assert_eq!(index.snapshot_progress().phase, PathIndexPhase::Scanning);
    let entries = Arc::new(vec![
        entry("src/a.rs", false),
        path_entry_from_parts(".".to_string(), true, PathBuf::from("/proj")),
    ]);
    index.apply_bridge_snapshot(PathBuf::from("/proj"), Arc::clone(&entries), 2);
    assert_eq!(index.snapshot_progress().phase, PathIndexPhase::Ready);
    let hits = index.match_query("a.rs", 10);
    assert!(
        hits.iter().any(|e| e.relative_display == "src/a.rs"),
        "{hits:?}"
    );
}

#[test]
fn search_repo_text_returns_ranked_line_hits() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    fs::write(
        dir.path().join("src/lib.rs"),
        "fn render_agent_turn_text() {}\nfn other() {}\n",
    )
    .expect("write lib");
    fs::write(
        dir.path().join("README.md"),
        "render_agent_turn_text is documented here\n",
    )
    .expect("write readme");

    let hits = search_repo_text(dir.path(), "render_agent_turn_text", 4);
    assert!(!hits.is_empty(), "expected repo text hits");
    assert_eq!(hits[0].path, "src/lib.rs");
    assert_eq!(hits[0].line_number, 1);
}

#[test]
fn search_repo_symbols_extracts_rust_symbols() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub struct RepoCapsule;\nimpl RepoCapsule {}\npub fn render_repo_capsule() {}\n",
    )
    .expect("write lib");

    let hits = search_repo_symbols(dir.path(), "RepoCapsule", 8);
    assert!(
        hits.iter()
            .any(|hit| hit.kind == "struct" && hit.name == "RepoCapsule")
    );
    assert!(
        hits.iter()
            .any(|hit| hit.kind == "impl" && hit.name.contains("RepoCapsule"))
    );
}
#[test]
fn search_repo_symbols_extracts_python_symbols() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    fs::write(
        dir.path().join("src/main.py"),
        "class AgentManager:\n    pass\n\nasync def fetch_data():\n    pass\n\n# def ignored():\n",
    )
    .expect("write python");

    let hits = search_repo_symbols(dir.path(), "Agent", 8);
    assert!(
        hits.iter()
            .any(|hit| hit.kind == "class" && hit.name == "AgentManager")
    );

    let hits_fn = search_repo_symbols(dir.path(), "fetch", 8);
    assert!(
        hits_fn
            .iter()
            .any(|hit| hit.kind == "def" && hit.name == "fetch_data")
    );
    let hits_ignored = search_repo_symbols(dir.path(), "ignored", 8);
    assert!(hits_ignored.is_empty());
}
#[test]
fn build_repo_capsule_includes_workspace_and_focus() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["crates/quorp", "crates/util"]

[package]
name = "quorp"
"#,
    )
    .expect("write cargo");
    fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn render_agent_turn_text() {}\n",
    )
    .expect("write lib");

    let capsule = build_repo_capsule(dir.path(), Some("render_agent_turn_text"), 4);
    assert_eq!(capsule.workspace_name.as_deref(), Some("quorp"));
    assert_eq!(capsule.workspace_members.len(), 2);
    assert!(
        capsule
            .focus_symbols
            .iter()
            .any(|symbol| symbol.name == "render_agent_turn_text")
    );
    assert!(capsule.focus_files.iter().any(|path| path == "src/lib.rs"));
}

#[test]
fn rank_path_entries_large_slice_under_budget() {
    let entries: Vec<PathEntry> = (0..50_000)
        .map(|i| entry(&format!("src/module_{i}.rs"), false))
        .collect();
    let start = std::time::Instant::now();
    let ranked = rank_path_entries(&entries, "module_42", 80);
    let elapsed = start.elapsed();
    assert_eq!(
        ranked.first().map(|e| e.relative_display.as_str()),
        Some("src/module_42.rs")
    );
    assert!(elapsed.as_millis() < 500, "took {elapsed:?}");
}
