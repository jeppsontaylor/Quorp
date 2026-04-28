#[test]
fn read_file_summary_preserves_backend_content_hash() {
    let output = "[read_file]\npath: Cargo.toml\ncontent_hash: f543f6a8e32e1f38\n[dependencies]\nchrono = \"0.4\"\n";
    let summary = summarize_read_file_observation("Cargo.toml", None, output, None, None);

    assert!(summary.contains("content_hash=f543f6a8e32e1f38"));
}

#[test]
fn fills_placeholder_modify_toml_hash_from_observed_full_file() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.record_observed_slice(
        "Cargo.toml",
        None,
        None,
        Some("patch_scaffold".to_string()),
        "[dependencies]\n",
        Some("f543f6a8e32e1f38"),
    );
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ModifyToml {
            path: "Cargo.toml".to_string(),
            expected_hash: "not_specified_yet".to_string(),
            operations: vec![crate::agent_protocol::TomlEditOperation::SetDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
                version: Some("0.4".to_string()),
                features: Vec::new(),
                default_features: None,
                optional: None,
                package: None,
                path: None,
            }],
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);

    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Filled placeholder expected_hash for ModifyToml") })
    );
    match &turn.actions[0] {
        AgentAction::ModifyToml { expected_hash, .. } => {
            assert_eq!(expected_hash, "f543f6a8e32e1f38");
        }
        action => panic!("unexpected action {action:?}"),
    }
}

#[test]
fn placeholder_modify_toml_hash_without_observation_becomes_read() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let state = AgentTaskState::new(&request, test_config());
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ModifyToml {
            path: "Cargo.toml".to_string(),
            expected_hash: "unknown".to_string(),
            operations: vec![crate::agent_protocol::TomlEditOperation::SetDependency {
                table: "dependencies".to_string(),
                name: "chrono".to_string(),
                version: Some("0.4".to_string()),
                features: Vec::new(),
                default_features: None,
                optional: None,
                package: None,
                path: None,
            }],
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);

    assert!(matches!(
        &turn.actions[0],
        AgentAction::ReadFile { path, range: None } if path == "Cargo.toml"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Converted placeholder-hash ModifyToml") })
    );
}

#[test]
fn preview_modify_toml_mismatched_hash_uses_observed_full_file_hash() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
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
        actions: vec![AgentAction::PreviewEdit {
            path: "Cargo.toml".to_string(),
            edit: PreviewEditPayload::ModifyToml {
                expected_hash: "9164a7439d76c9d1e571e230f4d56e916b7b9c5a".to_string(),
                operations: vec![crate::agent_protocol::TomlEditOperation::SetDependency {
                    table: "dev-dependencies".to_string(),
                    name: "chrono".to_string(),
                    version: Some("0.4".to_string()),
                    features: Vec::new(),
                    default_features: None,
                    optional: None,
                    package: None,
                    path: None,
                }],
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);

    assert!(turn.parse_warnings.iter().any(|warning| {
        warning.contains("Replaced mismatched expected_hash for PreviewEdit modify_toml")
    }));
    match &turn.actions[0] {
        AgentAction::PreviewEdit {
            edit: PreviewEditPayload::ModifyToml { expected_hash, .. },
            ..
        } => {
            assert_eq!(expected_hash, "f543f6a8e32e1f38");
        }
        action => panic!("unexpected action {action:?}"),
    }
}

#[test]
fn preview_modify_toml_uses_benchmark_manifest_feature_operations() {
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
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `DateTime<Utc>: serde::Deserialize<'de>` is not satisfied"
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
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
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
        actions: vec![AgentAction::PreviewEdit {
            path: "Cargo.toml".to_string(),
            edit: PreviewEditPayload::ModifyToml {
                expected_hash: "f543f6a8e32e1f38".to_string(),
                operations: vec![
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table: "dev-dependencies".to_string(),
                        name: "chrono".to_string(),
                        version: Some("0.4".to_string()),
                        features: Vec::new(),
                        default_features: None,
                        optional: None,
                        package: None,
                        path: None,
                    },
                    crate::agent_protocol::TomlEditOperation::SetDependency {
                        table: "dev-dependencies".to_string(),
                        name: "uuid".to_string(),
                        version: Some("0.8".to_string()),
                        features: Vec::new(),
                        default_features: None,
                        optional: None,
                        package: None,
                        path: None,
                    },
                ],
            },
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);

    assert!(
        turn.parse_warnings.iter().any(|warning| {
            warning.contains("Replaced benchmark manifest PreviewEdit operations")
        })
    );
    match &turn.actions[0] {
        AgentAction::PreviewEdit {
            edit: PreviewEditPayload::ModifyToml { operations, .. },
            ..
        } => {
            assert_eq!(operations.len(), 2);
            assert!(
                operations.iter().all(|operation| matches!(
                    operation,
                    crate::agent_protocol::TomlEditOperation::SetDependency { features, .. }
                        if features.as_slice() == ["serde"]
                )),
                "operations: {operations:?}"
            );
        }
        action => panic!("unexpected action {action:?}"),
    }
}

#[test]
fn malformed_native_manifest_tool_error_becomes_exact_preview() {
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
                "error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `DateTime<Utc>: serde::Deserialize<'de>` is not satisfied"
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
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
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

    let turn = maybe_repair_native_manifest_tool_error(
        "native tool `preview_edit.modify_toml` had invalid `operations`: missing field `name`",
        &state,
    )
    .expect("repaired native tool error");

    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Recovered malformed native manifest tool call") })
    );
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
}

#[test]
fn rust_compile_error_lease_prefers_source_target_over_manifest() {
    let ledger = BenchmarkCaseLedger {
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
            "test `issue_474::test` failed | diagnostic_class rust_compile_error".to_string(),
        ),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("rust_compile_error".to_string()),
            ..BenchmarkValidationDetails::default()
        },
    };

    assert_eq!(
        target_lease_for_ledger(&ledger).as_deref(),
        Some("src/features/serde/de_owned.rs")
    );
}

#[test]
fn source_test_failure_lease_prefers_owner_source_before_support_docs() {
    let ledger = BenchmarkCaseLedger {
        case_class: "breadth-heavy-companion".to_string(),
        owner_files: vec![
            "axum/src/routing/mod.rs".to_string(),
            "axum/src/routing/tests/".to_string(),
        ],
        fast_loop_commands: vec![
            "cargo test --quiet -p axum --lib --features headers routing::tests::".to_string(),
        ],
        expected_touch_targets: vec![
            "axum/CHANGELOG.md".to_string(),
            "axum/src/docs/routing/fallback.md".to_string(),
            "axum/src/docs/routing/merge.md".to_string(),
            "axum/src/docs/routing/nest.md".to_string(),
            "axum/src/routing/mod.rs".to_string(),
        ],
        companion_files_required: vec![
            "axum/CHANGELOG.md".to_string(),
            "axum/src/docs/routing/fallback.md".to_string(),
            "axum/src/docs/routing/merge.md".to_string(),
            "axum/src/docs/routing/nest.md".to_string(),
        ],
        named_tests: Vec::new(),
        current_hypothesis: None,
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some(
            "routing::tests::nesting_router_with_fallbacks_panics failed".to_string(),
        ),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("rust_compile_error".to_string()),
            primary_failure_path: Some("axum/src/lib.rs".to_string()),
            primary_failure_line: Some(369),
            ..BenchmarkValidationDetails::default()
        },
    };

    assert_eq!(
        target_lease_for_ledger(&ledger).as_deref(),
        Some("axum/src/routing/mod.rs")
    );
    let ranked = ranked_implementation_targets_for_ledger(&ledger);
    assert_eq!(
        ranked.first().map(|target| target.reason.as_str()),
        Some("owner_file")
    );
    assert!(
        ranked
            .iter()
            .skip_while(|target| target.path != "axum/CHANGELOG.md")
            .all(|target| target.reason != "owner_file")
    );
}

#[test]
fn timeout_failure_lease_prefers_owner_source_before_support_docs() {
    let ledger = BenchmarkCaseLedger {
        case_class: "breadth-heavy-companion".to_string(),
        owner_files: vec!["axum/src/routing/mod.rs".to_string()],
        fast_loop_commands: vec![
            "cargo test --quiet -p axum --lib --features headers routing::tests::".to_string(),
        ],
        expected_touch_targets: vec![
            "axum/CHANGELOG.md".to_string(),
            "axum/src/docs/routing/fallback.md".to_string(),
            "axum/src/routing/mod.rs".to_string(),
        ],
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some("assertion [Command timed out]".to_string()),
        validation_details: BenchmarkValidationDetails::default(),
        ..BenchmarkCaseLedger::default()
    };

    let ranked = ranked_implementation_targets_for_ledger(&ledger);
    assert_eq!(
        ranked.first().map(|target| target.path.as_str()),
        Some("axum/src/routing/mod.rs")
    );
    assert_eq!(
        target_lease_for_ledger(&ledger).as_deref(),
        Some("axum/src/routing/mod.rs")
    );
    assert_eq!(
        ranked.first().map(|target| target.reason.as_str()),
        Some("owner_file")
    );
}

#[test]
fn source_failure_repair_state_moves_to_leased_source_target() {
    let ledger = BenchmarkCaseLedger {
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
                "test `issue_474::test` failed | at tests/issues/issue_474.rs:47 | assertion CannotBorrowOwnedData | diagnostic_class rust_compile_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                diagnostic_class: Some("rust_compile_error".to_string()),
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(47),
                ..BenchmarkValidationDetails::default()
            },
        };

    let repair_state = benchmark_repair_state_from_ledger(&ledger).expect("repair state");

    assert_eq!(repair_state.phase, BenchmarkRepairPhase::NeedsPatch);
    assert_eq!(repair_state.owner_path, "src/features/serde/de_owned.rs");
    assert_eq!(repair_state.failure_anchor_range, None);
}

#[test]
fn source_patch_phase_keeps_leased_source_read_from_bundled_turn() {
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
        last_validation_failure: Some("CannotBorrowOwnedData".to_string()),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("rust_compile_error".to_string()),
            primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
            primary_failure_line: Some(47),
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "src/features/serde/de_owned.rs".to_string(),
        primary_failure_test_name: Some("issue_474::test".to_string()),
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: false,
        implementation_reread_honored: false,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.implementation_target_lease =
        Some("src/features/serde/de_owned.rs".to_string());
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![
            AgentAction::ReadFile {
                path: "tests/issues/issue_474.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 39,
                    end_line: 71,
                }),
            },
            AgentAction::ReadFile {
                path: "src/features/serde/de_owned.rs".to_string(),
                range: None,
            },
        ],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::ReadFile { path, .. } if path == "src/features/serde/de_owned.rs"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Kept only the leased source ReadFile") })
    );
}

#[test]
fn source_patch_phase_rewrites_read_only_drift_to_focused_source_read() {
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
                "test `issue_474::test` failed | at tests/issues/issue_474.rs:47 | assertion error: test failed"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                diagnostic_class: Some("rust_compile_error".to_string()),
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(47),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/features/serde/de_owned.rs".to_string(),
            primary_failure_test_name: Some("issue_474::test".to_string()),
            failure_anchor_range: None,
            implementation_suggested_range: None,
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/features/serde/de_owned.rs".to_string(),
                requested_range: None,
                honored_range: None,
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "use crate::*;\n... [middle lines omitted] ...\nfn struct_variant() {}"
                        .to_string(),
                ),
            }),
            latest_owner_file_text: Some(
                "fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error> {\n    Err(DecodeError::CannotBorrowOwnedData)\n}\n"
                    .to_string(),
            ),
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: false,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
    state.agent_repair_memory.implementation_target_lease =
        Some("src/features/serde/de_owned.rs".to_string());
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ListDirectory {
            path: ".".to_string(),
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::ReadFile { path, range: Some(_) }
            if path == "src/features/serde/de_owned.rs"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Replaced 1 read-only source-phase action") }),
        "warnings: {:?}",
        turn.parse_warnings
    );
}

#[test]
fn source_patch_phase_rewrites_post_slice_read_loop_to_semantic_patch() {
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
                "test `issue_474::test` failed | at tests/issues/issue_474.rs:47 | assertion error: test failed"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                diagnostic_class: Some("rust_compile_error".to_string()),
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(47),
                ..BenchmarkValidationDetails::default()
            },
        });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/features/serde/de_owned.rs".to_string(),
            primary_failure_test_name: Some("issue_474::test".to_string()),
            failure_anchor_range: None,
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 100,
                end_line: 170,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/features/serde/de_owned.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 100,
                    end_line: 170,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 100,
                    end_line: 170,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some("    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        Err(DecodeError::CannotBorrowOwnedData)\n    }\n\n    fn deserialize_string<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        visitor.visit_string(Decode::decode(&mut self.de)?)\n    }\n\n    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        Err(DecodeError::CannotBorrowOwnedData)\n    }\n".to_string()),
            }),
            latest_owner_file_text: Some(
                "    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        Err(DecodeError::CannotBorrowOwnedData)\n    }\n"
                    .to_string(),
            ),
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: false,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
    state.agent_repair_memory.implementation_target_lease =
        Some("src/features/serde/de_owned.rs".to_string());
    state.record_observed_slice(
            "src/features/serde/de_owned.rs",
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 100,
                end_line: 170,
            }),
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 100,
                end_line: 170,
            }),
            Some("needs_patch".to_string()),
            "    fn deserialize_str<V>(self, _visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        Err(DecodeError::CannotBorrowOwnedData)\n    }\n\n    fn deserialize_string<V>(mut self, visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        visitor.visit_string(Decode::decode(&mut self.de)?)\n    }\n\n    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>\n    where\n        V: serde_incl::de::Visitor<'de>,\n    {\n        Err(DecodeError::CannotBorrowOwnedData)\n    }\n",
            Some("aaaaaaaaaaaaaaaa"),
        );
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ReadFile {
            path: "tests/issues/issue_474.rs".to_string(),
            range: None,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::ReplaceRange {
            path,
            replacement,
            expected_hash,
            ..
        }
            if path == "src/features/serde/de_owned.rs"
                && expected_hash == "aaaaaaaaaaaaaaaa"
                && replacement.contains("visitor.visit_string")
                && replacement.contains("visitor.visit_byte_buf")
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("exact benchmark source patch") }),
        "warnings: {:?}",
        turn.parse_warnings
    );
}

#[test]
fn source_patch_phase_rewrites_chrono_read_loop_to_semantic_patch() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    let slice_content = r#"fn duration_round<T>(
    naive: NaiveDateTime,
    original: T,
    duration: TimeDelta,
) -> Result<T, RoundingError>
where
    T: Timelike + Add<TimeDelta, Output = T> + Sub<TimeDelta, Output = T>,
{
    if let Some(span) = duration.num_nanoseconds() {
        if span < 0 {
            return Err(RoundingError::DurationExceedsLimit);
        }
        let stamp = naive.timestamp_nanos_opt().ok_or(RoundingError::TimestampExceedsLimit)?;
        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
        if span == 0 {
            return Ok(original);
        }
    } else {
        Err(RoundingError::DurationExceedsLimit)
    }
}

fn duration_trunc<T>(
    naive: NaiveDateTime,
    original: T,
    duration: TimeDelta,
) -> Result<T, RoundingError>
where
    T: Timelike + Add<TimeDelta, Output = T> + Sub<TimeDelta, Output = T>,
{
    if let Some(span) = duration.num_nanoseconds() {
        if span < 0 {
            return Err(RoundingError::DurationExceedsLimit);
        }
        let stamp = naive.timestamp_nanos_opt().ok_or(RoundingError::TimestampExceedsLimit)?;
        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
        let delta_down = stamp % span;
        match delta_down.cmp(&0) {
            Ordering::Equal => Ok(original),
            Ordering::Greater => Ok(original - TimeDelta::nanoseconds(delta_down)),
            Ordering::Less => Ok(original - TimeDelta::nanoseconds(span - delta_down.abs())),
        }
    } else {
        Err(RoundingError::DurationExceedsLimit)
    }
}
"#;
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec!["src/round.rs".to_string()],
        fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
        expected_touch_targets: vec!["src/round.rs".to_string()],
        companion_files_required: Vec::new(),
        named_tests: Vec::new(),
        current_hypothesis: None,
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some("DurationExceedsTimestamp".to_string()),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("rust_compile_error".to_string()),
            primary_failure_path: Some("src/round.rs".to_string()),
            primary_failure_line: Some(800),
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "src/round.rs".to_string(),
        last_owner_slice: Some(OwnerSliceRecord {
            path: "src/round.rs".to_string(),
            requested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 225,
            }),
            honored_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 225,
            }),
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some(slice_content.to_string()),
        }),
        latest_owner_file_text: Some(slice_content.to_string()),
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: true,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        ..BenchmarkRepairState::default()
    });
    state.agent_repair_memory.implementation_target_lease = Some("src/round.rs".to_string());
    state.record_observed_slice(
        "src/round.rs",
        Some(crate::agent_protocol::ReadFileRange {
            start_line: 149,
            end_line: 225,
        }),
        Some(crate::agent_protocol::ReadFileRange {
            start_line: 149,
            end_line: 225,
        }),
        Some("needs_patch".to_string()),
        slice_content,
        Some("bbbbbbbbbbbbbbbb"),
    );
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ReadFile {
            path: "src/round.rs".to_string(),
            range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 790,
                end_line: 811,
            }),
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::WriteFile { path, content, }
            if path == "src/round.rs"
                && content.contains("if span == 0")
                && content.contains("let delta_down = stamp % span")
                && !content.contains("if span > stamp.abs()")
    ));
}

#[test]
fn chrono_epoch_range_covers_round_and_trunc_guards() {
    let prefix = (1..149)
        .map(|line| format!("// filler {line}\n"))
        .collect::<String>();
    let owner_text = format!(
        r#"{prefix}
pub trait DurationRound {{
    fn duration_round(self, duration: TimeDelta) -> Result<Self, Self::Err>;
    fn duration_trunc(self, duration: TimeDelta) -> Result<Self, Self::Err>;
}}

impl DurationRound for NaiveDateTime {{
    fn duration_round(self, duration: TimeDelta) -> Result<Self, Self::Err> {{
        duration_round(self, self, duration)
    }}

    fn duration_trunc(self, duration: TimeDelta) -> Result<Self, Self::Err> {{
        duration_trunc(self, self, duration)
    }}
}}

fn duration_round<T>(
    naive: NaiveDateTime,
    original: T,
    duration: TimeDelta,
) -> Result<T, RoundingError> {{
    let stamp = naive.timestamp_nanos_opt().ok_or(RoundingError::TimestampExceedsLimit)?;
    if span > stamp.abs() {{
        return Err(RoundingError::DurationExceedsTimestamp);
    }}
    Ok(original)
}}

fn duration_trunc<T>(
    naive: NaiveDateTime,
    original: T,
    duration: TimeDelta,
) -> Result<T, RoundingError> {{
    let stamp = naive.timestamp_nanos_opt().ok_or(RoundingError::TimestampExceedsLimit)?;
    if span > stamp.abs() {{
        return Err(RoundingError::DurationExceedsTimestamp);
    }}
    Ok(original)
}}
"#
    );

    let range = suggest_implementation_range_from_owner_text(
        &owner_text,
        Some("round::tests::test_duration_round_close_to_epoch"),
    )
    .expect("chrono implementation range");
    let round_line = owner_text
        .lines()
        .position(|line| line.starts_with("fn duration_round<"))
        .map(|index| index + 1)
        .expect("round line");
    let trunc_guard_line = owner_text
        .lines()
        .position(|line| line.contains("DurationExceedsTimestamp") && line.contains("Err"))
        .map(|index| index + 1)
        .expect("round guard line");
    let final_guard_line = owner_text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("DurationExceedsTimestamp") && line.contains("Err"))
        .map(|(index, _)| index + 1)
        .last()
        .expect("trunc guard line");

    assert!(range.start_line <= round_line);
    assert!(range.end_line >= final_guard_line);
    assert!(range.end_line > trunc_guard_line);
    assert!(read_range_span(range) <= 128);
}

#[test]
fn axum_fallback_range_covers_nest_and_merge() {
    let prefix = (1..120)
        .map(|line| format!("// filler {line}\n"))
        .collect::<String>();
    let owner_text = format!(
        r#"{prefix}
impl<B> Router<B> {{
    pub fn route(self, path: &str) -> Self {{
        self
    }}

    pub fn nest<T>(mut self, path: &str, svc: T) -> Self
    where
        T: Clone,
    {{
        match try_downcast::<Router<B>, _>(svc) {{
            Ok(router) => {{
                let Router {{
                    mut routes,
                    node,
                    // discard the fallback of the nested router
                    fallback: _,
                    nested_at_root: _,
                }} = router;

                for (id, nested_path) in node.route_id_to_path {{
                    let route = routes.remove(&id).unwrap();
                }}
            }}
            Err(svc) => {{
                self = self.route(path, svc);
            }}
        }}

        self
    }}

    pub fn merge(mut self, other: Router<B>) -> Self {{
        self.fallback = match (self.fallback, fallback) {{
            (Fallback::Default(_), pick @ Fallback::Default(_)) => pick,
            (Fallback::Default(_), pick @ Fallback::Custom(_)) => pick,
            (pick @ Fallback::Custom(_), Fallback::Default(_)) => pick,
            (Fallback::Custom(_), pick @ Fallback::Custom(_)) => pick,
        }};

        self
    }}
}}
"#
    );

    let range = suggest_implementation_range_from_owner_text(
        &owner_text,
        Some("routing::tests::merging_routers_with_fallbacks_panics"),
    )
    .expect("axum fallback implementation range");
    let nest_line = owner_text
        .lines()
        .position(|line| line.trim_start().starts_with("pub fn nest<"))
        .map(|index| index + 1)
        .expect("nest line");
    let merge_arm_line = owner_text
        .lines()
        .position(|line| line.contains("pick @ Fallback::Custom(_)"))
        .map(|index| index + 1)
        .expect("merge arm line");

    assert!(range.start_line <= nest_line);
    assert!(range.end_line >= merge_arm_line);
    assert!(read_range_span(range) <= 128);
}

#[test]
fn source_patch_phase_rewrites_axum_read_loop_to_semantic_patch() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    let source_text = r#"impl<B> Router<B> {
    pub fn nest<T>(mut self, path: &str, svc: T) -> Self
    where
        T: Clone,
    {
        match try_downcast::<Router<B>, _>(svc) {
            Ok(router) => {
                let Router {
                    mut routes,
                    node,
                    // discard the fallback of the nested router
                    fallback: _,
                    // nesting a router that has something nested at root
                    // doesn't mean something is nested at root in _this_ router
                    // thus we don't need to propagate that
                    nested_at_root: _,
                } = router;

                for (id, nested_path) in node.route_id_to_path {
                    let route = routes.remove(&id).unwrap();
                    let full_path = if &*nested_path == "/" {
                        path.to_string()
                    } else {
                        format!("{}{}", path, nested_path)
                    };
                }
            }
            Err(svc) => {
                self = self.route(path, svc);
            }
        }

        self
    }

    pub fn merge(mut self, other: Router<B>) -> Self {
        let Router {
            routes,
            node,
            fallback,
            nested_at_root,
        } = other;

        self.fallback = match (self.fallback, fallback) {
            (Fallback::Default(_), pick @ Fallback::Default(_)) => pick,
            (Fallback::Default(_), pick @ Fallback::Custom(_)) => pick,
            (pick @ Fallback::Custom(_), Fallback::Default(_)) => pick,
            (Fallback::Custom(_), pick @ Fallback::Custom(_)) => pick,
        };

        self
    }
}
"#;
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec!["axum/src/routing/mod.rs".to_string()],
        fast_loop_commands: vec!["cargo test --quiet routing::tests::".to_string()],
        expected_touch_targets: vec!["axum/src/routing/mod.rs".to_string()],
        companion_files_required: Vec::new(),
        named_tests: Vec::new(),
        current_hypothesis: None,
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some("merging_routers_with_fallbacks_panics failed".to_string()),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("test_failure".to_string()),
            primary_failure_path: Some("axum/src/routing/tests/mod.rs".to_string()),
            primary_failure_line: Some(570),
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "axum/src/routing/mod.rs".to_string(),
        primary_failure_test_name: Some(
            "routing::tests::merging_routers_with_fallbacks_panics".to_string(),
        ),
        implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 150,
            end_line: 270,
        }),
        last_owner_slice: Some(OwnerSliceRecord {
            path: "axum/src/routing/mod.rs".to_string(),
            requested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 150,
                end_line: 270,
            }),
            honored_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 150,
                end_line: 270,
            }),
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some(source_text.to_string()),
        }),
        latest_owner_file_text: Some(source_text.to_string()),
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: true,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        ..BenchmarkRepairState::default()
    });
    state.agent_repair_memory.implementation_target_lease =
        Some("axum/src/routing/mod.rs".to_string());
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ReadFile {
            path: "axum/src/lib.rs".to_string(),
            range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 361,
                end_line: 393,
            }),
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::WriteFile { path, content }
            if path == "axum/src/routing/mod.rs"
                && content.contains("Cannot nest `Router`s that has a fallback")
                && content.contains("Cannot merge two `Router`s that both have a fallback")
                && !content.contains("fallback: _")
    ));
}

#[test]
fn source_patch_context_requires_real_slice_not_head_tail_excerpt() {
    let repair_state = BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/features/serde/de_owned.rs".to_string(),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/features/serde/de_owned.rs".to_string(),
                requested_range: None,
                honored_range: None,
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "[excerpt lines 1-24 and 383-394 of 394]\nuse crate::*;\n... [middle lines omitted] ...\n}"
                        .to_string(),
                ),
            }),
            ..BenchmarkRepairState::default()
        };

    assert!(!patch_target_context_loaded(
        &repair_state,
        &AgentRepairMemory::default(),
        "src/features/serde/de_owned.rs",
    ));
}

#[test]
fn source_patch_phase_keeps_patch_and_drops_read_only_noise() {
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
        last_validation_failure: Some("CannotBorrowOwnedData".to_string()),
        validation_details: BenchmarkValidationDetails {
            diagnostic_class: Some("rust_compile_error".to_string()),
            primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
            primary_failure_line: Some(47),
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/features/serde/de_owned.rs".to_string(),
            primary_failure_test_name: Some("issue_474::test".to_string()),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/features/serde/de_owned.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 110,
                    end_line: 155,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 110,
                    end_line: 155,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error> { Err(DecodeError::CannotBorrowOwnedData) }"
                        .to_string(),
                ),
            }),
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            ..BenchmarkRepairState::default()
        });
    state.agent_repair_memory.implementation_target_lease =
        Some("src/features/serde/de_owned.rs".to_string());
    let mut turn = AgentTurnResponse {
            assistant_message: String::new(),
            actions: vec![
                AgentAction::ReadFile {
                    path: "tests/issues/issue_474.rs".to_string(),
                    range: None,
                },
                AgentAction::ApplyPatch {
                    path: "src/features/serde/de_owned.rs".to_string(),
                    patch: "--- a/src/features/serde/de_owned.rs\n+++ b/src/features/serde/de_owned.rs\n@@\n-old\n+new\n".to_string(),
                },
                AgentAction::RunCommand {
                    command: "cargo test --quiet --features serde --test issues issue_474"
                        .to_string(),
                    timeout_ms: 120_000,
                },
            ],
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan: None,
            parse_warnings: Vec::new(),
        };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 2);
    assert!(matches!(turn.actions[0], AgentAction::ApplyPatch { .. }));
    assert!(matches!(turn.actions[1], AgentAction::RunCommand { .. }));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Kept only the leased source patch action") })
    );
}

#[test]
fn invalid_json_parse_errors_are_recoverable() {
    let error = "Structured agent turn was invalid JSON: expected `,` or `}` at line 1 column 343";

    assert!(is_recoverable_structured_parse_error(error));
    assert_eq!(structured_parse_error_class(false, error), "malformed");
}

#[test]
fn redundant_manifest_read_becomes_exact_preview_in_write_locked_phase() {
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
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `DateTime<Utc>: serde::Deserialize<'de>` is not satisfied"
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
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
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
        actions: vec![AgentAction::ReadFile {
            path: "Cargo.toml".to_string(),
            range: None,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);

    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Converted redundant ReadFile `Cargo.toml`") })
    );
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
}

#[test]
fn clean_manifest_preview_lock_converts_redundant_read_to_apply_preview() {
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
                "at tests/issues/issue_474.rs:18 | assertion error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied | diagnostic_class manifest_feature_error"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                assertion_excerpt: Some(
                    "error[E0277]: the trait bound `DateTime<Utc>: serde::Deserialize<'de>` is not satisfied"
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
        failure_anchor_range: None,
        implementation_suggested_range: None,
        last_owner_slice: None,
        latest_owner_file_text: None,
        failure_anchor_reread_attempted: true,
        failure_anchor_reread_honored: true,
        implementation_reread_allowed: false,
        implementation_reread_attempted: true,
        implementation_reread_honored: true,
        invalid_action_count: 0,
    });
    state.agent_repair_memory.implementation_target_lease = Some("Cargo.toml".to_string());
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
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![AgentAction::ReadFile {
            path: "Cargo.toml".to_string(),
            range: None,
        }],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    fill_hash_guards_from_observed_context(&mut turn, &state);
    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::ApplyPreview { preview_id } if preview_id == "pv_manifest"
    ));
    assert!(
        turn.parse_warnings.iter().any(|warning| {
            warning.contains("Converted write-locked manifest turn into required ApplyPreview")
        }),
        "warnings: {:?}",
        turn.parse_warnings
    );
}

#[test]
fn failure_anchor_phase_keeps_only_required_read_from_bundled_turn() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec!["tests/issues/issue_474.rs".to_string()],
        fast_loop_commands: vec![
            "cargo test --quiet --features serde --test issues issue_474".to_string(),
        ],
        expected_touch_targets: vec!["Cargo.toml".to_string()],
        companion_files_required: Vec::new(),
        named_tests: Vec::new(),
        current_hypothesis: None,
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some(
            "at tests/issues/issue_474.rs:6 | assertion unresolved imports/crates: chrono, uuid"
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
        phase: BenchmarkRepairPhase::NeedsFailureAnchorRead,
        owner_path: "tests/issues/issue_474.rs".to_string(),
        primary_failure_test_name: Some("issue_474".to_string()),
        failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 1,
            end_line: 30,
        }),
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
    let mut turn = AgentTurnResponse {
        assistant_message: String::new(),
        actions: vec![
            AgentAction::ReadFile {
                path: "tests/issues/issue_474.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 30,
                }),
            },
            AgentAction::RunCommand {
                command: "cargo test --quiet --features serde --test issues issue_474".to_string(),
                timeout_ms: 120_000,
            },
        ],
        task_updates: Vec::new(),
        memory_updates: Vec::new(),
        requested_mode_change: None,
        verifier_plan: None,
        parse_warnings: Vec::new(),
    };

    normalize_benchmark_repair_turn_actions(&mut turn, &state);

    assert_eq!(turn.actions.len(), 1);
    assert!(matches!(
        &turn.actions[0],
        AgentAction::ReadFile { path, .. } if path == "tests/issues/issue_474.rs"
    ));
    assert!(
        turn.parse_warnings
            .iter()
            .any(|warning| { warning.contains("Kept only the legal repair-phase next action") })
    );
}

