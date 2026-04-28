use super::*;
use crate::quorp::tui::ChatUiEvent;
use crate::quorp::tui::TuiEvent;
use quorp_tools::edit::{set_executable_bit, write_full_file};
use serde_json::json;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;

#[cfg(unix)]
static MCP_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn capture_tool_events(
    output: String,
    event_rx: std::sync::mpsc::Receiver<TuiEvent>,
) -> Vec<TuiEvent> {
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match event_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(event) => {
                let is_finished =
                    matches!(
                        event,
                        TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _))
                            if output.is_empty()
                    ) || matches!(event, TuiEvent::Chat(ChatUiEvent::CommandFinished(_, _)));
                events.push(event);
                if is_finished {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    events
}

#[cfg(unix)]
fn write_test_script(path: &Path, content: &str) {
    write_full_file(path, content).expect("write script");
    set_executable_bit(path).expect("chmod script");
}

#[cfg(unix)]
fn mcp_test_guard() -> std::sync::MutexGuard<'static, ()> {
    MCP_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(unix)]
fn write_mcp_config(root: &Path, server_name: &str, command: &Path) {
    let config_dir = root.join(".quorp");
    std::fs::create_dir_all(&config_dir).expect("mkdir config");
    std::fs::write(
        config_dir.join("agent.toml"),
        format!(
            "[[mcp_servers]]\nname = \"{server_name}\"\ncommand = \"{}\"\n",
            command.display()
        ),
    )
    .expect("write config");
}

#[test]
fn validation_failure_explanation_extracts_anchors_without_patch_advice() {
    let output = "---- round::tests::chrono stdout ----\nthread panicked at src/round.rs:42:5\nassertion failed: expected left == right";

    let rendered = render_validation_failure_explanation("cargo test chrono", output);

    assert!(rendered.contains("[explain_validation_failure]"));
    assert!(rendered.contains("round::tests::chrono"));
    assert!(rendered.contains("src/round.rs:42:5"));
    assert!(!rendered.contains("replace with"));
}

#[test]
fn validation_failure_explanation_prioritizes_errors_over_warning_anchors() {
    let output = "warning: unexpected `cfg` condition value: `bench`\n --> tests/noise.rs:10:1\nerror[E0432]: unresolved import `serde`\n --> Cargo.toml:1:1\n";

    let rendered = render_validation_failure_explanation("cargo test", output);

    assert!(rendered.contains("diagnostic_class: manifest_dependency_error"));
    assert!(rendered.contains("target_class: manifest"));
    assert!(rendered.contains("primary_anchor: --> Cargo.toml:1:1"));
    assert!(!rendered.contains("primary_anchor: --> tests/noise.rs:10:1"));
}

#[test]
fn implementation_target_suggestions_rank_manifest_dependency_errors() {
    let output = "error[E0432]: unresolved import `serde`\n --> src/lib.rs:2:5\n";

    let rendered = render_implementation_target_suggestions(
        "cargo test",
        output,
        Some("tests/issues/issue_474.rs"),
        Some(12),
    );

    assert!(rendered.contains("[suggest_implementation_targets]"));
    assert!(rendered.contains("diagnostic_class: manifest_dependency_error"));
    assert!(rendered.contains("required_next_target: Cargo.toml"));
    assert!(rendered.contains("reason: test_evidence_only"));
}

#[test]
fn edit_anchor_suggestions_warn_about_repeated_hints() {
    let source =
        "fn alpha() {}\nlet repeated = 1;\nlet unique_anchor_value = 2;\nlet repeated = 3;\n";

    let rendered = render_edit_anchor_suggestions(
        "src/lib.rs",
        source,
        Some(ReadFileRange {
            start_line: 1,
            end_line: 4,
        }),
        Some("let repeated"),
    );

    assert!(rendered.contains("[suggest_edit_anchors]"));
    assert!(rendered.contains("line 3: let unique_anchor_value = 2;"));
    assert!(rendered.contains("search_hint_occurrences: 2"));
    assert!(rendered.contains("ReplaceBlock with range"));
}

#[test]
fn diagnose_redundant_workspace_prefix_suggests_workspace_relative_path() {
    let root = tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("crates").join("reconciliation-core")).expect("mkdir");

    let suggested =
        diagnose_redundant_workspace_prefix(root.path(), "workspace/crates/reconciliation-core");

    assert_eq!(suggested.as_deref(), Some("crates/reconciliation-core"));
}

#[test]
fn apply_patch_task_applies_unified_diff_update() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "old\nkeep\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    let request_path = file
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    spawn_apply_patch_task(
        event_tx,
        0,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        request_path,
        "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n keep\n".to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "new\nkeep\n");
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("Applied unified diff patch")
    )));
}

#[test]
fn apply_patch_task_accepts_hunk_only_patch_for_explicit_path() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "one\ntwo\nthree\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        0,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n".to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "one\nTWO\nthree\n");
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("Applied single-file hunk patch")
                && line.contains("M target.txt")
    )));
}

#[test]
fn apply_patch_task_accepts_unique_line_replacement_shorthand() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "alpha\n    beta\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        0,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        "/beta\n+    gamma\n".to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "alpha\n    gamma\n");
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("Applied single-line replacement shorthand")
                && line.contains("line 2")
    )));
}

#[test]
fn apply_patch_task_rejects_ambiguous_line_replacement_shorthand() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "alpha\nbeta\nbeta\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        0,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        "/beta\n+gamma\n".to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "alpha\nbeta\nbeta\n");
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::Error(_, message))
            if message.contains("line replacement shorthand is ambiguous")
                && message.contains("lines 2, 3")
    )));
}

#[test]
fn preview_edit_replace_block_reports_unique_match_without_mutating() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "alpha\nbeta\ngamma\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_preview_edit_task(
        event_tx,
        0,
        PreviewEditTaskRequest {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            path: "target.txt".to_string(),
            edit: PreviewEditPayload::ReplaceBlock {
                search_block: "beta".to_string(),
                replace_block: "BETA".to_string(),
                range: None,
            },
            responder: None,
        },
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "alpha\nbeta\ngamma\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("would_apply: true")
                && line.contains("matching_line_numbers: 2")
    )));
}

#[test]
fn preview_edit_replace_block_reports_rust_syntax_preflight_without_mutating() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("src").join("lib.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    write_full_file(&file, "fn alpha() {\n    let value = 1;\n}\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_preview_edit_task(
        event_tx,
        0,
        PreviewEditTaskRequest {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            path: "src/lib.rs".to_string(),
            edit: PreviewEditPayload::ReplaceBlock {
                search_block: "let value = 1;".to_string(),
                replace_block: "let value = ;".to_string(),
                range: Some(ReadFileRange {
                    start_line: 1,
                    end_line: 3,
                }),
            },
            responder: None,
        },
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "fn alpha() {\n    let value = 1;\n}\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("would_apply: true")
                && line.contains("syntax_preflight: failed")
    )));
}

#[test]
fn preview_edit_replace_block_reports_ambiguity_without_mutating() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "alpha\nbeta\nbeta\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_preview_edit_task(
        event_tx,
        0,
        PreviewEditTaskRequest {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            path: "target.txt".to_string(),
            edit: PreviewEditPayload::ReplaceBlock {
                search_block: "beta".to_string(),
                replace_block: "BETA".to_string(),
                range: None,
            },
            responder: None,
        },
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "alpha\nbeta\nbeta\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("would_apply: false")
                && line.contains("matching_line_numbers: 2,3")
    )));
}

#[test]
fn preview_edit_apply_patch_dry_runs_without_mutating() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "one\ntwo\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_preview_edit_task(
        event_tx,
        0,
        PreviewEditTaskRequest {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            path: "target.txt".to_string(),
            edit: PreviewEditPayload::ApplyPatch {
                patch: "@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n".to_string(),
            },
            responder: None,
        },
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "one\ntwo\n");
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("would_apply: true")
                && line.contains("patch_form: single_file_hunk")
    )));
}

#[test]
fn preview_replace_range_returns_apply_preview_id_without_mutating() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "one\ntwo\nthree\n").expect("bootstrap");
    let hash = stable_content_hash("two");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_preview_edit_task(
        event_tx,
        0,
        PreviewEditTaskRequest {
            cwd: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            path: "target.txt".to_string(),
            edit: PreviewEditPayload::ReplaceRange {
                range: ReadFileRange {
                    start_line: 2,
                    end_line: 2,
                },
                expected_hash: hash,
                replacement: "TWO".to_string(),
            },
            responder: None,
        },
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "one\ntwo\nthree\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("would_apply: true")
                && line.contains("preview_id: pv_")
                && line.contains("ApplyPreview")
    )));
}

#[test]
fn apply_patch_task_supports_add_delete_and_move() {
    let root = tempdir().expect("tempdir");
    let moved_source = root.path().join("move_source.txt");
    let deleted = root.path().join("delete.txt");
    let untouched = root.path().join("untouched.txt");
    write_full_file(&moved_source, "before").expect("bootstrap move");
    write_full_file(&deleted, "delete me").expect("bootstrap delete");
    write_full_file(&untouched, "stay put").expect("bootstrap untouched");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    let request_path = moved_source
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    spawn_apply_patch_task(
        event_tx,
        1,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        request_path,
        concat!(
            "--- /dev/null\n",
            "+++ b/added.txt\n",
            "@@ -0,0 +1,2 @@\n",
            "+hello\n",
            "+world\n",
            "--- a/delete.txt\n",
            "+++ /dev/null\n",
            "@@ -1 +0,0 @@\n",
            "-delete me\n",
            "rename from move_source.txt\n",
            "rename to moved.txt\n",
            "--- a/move_source.txt\n",
            "+++ b/moved.txt\n",
            "@@ -1 +1 @@\n",
            "-before\n",
            "+after\n"
        )
        .to_string(),
        None,
    );
    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(root.path().join("added.txt")).expect("read added"),
        "hello\nworld\n"
    );
    assert!(!deleted.exists());
    assert!(!moved_source.exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("moved.txt")).expect("read moved"),
        "after"
    );
    assert_eq!(
        std::fs::read_to_string(&untouched).expect("read untouched"),
        "stay put"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("A added.txt")
                && line.contains("D delete.txt")
                && line.contains("R move_source.txt -> moved.txt")
    )));
}

#[test]
fn apply_patch_task_rejects_malformed_hunks() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "old").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        2,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        "--- a/target.txt\n+++ b/target.txt\n@@ -1,2 +1,1 @@\n-old\n".to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("Malformed hunk")
        )),
        "{events:#?}"
    );
}

#[test]
fn apply_patch_task_supports_multiple_hunks_in_one_file() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file(&file, "one\ntwo\nthree\nfour\n").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        3,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        concat!(
            "--- a/target.txt\n",
            "+++ b/target.txt\n",
            "@@ -1,2 +1,2 @@\n",
            " one\n",
            "-two\n",
            "+TWO\n",
            "@@ -3,2 +3,2 @@\n",
            " three\n",
            "-four\n",
            "+FOUR\n",
        )
        .to_string(),
        None,
    );

    let _events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "one\nTWO\nthree\nFOUR\n");
}

#[test]
fn apply_patch_task_preserves_missing_trailing_newline() {
    let root = tempdir().expect("tempdir");
    let file = root.path().join("target.txt");
    write_full_file_allow_create(&file, "one\ntwo").expect("bootstrap");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        4,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "target.txt".to_string(),
        concat!(
            "--- a/target.txt\n",
            "+++ b/target.txt\n",
            "@@ -1,2 +1,2 @@\n",
            " one\n",
            "-two\n",
            "+three\n",
            "\\ No newline at end of file\n",
        )
        .to_string(),
        None,
    );

    let _events = capture_tool_events(String::new(), event_rx);
    let content = std::fs::read_to_string(&file).expect("read");
    assert_eq!(content, "one\nthree");
}

#[test]
fn apply_patch_task_accepts_placeholder_path_for_multi_file_diff() {
    let root = tempdir().expect("tempdir");
    let source = root.path().join("move_source.txt");
    let deleted = root.path().join("delete.txt");
    write_full_file(&source, "before\n").expect("bootstrap source");
    write_full_file(&deleted, "remove\n").expect("bootstrap deleted");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_apply_patch_task(
        event_tx,
        5,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "placeholder.txt".to_string(),
        concat!(
            "--- /dev/null\n",
            "+++ b/added.txt\n",
            "@@ -0,0 +1 @@\n",
            "+hello\n",
            "--- a/delete.txt\n",
            "+++ /dev/null\n",
            "@@ -1 +0,0 @@\n",
            "-remove\n",
            "rename from move_source.txt\n",
            "rename to moved.txt\n",
            "--- a/move_source.txt\n",
            "+++ b/moved.txt\n",
            "@@ -1 +1 @@\n",
            "-before\n",
            "+after\n",
        )
        .to_string(),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert_eq!(
        std::fs::read_to_string(root.path().join("added.txt")).expect("read added"),
        "hello\n"
    );
    assert!(!deleted.exists());
    assert!(!source.exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("moved.txt")).expect("read moved"),
        "after\n"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
            if line.contains("A added.txt")
                && line.contains("D delete.txt")
                && line.contains("R move_source.txt -> moved.txt")
    )));
}

#[test]
fn rollback_session_worktree_restores_all_touched_files() {
    let root = tempdir().expect("tempdir");
    let existing = root.path().join("existing.txt");
    let created = root.path().join("created.txt");
    write_full_file(&existing, "before").expect("bootstrap");
    stash_file_for_rollback(77, &existing);
    stash_file_for_rollback(77, &created);

    write_full_file(&existing, "after").expect("mutate existing");
    write_full_file_allow_create(&created, "new").expect("create new");

    rollback_session_worktree(77);

    assert_eq!(
        std::fs::read_to_string(&existing).expect("read existing"),
        "before"
    );
    assert!(!created.exists());
}

#[test]
fn render_mcp_tool_result_formats_structured_content() {
    let result = crate::quorp::tui::mcp_client::CallToolResult {
        content: vec![
            crate::quorp::tui::mcp_client::CallToolResultContent::Text {
                text: "hello".to_string(),
            },
            crate::quorp::tui::mcp_client::CallToolResultContent::Image {
                data: "aGVsbG8=".to_string(),
                mime_type: "image/png".to_string(),
            },
            crate::quorp::tui::mcp_client::CallToolResultContent::Resource {
                resource: json!({"uri":"file:///tmp/demo","kind":"resource"}),
            },
        ],
        is_error: Some(false),
    };

    let rendered = render_mcp_tool_result("demo", "inspect", &result).expect("render");
    assert!(rendered.contains("MCP demo/inspect"));
    assert!(rendered.contains("hello"));
    assert!(rendered.contains("[image result]"));
    assert!(rendered.contains("[resource result]"));
}

#[test]
fn render_mcp_tool_result_surfaces_tool_errors() {
    let result = crate::quorp::tui::mcp_client::CallToolResult {
        content: vec![crate::quorp::tui::mcp_client::CallToolResultContent::Text {
            text: "boom".to_string(),
        }],
        is_error: Some(true),
    };

    let error = render_mcp_tool_result("demo", "inspect", &result).expect_err("tool error");
    assert!(error.to_string().contains("returned an error"));
}

#[cfg(unix)]
#[test]
fn mcp_call_task_reports_missing_server_configuration() {
    let root = tempdir().expect("tempdir");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_mcp_call_task(
        event_tx,
        10,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "missing".to_string(),
        "echo".to_string(),
        json!({"value":"hi"}),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert!(events.iter().any(|event| matches!(
        event,
        TuiEvent::Chat(ChatUiEvent::Error(_, message))
            if message.contains("not configured")
    )));
}

#[cfg(unix)]
#[test]
fn mcp_call_task_executes_stdio_server_tool() {
    let _guard = mcp_test_guard();
    let root = tempdir().expect("tempdir");
    let script = root.path().join("fake-mcp.sh");
    write_test_script(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"1.0.0"}}}'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"tool ok"}]}}'
      exit 0
      ;;
  esac
done
"#,
    );
    write_mcp_config(root.path(), "fake", &script);

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_mcp_call_task(
        event_tx,
        11,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "fake".to_string(),
        "echo".to_string(),
        json!({"value":"hi"}),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("MCP fake/echo") && line.contains("tool ok")
        )),
        "events: {events:?}"
    );
}

#[cfg(unix)]
#[test]
fn mcp_call_task_surfaces_server_errors() {
    let _guard = mcp_test_guard();
    let root = tempdir().expect("tempdir");
    let script = root.path().join("fake-mcp-error.sh");
    write_test_script(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake-mcp","version":"1.0.0"}}}'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}'
      exit 0
      ;;
  esac
done
"#,
    );
    write_mcp_config(root.path(), "fake", &script);

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_mcp_call_task(
        event_tx,
        12,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "fake".to_string(),
        "echo".to_string(),
        json!({"value":"hi"}),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("boom")
        )),
        "events: {events:?}"
    );
}

#[cfg(unix)]
#[test]
fn mcp_call_task_handles_malformed_json_rpc_responses() {
    let _guard = mcp_test_guard();
    let root = tempdir().expect("tempdir");
    let script = root.path().join("fake-mcp-malformed.sh");
    write_test_script(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' 'not json'
      exit 0
      ;;
  esac
done
"#,
    );
    write_mcp_config(root.path(), "fake", &script);

    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_mcp_call_task(
        event_tx,
        13,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "fake".to_string(),
        "echo".to_string(),
        json!({"value":"hi"}),
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::Error(_, message))
                if message.contains("mcp_call_tool")
        )),
        "events: {events:?}"
    );
}

#[test]
fn search_text_task_returns_ranked_matches() {
    let root = tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
    std::fs::write(
        root.path().join("src/lib.rs"),
        "fn render_agent_turn_text() {}\n",
    )
    .expect("write");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_search_text_task(
        event_tx,
        7,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "render_agent_turn_text".to_string(),
        4,
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let saw_match = events.iter().any(|event| {
        matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("src/lib.rs:1")
                    && line.contains("render_agent_turn_text")
        )
    });
    assert!(saw_match, "expected formatted search hit in {events:?}");
}

#[test]
fn search_symbols_task_returns_symbol_hits() {
    let root = tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub struct RepoCapsule;\npub fn render_repo_capsule() {}\n",
    )
    .expect("write");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_search_symbols_task(
        event_tx,
        8,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        "RepoCapsule".to_string(),
        4,
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let saw_symbol = events.iter().any(|event| {
        matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("struct RepoCapsule")
        )
    });
    assert!(saw_symbol, "expected formatted symbol hit in {events:?}");
}

#[test]
fn repo_capsule_task_reports_workspace_members_and_focus_files() {
    let root = tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["crates/quorp"]

[package]
name = "quorp"
"#,
    )
    .expect("write cargo");
    std::fs::create_dir_all(root.path().join("src")).expect("mkdir");
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn render_agent_turn_text() {}\n",
    )
    .expect("write lib");
    let (event_tx, event_rx) = std::sync::mpsc::sync_channel(16);
    spawn_repo_capsule_task(
        event_tx,
        9,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        Some("render_agent_turn_text".to_string()),
        4,
        None,
    );

    let events = capture_tool_events(String::new(), event_rx);
    let saw_capsule = events.iter().any(|event| {
        matches!(
            event,
            TuiEvent::Chat(ChatUiEvent::CommandOutput(_, line))
                if line.contains("members: crates/quorp")
                    || line.contains("focus files:")
                    || line.contains("focus symbols:")
        )
    });
    assert!(saw_capsule, "expected repo capsule output in {events:?}");
}

#[test]
fn agent_tools_find_files_fallback_uses_ignore_walk() {
    let root = tempfile::tempdir().expect("root");
    std::fs::create_dir_all(root.path().join("src/bin")).expect("dirs");
    std::fs::write(root.path().join("src/lib.rs"), "").expect("lib");
    std::fs::write(root.path().join("src/bin/tool.rs"), "").expect("tool");
    let mut config = crate::quorp::tui::agent_context::AgentConfig::default();
    config.agent_tools.enabled = true;
    config.agent_tools.fd.command = "definitely-missing-fd".to_string();

    let output = find_files_with_config(root.path(), "tool", 10, &config).expect("find");
    assert!(output.contains("backend: ignore_walk"));
    assert!(output.contains("src/bin/tool.rs"));
}

#[test]
fn agent_tools_cargo_diagnostics_parse_json_records() {
    let output = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value `x` in this scope","code":{"code":"E0425"},"spans":[{"file_name":"src/lib.rs","line_start":7,"column_start":13,"is_primary":true}]}}"#;
    let diagnostics = parse_cargo_json_diagnostics(output, 10);
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("error[E0425]"));
    assert!(diagnostics[0].contains("src/lib.rs:7:13"));
    assert!(diagnostics[0].contains("cannot find value"));
}
