use super::*;
use quorp_agent_core::ReadFileRange;
use tempfile::tempdir;

#[test]
fn read_file_contents_rejects_binary_and_truncates() {
    let root = tempdir().expect("tempdir");
    let huge = root.path().join("huge.txt");
    let bytes = vec![b'a'; FILE_READ_LIMIT_BYTES + 123];
    std::fs::write(&huge, &bytes).expect("write");
    let output = read_file_contents(&huge, None).expect("read");
    assert!(output.ends_with(FILE_READ_TRUNCATION_MARKER));
    assert_eq!(
        output.len(),
        FILE_READ_LIMIT_BYTES + FILE_READ_TRUNCATION_MARKER.len()
    );

    let binary = root.path().join("binary.bin");
    std::fs::write(&binary, [0xff, 0x00]).expect("write");
    assert!(read_file_contents(&binary, None).is_err());
}

#[test]
fn read_file_contents_honors_requested_range() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("sample.txt");
    std::fs::write(&file, "one\ntwo\nthree\nfour\n").expect("write");

    let output = read_file_contents(
        &file,
        Some(ReadFileRange {
            start_line: 2,
            end_line: 3,
        }),
    )
    .expect("read");

    assert_eq!(output, "two\nthree");
}

#[test]
fn list_directory_entries_orders_and_truncates() {
    let root = tempdir().expect("tempdir");
    for index in 0..(DIRECTORY_LIST_LIMIT + 20) {
        let path = root.path().join(format!("file-{index:04}.txt"));
        std::fs::write(path, b"x").expect("write");
    }
    let entries = list_directory_entries(root.path()).expect("list");
    assert_eq!(entries.len(), DIRECTORY_LIST_LIMIT);
    assert!(entries.windows(2).all(|window| window[0] <= window[1]));

    let long_file = root.path().join("a".repeat(DIRECTORY_NAME_LIMIT + 20));
    std::fs::write(&long_file, b"x").expect("write long");
    let entries = list_directory_entries(root.path()).expect("list");
    assert!(
        entries
            .iter()
            .any(|entry| entry.len() <= DIRECTORY_NAME_LIMIT)
    );
}

#[test]
fn write_full_file_replaces_content_and_requires_parent_dir() {
    let root = tempdir().expect("tempdir");
    let path = root.path().join("nested").join("file.txt");
    assert!(write_full_file(&path, "new").is_err());

    let file = root.path().join("existing.txt");
    write_full_file(&file, "before").expect("write");
    write_full_file(&file, "after").expect("rewrite");
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "after");
}

#[cfg(unix)]
#[test]
fn set_executable_bit_marks_regular_file_executable() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().expect("tempdir");
    let file = root.path().join("script.sh");
    write_full_file(&file, "#!/bin/sh\necho hi\n").expect("write");
    set_executable_bit(&file).expect("chmod");
    let mode = std::fs::metadata(&file)
        .expect("metadata")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
}

#[test]
fn replace_range_uses_stable_hash_and_preserves_surrounding_content() {
    let current = "one\ntwo\nthree\n";
    let range = ReadFileRange {
        start_line: 2,
        end_line: 2,
    };
    let expected_hash = stable_content_hash("two");
    let updated =
        perform_range_replacement(current, range, &expected_hash, "TWO").expect("replace");
    assert_eq!(updated, "one\nTWO\nthree\n");
    let stale = perform_range_replacement(current, range, "0000000000000000", "TWO")
        .expect_err("stale hash");
    assert!(stale.to_string().contains("hash mismatch"));
}

#[test]
fn modify_toml_sets_and_removes_dependency_with_full_file_hash() {
    let current = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
    let expected_hash = stable_content_hash(current);
    let updated = apply_toml_operations(
        current,
        &expected_hash,
        &[TomlEditOperation::SetDependency {
            table: "dependencies".to_string(),
            name: "chrono".to_string(),
            version: Some("0.4".to_string()),
            features: vec!["clock".to_string()],
            default_features: Some(false),
            optional: None,
            package: None,
            path: None,
        }],
    )
    .expect("set dependency");
    assert!(updated.contains("[dependencies]"));
    assert!(updated.contains("chrono"));
    assert!(updated.parse::<toml_edit::DocumentMut>().is_ok());

    let updated_hash = stable_content_hash(&updated);
    let removed = apply_toml_operations(
        &updated,
        &updated_hash,
        &[TomlEditOperation::RemoveDependency {
            table: "dependencies".to_string(),
            name: "chrono".to_string(),
        }],
    )
    .expect("remove dependency");
    assert!(!removed.contains("chrono"));
}
