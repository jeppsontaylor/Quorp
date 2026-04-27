//! Sandbox backends for agent and benchmark runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use quorp_core::SandboxMode;
use tempfile::TempDir;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone)]
pub struct SandboxRequest {
    pub source_workspace: PathBuf,
    pub run_id: String,
    pub attempt: usize,
    pub mode: SandboxMode,
    pub keep_sandbox: bool,
}

#[derive(Debug)]
pub struct SandboxLease {
    workspace_path: PathBuf,
    sandbox_root: PathBuf,
    mode: SandboxMode,
    backend: SandboxBackend,
    source_workspace: PathBuf,
    _temp_dir: Option<TempDir>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    Host,
    GitWorktree,
    TmpCopy,
}

impl SandboxLease {
    pub fn workspace_path(&self) -> &Path {
        &self.workspace_path
    }

    pub fn sandbox_root(&self) -> &Path {
        &self.sandbox_root
    }

    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    pub fn backend(&self) -> SandboxBackend {
        self.backend
    }

    pub fn source_workspace(&self) -> &Path {
        &self.source_workspace
    }
}

pub fn create_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    match request.mode {
        SandboxMode::Host => Ok(SandboxLease {
            workspace_path: request.source_workspace.clone(),
            sandbox_root: request.source_workspace.clone(),
            mode: SandboxMode::Host,
            backend: SandboxBackend::Host,
            source_workspace: request.source_workspace,
            _temp_dir: None,
        }),
        SandboxMode::TmpCopy => create_isolated_sandbox(request),
    }
}

pub fn create_isolated_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    if source_is_git_worktree(&request.source_workspace) {
        match create_git_worktree_sandbox(request.clone()) {
            Ok(lease) => return Ok(lease),
            Err(error) => {
                log_sandbox_fallback(&request.source_workspace, &error);
            }
        }
    }
    create_tmp_copy_sandbox(request)
}

pub fn create_git_worktree_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    let temp_root = Path::new("/tmp").join("quorp");
    fs::create_dir_all(&temp_root)
        .with_context(|| format!("failed to create {}", temp_root.display()))?;
    let prefix = format!(
        "{}-attempt-{}-worktree-",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    let temp_dir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&temp_root)
        .with_context(|| format!("failed to create sandbox under {}", temp_root.display()))?;
    let sandbox_root = temp_dir.path().to_path_buf();
    let workspace_path = sandbox_root.join("workspace");
    let branch_name = format!(
        "quorp/{}-attempt-{}",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    run_git(
        &request.source_workspace,
        &[
            "worktree",
            "add",
            "--detach",
            workspace_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-utf8 sandbox path"))?,
            "HEAD",
        ],
    )
    .with_context(|| {
        format!(
            "failed to create git worktree sandbox `{branch_name}` from {}",
            request.source_workspace.display()
        )
    })?;
    let temp_dir = if request.keep_sandbox {
        let path = temp_dir.keep();
        return Ok(SandboxLease {
            workspace_path,
            sandbox_root: path,
            mode: SandboxMode::TmpCopy,
            backend: SandboxBackend::GitWorktree,
            source_workspace: request.source_workspace,
            _temp_dir: None,
        });
    } else {
        Some(temp_dir)
    };
    Ok(SandboxLease {
        workspace_path,
        sandbox_root,
        mode: SandboxMode::TmpCopy,
        backend: SandboxBackend::GitWorktree,
        source_workspace: request.source_workspace,
        _temp_dir: temp_dir,
    })
}

pub fn create_tmp_copy_sandbox(request: SandboxRequest) -> anyhow::Result<SandboxLease> {
    let temp_root = Path::new("/tmp").join("quorp");
    fs::create_dir_all(&temp_root)
        .with_context(|| format!("failed to create {}", temp_root.display()))?;
    let prefix = format!(
        "{}-attempt-{}-",
        sanitize_path_component(&request.run_id),
        request.attempt
    );
    let temp_dir = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&temp_root)
        .with_context(|| format!("failed to create sandbox under {}", temp_root.display()))?;
    let sandbox_root = temp_dir.path().to_path_buf();
    let workspace_path = sandbox_root.join("workspace");
    copy_workspace(&request.source_workspace, &workspace_path)?;
    let temp_dir = if request.keep_sandbox {
        let path = temp_dir.keep();
        return Ok(SandboxLease {
            workspace_path,
            sandbox_root: path,
            mode: SandboxMode::TmpCopy,
            backend: SandboxBackend::TmpCopy,
            source_workspace: request.source_workspace,
            _temp_dir: None,
        });
    } else {
        Some(temp_dir)
    };
    Ok(SandboxLease {
        workspace_path,
        sandbox_root,
        mode: SandboxMode::TmpCopy,
        backend: SandboxBackend::TmpCopy,
        source_workspace: request.source_workspace,
        _temp_dir: temp_dir,
    })
}

fn source_is_git_worktree(source: &Path) -> bool {
    source.join(".git").exists() || run_git(source, &["rev-parse", "--is-inside-work-tree"]).is_ok()
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    #[allow(clippy::disallowed_methods)]
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn log_sandbox_fallback(source: &Path, error: &anyhow::Error) {
    log::warn!(
        "falling back from git-worktree sandbox to tmp-copy for {}: {error:#}",
        source.display()
    );
}

fn copy_workspace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to canonicalize {}", source.display()))?;
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    for entry in WalkDir::new(&source)
        .into_iter()
        .filter_entry(include_entry)
    {
        let entry = entry.with_context(|| format!("failed to walk {}", source.display()))?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .with_context(|| format!("failed to strip {}", source.display()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        } else if entry.file_type().is_symlink() {
            copy_symlink(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn include_entry(entry: &DirEntry) -> bool {
    let Some(name) = entry.file_name().to_str() else {
        return true;
    };
    !matches!(
        name,
        ".git" | "target" | ".quorp-runs" | ".DS_Store" | "node_modules"
    )
}

#[cfg(unix)]
fn copy_symlink(source: &Path, target: &Path) -> anyhow::Result<()> {
    let link_target = fs::read_link(source)
        .with_context(|| format!("failed to read symlink {}", source.display()))?;
    std::os::unix::fs::symlink(link_target, target)
        .with_context(|| format!("failed to create symlink {}", target.display()))
}

#[cfg(not(unix))]
fn copy_symlink(source: &Path, target: &Path) -> anyhow::Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to inspect symlink target {}", source.display()))?;
    if metadata.is_dir() {
        fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))
    } else {
        fs::copy(source, target)
            .with_context(|| format!("failed to copy {}", source.display()))
            .map(|_| ())
    }
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_copy_sandbox_leaves_source_untouched() {
        let source = tempfile::tempdir().expect("source tempdir");
        let source_file = source.path().join("src.txt");
        fs::write(&source_file, "original").expect("write source");

        let lease = create_sandbox(SandboxRequest {
            source_workspace: source.path().to_path_buf(),
            run_id: "run/one".to_string(),
            attempt: 1,
            mode: SandboxMode::TmpCopy,
            keep_sandbox: false,
        })
        .expect("sandbox");

        fs::write(lease.workspace_path().join("src.txt"), "changed").expect("write sandbox");

        assert_eq!(
            fs::read_to_string(source_file).expect("read source"),
            "original"
        );
        assert_eq!(
            fs::read_to_string(lease.workspace_path().join("src.txt")).expect("read sandbox"),
            "changed"
        );
        assert_eq!(lease.backend(), SandboxBackend::TmpCopy);
    }

    #[test]
    fn git_worktree_sandbox_leaves_source_untouched() {
        #[allow(clippy::disallowed_methods)]
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let source = tempfile::tempdir().expect("source tempdir");
        fs::write(source.path().join("src.txt"), "original").expect("write source");
        run_git(source.path(), &["init"]).expect("git init");
        run_git(source.path(), &["config", "user.email", "test@example.com"]).expect("email");
        run_git(source.path(), &["config", "user.name", "Test User"]).expect("name");
        run_git(source.path(), &["add", "src.txt"]).expect("add");
        run_git(source.path(), &["commit", "-m", "initial"]).expect("commit");

        let lease = create_sandbox(SandboxRequest {
            source_workspace: source.path().to_path_buf(),
            run_id: "run/git".to_string(),
            attempt: 1,
            mode: SandboxMode::TmpCopy,
            keep_sandbox: false,
        })
        .expect("sandbox");

        fs::write(lease.workspace_path().join("src.txt"), "changed").expect("write sandbox");

        assert_eq!(lease.backend(), SandboxBackend::GitWorktree);
        assert_eq!(
            fs::read_to_string(source.path().join("src.txt")).expect("read source"),
            "original"
        );
        assert_eq!(
            fs::read_to_string(lease.workspace_path().join("src.txt")).expect("read sandbox"),
            "changed"
        );
    }
}
