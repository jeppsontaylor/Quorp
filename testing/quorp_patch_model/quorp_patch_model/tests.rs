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

#[test]
fn round_trip_edit_intent() {
    let intent = EditIntent::TomlSet {
        path: PathBuf::from("Cargo.toml"),
        table: "dependencies".to_string(),
        key: "chrono".to_string(),
    };
    let json = serde_json::to_string(&intent).unwrap();
    let decoded: EditIntent = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, intent);
    assert_eq!(decoded.kind_label(), "toml_set");
}
