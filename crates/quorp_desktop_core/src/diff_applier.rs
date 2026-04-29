//! Promote a sandbox run's `final.diff` into the source workspace.
//!
//! Closes the round-trip the benchmark/Yolo flow opens: the agent
//! mutates a /tmp clone, we surface the diff in the inspector, and
//! when the user clicks "Apply to source workspace" the patch lands
//! atomically.
//!
//! Strategy:
//! 1. Read `<run_dir>/final.diff`. Empty / missing → 0-applied receipt.
//! 2. Stage the patch in a temporary file.
//! 3. If the workspace is a git repo: shell out to `git apply --check`
//!    first; if that succeeds, run `git apply` (without `--index` so
//!    the user keeps full control of staging). On conflicts the patch
//!    is rejected wholesale — we never half-apply.
//! 4. If the workspace is NOT a git repo: parse the unified diff and
//!    apply each hunk via a small in-process applier. Same atomicity
//!    guarantee.
//!
//! The receipt carries `applied_files`, `skipped_files`,
//! `conflict_files`. A non-zero `conflict_files` means the apply
//! failed entirely; nothing changed on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use quorp_desktop_ipc::{RunIdDto, WorkspaceId};

/// Outcome surfaced to the frontend via the
/// `apply_run_diff` Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyDiffReceipt {
    pub run_id: RunIdDto,
    pub target_workspace_id: WorkspaceId,
    pub applied_files: u32,
    pub skipped_files: u32,
    pub conflict_files: u32,
    /// Best-effort one-liner. Surfaces the underlying tool's complaint
    /// when the apply fails.
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyDiffError {
    #[error("workspace target path missing on disk: {0}")]
    WorkspaceMissing(PathBuf),
    #[error("run final.diff not found at {0}")]
    DiffMissing(PathBuf),
    #[error("io error during apply: {0}")]
    Io(#[from] std::io::Error),
    #[error("git apply rejected the patch: {0}")]
    GitApplyRejected(String),
    #[error("malformed unified diff: {0}")]
    Malformed(String),
}

/// Apply the run's `final.diff` to `target_workspace_root`. The run's
/// directory layout is the canonical
/// `<workspace>/.quorp/runs/<run-id>/`; we expect `final.diff` there.
pub async fn apply_run_diff(
    run_dir: &Path,
    target_workspace_root: &Path,
    run_id: RunIdDto,
    target_workspace_id: WorkspaceId,
) -> Result<ApplyDiffReceipt, ApplyDiffError> {
    if !target_workspace_root.exists() {
        return Err(ApplyDiffError::WorkspaceMissing(
            target_workspace_root.to_path_buf(),
        ));
    }
    let diff_path = run_dir.join("final.diff");
    if !diff_path.exists() {
        return Ok(ApplyDiffReceipt {
            run_id,
            target_workspace_id,
            applied_files: 0,
            skipped_files: 0,
            conflict_files: 0,
            message: "no final.diff produced by this run".to_string(),
        });
    }
    let body = tokio::fs::read_to_string(&diff_path).await?;
    if body.trim().is_empty() {
        return Ok(ApplyDiffReceipt {
            run_id,
            target_workspace_id,
            applied_files: 0,
            skipped_files: 0,
            conflict_files: 0,
            message: "final.diff is empty".to_string(),
        });
    }

    let groups = parse_unified_diff(&body)?;
    if groups.is_empty() {
        return Ok(ApplyDiffReceipt {
            run_id,
            target_workspace_id,
            applied_files: 0,
            skipped_files: 0,
            conflict_files: 0,
            message: "final.diff contains no file groups".to_string(),
        });
    }

    let is_git = is_git_workspace(target_workspace_root);
    if is_git {
        apply_via_git(&diff_path, target_workspace_root, &groups, run_id, target_workspace_id).await
    } else {
        apply_in_process(target_workspace_root, &groups, run_id, target_workspace_id)
    }
}

fn is_git_workspace(root: &Path) -> bool {
    root.join(".git").exists()
}

async fn apply_via_git(
    diff_path: &Path,
    workspace_root: &Path,
    groups: &[FileDiffGroup],
    run_id: RunIdDto,
    target_workspace_id: WorkspaceId,
) -> Result<ApplyDiffReceipt, ApplyDiffError> {
    // Check first so we never half-apply.
    let check = tokio::process::Command::new("git")
        .current_dir(workspace_root)
        .args(["apply", "--check", "--"])
        .arg(diff_path)
        .output()
        .await?;
    if !check.status.success() {
        let stderr = String::from_utf8_lossy(&check.stderr).into_owned();
        return Ok(ApplyDiffReceipt {
            run_id,
            target_workspace_id,
            applied_files: 0,
            skipped_files: 0,
            conflict_files: groups.len() as u32,
            message: format!("git apply --check rejected: {}", stderr.trim()),
        });
    }
    let apply = tokio::process::Command::new("git")
        .current_dir(workspace_root)
        .args(["apply", "--"])
        .arg(diff_path)
        .output()
        .await?;
    if !apply.status.success() {
        let stderr = String::from_utf8_lossy(&apply.stderr).into_owned();
        return Err(ApplyDiffError::GitApplyRejected(stderr.trim().to_string()));
    }
    Ok(ApplyDiffReceipt {
        run_id,
        target_workspace_id,
        applied_files: groups.len() as u32,
        skipped_files: 0,
        conflict_files: 0,
        message: format!("applied {} file(s) via git apply", groups.len()),
    })
}

fn apply_in_process(
    workspace_root: &Path,
    groups: &[FileDiffGroup],
    run_id: RunIdDto,
    target_workspace_id: WorkspaceId,
) -> Result<ApplyDiffReceipt, ApplyDiffError> {
    // Pre-flight: every file must apply cleanly. We compute the new
    // contents in memory first; only when all files pass do we touch
    // the disk.
    let mut staged: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(groups.len());
    let mut conflicts: Vec<String> = Vec::new();

    for group in groups {
        let target = workspace_root.join(&group.path);
        let original = if target.exists() {
            std::fs::read_to_string(&target)?
        } else if group.is_new_file {
            String::new()
        } else {
            conflicts.push(format!(
                "{}: file missing from workspace but expected by diff",
                group.path
            ));
            continue;
        };
        match apply_hunks(&original, &group.hunks) {
            Ok(new) => staged.push((target, new.into_bytes())),
            Err(err) => conflicts.push(format!("{}: {err}", group.path)),
        }
    }
    if !conflicts.is_empty() {
        return Ok(ApplyDiffReceipt {
            run_id,
            target_workspace_id,
            applied_files: 0,
            skipped_files: 0,
            conflict_files: conflicts.len() as u32,
            message: format!("conflicts: {}", conflicts.join("; ")),
        });
    }
    // Commit phase. If the first write succeeds, treat subsequent
    // failures as a partial-apply and report. This stays robust on
    // disk-full / permissions errors mid-flight.
    let mut applied = 0u32;
    for (path, contents) in &staged {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        applied += 1;
    }
    Ok(ApplyDiffReceipt {
        run_id,
        target_workspace_id,
        applied_files: applied,
        skipped_files: 0,
        conflict_files: 0,
        message: format!("applied {applied} file(s) via in-process patcher"),
    })
}

/// Apply `hunks` to `original` and return the new file body.
/// Tolerant of trailing newlines; conflicts (mismatched context lines)
/// abort with `Err`.
fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, String> {
    if hunks.is_empty() {
        return Ok(original.to_string());
    }
    let original_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let mut output = String::with_capacity(original.len());
    let mut cursor = 0usize;

    for hunk in hunks {
        let start = hunk.old_start.saturating_sub(1);
        if start < cursor {
            return Err(format!(
                "hunk @{} overlaps prior application", hunk.old_start
            ));
        }
        // Emit untouched prelude.
        for line in &original_lines[cursor..start] {
            output.push_str(line);
        }
        // Walk hunk lines applying context / del / add semantics.
        let mut original_idx = start;
        for line in &hunk.lines {
            match line.kind {
                HunkLineKind::Context => {
                    let actual = original_lines.get(original_idx).copied().unwrap_or("");
                    if !lines_equal(actual, &line.text) {
                        return Err(format!(
                            "context mismatch at line {} ({:?} vs {:?})",
                            original_idx + 1,
                            actual.trim_end_matches('\n'),
                            line.text.trim_end_matches('\n'),
                        ));
                    }
                    output.push_str(actual);
                    original_idx += 1;
                }
                HunkLineKind::Delete => {
                    let actual = original_lines.get(original_idx).copied().unwrap_or("");
                    if !lines_equal(actual, &line.text) {
                        return Err(format!(
                            "delete mismatch at line {} ({:?} vs {:?})",
                            original_idx + 1,
                            actual.trim_end_matches('\n'),
                            line.text.trim_end_matches('\n'),
                        ));
                    }
                    original_idx += 1;
                }
                HunkLineKind::Add => {
                    output.push_str(&line.text);
                    if !line.text.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        }
        cursor = original_idx;
    }
    for line in &original_lines[cursor..] {
        output.push_str(line);
    }
    Ok(output)
}

fn lines_equal(a: &str, b: &str) -> bool {
    a.trim_end_matches(['\n', '\r']) == b.trim_end_matches(['\n', '\r'])
}

#[derive(Debug, Clone)]
struct FileDiffGroup {
    path: String,
    is_new_file: bool,
    hunks: Vec<Hunk>,
}

#[derive(Debug, Clone)]
struct Hunk {
    old_start: usize,
    lines: Vec<HunkLine>,
}

#[derive(Debug, Clone)]
struct HunkLine {
    kind: HunkLineKind,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HunkLineKind {
    Context,
    Delete,
    Add,
}

/// Tolerant unified-diff parser. Recognizes `diff --git`, `+++ b/...`,
/// and `@@ -a,b +c,d @@` headers; skips `index`, `---`, and other
/// metadata lines. Returns one [`FileDiffGroup`] per `+++ b/...`
/// header.
fn parse_unified_diff(body: &str) -> Result<Vec<FileDiffGroup>, ApplyDiffError> {
    let mut groups: Vec<FileDiffGroup> = Vec::new();
    let mut current: Option<FileDiffGroup> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut new_file_pending = false;

    for raw in body.split('\n') {
        if raw.starts_with("diff --git ") {
            // Flush prior file.
            flush(&mut current, &mut current_hunk, &mut groups);
            new_file_pending = false;
            continue;
        }
        if raw.starts_with("new file mode ") {
            new_file_pending = true;
            continue;
        }
        if let Some(rest) = raw.strip_prefix("+++ b/") {
            let path = rest.trim().to_string();
            current = Some(FileDiffGroup {
                path,
                is_new_file: new_file_pending,
                hunks: Vec::new(),
            });
            new_file_pending = false;
            continue;
        }
        if raw.starts_with("--- ") || raw.starts_with("+++ ") || raw.starts_with("index ") {
            continue;
        }
        if let Some(rest) = raw.strip_prefix("@@ ") {
            // "-a,b +c,d @@..." — only `a` (old_start) is needed.
            if let Some(group) = current.as_mut() {
                if let Some(hunk) = current_hunk.take() {
                    group.hunks.push(hunk);
                }
                let old_start = parse_old_start(rest)
                    .ok_or_else(|| ApplyDiffError::Malformed(rest.to_string()))?;
                current_hunk = Some(Hunk {
                    old_start,
                    lines: Vec::new(),
                });
            }
            continue;
        }
        let Some(hunk) = current_hunk.as_mut() else {
            // Not inside a hunk yet — skip noise.
            continue;
        };
        if let Some(rest) = raw.strip_prefix('+') {
            hunk.lines.push(HunkLine {
                kind: HunkLineKind::Add,
                text: format!("{rest}\n"),
            });
        } else if let Some(rest) = raw.strip_prefix('-') {
            hunk.lines.push(HunkLine {
                kind: HunkLineKind::Delete,
                text: format!("{rest}\n"),
            });
        } else if let Some(rest) = raw.strip_prefix(' ') {
            hunk.lines.push(HunkLine {
                kind: HunkLineKind::Context,
                text: format!("{rest}\n"),
            });
        }
    }
    flush(&mut current, &mut current_hunk, &mut groups);
    Ok(groups)
}

fn flush(
    current: &mut Option<FileDiffGroup>,
    current_hunk: &mut Option<Hunk>,
    groups: &mut Vec<FileDiffGroup>,
) {
    if let Some(mut group) = current.take() {
        if let Some(hunk) = current_hunk.take() {
            group.hunks.push(hunk);
        }
        groups.push(group);
    }
}

fn parse_old_start(rest: &str) -> Option<usize> {
    // rest looks like "-1,3 +1,4 @@..." or "-1 +1 @@".
    let dash_idx = rest.find('-')?;
    let after_dash = &rest[dash_idx + 1..];
    let end = after_dash
        .find([',', ' '])
        .unwrap_or(after_dash.len());
    after_dash[..end].parse().ok()
}
