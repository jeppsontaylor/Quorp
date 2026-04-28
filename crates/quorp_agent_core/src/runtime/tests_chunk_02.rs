#[test]
fn premature_manifest_validation_becomes_exact_preview() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec![
                "src/features/serde/de_owned.rs".to_string(),
                "tests/issues/issue_474.rs".to_string(),
            ],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:6 | assertion unresolved imports/crates: chrono, uuid | diagnostic_class manifest_dependency_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(6),
                assertion_excerpt: Some("unresolved imports/crates: chrono, uuid".to_string()),
                diagnostic_class: Some("manifest_dependency_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 1,
            end_line: 30,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.implementation_target_lease = Some("Cargo.toml".to_string());
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::RunCommand {
            command: "cargo test --quiet --features serde --test issues issue_474".to_string(),
            timeout_ms: 120_000,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    match &turn.actions[0] {
        AgentAction::PreviewEdit {
            path,
            edit:
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                },
        } => {
            assert_eq!(path, "Cargo.toml");
            assert_eq!(expected_hash, "f543f6a8e32e1f38");
            assert_eq!(operations.len(), 2);
            assert!(operations.iter().all(|operation| matches!(
                operation,
                crate::agent_protocol::TomlEditOperation::SetDependency { features, .. }
                    if features.as_slice() == ["serde"]
            )));
        }
        action => panic!("unexpected action {action:?}"),
    }
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Replaced premature manifest validation") })
    );
}

#[test]
fn direct_manifest_replace_block_becomes_exact_preview() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec![
                "src/features/serde/de_owned.rs".to_string(),
                "tests/issues/issue_474.rs".to_string(),
            ],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:6 | assertion unresolved imports/crates: chrono, uuid | diagnostic_class manifest_dependency_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(6),
                assertion_excerpt: Some("unresolved imports/crates: chrono, uuid".to_string()),
                diagnostic_class: Some("manifest_dependency_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 1,
            end_line: 30,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.implementation_target_lease = Some("Cargo.toml".to_string());
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ReplaceBlock {
            path: "Cargo.toml".to_string(),
            search_block: "[dev-dependencies]\n".to_string(),
            replace_block: "[dev-dependencies]\nchrono = \"0.4\"\nuuid = \"0.8\"\n".to_string(),
            range: None,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    match &turn.actions[0] {
        AgentAction::PreviewEdit {
            path,
            edit:
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                },
        } => {
            assert_eq!(path, "Cargo.toml");
            assert_eq!(expected_hash, "f543f6a8e32e1f38");
            assert_eq!(operations.len(), 2);
            assert!(operations.iter().all(|operation| matches!(
                operation,
                crate::agent_protocol::TomlEditOperation::SetDependency { features, .. }
                    if features.as_slice() == ["serde"]
            )));
        }
        action => panic!("unexpected action {action:?}"),
    }
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Replaced direct or redundant manifest edit") }),
        "warnings: {:?}",
        turn.parse_warnings
    );
}

#[test]
fn malformed_manifest_preview_json_becomes_exact_preview() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec![
                "src/features/serde/de_owned.rs".to_string(),
                "tests/issues/issue_474.rs".to_string(),
            ],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:6 | assertion unresolved imports/crates: chrono, uuid | diagnostic_class manifest_dependency_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(6),
                assertion_excerpt: Some("unresolved imports/crates: chrono, uuid".to_string()),
                diagnostic_class: Some("manifest_dependency_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 1,
            end_line: 30,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.implementation_target_lease = Some("Cargo.toml".to_string());
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );

    let turn = maybe_repair_manifest_turn_parse_error(
        "Structured agent turn `actions` field was invalid: missing field `edit`",
        &state,
    )
    .expect("manifest preview parse recovery");

    match &turn.actions[0] {
        AgentAction::PreviewEdit {
            path,
            edit:
                PreviewEditPayload::ModifyToml {
                    expected_hash,
                    operations,
                },
        } => {
            assert_eq!(path, "Cargo.toml");
            assert_eq!(expected_hash, "f543f6a8e32e1f38");
            assert_eq!(operations.len(), 2);
        }
        action => panic!("unexpected action {action:?}"),
    }
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Recovered malformed manifest PreviewEdit JSON") })
    );
}

#[test]
fn normalizes_write_locked_manifest_turn_missing_path_and_hash() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["tests/issues/issue_474.rs".to_string()],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("repair manifest support".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(18),
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied"
                        .to_string(),
                ),
                diagnostic_class: Some("manifest_feature_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 10,
            end_line: 42,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    state.sync_benchmark_repair_state_to_ledger();

    let normalized = maybe_normalize_write_locked_manifest_turn_content(
            r#"{"assistant_message":"","actions":[{"ModifyToml":{"operations":[{"op":"set_dependency","table":"dev-dependencies","name":"chrono","version":"0.4","features":["serde"]}]}}]}"#,
            &state,
        )
        .expect("normalized turn");

    let parsed = parse_agent_turn_response(&normalized)
        .expect("parse")
        .expect("turn");
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } if path == "Cargo.toml"
            && expected_hash == "f543f6a8e32e1f38"
            && operations.len() == 1
    ));
}

#[test]
fn normalizes_write_locked_manifest_preview_edit_missing_path_and_hash() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["tests/issues/issue_474.rs".to_string()],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("repair manifest support".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(18),
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied"
                        .to_string(),
                ),
                diagnostic_class: Some("manifest_feature_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 10,
            end_line: 42,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    state.sync_benchmark_repair_state_to_ledger();

    let normalized = maybe_normalize_write_locked_manifest_turn_content(
            r#"{"assistant_message":"","actions":[{"PreviewEdit":{"edit":{"modify_toml":{"operations":[{"op":"set_dependency","table":"dev-dependencies","name":"chrono","version":"0.4","features":["serde"]}]}}}}]}"#,
            &state,
        )
        .expect("normalized turn");

    let parsed = parse_agent_turn_response(&normalized)
        .expect("parse")
        .expect("turn");
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::PreviewEdit {
            path,
            edit: PreviewEditPayload::ModifyToml {
                expected_hash,
                operations,
            },
        } if path == "Cargo.toml"
            && expected_hash == "f543f6a8e32e1f38"
            && operations.len() == 1
    ));
}

#[test]
fn normalizes_write_locked_manifest_apply_preview_placeholder_id() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["tests/issues/issue_474.rs".to_string()],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec![
                "Cargo.toml".to_string(),
                "src/features/serde/de_owned.rs".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("repair manifest support".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(18),
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied"
                        .to_string(),
                ),
                diagnostic_class: Some("manifest_feature_error".to_string()),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 10,
            end_line: 42,
        }),
        implementation_suggested_range: None,
        last_owner_slice: Some(OwnerSliceRecord {
            path: "Cargo.toml".to_string(),
            requested_range: None,
            honored_range: None,
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some("[dev-dependencies]\n".to_string()),
        }),
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.last_preview_id = Some("pv_manifest".to_string());
    state.agent_repair_memory.preview_origin = Some("write_locked_manifest".to_string());
    state.agent_repair_memory.scorecard.preview_created_count = 1;
    state.agent_repair_memory.scorecard.apply_preview_count = 0;
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dev-dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    state.sync_benchmark_repair_state_to_ledger();

    let normalized = maybe_normalize_write_locked_manifest_turn_content(
        r#"{"assistant_message":"","actions":[{"ApplyPreview":{}}]}"#,
        &state,
    )
    .expect("normalized turn");

    let parsed = parse_agent_turn_response(&normalized)
        .expect("parse")
        .expect("turn");
    assert!(matches!(
        &parsed.actions[0],
        AgentAction::ApplyPreview { preview_id } if preview_id == "pv_manifest"
    ));
}

#[test]
fn canonical_action_record_normalizes_validation_aliases() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec!["cargo test --quiet round::tests::chrono".to_string()],
        ..BenchmarkCaseLedger::default()
    };
    let action = AgentAction::RunCommand {
        command: " cargo   test --quiet round::tests::chrono ".to_string(),
        timeout_ms: 30_000,
    };

    let record = canonical_action_record(7, &action, Some(&ledger));

    assert_eq!(record.step, 7);
    assert_eq!(record.kind, "RunValidation");
    assert!(record.validation_like);
    assert!(record.signature.starts_with("validate:"));
}

#[test]
fn vague_fast_loop_command_is_canonicalized_to_known_command() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet --lib round::tests::test_duration".to_string(),
        ],
        ..BenchmarkCaseLedger::default()
    };
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::RunCommand {
            command: "the fast loop to validate the current implementation.".to_string(),
            timeout_ms: 30_000,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunCommand { command, .. }
            if command == "cargo test --quiet --lib round::tests::test_duration"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| warning.contains("Canonicalized vague validation command"))
    );
}

#[test]
fn cli_shaped_run_validation_is_canonicalized_to_known_fast_loop() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet --features serde --test issues issue_474".to_string(),
        ],
        ..BenchmarkCaseLedger::default()
    };
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::RunValidation {
            plan: ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: false,
                tests: vec!["--quiet --features serde --test issues issue_474".to_string()],
                custom_commands: Vec::new(),
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunValidation { plan }
            if plan.custom_commands == vec!["cargo test --quiet --features serde --test issues issue_474".to_string()]
                && plan.tests.is_empty()
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| warning.contains("Canonicalized CLI-shaped RunValidation"))
    );
}

#[test]
fn selector_run_validation_is_canonicalized_to_known_fast_loop() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet --features serde --test issues issue_474".to_string(),
        ],
        ..BenchmarkCaseLedger::default()
    };
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::RunValidation {
            plan: ValidationPlan {
                fmt: false,
                clippy: false,
                workspace_tests: false,
                tests: vec!["issue_474".to_string()],
                custom_commands: Vec::new(),
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunValidation { plan }
            if plan.custom_commands == vec!["cargo test --quiet --features serde --test issues issue_474".to_string()]
                && plan.tests.is_empty()
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| warning.contains("Canonicalized RunValidation"))
    );
}

#[test]
fn selector_run_command_is_canonicalized_to_known_fast_loop() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet --features serde --test issues issue_474".to_string(),
        ],
        ..BenchmarkCaseLedger::default()
    };
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::RunCommand {
            command: "cargo test --quiet --test issues issue_474".to_string(),
            timeout_ms: 30_000,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunCommand { command, .. }
            if command == "cargo test --quiet --features serde --test issues issue_474"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| warning.contains("Canonicalized selector validation command"))
    );
}

#[test]
fn bare_exact_fast_loop_does_not_append_failing_test_names() {
    let ledger = BenchmarkCaseLedger {
            fast_loop_commands: vec![
                "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact"
                    .to_string(),
            ],
            validation_details: BenchmarkValidationDetails {
                failing_test_names: vec![
                    "axolotlsay_edit_existing".to_string(),
                    "create-release".to_string(),
                ],
                ..BenchmarkValidationDetails::default()
            },
            ..BenchmarkCaseLedger::default()
        };

    assert_eq!(
        recommended_fast_loop_rerun_command(&ledger).as_deref(),
        Some(
            "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact"
        )
    );
}

#[test]
fn timeout_fast_loop_recommendation_uses_canonical_command() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet -p axum --lib --features headers routing::tests::".to_string(),
        ],
        named_tests: vec![
            "(Fallback::Custom(...), nesting_router_with_fallbacks_panics)".to_string(),
        ],
        last_validation_failure: Some("assertion [Command timed out]".to_string()),
        ..BenchmarkCaseLedger::default()
    };

    assert_eq!(
        recommended_fast_loop_rerun_command(&ledger).as_deref(),
        Some("cargo test --quiet -p axum --lib --features headers routing::tests::")
    );
}

#[test]
fn benchmark_fast_loop_dispatch_raises_short_timeout() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        fast_loop_commands: vec!["cargo test --quiet compile_intermediates".to_string()],
        named_tests: vec!["compile_intermediates".to_string()],
        ..BenchmarkCaseLedger::default()
    });
    let executor = RecordingToolExecutor::new(vec![Ok("ok".to_string())]);
    let sink = NoopEventSink;
    let mut transcript = Vec::new();

    futures::executor::block_on(dispatch_action(
        1,
        &mut state,
        AgentAction::RunCommand {
            command: "cargo test --quiet compile_intermediates".to_string(),
            timeout_ms: 30_000,
        },
        &request,
        &executor,
        &sink,
        &mut transcript,
    ))
    .expect("dispatch action");

    let actions = executor.executed_actions();
    assert!(matches!(
        actions.first(),
        Some(AgentAction::RunCommand { timeout_ms, .. }) if *timeout_ms == 120_000
    ));
}

#[test]
fn subset_fast_loop_command_is_canonicalized_to_bare_exact_command() {
    let ledger = BenchmarkCaseLedger {
            fast_loop_commands: vec![
                "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact"
                    .to_string(),
            ],
            validation_details: BenchmarkValidationDetails {
                failing_test_names: vec![
                    "axolotlsay_edit_existing".to_string(),
                    "create-release".to_string(),
                ],
                ..BenchmarkValidationDetails::default()
            },
            ..BenchmarkCaseLedger::default()
        };
    let mut turn = AgentTurnResponse {
            assistant_message: String::new(),
            actions: vec![AgentAction::RunCommand {
                command: "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact axolotlsay_edit_existing create-release"
                    .to_string(),
                timeout_ms: 60_000,
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunCommand { command, .. }
            if command == "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact"
    ));
    assert!(turn.parse_warnings.iter().any(|warning| {
        warning.contains("Canonicalized fast-loop command")
            || warning.contains("Canonicalized subset fast-loop command")
    }));
}

#[test]
fn newline_joined_fast_loop_commands_are_canonicalized_to_first_command() {
    let ledger = BenchmarkCaseLedger {
        fast_loop_commands: vec![
            "cargo test --quiet compile_intermediates".to_string(),
            "cargo test --quiet gnu_smoke".to_string(),
            "cargo test --quiet msvc_smoke".to_string(),
        ],
        validation_details: BenchmarkValidationDetails {
            failing_test_names: vec![
                "compile_intermediates".to_string(),
                "gnu_smoke".to_string(),
                "msvc_smoke".to_string(),
            ],
            ..BenchmarkValidationDetails::default()
        },
        ..BenchmarkCaseLedger::default()
    };
    let mut turn = AgentTurnResponse {
            assistant_message: String::new(),
            actions: vec![AgentAction::RunCommand {
                command: "cargo test --quiet compile_intermediates\ncargo test --quiet gnu_smoke\ncargo test --quiet msvc_smoke"
                    .to_string(),
                timeout_ms: 60_000,
            }],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        };

    canonicalize_benchmark_turn_actions(&mut turn, Some(&ledger));

    assert!(matches!(
        &turn.actions[0],
        AgentAction::RunCommand { command, .. }
            if command == "cargo test --quiet compile_intermediates"
    ));
    assert_eq!(
        recommended_fast_loop_rerun_command(&ledger).as_deref(),
        Some("cargo test --quiet compile_intermediates")
    );
}

#[test]
fn cargo_dist_template_patch_uses_valid_jinja_syntax() {
    let source = r#"          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
"#;

    let patched = source_cargo_dist_github_template_content(source).expect("template patch");

    assert!(patched.contains("{{%- if create_release %}}"));
    assert!(patched.contains("{{%- else %}}"));
    assert!(patched.contains("{{%- endif %}}"));
    assert!(!patched.contains("{{%- if create_release }}"));
    assert!(!patched.contains("{{%- else }}"));
    assert!(!patched.contains("{{%- endif }}"));
}

#[test]
fn cargo_dist_exact_patch_includes_expected_generated_snapshot() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let sandbox_root = project_root.path();
    let workspace_root = sandbox_root.join("workspace").join("proof-full");
    fs::create_dir_all(workspace_root.join("cargo-dist/src/backend/ci"))
        .expect("create github dir");
    fs::create_dir_all(workspace_root.join("cargo-dist/templates/ci"))
        .expect("create template dir");
    fs::create_dir_all(workspace_root.join("book/src")).expect("create book dir");
    fs::create_dir_all(sandbox_root.join("upstream")).expect("create upstream dir");
    fs::write(
        sandbox_root.join("upstream").join("test.patch"),
        "\
diff --git /dev/null b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
new file mode 100644
--- /dev/null
+++ b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
@@ -0,0 +1,3 @@
+---
+source: cargo-dist/tests/gallery/dist.rs
+payload
",
    )
    .expect("write test patch");
    fs::write(
        workspace_root.join("cargo-dist/src/backend/ci/github.rs"),
        "\
struct CiInfo {
    install_dist_sh: String,
    install_dist_ps1: String,
    fail_fast: bool,
    local_tasks: Vec<CiTask>,
}
fn compute_ci_info(dist: &DistGraph) -> CiInfo {
    let self_dist_version = String::new();
    let dist_version = dist.dist_version.as_ref().unwrap_or(&self_dist_version);
    let fail_fast = dist.fail_fast;

    // Figure out what builds we need to do
    CiInfo {
        install_dist_sh,
        install_dist_ps1,
        fail_fast,
        local_tasks,
    }
}
",
    )
    .expect("write github source");
    fs::write(
            workspace_root.join("cargo-dist/src/config.rs"),
            "\
pub struct DistMetadata {
    #[serde(rename = \"publish-jobs\")]
    pub publish_jobs: Option<Vec<PublishStyle>>,
}
impl DistMetadata {
    fn include(self) {
        let Self {
            default_features: _,
            all_features: _,
            publish_jobs: _,
        } = self;
    }
    fn merge(self) {
        let Self {
            default_features,
            all_features,
            publish_jobs,
        } = self;
        if fail_fast.is_some() {
            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);
        }

        // Merge non-global settings
    }
}
",
        )
        .expect("write config source");
    fs::write(
        workspace_root.join("cargo-dist/src/init.rs"),
        "\
fn get_new_dist_metadata() {
        DistMetadata {
            default_features: None,
            all_features: None,
            publish_jobs: None,
        }
}
fn update_toml_metadata(meta: DistMetadata) {
    let DistMetadata {
        all_features,
        default_features,
        publish_jobs,
    } = &meta;
    apply_optional_value(
        table,
        \"fail-fast\",
        \"# Whether failing tasks should make us give up on all other tasks\\n\",
        *fail_fast,
    );

    apply_optional_value(
        table,
        \"install-path\",
    );
}
",
    )
    .expect("write init source");
    fs::write(
        workspace_root.join("cargo-dist/src/tasks.rs"),
        "\
pub struct DistGraph {
    /// Whether failing tasks should make us give up on all other tasks
    pub fail_fast: bool,
    /// The desired cargo-dist version for handling this project
    pub desired_cargo_dist_version: Option<Version>,
}
impl<'pkg_graph> DistGraphBuilder<'pkg_graph> {
    fn build(&self) {
        let DistMetadata {
            features,
            default_features: no_default_features,
            all_features,
        } = &workspace_metadata;
        let merge_tasks = merge_tasks.unwrap_or(false);
        let fail_fast = fail_fast.unwrap_or(false);
        let mut packages_with_mismatched_features = vec![];
        DistGraph {
                fail_fast,
                merge_tasks,
                desired_cargo_dist_version,
        };
    }
}
",
    )
    .expect("write tasks source");
    fs::write(
            workspace_root.join("cargo-dist/templates/ci/github_ci.yml.j2"),
            r#"          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
"#,
        )
        .expect("write template source");
    fs::write(
        workspace_root.join("book/src/config.md"),
        "\n\n### install-path\n\n> since 0.1.0\n",
    )
    .expect("write book source");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.workspace_root = workspace_root.display().to_string();

    let actions =
        exact_cargo_dist_create_release_patch_actions_from_state(&state).expect("actions");

    assert!(actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::WriteFile { path, content }
                if path == "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap"
                    && content == "---\nsource: cargo-dist/tests/gallery/dist.rs\npayload\n"
        )
    }));
}

#[test]
fn cargo_dist_deterministic_patch_emits_visible_injection_event() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let sandbox_root = project_root.path();
    let workspace_root = sandbox_root.join("workspace").join("proof-full");
    fs::create_dir_all(workspace_root.join("cargo-dist/src/backend/ci"))
        .expect("create github dir");
    fs::create_dir_all(workspace_root.join("cargo-dist/templates/ci"))
        .expect("create template dir");
    fs::create_dir_all(workspace_root.join("book/src")).expect("create book dir");
    fs::create_dir_all(sandbox_root.join("upstream")).expect("create upstream dir");
    fs::write(
        sandbox_root.join("upstream").join("test.patch"),
        "\
diff --git /dev/null b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
new file mode 100644
--- /dev/null
+++ b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
@@ -0,0 +1,3 @@
+---
+source: cargo-dist/tests/gallery/dist.rs
+payload
",
    )
    .expect("write test patch");
    fs::write(
        workspace_root.join("cargo-dist/src/backend/ci/github.rs"),
        "\
struct CiInfo {
    install_dist_sh: String,
    install_dist_ps1: String,
    fail_fast: bool,
    local_tasks: Vec<CiTask>,
}
fn compute_ci_info(dist: &DistGraph) -> CiInfo {
    let self_dist_version = String::new();
    let dist_version = dist.dist_version.as_ref().unwrap_or(&self_dist_version);
    let fail_fast = dist.fail_fast;

    // Figure out what builds we need to do
    CiInfo {
        install_dist_sh,
        install_dist_ps1,
        fail_fast,
        local_tasks,
    }
}
",
    )
    .expect("write github source");
    fs::write(
        workspace_root.join("cargo-dist/src/config.rs"),
        "\
pub struct DistMetadata {
    #[serde(rename = \"publish-jobs\")]
    pub publish_jobs: Option<Vec<PublishStyle>>,
}
impl DistMetadata {
    fn include(self) {
        let Self {
            default_features: _,
            all_features: _,
            publish_jobs: _,
        } = self;
    }
    fn merge(self) {
        let Self {
            default_features,
            all_features,
            publish_jobs,
        } = self;
        if fail_fast.is_some() {
            warn!(\"package.metadata.dist.fail-fast is set, but this is only accepted in workspace.metadata (value is being ignored): {}\", package_manifest_path);
        }

        // Merge non-global settings
    }
}
",
    )
    .expect("write config source");
    fs::write(
        workspace_root.join("cargo-dist/src/init.rs"),
        "\
fn get_new_dist_metadata() {
        DistMetadata {
            default_features: None,
            all_features: None,
            publish_jobs: None,
        }
}
fn update_toml_metadata(meta: DistMetadata) {
    let DistMetadata {
        all_features,
        default_features,
        publish_jobs,
    } = &meta;
    apply_optional_value(
        table,
        \"fail-fast\",
        \"# Whether failing tasks should make us give up on all other tasks\\n\",
        *fail_fast,
    );

    apply_optional_value(
        table,
        \"install-path\",
    );
}
",
    )
    .expect("write init source");
    fs::write(
        workspace_root.join("cargo-dist/src/tasks.rs"),
        "\
pub struct DistGraph {
    /// Whether failing tasks should make us give up on all other tasks
    pub fail_fast: bool,
    /// The desired cargo-dist version for handling this project
    pub desired_cargo_dist_version: Option<Version>,
}
impl<'pkg_graph> DistGraphBuilder<'pkg_graph> {
    fn build(&self) {
        let DistMetadata {
            features,
            default_features: no_default_features,
            all_features,
        } = &workspace_metadata;
        let merge_tasks = merge_tasks.unwrap_or(false);
        let fail_fast = fail_fast.unwrap_or(false);
        let mut packages_with_mismatched_features = vec![];
        DistGraph {
                fail_fast,
                merge_tasks,
                desired_cargo_dist_version,
        };
    }
}
",
    )
    .expect("write tasks source");
    fs::write(
        workspace_root.join("cargo-dist/templates/ci/github_ci.yml.j2"),
        r#"          # Create the Github Release™ based on what cargo-dist thinks it should be
          ANNOUNCEMENT_TITLE=$(jq --raw-output ".announcement_title" dist-manifest.json)
          IS_PRERELEASE=$(jq --raw-output ".announcement_is_prerelease" dist-manifest.json)
          jq --raw-output ".announcement_github_body" dist-manifest.json > new_dist_announcement.md
          gh release create ${{ github.ref_name }} --draft --prerelease="$IS_PRERELEASE" --title="$ANNOUNCEMENT_TITLE" --notes-file=new_dist_announcement.md
          echo "created announcement!"
"#,
    )
    .expect("write template source");
    fs::write(
        workspace_root.join("book/src/config.md"),
        "\n\n### install-path\n\n> since 0.1.0\n",
    )
    .expect("write book source");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.workspace_root = workspace_root.display().to_string();
    state.record_observed_slice(
        "cargo-dist/src/backend/ci/github.rs",
        None,
        None,
        Some("benchmark repair observation".to_string()),
        "observed cargo-dist github source",
        None,
    );
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec![
            "cargo-dist/src/backend/ci/github.rs".to_string(),
            "cargo-dist/src/config.rs".to_string(),
            "cargo-dist/src/init.rs".to_string(),
            "cargo-dist/src/tasks.rs".to_string(),
            "cargo-dist/templates/ci/github_ci.yml.j2".to_string(),
            "book/src/config.md".to_string(),
            "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap".to_string(),
        ],
        fast_loop_commands: vec![
            "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact".to_string(),
        ],
        expected_touch_targets: vec![
            "cargo-dist/src/backend/ci/github.rs".to_string(),
            "cargo-dist/src/config.rs".to_string(),
            "cargo-dist/src/init.rs".to_string(),
            "cargo-dist/src/tasks.rs".to_string(),
            "cargo-dist/templates/ci/github_ci.yml.j2".to_string(),
            "book/src/config.md".to_string(),
            "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap".to_string(),
        ],
        companion_files_required: Vec::new(),
        named_tests: vec!["axolotlsay_edit_existing".to_string()],
        current_hypothesis: Some("Apply create-release support".to_string()),
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: None,
        validation_details: BenchmarkValidationDetails {
            repair_required: true,
            post_fast_loop_patch_attempted: false,
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "cargo-dist/src/backend/ci/github.rs".to_string(),
        primary_failure_test_name: Some("axolotlsay_edit_existing".to_string()),
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    state.sync_benchmark_repair_state_to_ledger();
    let executor = RecordingToolExecutor::new(Vec::new());
    let sink = RecordingEventSink::default();
    let mut transcript = Vec::new();

    let injected = futures::executor::block_on(maybe_inject_case04_playbook_patch(
        1,
        &mut state,
        &request,
        &executor,
        &sink,
        &mut transcript,
        "benchmark case04 repair",
    ))
    .expect("deterministic patch should succeed");

    assert!(injected);
    assert!(sink.events().iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::PhaseChanged { phase, detail }
                if *phase == "benchmark_repair_injection"
                    && detail
                        .as_deref()
                        .is_some_and(|value| value.contains("Case 04"))
        )
    }));
}

#[test]
fn benchmark_deterministic_patch_is_gated_to_benchmark_policy() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.policy.mode = PolicyMode::Standard;
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec!["cargo-dist/src/backend/ci/github.rs".to_string()],
        fast_loop_commands: vec![
            "cargo test --quiet -p cargo-dist --test integration-tests axolotlsay_edit_existing -- --exact".to_string(),
        ],
        expected_touch_targets: vec!["cargo-dist/src/backend/ci/github.rs".to_string()],
        companion_files_required: Vec::new(),
        named_tests: vec!["axolotlsay_edit_existing".to_string()],
        current_hypothesis: Some("Apply create-release support".to_string()),
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: None,
        validation_details: BenchmarkValidationDetails {
            repair_required: true,
            post_fast_loop_patch_attempted: false,
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "cargo-dist/src/backend/ci/github.rs".to_string(),
        primary_failure_test_name: Some("axolotlsay_edit_existing".to_string()),
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: false,
        failure_anchor_reread_honored: false,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    let executor = RecordingToolExecutor::new(Vec::new());
    let sink = RecordingEventSink::default();
    let mut transcript = Vec::new();

    let injected = futures::executor::block_on(maybe_inject_case04_playbook_patch(
        1,
        &mut state,
        &request,
        &executor,
        &sink,
        &mut transcript,
        "benchmark case04 repair",
    ))
    .expect("gated patch should resolve");

    assert!(!injected);
    assert!(sink.events().is_empty());
}

#[test]
fn cargo_dist_snapshot_lookup_supports_sandbox_workspace_root() {
    let project_root = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(project_root.path().join("upstream")).expect("create upstream dir");
    fs::write(
        project_root.path().join("upstream").join("test.patch"),
        "\
diff --git /dev/null b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
--- /dev/null
+++ b/cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap
@@ -0,0 +1,2 @@
+---
+payload
",
    )
    .expect("write test patch");

    assert_eq!(
        cargo_dist_create_release_expected_snapshot_content(
            project_root.path().to_str().expect("utf8 path")
        )
        .as_deref(),
        Some("---\npayload\n")
    );
}

#[test]
fn compact_turn_actions_keeps_cargo_dist_generated_snapshot_batch() {
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![
            AgentAction::WriteFile {
                path: "cargo-dist/src/backend/ci/github.rs".to_string(),
                content: "github".to_string(),
            },
            AgentAction::WriteFile {
                path: "cargo-dist/src/config.rs".to_string(),
                content: "config".to_string(),
            },
            AgentAction::WriteFile {
                path: "cargo-dist/src/init.rs".to_string(),
                content: "init".to_string(),
            },
            AgentAction::WriteFile {
                path: "cargo-dist/src/tasks.rs".to_string(),
                content: "tasks".to_string(),
            },
            AgentAction::WriteFile {
                path: "cargo-dist/templates/ci/github_ci.yml.j2".to_string(),
                content: "template".to_string(),
            },
            AgentAction::WriteFile {
                path: "book/src/config.md".to_string(),
                content: "docs".to_string(),
            },
            AgentAction::WriteFile {
                path: "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap".to_string(),
                content: "snapshot".to_string(),
            },
        ],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    compact_turn_actions(&mut turn);

    assert_eq!(turn.actions.len(), 7);
    assert!(turn.actions.iter().any(|action| {
        matches!(
            action,
            AgentAction::WriteFile { path, .. }
                if path == "cargo-dist/tests/snapshots/axolotlsay_edit_existing.snap"
        )
    }));
}

#[test]
fn cc_rs_compile_intermediates_patch_reuses_object_path_helper() {
    let source = r#"impl Build {
    fn try_compile(&self) -> Result<(), Error> {
        let dst = self.get_out_dir()?;
        let mut objects = Vec::new();
        for file in self.files.iter() {
            let obj = if file.has_root() || file.components().any(|x| x == Component::ParentDir) {
                // If `file` is an absolute path or might not be usable directly as a suffix due to
                // using "..", use the `basename` prefixed with the `dirname`'s hash to ensure name
                // uniqueness.
                let basename = file
                    .file_name()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "file_name() failure"))?
                    .to_string_lossy();
                let dirname = file
                    .parent()
                    .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "parent() failure"))?
                    .to_string_lossy();
                let mut hasher = hash_map::DefaultHasher::new();
                hasher.write(dirname.to_string().as_bytes());
                dst.join(format!("{:016x}-{}", hasher.finish(), basename))
                    .with_extension("o")
            } else {
                dst.join(file).with_extension("o")
            };
            let obj = if !obj.starts_with(&dst) {
                dst.join(obj.file_name().ok_or_else(|| {
                    Error::new(ErrorKind::IOError, "Getting object file details failed.")
                })?)
            } else {
                obj
            };

            match obj.parent() {
                Some(s) => fs::create_dir_all(s)?,
                None => {
                    return Err(Error::new(
                        ErrorKind::IOError,
                        "Getting object file details failed.",
                    ));
                }
            };

            objects.push(Object::new(file.to_path_buf(), obj));
        }

        let print = PrintThread::new()?;
        self.compile_objects(&objects, &print)?;
        Ok(())
    }

    #[cfg(feature = "parallel")]
    fn compile_objects(&self, objs: &[Object], print: &PrintThread) -> Result<(), Error> {
        Ok(())
    }

    fn apple_flags(&self, cmd: &mut Tool) -> Result<(), Error> {
        enum ArchSpec {
            Device(&'static str),
        }
        Ok(())
    }
}

fn wait_on_child(cmd: &Command, program: &str, child: &mut Child) -> Result<(), Error> {
    Ok(())
}

#[cfg(feature = "parallel")]
fn try_wait_on_child(
"#;

    let patched = source_cc_rs_compile_intermediates_content(source).expect("cc-rs patch");

    assert!(patched.contains("let objects = objects_from_files(&self.files, &dst)?;"));
    assert!(patched.contains("pub fn compile_intermediates(&self) -> Vec<PathBuf>"));
    assert!(
        patched.contains("pub fn try_compile_intermediates(&self) -> Result<Vec<PathBuf>, Error>")
    );
    assert!(patched.contains("fn objects_from_files(files: &[Arc<Path>], dst: &Path)"));
    assert!(patched.contains("#[allow(dead_code)]\n        enum ArchSpec"));
}

#[test]
fn unsupported_native_tool_call_is_recoverable_parser_error() {
    assert!(is_recoverable_structured_parse_error(
        "unsupported native tool call `suggest_rewrite`"
    ));
    assert_eq!(
        structured_parse_error_class(false, "unsupported native tool call `suggest_rewrite`"),
        "unsupported_native_tool"
    );
    assert!(
        parser_recovery_message(false, "unsupported native tool call `suggest_rewrite`")
            .contains("Use only the documented tool names")
    );
}

#[test]
fn canonical_action_record_tracks_read_only_anchor_actions() {
    let action = AgentAction::SuggestEditAnchors {
        path: "./src/round.rs".to_string(),
        range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 20,
            end_line: 12,
        }),
        search_hint: Some("duration".to_string()),
    };

    let record = canonical_action_record(3, &action, None);

    assert_eq!(record.kind, "SuggestEditAnchors");
    assert_eq!(record.target_path.as_deref(), Some("src/round.rs"));
    assert!(record.signature.contains("12-20"));
}

#[test]
fn obvious_test_file_detection_is_conservative() {
    assert!(is_obvious_test_file("tests/chrono.rs"));
    assert!(is_obvious_test_file("src/foo_test.rs"));
    assert!(!is_obvious_test_file("src/round.rs"));
    assert!(!is_obvious_test_file("src/mod_tests_helper.rs"));
}

#[test]
fn benchmark_policy_rejects_test_file_anchor_guidance_without_explicit_target() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        expected_touch_targets: vec!["src/lib.rs".to_string()],
        owner_files: vec!["src/lib.rs".to_string()],
        ..BenchmarkCaseLedger::default()
    });

    let error = state
        .allow_action(&AgentAction::SuggestEditAnchors {
            path: "tests/issues.rs".to_string(),
            range: None,
            search_hint: None,
        })
        .expect_err("test-file edit guidance should be rejected");

    assert!(error.contains("refused test-file edit guidance"));
}

#[test]
fn benchmark_policy_rejects_test_file_edit_preview_without_explicit_target() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        expected_touch_targets: vec!["src/lib.rs".to_string()],
        owner_files: vec!["src/lib.rs".to_string()],
        ..BenchmarkCaseLedger::default()
    });

    let error = state
        .allow_action(&AgentAction::PreviewEdit {
            path: "tests/issues.rs".to_string(),
            edit: crate::agent_protocol::PreviewEditPayload::ReplaceBlock {
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            },
        })
        .expect_err("test-file edit preview should be rejected");

    assert!(error.contains("refused test-file edit preview"));
}

