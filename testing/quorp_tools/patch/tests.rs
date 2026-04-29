use super::*;
use tempfile::tempdir;

#[test]
fn sanitize_project_path_rejects_traversal_and_external_absolute() {
    let root = tempdir().expect("tempdir");
    let outside = tempdir().expect("outside");
    let file = outside.path().join("secret");
    std::fs::write(&file, "x").expect("write");

    assert!(sanitize_project_path(root.path(), root.path(), "../outside").is_err());
    assert!(sanitize_project_path(root.path(), root.path(), &file.to_string_lossy()).is_err());
}

#[test]
fn sanitize_project_path_allows_relative_in_root() {
    let root = tempdir().expect("tempdir");
    let candidate =
        sanitize_project_path(root.path(), root.path(), "src/main.rs").expect("sanitized");
    assert_eq!(candidate, root.path().join("src/main.rs"));
}

#[test]
fn sanitize_project_path_allows_absolute_paths_inside_root() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("src").join("main.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, "fn main() {}\n").expect("write");

    let candidate = sanitize_project_path(root.path(), root.path(), &file.to_string_lossy())
        .expect("sanitized");
    assert_eq!(candidate, file.canonicalize().expect("canonical"));
}

#[test]
fn test_perform_block_replacement_exact_match() {
    let current = "line 1\nline 2\nline 3\nline 4\n";
    let search = "line 2\nline 3\n";
    let replace = "line 2 modified\nline 3 modified\n";
    let result = perform_block_replacement(current, search, replace, None).unwrap();
    assert_eq!(result, "line 1\nline 2 modified\nline 3 modified\nline 4\n");
}

#[test]
fn test_perform_block_replacement_fuzzy_trailing_whitespace() {
    let current = "fn foo() {\n    let x = 1; \n    let y = 2;\n}\n";
    let search = "    let x = 1;\n    let y = 2;";
    let replace = "    let x = 100;\n    let y = 200;";
    let result = perform_block_replacement(current, search, replace, None).unwrap();
    assert_eq!(
        result,
        "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
    );
}

#[test]
fn test_perform_block_replacement_ambiguous() {
    let current = "a\nb\nc\nb\nd\n";
    let search = "b\n";
    let replace = "x\n";
    let err = perform_block_replacement(current, search, replace, None).unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
    assert!(err.to_string().contains("lines 2, 4"));
}

#[test]
fn test_perform_block_replacement_not_found() {
    let current = "a\nb\nc\n";
    let search = "d\n";
    let replace = "x\n";
    let err = perform_block_replacement(current, search, replace, None).unwrap_err();
    assert!(err.to_string().contains("Could not find"));
}

#[test]
fn test_try_parse_search_replace_blocks() {
    let patch = "\
Here is my patch!
<<<<
fn foo() {
====
fn foo(bar: i32) {
>>>>
Done.";
    let blocks = try_parse_search_replace_blocks(patch).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].0, "fn foo() {");
    assert_eq!(blocks[0].1, "fn foo(bar: i32) {");
}

#[test]
fn test_perform_block_replacement_fuzzy_leading_whitespace() {
    let current = "fn foo() {\n    let x = 1;\n    let y = 2;\n}\n";
    let search = "let x = 1;\nlet y = 2;";
    let replace = "    let x = 100;\n    let y = 200;";
    let result = perform_block_replacement(current, search, replace, None).unwrap();
    assert_eq!(
        result,
        "fn foo() {\n    let x = 100;\n    let y = 200;\n}\n"
    );
}

#[test]
fn test_perform_block_replacement_ranged_disambiguates() {
    let current = "a\nb\nc\nb\nd\n";
    let search = "b\n";
    let replace = "x\n";
    let result = perform_block_replacement(
        current,
        search,
        replace,
        Some(ReadFileRange {
            start_line: 4,
            end_line: 4,
        }),
    )
    .unwrap();
    assert_eq!(result, "a\nb\nc\nx\nd\n");
}

#[test]
fn test_perform_block_replacement_ranged_stale_range_fails() {
    let current = "a\nb\nc\nb\nd\n";
    let search = "b\n";
    let replace = "x\n";
    let err = perform_block_replacement(
        current,
        search,
        replace,
        Some(ReadFileRange {
            start_line: 5,
            end_line: 5,
        }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("none are fully inside"));
    assert!(err.to_string().contains("lines 2, 4"));
}

#[test]
fn test_perform_block_replacement_ranged_still_ambiguous_fails() {
    let current = "a\nb\nc\nb\nd\n";
    let search = "b\n";
    let replace = "x\n";
    let err = perform_block_replacement(
        current,
        search,
        replace,
        Some(ReadFileRange {
            start_line: 1,
            end_line: 5,
        }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
    assert!(err.to_string().contains("requested range 1-5"));
}

#[test]
fn test_perform_block_replacement_accepts_literal_newline_escape_fallback() {
    let current = "pub fn preview_change_reason(delayed_change: bool) -> &'static str {\n    if delayed_change {\n        \"immediate\"\n    } else {\n        \"immediate\"\n    }\n}\n";
    let search = "if delayed_change {\\n        \"immediate\"\\n    } else {\\n        \"immediate\"\\n    }";
    let replace = "if delayed_change {\\n        \"scheduled_at_period_end\"\\n    } else {\\n        \"immediate\"\\n    }";
    let result = perform_block_replacement(
        current,
        search,
        replace,
        Some(ReadFileRange {
            start_line: 1,
            end_line: 7,
        }),
    )
    .unwrap();

    assert!(result.contains("\"scheduled_at_period_end\""));
    assert!(!result.contains("\\n"));
}

#[test]
fn resolve_file_patches_rejects_out_of_root_targets() {
    let root = tempdir().expect("tempdir");
    let file_patches = vec![FilePatch {
        path: "../escape.txt".to_string(),
        operation: PatchOperation::Update,
        hunks: vec![Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![PatchLine::Context("safe".to_string())],
        }],
    }];

    let error = resolve_file_patches(root.path(), root.path(), &file_patches).expect_err("reject");
    assert!(
        error
            .to_string()
            .contains("Parent directory traversal is not allowed")
    );
}

#[test]
fn apply_hunks_rejects_ambiguous_matches() {
    let error = apply_hunks(
        "same\nline\nsame\nline",
        &[Hunk {
            old_start: 99,
            old_count: 2,
            new_start: 99,
            new_count: 2,
            lines: vec![
                PatchLine::Context("same".to_string()),
                PatchLine::Remove("line".to_string()),
                PatchLine::Add("updated".to_string()),
            ],
        }],
    )
    .expect_err("ambiguous hunk");

    assert!(error.to_string().contains("Patch hunk is ambiguous"));
}

#[test]
fn apply_hunks_rejects_malformed_line_counts() {
    let error = apply_hunks(
        "old",
        &[Hunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 1,
            lines: vec![PatchLine::Remove("old".to_string())],
        }],
    )
    .expect_err("malformed hunk");

    assert!(error.to_string().contains("Malformed hunk"));
}
