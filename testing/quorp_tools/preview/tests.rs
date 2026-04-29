use super::*;

#[test]
fn parse_line_replacement_shorthand_accepts_simple_two_line_patch() {
    let patch = "/foo\n+bar\n";
    let parsed = try_parse_line_replacement_shorthand(patch).expect("parse");
    assert_eq!(
        parsed,
        Some(LineReplacementShorthand {
            search: "foo".to_string(),
            replace: "bar".to_string(),
        })
    );
}

#[test]
fn normalize_single_file_hunk_patch_rewrites_hunk_only_input() {
    let (normalized, is_single_file) =
        normalize_single_file_hunk_patch("src/lib.rs", "@@ -1 +1 @@\n-old\n+new\n")
            .expect("normalize");
    assert!(is_single_file);
    assert_eq!(
        normalized.as_deref(),
        Some("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n")
    );
}

#[test]
fn normalize_single_file_hunk_patch_rejects_newline_paths() {
    let error =
        normalize_single_file_hunk_patch("src/lib.rs\n+++ b/other.rs", "@@ -1 +1 @@\n-old\n+new\n")
            .expect_err("reject newline path");

    assert!(error.to_string().contains("cannot contain newlines"));
}

#[test]
fn syntax_preflight_for_toml_reports_parser_acceptance() {
    let output = syntax_preflight_for_preview("Cargo.toml", "[package]\nname = \"demo\"\n");
    assert!(output.contains("syntax_preflight: passed"));
    assert!(output.contains("TOML parser accepted scratch content"));
}

#[test]
fn syntax_preflight_for_non_rust_files_is_unavailable() {
    let output = syntax_preflight_for_preview("notes.txt", "hello");
    assert!(output.contains("syntax_preflight: unavailable"));
}

#[test]
fn line_replacement_shorthand_updates_exact_line() {
    let mut content = "alpha\nbeta\n".to_string();
    let line_number =
        perform_line_replacement_shorthand(&mut content, "beta", "gamma").expect("replace");
    assert_eq!(line_number, 2);
    assert_eq!(content, "alpha\ngamma\n");
}

#[test]
fn line_replacement_shorthand_rejects_missing_search() {
    let mut content = "alpha\nbeta\n".to_string();
    let error =
        perform_line_replacement_shorthand(&mut content, "delta", "gamma").expect_err("reject");
    assert!(error.to_string().contains("found no lines"));
}

#[test]
fn preview_record_round_trips_in_cache() {
    let record = PreviewRecord {
        preview_id: String::new(),
        path: "src/lib.rs".to_string(),
        target_path: PathBuf::from("/tmp/quorp-test/src/lib.rs"),
        base_hash: "hash".to_string(),
        edit_kind: "replace_range".to_string(),
        updated_content: "new".to_string(),
        syntax_status: "passed".to_string(),
    };
    let preview_id = store_preview_record(record).expect("store");
    let loaded = load_preview_record(&preview_id).expect("load");
    assert_eq!(loaded.preview_id, preview_id);
    assert_eq!(loaded.path, "src/lib.rs");
}
