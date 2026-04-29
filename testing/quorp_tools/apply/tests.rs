use super::*;
use crate::edit::write_full_file;

#[test]
fn apply_patch_edit_stashes_before_unified_diff_write() {
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "old\n").expect("bootstrap");
    let mut stashed = Vec::new();

    let summary = apply_patch_edit(
        root.path(),
        root.path(),
        "target.txt",
        "--- a/target.txt\n+++ b/target.txt\n@@ -1 +1 @@\n-old\n+new\n",
        |path| stashed.push(path.to_path_buf()),
    )
    .expect("apply");

    assert_eq!(std::fs::read_to_string(file).expect("read"), "new\n");
    assert_eq!(
        stashed,
        vec![
            root.path()
                .join("target.txt")
                .canonicalize()
                .expect("canonical target")
        ]
    );
    assert!(summary.contains("Applied unified diff patch"));
    assert!(summary.contains("M target.txt"));
}

#[test]
fn apply_patch_edit_rejects_malformed_hunks() {
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "old").expect("bootstrap");
    let mut stashed = Vec::new();

    let error = apply_patch_edit(
        root.path(),
        root.path(),
        "target.txt",
        "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,1 @@\n-old\n",
        |path| stashed.push(path.to_path_buf()),
    )
    .expect_err("malformed hunk");

    assert!(error.to_string().contains("Malformed hunk"));
    assert!(stashed.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.path().join("target.txt")).expect("read"),
        "old"
    );
}

#[test]
fn apply_patch_edit_search_replace_uses_patch_vm_receipt() {
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "old\nkeep\n").expect("bootstrap");
    let mut stashed = Vec::new();

    let summary = apply_patch_edit(
        root.path(),
        root.path(),
        "target.txt",
        "<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n",
        |path| stashed.push(path.to_path_buf()),
    )
    .expect("apply");

    assert_eq!(
        std::fs::read_to_string(file).expect("read"),
        "new\n\nkeep\n"
    );
    assert_eq!(stashed.len(), 1);
    assert!(summary.contains("Applied search/replace blocks"));
    assert!(summary.contains("patch_vm_receipt"));
    assert!(summary.contains("patch_vm_receipt_v2"));
    assert!(summary.contains("provenance: ApplyPatch"));
}
