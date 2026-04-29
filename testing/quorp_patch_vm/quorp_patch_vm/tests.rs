use std::path::{Path, PathBuf};

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
fn failed_apply_rolls_back_previous_changes_and_preserves_original_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let first = root.path().join("a.txt");
    let second = root.path().join("b.txt");
    std::fs::write(&first, "a\n").expect("write a");
    std::fs::write(&second, "b\n").expect("write b");

    let original_first = std::fs::read(&first).expect("read a");
    let original_second = std::fs::read(&second).expect("read b");

    let vm = PatchVm::new();
    let patch_id = quorp_ids::PatchId::new("patch-rollback");
    let changes = vec![
        FileChange {
            path: first.clone(),
            display_path: "a.txt".to_string(),
            expected_hash: Some(hash_bytes(b"a\n")),
            kind: FileChangeKind::Update {
                content: b"aa\n".to_vec(),
            },
        },
        FileChange {
            path: second.clone(),
            display_path: "b.txt".to_string(),
            expected_hash: None,
            kind: FileChangeKind::Add {
                content: b"bb\n".to_vec(),
            },
        },
    ];

    let preview = vm
        .preview_file_changes(&patch_id, &changes, PatchVmPolicy::default())
        .expect("preview");
    let error = vm
        .apply_file_changes(
            &patch_id,
            &changes,
            PatchApplyProof::PreviewId(&preview.preview_id),
            PatchVmPolicy::default(),
        )
        .expect_err("apply should fail on existing add target");

    assert!(error.to_string().contains("refusing to add existing file"));
    assert_eq!(std::fs::read(&first).expect("read a"), original_first);
    assert_eq!(std::fs::read(&second).expect("read b"), original_second);
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

#[test]
fn write_lease_binds_path_and_allowed_operation() {
    let lease = WriteLease {
        path: PathBuf::from("src/lib.rs"),
        range: None,
        expected_hash: Some(hash_bytes(b"old\n")),
        allowed_operations: vec![WriteLeaseOperation::WriteFile],
        reason: "lease".to_string(),
        expiry_turn: Some(3),
    };

    assert!(lease.permits_path(Path::new("src/lib.rs")));
    assert!(lease.permits_operation(WriteLeaseOperation::WriteFile));
    assert!(
        lease
            .validate_path(Path::new("src/lib.rs"), WriteLeaseOperation::WriteFile)
            .is_ok()
    );
    assert!(
        lease
            .validate_path(Path::new("src/main.rs"), WriteLeaseOperation::WriteFile)
            .is_err()
    );
}

#[test]
fn patch_receipt_v2_carries_diff_hash_and_smallest_edit_flag() {
    let receipt = PatchReceipt {
        patch_id: quorp_ids::PatchId::new("patch-v2"),
        provenance: EditProvenance::WriteFile {
            path: PathBuf::from("src/lib.rs"),
        },
        outcome: ApplyOutcome::Applied,
        preview_id: "preview-v2".to_string(),
        touched_paths: vec![PathBuf::from("src/lib.rs")],
        rollback_tokens: vec![],
    };
    let receipt_v2 = PatchReceiptV2::from_receipt(
        &receipt,
        hash_bytes(b"old\n"),
        hash_bytes(b"new\n"),
        true,
        vec!["packet-a".to_string()],
    );

    assert_eq!(receipt_v2.intent_kind, "write_file");
    assert_eq!(receipt_v2.before_hash.0.len(), 64);
    assert_eq!(receipt_v2.after_hash.0.len(), 64);
    assert_eq!(receipt_v2.verifier_packet_ids, vec!["packet-a".to_string()]);
    assert!(receipt_v2.smallest_safe_edit);
    assert_eq!(receipt_v2.normalized_diff_hash.len(), 64);
}

#[test]
fn write_amplification_flags_broad_source_write() {
    let after_content = vec!["line".to_string(); 201].join("\n");
    let amplification = WriteAmplification::from_content_change(
        "write_file",
        Some(b"one\n"),
        after_content.as_bytes(),
    );

    assert!(amplification.is_broad_source_write());
    assert!(!smallest_safe_edit(&amplification));
}
