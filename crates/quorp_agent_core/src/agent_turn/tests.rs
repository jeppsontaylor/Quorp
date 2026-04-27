use super::*;

fn sample_turn_json() -> &'static str {
    r#"{"assistant_message":"ok","actions":[{"ReadFile":{"path":"src/main.rs"}}],"task_updates":[],"memory_updates":[],"requested_mode_change":null,"verifier_plan":null}"#
}

#[test]
fn parses_raw_json_turn() {
    let parsed = parse_agent_turn_response(sample_turn_json())
        .expect("parse")
        .expect("turn");
    assert_eq!(parsed.assistant_message, "ok");
    assert_eq!(parsed.actions.len(), 1);
    assert!(parsed.parse_warnings.is_empty());
}

#[test]
fn parses_flat_read_file_action_schema() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "Reading the suggested failing slice.",
                "actions": [
                    {
                        "action": "read_file",
                        "path": "src/round.rs",
                        "line_range": [769, 802]
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert!(!parsed.parse_warnings.is_empty());
    assert_eq!(
        parsed.actions[0],
        AgentAction::ReadFile {
            path: "src/round.rs".to_string(),
            range: Some(ReadFileRange {
                start_line: 769,
                end_line: 802,
            }),
        }
    );
}

#[test]
fn parses_snake_case_tagged_actions_inside_actions_array() {
    let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "",
                "actions": [
                    {"read_file": {"path": "src/lib.rs", "range": [4, 8]}},
                    {"replace_block": {"path": "src/lib.rs", "search": "old", "replace_with": "new", "line_range": [4, 8]}},
                    {"run_validation": {"plan": {"tests": ["lib::tests::smoke"]}}}
                ]
            }"#,
        )
        .expect("parse")
        .expect("turn");

    assert_eq!(parsed.actions.len(), 3);
    assert!(matches!(
        parsed.actions[0],
        AgentAction::ReadFile {
            range: Some(ReadFileRange {
                start_line: 4,
                end_line: 8
            }),
            ..
        }
    ));
    assert!(matches!(
        parsed.actions[1],
        AgentAction::ReplaceBlock {
            range: Some(ReadFileRange {
                start_line: 4,
                end_line: 8
            }),
            ..
        }
    ));
    assert!(matches!(
        parsed.actions[2],
        AgentAction::RunValidation { .. }
    ));
    assert!(!parsed.parse_warnings.is_empty());
}

#[test]
fn repairs_json_like_unquoted_keys_for_remote_models() {
    let parsed = parse_agent_turn_response(
        r#"{
                actions: [
                    {
                        ModifyToml: {
                            path: Cargo.toml,
                            expected_hash: d90cc110472497e2,
                            operations: [
                                {
                                    op: set_dependency,
                                    table: dependencies,
                                    name: chrono,
                                    version: "0.4"
                                }
                            ]
                        }
                    }
                ],
                assistant_message: patching
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert!(
        parsed
            .parse_warnings
            .iter()
            .any(|warning| { warning.contains("Repaired JSON-like model object syntax") })
    );
    assert_eq!(parsed.assistant_message, "patching");
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } if path == "Cargo.toml"
            && expected_hash == "d90cc110472497e2"
            && matches!(
                &operations[0],
                crate::agent_protocol::TomlEditOperation::SetDependency {
                    table,
                    name,
                    version,
                    ..
                } if table == "dependencies"
                    && name == "chrono"
                    && version.as_deref() == Some("0.4")
            )
    ));
}

#[test]
fn parses_preview_edit_tagged_and_flat_forms() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "",
                "actions": [
                    {
                        "preview_edit": {
                            "path": "src/lib.rs",
                            "edit": {
                                "replace_block": {
                                    "search_block": "old",
                                    "replace_block": "new",
                                    "range": [10, 12]
                                }
                            }
                        }
                    },
                    {
                        "action": "preview_edit",
                        "path": "src/lib.rs",
                        "patch": "@@ -1 +1 @@\n-old\n+new\n"
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(parsed.actions.len(), 2);
    assert!(matches!(
        parsed.actions[0],
        AgentAction::PreviewEdit {
            edit: PreviewEditPayload::ReplaceBlock {
                range: Some(ReadFileRange {
                    start_line: 10,
                    end_line: 12
                }),
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        parsed.actions[1],
        AgentAction::PreviewEdit {
            edit: PreviewEditPayload::ApplyPatch { .. },
            ..
        }
    ));
}

#[test]
fn parses_intent_edit_actions_and_preview_payloads() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "",
                "actions": [
                    {
                        "replace_range": {
                            "path": "src/lib.rs",
                            "range": [4, 6],
                            "content_hash": "0123456789abcdef",
                            "replacement": "new lines"
                        }
                    },
                    {
                        "ModifyToml": {
                            "path": "Cargo.toml",
                            "expected_hash": "fedcba9876543210",
                            "operations": [
                                {
                                    "table": "dependencies",
                                    "name": "chrono",
                                    "version": "0.4",
                                    "default-features": false
                                }
                            ]
                        }
                    },
                    {
                        "action": "preview_edit",
                        "path": "src/lib.rs",
                        "range": [10, 11],
                        "expected_hash": "aaaaaaaaaaaaaaaa",
                        "replacement": "line"
                    },
                    {
                        "modify_toml": {
                            "path": "Cargo.toml",
                            "operations": [
                                {
                                    "table": "dependencies",
                                    "name": "uuid",
                                    "version": "1"
                                }
                            ]
                        }
                    },
                    {
                        "apply_preview": {
                            "preview_id": "pv_abc123"
                        }
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert!(matches!(
        &parsed.actions[0],
        AgentAction::ReplaceRange {
            path,
            range,
            expected_hash,
            ..
        } if path == "src/lib.rs" && range.start_line == 4 && expected_hash == "0123456789abcdef"
    ));
    assert!(matches!(
        &parsed.actions[1],
        AgentAction::ModifyToml {
            path,
            operations,
            ..
        } if path == "Cargo.toml" && operations.len() == 1
    ));
    assert!(matches!(
        &parsed.actions[2],
        AgentAction::PreviewEdit {
            edit: PreviewEditPayload::ReplaceRange { range, .. },
            ..
        } if range.start_line == 10
    ));
    assert!(matches!(
        &parsed.actions[3],
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } if path == "Cargo.toml"
            && expected_hash == "not_specified_yet"
            && operations.len() == 1
    ));
    assert!(matches!(
        &parsed.actions[4],
        AgentAction::ApplyPreview { preview_id } if preview_id == "pv_abc123"
    ));
}

#[test]
fn parses_line_oriented_modify_toml_with_placeholder_hash_when_missing() {
    let missing_hash = parse_agent_turn_response(
            r#"modify_toml Cargo.toml [operations [{"type":"set_dependency","table":"dependencies","name":"chrono","version":"0.4"}]]"#,
        )
        .expect("parse")
        .expect("turn");

    assert!(matches!(
        &missing_hash.actions[0],
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } if path == "Cargo.toml"
            && expected_hash == "not_specified_yet"
            && operations.len() == 1
    ));

    let parsed = parse_agent_turn_response(
            r#"modify_toml Cargo.toml expected_hash=d90cc110472497e2 [operations [{"type":"set_dependency","name":"chrono","version":"0.4"}]]"#,
        )
        .expect("parse")
        .expect("turn");

    match &parsed.actions[0] {
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => {
            assert_eq!(path, "Cargo.toml");
            assert_eq!(expected_hash, "d90cc110472497e2");
            assert!(matches!(
                &operations[0],
                crate::agent_protocol::TomlEditOperation::SetDependency {
                    table,
                    name,
                    ..
                } if table == "dependencies" && name == "chrono"
            ));
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn parses_modify_toml_dependency_name_aliases() {
    let parsed = parse_agent_turn_response(
        r#"{
                "actions": [
                    {
                        "modify_toml": {
                            "path": "Cargo.toml",
                            "expected_hash": "0123456789abcdef",
                            "operations": [
                                {"set_dependency": {"dependency_name": "chrono", "version": "0.4"}},
                                {"type": "set_dependency", "dependency": "uuid", "version": "1"},
                                {"set_dependency": {"rand": "0.8"}}
                            ]
                        }
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    match &parsed.actions[0] {
        AgentAction::ModifyToml { operations, .. } => {
            assert!(matches!(
                &operations[0],
                crate::agent_protocol::TomlEditOperation::SetDependency {
                    table,
                    name,
                    ..
                } if table == "dependencies" && name == "chrono"
            ));
            assert!(matches!(
                &operations[1],
                crate::agent_protocol::TomlEditOperation::SetDependency {
                    table,
                    name,
                    ..
                } if table == "dependencies" && name == "uuid"
            ));
            assert!(matches!(
                &operations[2],
                crate::agent_protocol::TomlEditOperation::SetDependency {
                    table,
                    name,
                    version,
                    ..
                } if table == "dependencies" && name == "rand" && version.as_deref() == Some("0.8")
            ));
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn parses_flat_run_command_action_schema() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "Rerunning the fast loop.",
                "actions": [
                    {
                        "action": "run_command",
                        "command": "cargo test --quiet --lib round::tests::",
                        "timeout_ms": 30000
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::RunCommand {
            command: "cargo test --quiet --lib round::tests::".to_string(),
            timeout_ms: 30000,
        }
    );
}

#[test]
fn parses_tagged_action_with_extra_metadata() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search_block": "old",
                            "replace_block": "new"
                        },
                        "range": [243, 246]
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert!(!parsed.parse_warnings.is_empty());
    assert_eq!(
        parsed.actions[0],
        AgentAction::ReplaceBlock {
            path: "src/round.rs".to_string(),
            search_block: "old".to_string(),
            replace_block: "new".to_string(),
            range: Some(ReadFileRange {
                start_line: 243,
                end_line: 246,
            }),
        }
    );
}

#[test]
fn parses_tagged_action_alias_fields() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search_block": "old",
                            "replace_with": "new"
                        }
                    },
                    {
                        "ReadFile": {
                            "path": "src/round.rs",
                            "range": [160, 250]
                        }
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::ReplaceBlock {
            path: "src/round.rs".to_string(),
            search_block: "old".to_string(),
            replace_block: "new".to_string(),
            range: None,
        }
    );
    assert_eq!(
        parsed.actions[1],
        AgentAction::ReadFile {
            path: "src/round.rs".to_string(),
            range: Some(ReadFileRange {
                start_line: 160,
                end_line: 250,
            }),
        }
    );
}

#[test]
fn parses_ranged_replace_block_forms() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "patching",
                "actions": [
                    {
                        "ReplaceBlock": {
                            "path": "src/round.rs",
                            "search": "old",
                            "new": "new",
                            "range": [170, 220]
                        }
                    },
                    {
                        "action": "replace_block",
                        "path": "src/round.rs",
                        "search": "other old",
                        "replace_with": "other new",
                        "line_range": [221, 240]
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::ReplaceBlock {
            path: "src/round.rs".to_string(),
            search_block: "old".to_string(),
            replace_block: "new".to_string(),
            range: Some(ReadFileRange {
                start_line: 170,
                end_line: 220,
            }),
        }
    );
    assert_eq!(
        parsed.actions[1],
        AgentAction::ReplaceBlock {
            path: "src/round.rs".to_string(),
            search_block: "other old".to_string(),
            replace_block: "other new".to_string(),
            range: Some(ReadFileRange {
                start_line: 221,
                end_line: 240,
            }),
        }
    );
}

#[test]
fn parses_read_only_repair_assistance_actions() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "Need anchors before patching.",
                "actions": [
                    {
                        "action": "explain_validation_failure",
                        "command": "cargo test round",
                        "output": "thread panicked at src/round.rs:42:5"
                    },
                    {
                        "action": "suggest_edit_anchors",
                        "path": "src/round.rs",
                        "range": [40, 52],
                        "hint": "round_duration"
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::ExplainValidationFailure {
            command: "cargo test round".to_string(),
            output: "thread panicked at src/round.rs:42:5".to_string(),
        }
    );
    assert_eq!(
        parsed.actions[1],
        AgentAction::SuggestEditAnchors {
            path: "src/round.rs".to_string(),
            range: Some(ReadFileRange {
                start_line: 40,
                end_line: 52,
            }),
            search_hint: Some("round_duration".to_string()),
        }
    );
}

#[test]
fn parses_snake_case_implementation_target_action_wrapper() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "Need target ranking.",
                "actions": [
                    {
                        "suggest_implementation_targets": {
                            "cmd": "cargo test issue_474",
                            "stderr": "error[E0432]: unresolved import `serde`",
                            "path": "tests/issues/issue_474.rs",
                            "line": 12
                        }
                    }
                ]
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::SuggestImplementationTargets {
            command: "cargo test issue_474".to_string(),
            output: "error[E0432]: unresolved import `serde`".to_string(),
            failing_path: Some("tests/issues/issue_474.rs".to_string()),
            failing_line: Some(12),
        }
    );
}

#[test]
fn parses_line_oriented_tool_actions() {
    let parsed = parse_agent_turn_response(
            "read_file src/round.rs range=[781, 813]\nrun_validation: tests(round::tests::test_duration_round_close_to_epoch)",
        )
        .expect("parse")
        .expect("turn");

    assert_eq!(parsed.actions.len(), 2);
    assert!(!parsed.parse_warnings.is_empty());
    assert_eq!(
        parsed.actions[0],
        AgentAction::ReadFile {
            path: "src/round.rs".to_string(),
            range: Some(ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
        }
    );
    assert_eq!(
        parsed.actions[1],
        AgentAction::RunValidation {
            plan: ValidationPlan {
                tests: vec!["round::tests::test_duration_round_close_to_epoch".to_string()],
                ..ValidationPlan::default()
            },
        }
    );
}

#[test]
fn parses_line_oriented_anchor_suggestion() {
    let parsed =
        parse_agent_turn_response("suggest_edit_anchors src/round.rs range=[40, 52] hint=span")
            .expect("parse")
            .expect("turn");

    assert_eq!(
        parsed.actions[0],
        AgentAction::SuggestEditAnchors {
            path: "src/round.rs".to_string(),
            range: Some(ReadFileRange {
                start_line: 40,
                end_line: 52,
            }),
            search_hint: Some("span".to_string()),
        }
    );
}

#[test]
fn parses_line_oriented_apply_patch_payloads() {
    let parsed = parse_agent_turn_response(
            "apply_patch Cargo.toml [{\"patch\":\"--- a/Cargo.toml\\n+++ b/Cargo.toml\\n@@ -1 +1 @@\\n-old\\n+new\\n\"}]",
        )
        .expect("parse")
        .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::ApplyPatch { path, patch }
            if path == "Cargo.toml" && patch.contains("@@ -1 +1 @@")
    ));
}

#[test]
fn parses_multiline_line_oriented_preview_edit_payload() {
    let parsed = parse_agent_turn_response(
            "preview_edit Cargo.toml patch=\"--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1 @@\n-old\n+new\n\"",
        )
        .expect("parse")
        .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::PreviewEdit {
            path,
            edit: PreviewEditPayload::ApplyPatch { patch }
        } if path == "Cargo.toml" && patch.contains("+new")
    ));
}

#[test]
fn line_oriented_parser_rejects_mixed_prose() {
    let parsed =
        parse_agent_turn_response("I will inspect first.\nread_file src/round.rs range=[781, 813]")
            .expect("parse");

    assert!(parsed.is_none());
}

#[test]
fn parses_fenced_json_turn() {
    let wrapped = format!("```json\n{}\n```", sample_turn_json());
    let parsed = parse_agent_turn_response(&wrapped)
        .expect("parse")
        .expect("turn");
    assert_eq!(parsed.assistant_message, "ok");
}

#[test]
fn parses_json_wrapped_in_explanatory_text() {
    let wrapped = format!(
        "I found the next action.\n{}\nThis should be executed next.",
        sample_turn_json()
    );
    let parsed = parse_agent_turn_response(&wrapped)
        .expect("parse")
        .expect("turn");
    assert_eq!(parsed.assistant_message, "ok");
}

#[test]
fn parses_json_with_trailing_prose() {
    let wrapped = format!(
        "{}\n\nI will inspect the workspace next.",
        sample_turn_json()
    );
    let parsed = parse_agent_turn_response(&wrapped)
        .expect("parse")
        .expect("turn");
    assert_eq!(parsed.assistant_message, "ok");
    assert!(!parsed.parse_warnings.is_empty());
}

#[test]
fn parses_json_with_trailing_fenced_text() {
    let wrapped = format!(
        "{}\n```text\nI will inspect the workspace next.\n```",
        sample_turn_json()
    );
    let parsed = parse_agent_turn_response(&wrapped)
        .expect("parse")
        .expect("turn");
    assert_eq!(parsed.assistant_message, "ok");
    assert!(!parsed.parse_warnings.is_empty());
}

#[test]
fn ignores_plain_text_without_json() {
    let parsed = parse_agent_turn_response("plain text only").expect("parse");
    assert!(parsed.is_none());
}

#[test]
fn incomplete_embedded_json_is_recoverable_error() {
    let error = parse_agent_turn_response(
            "I have the fix.\n```json\n{\"assistant_message\":\"patching\",\"actions\":[{\"ReadFile\":{\"path\":\"src/lib.rs\"}}]\n```",
        )
        .expect_err("incomplete embedded JSON should error");
    assert!(error.contains("EOF while parsing"));
}

#[test]
fn parses_deepseek_style_optional_metadata_leniently() {
    let parsed = parse_agent_turn_response(
            r#"{
                "assistant_message": "I will inspect the objective first.",
                "actions": [
                    {
                        "ReadFile": {
                            "path": "README.md"
                        }
                    }
                ],
                "task_updates": [
                    {
                        "status": "Read the objective file to understand the requirements and context for the challenge.",
                        "progress": "File read initiated."
                    }
                ],
                "memory_updates": [
                    {
                        "type": "FileContent",
                        "path": "README.md",
                        "content": "Objective summary."
                    }
                ],
                "requested_mode_change": null,
                "verifier_plan": null,
                "extra_field": {"ignored": true}
            }"#,
        )
        .expect("parse")
        .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert_eq!(parsed.task_updates.len(), 1);
    assert_eq!(parsed.task_updates[0].title, "File read initiated.");
    assert_eq!(parsed.task_updates[0].status, TaskStatus::Pending);
    assert_eq!(parsed.memory_updates.len(), 1);
    assert_eq!(parsed.memory_updates[0].kind, "FileContent");
    assert_eq!(parsed.memory_updates[0].path.as_deref(), Some("README.md"));
    assert!(!parsed.parse_warnings.is_empty());
}

#[test]
fn malformed_optional_metadata_does_not_drop_valid_actions() {
    let parsed = parse_agent_turn_response(
        r#"{
                "assistant_message": "valid",
                "actions": [{"ReadFile":{"path":"src/main.rs"}}],
                "task_updates": {"status":"bad"},
                "requested_mode_change": {"bad": true}
            }"#,
    )
    .expect("parse")
    .expect("turn");

    assert_eq!(parsed.actions.len(), 1);
    assert_eq!(parsed.assistant_message, "valid");
    assert!(!parsed.parse_warnings.is_empty());
}
