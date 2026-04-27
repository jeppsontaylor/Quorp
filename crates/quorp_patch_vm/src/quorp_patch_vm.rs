//! Semantic patch VM. Validates preconditions, previews file changes, applies
//! them with rollback tokens, and preserves stable receipts.

pub use quorp_patch_model::*;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeKind {
    Add { content: Vec<u8> },
    Update { content: Vec<u8> },
    Delete,
    Move { target: PathBuf, content: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub display_path: String,
    pub expected_hash: Option<FileHash>,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchApplyProof<'a> {
    HashesOnly,
    PreviewId(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchVmPolicy {
    pub allow_full_file_rewrite: bool,
    pub max_files: usize,
}

impl Default for PatchVmPolicy {
    fn default() -> Self {
        Self {
            allow_full_file_rewrite: false,
            max_files: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchRisk {
    Low,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPreview {
    pub preview_id: String,
    pub risk: PatchRisk,
    pub touched_paths: Vec<PathBuf>,
    pub rollback_tokens: Vec<RollbackToken>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchApplyReport {
    pub outcome: ApplyOutcome,
    pub preview_id: String,
    pub touched_paths: Vec<PathBuf>,
    pub rollback_tokens: Vec<RollbackToken>,
}

impl PatchVm {
    pub fn new() -> Self {
        Self
    }

    /// Validate a patch plan's preconditions against the supplied bytes
    /// for each referenced file. The full apply path lands during the
    /// runtime integration phase.
    pub fn validate(
        &self,
        plan: &PatchPlan,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), QuorpError> {
        for op in &plan.ops {
            if let Some((expected, actual)) = expected_hash_pair(op, files) {
                check_file_hash(&actual, expected)?;
            }
        }
        Ok(())
    }

    pub fn preview_file_changes(
        &self,
        patch_id: &quorp_ids::PatchId,
        changes: &[FileChange],
        policy: PatchVmPolicy,
    ) -> Result<PatchPreview, QuorpError> {
        validate_change_count(changes, policy)?;
        let mut rollback_tokens = Vec::new();
        let mut touched_paths = BTreeSet::new();
        let mut preview_hasher = Sha256::new();
        preview_hasher.update(patch_id.as_str().as_bytes());

        for change in changes {
            validate_change(change, policy)?;
            append_change_fingerprint(&mut preview_hasher, change);
            touched_paths.insert(change.path.clone());
            if let FileChangeKind::Move { target, .. } = &change.kind {
                touched_paths.insert(target.clone());
            }

            if let Some(previous_bytes) = read_existing(&change.path)? {
                let pre_image_hash = hash_bytes(&previous_bytes);
                if let Some(expected_hash) = &change.expected_hash {
                    check_file_hash(&previous_bytes, expected_hash)?;
                } else if requires_existing_hash(change) {
                    return Err(QuorpError::PreconditionFailed(format!(
                        "missing expected hash for {}",
                        change.display_path
                    )));
                }
                preview_hasher.update(pre_image_hash.0.as_bytes());
                rollback_tokens.push(RollbackToken {
                    patch_id: patch_id.clone(),
                    file: change.path.clone(),
                    pre_image_hash,
                    previous_bytes,
                });
            } else if requires_existing_hash(change) {
                return Err(QuorpError::PreconditionFailed(format!(
                    "cannot patch missing file {}",
                    change.display_path
                )));
            }

            match &change.kind {
                FileChangeKind::Add { content }
                | FileChangeKind::Update { content }
                | FileChangeKind::Move { content, .. } => {
                    preview_hasher.update(hash_bytes(content).0.as_bytes());
                }
                FileChangeKind::Delete => {
                    preview_hasher.update(b"delete");
                }
            }
        }

        let preview_id = format!("{:x}", preview_hasher.finalize());
        Ok(PatchPreview {
            preview_id,
            risk: classify_risk(changes),
            touched_paths: touched_paths.into_iter().collect(),
            rollback_tokens,
        })
    }

    pub fn apply_file_changes(
        &self,
        patch_id: &quorp_ids::PatchId,
        changes: &[FileChange],
        proof: PatchApplyProof<'_>,
        policy: PatchVmPolicy,
    ) -> Result<PatchApplyReport, QuorpError> {
        let preview = self.preview_file_changes(patch_id, changes, policy)?;
        if preview.risk == PatchRisk::High {
            match proof {
                PatchApplyProof::PreviewId(preview_id) if preview_id == preview.preview_id => {}
                PatchApplyProof::PreviewId(_) => {
                    return Err(QuorpError::PreconditionFailed(
                        "preview id does not match current patch plan".to_string(),
                    ));
                }
                PatchApplyProof::HashesOnly => {
                    return Err(QuorpError::PreconditionFailed(
                        "high-risk patch requires a matching preview id".to_string(),
                    ));
                }
            }
        }

        let mut applied = Vec::new();
        for change in changes {
            if let Err(error) = apply_change(change) {
                rollback_applied(&applied, &preview.rollback_tokens);
                return Err(error);
            }
            applied.push(change.path.clone());
            if let FileChangeKind::Move { target, .. } = &change.kind {
                applied.push(target.clone());
            }
        }

        Ok(PatchApplyReport {
            outcome: ApplyOutcome::Applied,
            preview_id: preview.preview_id,
            touched_paths: preview.touched_paths,
            rollback_tokens: preview.rollback_tokens,
        })
    }
}

fn validate_change_count(changes: &[FileChange], policy: PatchVmPolicy) -> Result<(), QuorpError> {
    if changes.is_empty() {
        return Err(QuorpError::InvalidInput(
            "patch plan contains no file changes".to_string(),
        ));
    }
    if changes.len() > policy.max_files {
        return Err(QuorpError::InvalidInput(format!(
            "patch plan touches {} files, above max {}",
            changes.len(),
            policy.max_files
        )));
    }
    Ok(())
}

fn validate_change(change: &FileChange, policy: PatchVmPolicy) -> Result<(), QuorpError> {
    if change.display_path.trim().is_empty() {
        return Err(QuorpError::InvalidInput(
            "patch change has empty display path".to_string(),
        ));
    }
    if !policy.allow_full_file_rewrite && is_large_full_file_update(change) {
        return Err(QuorpError::PreconditionFailed(format!(
            "full-file rewrite for {} requires explicit full-file permission",
            change.display_path
        )));
    }
    Ok(())
}

fn is_large_full_file_update(change: &FileChange) -> bool {
    match &change.kind {
        FileChangeKind::Update { content } | FileChangeKind::Move { content, .. } => {
            content.len() > 256 * 1024
        }
        FileChangeKind::Add { .. } | FileChangeKind::Delete => false,
    }
}

fn classify_risk(changes: &[FileChange]) -> PatchRisk {
    if changes.len() > 1
        || changes.iter().any(|change| {
            matches!(
                change.kind,
                FileChangeKind::Delete | FileChangeKind::Move { .. }
            )
        })
    {
        PatchRisk::High
    } else {
        PatchRisk::Low
    }
}

fn requires_existing_hash(change: &FileChange) -> bool {
    matches!(
        change.kind,
        FileChangeKind::Update { .. } | FileChangeKind::Delete | FileChangeKind::Move { .. }
    )
}

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>, QuorpError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(QuorpError::Internal(format!(
            "failed to read {}: {error}",
            path.display()
        ))),
    }
}

fn apply_change(change: &FileChange) -> Result<(), QuorpError> {
    match &change.kind {
        FileChangeKind::Add { content } => {
            if change.path.exists() {
                return Err(QuorpError::PreconditionFailed(format!(
                    "refusing to add existing file {}",
                    change.display_path
                )));
            }
            write_bytes(&change.path, content)
        }
        FileChangeKind::Update { content } => write_bytes(&change.path, content),
        FileChangeKind::Delete => {
            if change.path.exists() {
                fs::remove_file(&change.path).map_err(|error| {
                    QuorpError::Internal(format!(
                        "failed to delete {}: {error}",
                        change.path.display()
                    ))
                })?;
            }
            Ok(())
        }
        FileChangeKind::Move { target, content } => {
            write_bytes(target, content)?;
            if change.path.exists() {
                fs::remove_file(&change.path).map_err(|error| {
                    QuorpError::Internal(format!(
                        "failed to remove moved source {}: {error}",
                        change.path.display()
                    ))
                })?;
            }
            Ok(())
        }
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), QuorpError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            QuorpError::Internal(format!(
                "failed to create parent directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, bytes).map_err(|error| {
        QuorpError::Internal(format!("failed to write {}: {error}", path.display()))
    })
}

fn rollback_applied(applied_paths: &[PathBuf], rollback_tokens: &[RollbackToken]) {
    let token_files = rollback_tokens
        .iter()
        .map(|token| token.file.clone())
        .collect::<BTreeSet<_>>();
    for token in rollback_tokens.iter().rev() {
        if applied_paths.iter().any(|path| path == &token.file) {
            if let Some(parent) = token.file.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                eprintln!(
                    "failed to recreate rollback parent {}: {error}",
                    parent.display()
                );
            }
            if let Err(error) = fs::write(&token.file, &token.previous_bytes) {
                eprintln!(
                    "failed to restore rollback file {}: {error}",
                    token.file.display()
                );
            }
        }
    }
    for path in applied_paths.iter().rev() {
        if !token_files.contains(path)
            && path.exists()
            && let Err(error) = fs::remove_file(path)
        {
            eprintln!(
                "failed to remove rollback-created file {}: {error}",
                path.display()
            );
        }
    }
}

fn append_change_fingerprint(hasher: &mut Sha256, change: &FileChange) {
    hasher.update(change.display_path.as_bytes());
    hasher.update(b"\0");
    match &change.kind {
        FileChangeKind::Add { .. } => hasher.update(b"add"),
        FileChangeKind::Update { .. } => hasher.update(b"update"),
        FileChangeKind::Delete => hasher.update(b"delete"),
        FileChangeKind::Move { target, .. } => {
            hasher.update(b"move");
            hasher.update(target.to_string_lossy().as_bytes());
        }
    }
}

fn expected_hash_pair<'a>(
    op: &'a PatchOp,
    files: &'a [(String, Vec<u8>)],
) -> Option<(&'a FileHash, Vec<u8>)> {
    let (path, hash) = match op {
        PatchOp::ReplaceFunctionBody {
            file, file_hash, ..
        }
        | PatchOp::InsertMatchArm {
            file, file_hash, ..
        }
        | PatchOp::AddEnumVariant {
            file, file_hash, ..
        }
        | PatchOp::AddStructField {
            file, file_hash, ..
        }
        | PatchOp::AddImplBlock {
            file, file_hash, ..
        }
        | PatchOp::AddUseImport {
            file, file_hash, ..
        }
        | PatchOp::WrapWith {
            file, file_hash, ..
        }
        | PatchOp::DeleteSymbol {
            file, file_hash, ..
        } => (file.to_string_lossy().to_string(), file_hash),
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

    #[test]
    fn preview_and_apply_low_risk_update_with_hashes_only() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("src/lib.rs");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, "pub fn value() -> i32 { 1 }\n").expect("write");

        let vm = PatchVm::new();
        let patch_id = quorp_ids::PatchId::new("patch-low");
        let change = FileChange {
            path: file.clone(),
            display_path: "src/lib.rs".to_string(),
            expected_hash: Some(hash_bytes(b"pub fn value() -> i32 { 1 }\n")),
            kind: FileChangeKind::Update {
                content: b"pub fn value() -> i32 { 2 }\n".to_vec(),
            },
        };

        let preview = vm
            .preview_file_changes(
                &patch_id,
                std::slice::from_ref(&change),
                PatchVmPolicy::default(),
            )
            .expect("preview");
        assert_eq!(preview.risk, PatchRisk::Low);

        let report = vm
            .apply_file_changes(
                &patch_id,
                &[change],
                PatchApplyProof::HashesOnly,
                PatchVmPolicy::default(),
            )
            .expect("apply");
        assert_eq!(report.outcome, ApplyOutcome::Applied);
        assert_eq!(
            std::fs::read_to_string(file).expect("read"),
            "pub fn value() -> i32 { 2 }\n"
        );
        assert_eq!(report.rollback_tokens.len(), 1);
    }

    #[test]
    fn high_risk_multi_file_patch_requires_matching_preview_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = root.path().join("a.txt");
        let second = root.path().join("b.txt");
        std::fs::write(&first, "a\n").expect("write a");
        std::fs::write(&second, "b\n").expect("write b");

        let vm = PatchVm::new();
        let patch_id = quorp_ids::PatchId::new("patch-high");
        let changes = vec![
            FileChange {
                path: first,
                display_path: "a.txt".to_string(),
                expected_hash: Some(hash_bytes(b"a\n")),
                kind: FileChangeKind::Update {
                    content: b"aa\n".to_vec(),
                },
            },
            FileChange {
                path: second,
                display_path: "b.txt".to_string(),
                expected_hash: Some(hash_bytes(b"b\n")),
                kind: FileChangeKind::Update {
                    content: b"bb\n".to_vec(),
                },
            },
        ];

        let preview = vm
            .preview_file_changes(&patch_id, &changes, PatchVmPolicy::default())
            .expect("preview");
        assert_eq!(preview.risk, PatchRisk::High);

        let error = vm
            .apply_file_changes(
                &patch_id,
                &changes,
                PatchApplyProof::HashesOnly,
                PatchVmPolicy::default(),
            )
            .expect_err("hash-only high-risk apply");
        assert!(error.to_string().contains("requires a matching preview id"));

        vm.apply_file_changes(
            &patch_id,
            &changes,
            PatchApplyProof::PreviewId(&preview.preview_id),
            PatchVmPolicy::default(),
        )
        .expect("apply");
    }

    #[test]
    fn stale_expected_hash_rejects_without_mutating() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("target.txt");
        std::fs::write(&file, "changed\n").expect("write");

        let vm = PatchVm::new();
        let patch_id = quorp_ids::PatchId::new("patch-stale");
        let change = FileChange {
            path: file.clone(),
            display_path: "target.txt".to_string(),
            expected_hash: Some(hash_bytes(b"original\n")),
            kind: FileChangeKind::Update {
                content: b"new\n".to_vec(),
            },
        };

        let error = vm
            .apply_file_changes(
                &patch_id,
                &[change],
                PatchApplyProof::HashesOnly,
                PatchVmPolicy::default(),
            )
            .expect_err("stale hash");
        assert!(matches!(error, QuorpError::PreconditionFailed(_)));
        assert_eq!(std::fs::read_to_string(file).expect("read"), "changed\n");
    }

    #[test]
    fn full_file_rewrite_policy_rejects_large_update() {
        let root = tempfile::tempdir().expect("tempdir");
        let file = root.path().join("large.txt");
        std::fs::write(&file, "small\n").expect("write");

        let vm = PatchVm::new();
        let patch_id = quorp_ids::PatchId::new("patch-large");
        let change = FileChange {
            path: file,
            display_path: "large.txt".to_string(),
            expected_hash: Some(hash_bytes(b"small\n")),
            kind: FileChangeKind::Update {
                content: vec![b'x'; 256 * 1024 + 1],
            },
        };

        let error = vm
            .preview_file_changes(&patch_id, &[change], PatchVmPolicy::default())
            .expect_err("large rewrite");
        assert!(error.to_string().contains("full-file rewrite"));
    }
}
