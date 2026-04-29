use std::path::{Path, PathBuf};

use quorp_ids::QuorpError;
use quorp_repo_graph::LineRange;
use serde::{Deserialize, Serialize};

use crate::FileHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteLeaseOperation {
    WriteFile,
    ReplaceRange,
    ModifyToml,
    ApplyPatch,
    ReplaceBlock,
    SetExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteLease {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<FileHash>,
    pub allowed_operations: Vec<WriteLeaseOperation>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry_turn: Option<usize>,
}

impl WriteLease {
    pub fn permits_path(&self, path: &Path) -> bool {
        self.path == path
    }

    pub fn permits_operation(&self, operation: WriteLeaseOperation) -> bool {
        self.allowed_operations.contains(&operation)
    }

    pub fn validate_path(
        &self,
        path: &Path,
        operation: WriteLeaseOperation,
    ) -> Result<(), QuorpError> {
        if !self.permits_operation(operation) {
            return Err(QuorpError::PreconditionFailed(format!(
                "write lease on {} does not allow {:?}",
                self.path.display(),
                operation
            )));
        }
        if !self.permits_path(path) {
            return Err(QuorpError::PreconditionFailed(format!(
                "write lease is anchored to {} not {}",
                self.path.display(),
                path.display()
            )));
        }
        Ok(())
    }
}
