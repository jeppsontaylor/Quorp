#![allow(unused)]
//! Path list for @-mentions (Phase 3g).
//!
//! [`PathIndex::new_project_backed`] applies snapshots supplied by the native TUI backend, while
//! [`PathIndex::new`] runs an `ignore` + `notify` disk walk directly. Tests use both forms depending
//! on whether they want injected backend state or a real directory scan.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use ignore::WalkBuilder;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// High-level state for UI (title bar / status).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathIndexPhase {
    Scanning,
    Ready,
}

/// Cheap snapshot for drawing; safe to call every frame.
#[derive(Clone, Debug)]
pub struct PathIndexProgress {
    pub phase: PathIndexPhase,
    /// Paths visited in the current scan (monotonic while scanning).
    pub files_seen: u64,
    /// Last published entry count (includes the synthetic `.` root row).
    pub entry_count: usize,
    pub root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PathIndexSnapshot {
    pub root: PathBuf,
    pub entries: Arc<Vec<PathEntry>>,
    pub files_seen: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathEntry {
    pub relative_display: String,
    pub lowercase_rel: String,
    /// ASCII-only OR mask for fast rejection (substring/subsequence need all query ASCII bytes present).
    pub ascii_char_mask: u128,
    pub is_directory: bool,
    pub abs_path: PathBuf,
}

#[derive(Clone, Debug)]
struct PathIndexData {
    root: PathBuf,
    entries: Arc<Vec<PathEntry>>,
}

enum Op {
    SetRoot(PathBuf),
    Refresh,
    Shutdown,
}

/// Shared index updated by a background thread (initial walk + notify debounce), or by injected
/// backend snapshots in project-backed mode.
pub struct PathIndex {
    shared: Arc<RwLock<PathIndexData>>,
    op_tx: Option<std::sync::mpsc::Sender<Op>>,
    handle: Option<JoinHandle<()>>,
    scanning: Arc<AtomicBool>,
    files_seen: Arc<AtomicU64>,
    project_backed: bool,
    display_root_watch: Option<Arc<RwLock<PathBuf>>>,
}

pub(crate) fn path_entry_from_parts(
    relative_display: String,
    is_directory: bool,
    abs_path: PathBuf,
) -> PathEntry {
    let lowercase_rel = relative_display.to_lowercase();
    let ascii_char_mask = ascii_char_mask_from_str(&lowercase_rel);
    PathEntry {
        relative_display,
        lowercase_rel,
        ascii_char_mask,
        is_directory,
        abs_path,
    }
}

fn ascii_char_mask_from_str(s: &str) -> u128 {
    let mut m = 0u128;
    for b in s.bytes() {
        if b < 128 {
            m |= 1u128 << b;
        }
    }
    m
}

fn ascii_query_mask(query: &str) -> Option<u128> {
    let mut m = 0u128;
    let mut any = false;
    for b in query.bytes() {
        if b < 128 {
            any = true;
            m |= 1u128 << b;
        } else {
            return None;
        }
    }
    if any { Some(m) } else { None }
}

impl PathIndex {
    pub fn new(initial_root: PathBuf) -> Self {
        let shared = Arc::new(RwLock::new(PathIndexData {
            root: initial_root.clone(),
            entries: Arc::new(Vec::new()),
        }));
        let scanning = Arc::new(AtomicBool::new(false));
        let files_seen = Arc::new(AtomicU64::new(0));
        let (op_tx, op_rx) = std::sync::mpsc::channel::<Op>();
        let shared_worker = Arc::clone(&shared);
        let refresh_tx = op_tx.clone();
        let scan_flag = Arc::clone(&scanning);
        let seen_counter = Arc::clone(&files_seen);
        let handle = std::thread::spawn(move || {
            worker_loop(
                shared_worker,
                op_rx,
                refresh_tx,
                initial_root,
                scan_flag,
                seen_counter,
            );
        });
        Self {
            shared,
            op_tx: Some(op_tx),
            handle: Some(handle),
            scanning,
            files_seen,
            project_backed: false,
            display_root_watch: None,
        }
    }

    /// Path list is driven by injected snapshots instead of a local `ignore` walk.
    /// `display_root_watch` is updated by [`PathIndex::set_root`] so the backend can rescope entries
    /// when the file tree root changes.
    pub fn new_project_backed(
        initial_root: PathBuf,
        display_root_watch: Arc<RwLock<PathBuf>>,
    ) -> Self {
        if let Ok(mut w) = display_root_watch.write() {
            *w = initial_root.clone();
        }
        let shared = Arc::new(RwLock::new(PathIndexData {
            root: initial_root.clone(),
            entries: Arc::new(Vec::new()),
        }));
        let scanning = Arc::new(AtomicBool::new(true));
        let files_seen = Arc::new(AtomicU64::new(0));
        Self {
            shared,
            op_tx: None,
            handle: None,
            scanning,
            files_seen,
            project_backed: true,
            display_root_watch: Some(display_root_watch),
        }
    }

    pub fn set_root(&self, root: PathBuf) {
        if let Some(tx) = &self.op_tx {
            let _ = tx.send(Op::SetRoot(root));
        } else if self.project_backed {
            if let Some(watch) = &self.display_root_watch {
                if let Ok(mut w) = watch.write() {
                    *w = root.clone();
                }
            }
            self.scanning.store(true, Ordering::Release);
            self.files_seen.store(0, Ordering::Release);
            if let Ok(mut g) = self.shared.write() {
                g.root = root;
                g.entries = Arc::new(Vec::new());
            }
        }
    }

    pub fn apply_bridge_snapshot(
        &self,
        root: PathBuf,
        entries: Arc<Vec<PathEntry>>,
        files_seen: u64,
    ) {
        if !self.project_backed {
            return;
        }
        if let Ok(mut g) = self.shared.write() {
            g.root = root;
            g.entries = entries;
        }
        self.files_seen.store(files_seen, Ordering::Release);
        self.scanning.store(false, Ordering::Release);
    }

    pub fn root(&self) -> PathBuf {
        self.shared.read().map(|g| g.root.clone()).unwrap_or_default()
    }

    pub fn snapshot_progress(&self) -> PathIndexProgress {
        let scanning = self.scanning.load(Ordering::Acquire);
        let phase = if scanning {
            PathIndexPhase::Scanning
        } else {
            PathIndexPhase::Ready
        };
        let files_seen = self.files_seen.load(Ordering::Relaxed);
        let (entry_count, root) = match self.shared.read() {
            Ok(g) => (g.entries.len(), g.root.clone()),
            Err(_) => (0, PathBuf::new()),
        };
        PathIndexProgress {
            phase,
            files_seen,
            entry_count,
            root,
        }
    }

    /// Blocks until a scan finishes and the published root matches `expected_root` with a non-empty index.
    pub fn blocking_wait_for_ready(&self, expected_root: &Path, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if !self.scanning.load(Ordering::Acquire) {
                if let Ok(g) = self.shared.read() {
                    if g.root.as_path() == expected_root && !g.entries.is_empty() {
                        return true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Blocks until at least one entry exists (can be wrong after `set_root` before the worker runs).
    pub fn blocking_wait_for_entries(&self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if let Ok(g) = self.shared.read() {
                if !g.entries.is_empty() {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    pub fn match_query(&self, query: &str, limit: usize) -> Vec<PathEntry> {
        let guard = match self.shared.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        rank_path_entries(guard.entries.as_slice(), query, limit)
    }
}

/// Deterministic ranking for tests and [`PathIndex::match_query`].
pub(crate) fn rank_path_entries(entries: &[PathEntry], query: &str, limit: usize) -> Vec<PathEntry> {
    let qmask = ascii_query_mask(query);
    let mut scored: Vec<(f64, &PathEntry)> = entries
        .iter()
        .filter(|e| {
            if let Some(qm) = qmask {
                (e.ascii_char_mask & qm) == qm
            } else {
                true
            }
        })
        .filter_map(|e| score_match(&e.lowercase_rel, query).map(|s| (s, e)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.relative_display.cmp(&b.1.relative_display))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, e)| e.clone())
        .collect()
}

impl Drop for PathIndex {
    fn drop(&mut self) {
        if let Some(tx) = self.op_tx.take() {
            let _ = tx.send(Op::Shutdown);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(
    shared: Arc<RwLock<PathIndexData>>,
    op_rx: std::sync::mpsc::Receiver<Op>,
    refresh_tx: std::sync::mpsc::Sender<Op>,
    mut root: PathBuf,
    scanning: Arc<AtomicBool>,
    files_seen: Arc<AtomicU64>,
) {
    let mut _watcher: Option<RecommendedWatcher> = None;

    let do_rebuild = |root: &Path,
                      shared: &Arc<RwLock<PathIndexData>>,
                      scanning: &Arc<AtomicBool>,
                      files_seen: &Arc<AtomicU64>| {
        scanning.store(true, Ordering::Release);
        files_seen.store(0, Ordering::Release);
        let entries = walk_project(root, || {
            files_seen.fetch_add(1, Ordering::Relaxed);
        });
        let arc_entries = Arc::new(entries);
        if let Ok(mut w) = shared.write() {
            w.root = root.to_path_buf();
            w.entries = arc_entries;
        }
        scanning.store(false, Ordering::Release);
    };

    let install_watcher = |root: &Path, refresh_tx: &std::sync::mpsc::Sender<Op>| -> Option<RecommendedWatcher> {
        let tx = refresh_tx.clone();
        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    use notify::EventKind;
                    if matches!(event.kind, EventKind::Access(_)) {
                        return;
                    }
                }
                let _ = tx.send(Op::Refresh);
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("tui: path index watcher create failed: {e}");
                return None;
            }
        };
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            log::warn!("tui: path index watch failed: {e}");
            return None;
        }
        Some(watcher)
    };

    do_rebuild(&root, &shared, &scanning, &files_seen);
    _watcher = install_watcher(&root, &refresh_tx);

    while let Ok(op) = op_rx.recv() {
        match op {
            Op::Shutdown => break,
            Op::SetRoot(new_root) => {
                _watcher = None;
                root = new_root;
                do_rebuild(&root, &shared, &scanning, &files_seen);
                _watcher = install_watcher(&root, &refresh_tx);
            }
            Op::Refresh => {
                std::thread::sleep(DEBOUNCE);
                let mut rebuilt_for_set_root = false;
                loop {
                    match op_rx.try_recv() {
                        Ok(Op::Refresh) => continue,
                        Ok(Op::SetRoot(p)) => {
                            _watcher = None;
                            root = p;
                            do_rebuild(&root, &shared, &scanning, &files_seen);
                            _watcher = install_watcher(&root, &refresh_tx);
                            rebuilt_for_set_root = true;
                            break;
                        }
                        Ok(Op::Shutdown) => return,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }
                if !rebuilt_for_set_root {
                    do_rebuild(&root, &shared, &scanning, &files_seen);
                }
            }
        }
    }
}

fn score_match(lowercase_path: &str, query: &str) -> Option<f64> {
    let q = query.trim();
    if q.is_empty() {
        return Some(500.0);
    }
    let ql = q.to_lowercase();
    if lowercase_path.contains(&ql) {
        return Some(1000.0 + 100.0 / lowercase_path.len().max(1) as f64);
    }
    if subsequence_match(lowercase_path, &ql) {
        return Some(100.0);
    }
    None
}

fn subsequence_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut it = haystack.chars();
    for c in needle.chars() {
        loop {
            match it.next() {
                Some(h) if h == c => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

fn walk_project(root: &Path, mut on_path: impl FnMut()) -> Vec<PathEntry> {
    let root = match root.canonicalize() {
        Ok(p) => p,
        Err(_) => root.to_path_buf(),
    };

    let mut out = Vec::new();
    out.push(path_entry_from_parts(".".to_string(), true, root.clone()));
    on_path();

    let mut builder = WalkBuilder::new(&root);
    builder.standard_filters(true);
    for walk_result in builder.build().flatten() {
        on_path();
        let path = walk_result.path();
        if path == root.as_path() {
            continue;
        }
        let rel = match path.strip_prefix(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let relative_display = rel.to_string_lossy().replace('\\', "/");
        if relative_display.is_empty() {
            continue;
        }
        let is_directory = walk_result
            .file_type()
            .is_some_and(|ft| ft.is_dir());
        let abs_path = path.to_path_buf();
        out.push(path_entry_from_parts(relative_display, is_directory, abs_path));
    }
    out.sort_by(|a, b| {
        a.relative_display
            .to_lowercase()
            .cmp(&b.relative_display.to_lowercase())
    });
    out
}

#[cfg(test)]
mod path_index_tests {
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
        let entries = vec![
            entry("b", false),
            entry("a", false),
            entry(".", true),
        ];
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
        let entries = vec![
            entry("src/lib.rs", false),
            entry("other.txt", false),
        ];
        let ranked = rank_path_entries(&entries, "lib", 10);
        assert_eq!(ranked[0].relative_display, "src/lib.rs");
    }

    #[test]
    fn rank_limit_truncates() {
        let entries: Vec<PathEntry> = (0..20)
            .map(|i| entry(&format!("f{i}"), false))
            .collect();
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
            names.iter().any(|n| *n == "hello.txt"),
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
        assert!(
            m.iter().any(|e| e.relative_display == "."),
            "{m:?}"
        );
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
    fn rank_path_entries_large_slice_under_budget() {
        let entries: Vec<PathEntry> = (0..50_000)
            .map(|i| entry(&format!("src/module_{i}.rs"), false))
            .collect();
        let start = std::time::Instant::now();
        let ranked = rank_path_entries(&entries, "module_42", 80);
        let elapsed = start.elapsed();
        assert_eq!(ranked.first().map(|e| e.relative_display.as_str()), Some("src/module_42.rs"));
        assert!(elapsed.as_millis() < 500, "took {elapsed:?}");
    }
}
