//! Phase 3g: builds @-mention path lists from [`project::Project`] / [`worktree::Snapshot`]
//! (gitignore-aware scan) and pushes [`crate::quorp::tui::TuiEvent::PathIndexSnapshot`] to the ratatui
//! thread. Pair with [`crate::quorp::tui::path_index::PathIndex::new_project_backed`] on the UI side.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Entity};
use project::{EntryKind, Project};

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::path_index::{PathEntry, path_entry_from_parts};

fn worktree_scan_signature(project: &Project, cx: &App) -> u64 {
    project
        .visible_worktrees(cx)
        .map(|wt| wt.read(cx).snapshot().scan_id() as u64)
        .fold(0u64, u64::wrapping_add)
}

pub(crate) fn collect_path_entries_from_project(
    project: &Entity<Project>,
    display_root: &Path,
    cx: &App,
) -> Vec<PathEntry> {
    let root_abs = std::fs::canonicalize(display_root).unwrap_or_else(|_| display_root.to_path_buf());
    let mut out = Vec::new();
    out.push(path_entry_from_parts(".".to_string(), true, root_abs.clone()));

    let project_read = project.read(cx);
    for wt in project_read.visible_worktrees(cx) {
        let wt_read = wt.read(cx);
        let snapshot = wt_read.snapshot();
        for entry in snapshot.entries(false, 0) {
            let abs_path = snapshot.absolutize(entry.path.as_ref());
            let rel = match abs_path.strip_prefix(&root_abs) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let relative_display = rel.to_string_lossy().replace('\\', "/");
            if relative_display.is_empty() {
                continue;
            }
            let is_directory = matches!(
                entry.kind,
                EntryKind::Dir | EntryKind::UnloadedDir | EntryKind::PendingDir,
            );
            out.push(path_entry_from_parts(
                relative_display,
                is_directory,
                abs_path,
            ));
        }
    }

    out.sort_by(|a, b| {
        a.relative_display
            .to_lowercase()
            .cmp(&b.relative_display.to_lowercase())
    });
    out
}

/// Periodically snapshots visible worktrees and publishes path rows when the worktree scan id set
/// or display root changes.
pub fn spawn_path_index_bridge_loop(
    project: Entity<Project>,
    async_app: gpui::AsyncApp,
    path_index_display_root: Arc<std::sync::RwLock<PathBuf>>,
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
) -> gpui::Task<()> {
    async_app.spawn(async move |async_cx| {
        let mut last_key: Option<(u64, PathBuf)> = None;
        loop {
            let display_root = match path_index_display_root.read() {
                Ok(g) => g.clone(),
                Err(e) => {
                    log::error!("path index bridge: display root lock poisoned: {e}");
                    break;
                }
            };

            let (sig, entries) = async_cx.update(|cx| {
                let sig = project.read_with(cx, |proj, cx| worktree_scan_signature(proj, cx));
                let entries = collect_path_entries_from_project(&project, &display_root, cx);
                (sig, entries)
            });

            let key = (sig, display_root.clone());
            if last_key.as_ref() != Some(&key) {
                last_key = Some(key);
                let arc_entries = Arc::new(entries);
                let files_seen = arc_entries.len() as u64;
                if let Err(e) = event_tx.send(TuiEvent::PathIndexSnapshot {
                    root: display_root,
                    entries: arc_entries,
                    files_seen,
                }) {
                    log::error!("path index bridge: UI channel closed: {e}");
                    break;
                }
            }

            async_cx
                .background_executor()
                .timer(Duration::from_millis(400))
                .await;
        }
    })
}
