#![allow(dead_code)]

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::{FileHash, WriteAmplification};

pub fn normalized_diff_hash(
    intent_kind: &str,
    before_hash: &FileHash,
    after_hash: &FileHash,
    touched_paths: &[PathBuf],
    rollback_token_count: usize,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(intent_kind.as_bytes());
    hasher.update(b"\0");
    hasher.update(before_hash.0.as_bytes());
    hasher.update(b"\0");
    hasher.update(after_hash.0.as_bytes());
    hasher.update(b"\0");
    for path in touched_paths {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(rollback_token_count.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn smallest_safe_edit(amplification: &WriteAmplification) -> bool {
    amplification.after_lines <= 200 && amplification.changed_lines_estimate <= 200
}
