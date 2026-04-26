use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RustbenchMaterialization {
    pub repo: String,
    pub base_commit: String,
    pub workspace_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RustbenchUpstreamMetadata {
    repo: String,
    base_commit: String,
}

pub fn maybe_materialize_rustbench_workspace(
    sandbox_root: &Path,
    condition: &str,
) -> anyhow::Result<Option<RustbenchMaterialization>> {
    let workspace_dir = sandbox_root.join("workspace").join(condition);
    if workspace_dir.exists() {
        return Ok(None);
    }
    let metadata_path = sandbox_root.join("upstream").join("metadata.json");
    if !metadata_path.exists() {
        return Ok(None);
    }
    let metadata: RustbenchUpstreamMetadata = serde_json::from_str(
        &fs::read_to_string(&metadata_path)
            .with_context(|| format!("failed to read {}", metadata_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", metadata_path.display()))?;
    let parent = workspace_dir.parent().ok_or_else(|| {
        anyhow::anyhow!("workspace path had no parent: {}", workspace_dir.display())
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let repo_url = format!("https://github.com/{}.git", metadata.repo);
    run_git_command(
        None,
        &[
            "clone",
            "--quiet",
            "--no-tags",
            "--filter=blob:none",
            repo_url.as_str(),
            workspace_dir.to_str().ok_or_else(|| {
                anyhow::anyhow!("non-utf8 workspace path {}", workspace_dir.display())
            })?,
        ],
    )?;
    run_git_command(
        Some(&workspace_dir),
        &["checkout", "--quiet", &metadata.base_commit],
    )?;
    let test_patch = sandbox_root.join("upstream").join("test.patch");
    if test_patch.exists() {
        run_git_command(
            Some(&workspace_dir),
            &[
                "apply",
                test_patch.to_str().ok_or_else(|| {
                    anyhow::anyhow!("non-utf8 patch path {}", test_patch.display())
                })?,
            ],
        )?;
    }
    run_git_command(Some(&workspace_dir), &["add", "."])?;
    run_git_command(
        Some(&workspace_dir),
        &[
            "-c",
            "user.name=quorp",
            "-c",
            "user.email=quorp@example.com",
            "commit",
            "-qm",
            "Challenge baseline",
        ],
    )?;
    Ok(Some(RustbenchMaterialization {
        repo: metadata.repo,
        base_commit: metadata.base_commit,
        workspace_dir,
    }))
}

fn run_git_command(cwd: Option<&Path>, args: &[&str]) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("git {} failed with status {status}", args.join(" "))
    }
}
