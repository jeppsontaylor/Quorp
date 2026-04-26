//! Semantic patch VM. Validates preconditions, plans operations, applies
//! atomically, records rollback tokens.
//!
//! Phase 7 ships the precondition-checking surface and the file-hash
//! helper. Tree-sitter and rust-analyzer wiring follow in the runtime
//! integration phase.

pub use quorp_patch_model::*;

use quorp_ids::QuorpError;
use sha2::{Digest, Sha256};

/// Compute the canonical SHA-256 file hash used for precondition checks.
/// Wrapped in `FileHash` for type clarity.
pub fn hash_bytes(bytes: &[u8]) -> FileHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    FileHash(format!("{digest:x}"))
}

/// Precondition gate: confirm the on-disk bytes still match the recorded
/// hash before applying a patch.
pub fn check_file_hash(actual_bytes: &[u8], expected: &FileHash) -> Result<(), QuorpError> {
    let actual = hash_bytes(actual_bytes);
    if &actual == expected {
        Ok(())
    } else {
        Err(QuorpError::PreconditionFailed(format!(
            "file hash mismatch: expected {} got {}",
            expected.0, actual.0
        )))
    }
}

#[derive(Debug, Default)]
pub struct PatchVm;

impl PatchVm {
    pub fn new() -> Self {
        Self
    }

    /// Validate a patch plan's preconditions against the supplied bytes
    /// for each referenced file. The full apply path lands during the
    /// runtime integration phase.
    pub fn validate(&self, plan: &PatchPlan, files: &[(String, Vec<u8>)]) -> Result<(), QuorpError> {
        for op in &plan.ops {
            if let Some((expected, actual)) = expected_hash_pair(op, files) {
                check_file_hash(&actual, expected)?;
            }
        }
        Ok(())
    }
}

fn expected_hash_pair<'a>(
    op: &'a PatchOp,
    files: &'a [(String, Vec<u8>)],
) -> Option<(&'a FileHash, Vec<u8>)> {
    let (path, hash) = match op {
        PatchOp::ReplaceFunctionBody { file, file_hash, .. }
        | PatchOp::InsertMatchArm { file, file_hash, .. }
        | PatchOp::AddEnumVariant { file, file_hash, .. }
        | PatchOp::AddStructField { file, file_hash, .. }
        | PatchOp::AddImplBlock { file, file_hash, .. }
        | PatchOp::AddUseImport { file, file_hash, .. }
        | PatchOp::WrapWith { file, file_hash, .. }
        | PatchOp::DeleteSymbol { file, file_hash, .. } => {
            (file.to_string_lossy().to_string(), file_hash)
        }
        PatchOp::RenameSymbol { .. } => return None,
    };
    files
        .iter()
        .find(|(name, _)| name == &path)
        .map(|(_, bytes)| (hash, bytes.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_round_trip() {
        let bytes = b"hello world";
        let h = hash_bytes(bytes);
        assert!(h.0.len() == 64);
        check_file_hash(bytes, &h).unwrap();
    }

    #[test]
    fn check_rejects_changed_bytes() {
        let h = hash_bytes(b"original");
        let err = check_file_hash(b"changed", &h).unwrap_err();
        assert!(matches!(err, QuorpError::PreconditionFailed(_)));
    }
}
