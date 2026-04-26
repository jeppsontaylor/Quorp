//! Typed tool protocol, path guards, and repository search primitives.

pub mod apply;
pub mod edit;
pub mod patch;
pub mod path_index;
pub mod preview;

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use quorp_core::PermissionMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolRequest {
    Read {
        path: PathBuf,
    },
    Search {
        query: String,
        path: Option<PathBuf>,
    },
    Patch {
        path: PathBuf,
        unified_diff: String,
    },
    Shell {
        command: String,
        cwd: PathBuf,
    },
    Git {
        args: Vec<String>,
        cwd: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolResult {
    pub success: bool,
    pub summary: String,
    pub raw_log_path: Option<PathBuf>,
    pub exit_code: Option<i32>,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathGuard {
    workspace_root: PathBuf,
    permission_mode: PermissionMode,
}

impl PathGuard {
    pub fn new(workspace_root: impl Into<PathBuf>, permission_mode: PermissionMode) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            permission_mode,
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn ensure_editable(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };
        if self.permission_mode == PermissionMode::FullPermissions {
            return Ok(resolved);
        }
        let canonical_workspace =
            std::fs::canonicalize(&self.workspace_root).with_context(|| {
                format!(
                    "failed to canonicalize workspace {}",
                    self.workspace_root.display()
                )
            })?;
        let canonical_parent = resolved
            .parent()
            .map(std::fs::canonicalize)
            .transpose()
            .with_context(|| format!("failed to canonicalize parent of {}", resolved.display()))?;
        let Some(canonical_parent) = canonical_parent else {
            anyhow::bail!("path {} has no parent", resolved.display());
        };
        if !canonical_parent.starts_with(&canonical_workspace) {
            anyhow::bail!(
                "path {} is outside workspace {}",
                resolved.display(),
                canonical_workspace.display()
            );
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_guard_rejects_edits_outside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let guard = PathGuard::new(workspace.path(), PermissionMode::Ask);

        let error = guard
            .ensure_editable(Path::new("/tmp/not-in-this-workspace.txt"))
            .expect_err("outside path rejected");

        assert!(error.to_string().contains("outside workspace"));
    }
}
