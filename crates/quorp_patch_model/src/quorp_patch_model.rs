//! Domain types for the semantic patch VM.

#![allow(dead_code)]

use std::path::PathBuf;

use quorp_ids::PatchId;
use quorp_repo_graph::{LineRange, SymbolPath};
use serde::{Deserialize, Serialize};

/// A blake3-derived file content hash used for precondition checking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileHash(pub String);

/// Position where to insert a new node relative to a sibling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsertPosition {
    Before,
    After,
    AtEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PatchOp {
    ReplaceFunctionBody {
        file: PathBuf,
        file_hash: FileHash,
        symbol_path: SymbolPath,
        new_body: String,
    },
    InsertMatchArm {
        file: PathBuf,
        file_hash: FileHash,
        match_anchor: String,
        arm_pattern: String,
        arm_body: String,
        position: InsertPosition,
    },
    AddEnumVariant {
        file: PathBuf,
        file_hash: FileHash,
        enum_path: SymbolPath,
        variant_decl: String,
    },
    AddStructField {
        file: PathBuf,
        file_hash: FileHash,
        struct_path: SymbolPath,
        field_name: String,
        field_type: String,
        default_init: Option<String>,
    },
    AddImplBlock {
        file: PathBuf,
        file_hash: FileHash,
        target_type: SymbolPath,
        trait_path: Option<SymbolPath>,
        items: String,
    },
    RenameSymbol {
        from: SymbolPath,
        to: String,
    },
    AddUseImport {
        file: PathBuf,
        file_hash: FileHash,
        path: String,
    },
    WrapWith {
        file: PathBuf,
        file_hash: FileHash,
        range: LineRange,
        prefix: String,
        suffix: String,
    },
    DeleteSymbol {
        file: PathBuf,
        file_hash: FileHash,
        symbol_path: SymbolPath,
        force: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPlan {
    pub patch_id: PatchId,
    pub ops: Vec<PatchOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditProvenance {
    WriteFile { path: PathBuf },
    ApplyPatch { path: PathBuf },
    ReplaceBlock { path: PathBuf },
    ReplaceRange { path: PathBuf },
    ModifyToml { path: PathBuf },
    ApplyPreview { preview_id: String },
    SetExecutable { path: PathBuf },
    SemanticPatch,
    Unknown { action: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchTransaction {
    pub patch_id: PatchId,
    pub provenance: EditProvenance,
    pub touched_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackToken {
    pub patch_id: PatchId,
    pub file: PathBuf,
    pub pre_image_hash: FileHash,
    pub previous_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplyOutcome {
    Applied,
    Rejected,
    PartiallyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReceipt {
    pub patch_id: PatchId,
    pub provenance: EditProvenance,
    pub outcome: ApplyOutcome,
    pub preview_id: String,
    pub touched_paths: Vec<PathBuf>,
    pub rollback_tokens: Vec<RollbackToken>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_replace_function_body() {
        let op = PatchOp::ReplaceFunctionBody {
            file: PathBuf::from("crates/quorp/src/main.rs"),
            file_hash: FileHash("deadbeef".into()),
            symbol_path: SymbolPath::new("crate::main"),
            new_body: "{ println!(\"hi\"); }".into(),
        };
        let json = serde_json::to_string(&op).unwrap();
        let _back: PatchOp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn round_trip_patch_receipt() {
        let receipt = PatchReceipt {
            patch_id: PatchId::new("patch-1"),
            provenance: EditProvenance::ReplaceRange {
                path: PathBuf::from("src/lib.rs"),
            },
            outcome: ApplyOutcome::Applied,
            preview_id: "preview-1".to_string(),
            touched_paths: vec![PathBuf::from("src/lib.rs")],
            rollback_tokens: vec![RollbackToken {
                patch_id: PatchId::new("patch-1"),
                file: PathBuf::from("src/lib.rs"),
                pre_image_hash: FileHash("abc".to_string()),
                previous_bytes: b"old".to_vec(),
            }],
        };

        let json = serde_json::to_string(&receipt).unwrap();
        let decoded: PatchReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, receipt);
    }
}
