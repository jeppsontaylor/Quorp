    use super::*;
    use futures::FutureExt;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct RecordingToolExecutor {
        outcomes: Mutex<VecDeque<Result<String, String>>>,
        actions: Mutex<Vec<AgentAction>>,
        rollback_flags: Mutex<Vec<bool>>,
    }

    impl RecordingToolExecutor {
        fn new(outcomes: Vec<Result<String, String>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                actions: Mutex::new(Vec::new()),
                rollback_flags: Mutex::new(Vec::new()),
            }
        }

        fn executed_actions(&self) -> Vec<AgentAction> {
            self.actions.lock().expect("actions lock").clone()
        }

        fn rollback_flags(&self) -> Vec<bool> {
            self.rollback_flags
                .lock()
                .expect("rollback flags lock")
                .clone()
        }
    }

    impl ToolExecutor for RecordingToolExecutor {
        fn execute<'a>(
            &'a self,
            request: ToolExecutionRequest,
        ) -> BoxFuture<'a, Result<ToolExecutionResult, String>> {
            async move {
                self.actions
                    .lock()
                    .expect("actions lock")
                    .push(request.action.clone());
                self.rollback_flags
                    .lock()
                    .expect("rollback flags lock")
                    .push(request.enable_rollback_on_validation_failure);
                let response = self
                    .outcomes
                    .lock()
                    .expect("outcomes lock")
                    .pop_front()
                    .unwrap_or_else(|| Ok("ok".to_string()));
                let outcome = match response {
                    Ok(output) => ActionOutcome::Success {
                        action: request.action,
                        output,
                    },
                    Err(error) => ActionOutcome::Failure {
                        action: request.action,
                        error,
                    },
                };
                Ok(ToolExecutionResult { outcome })
            }
            .boxed()
        }
    }

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

        assert!(turn.parse_warnings.iter().any(|warning| {
            warning.contains("Filled placeholder expected_hash for ModifyToml")
        }));
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

        assert!(turn.parse_warnings.iter().any(|warning| {
            warning.contains("Replaced benchmark manifest PreviewEdit operations")
        }));
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
            turn.parse_warnings.iter().any(|warning| {
                warning.contains("Recovered malformed native manifest tool call")
            })
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
    fn source_patch_phase_rewrites_post_slice_read_loop_to_exact_patch() {
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
    fn source_patch_phase_rewrites_chrono_read_loop_to_epoch_round_patch() {
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
            AgentAction::WriteFile {
                path,
                content,
            }
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
    fn source_patch_phase_rewrites_axum_read_loop_to_fallback_patch() {
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
            last_validation_failure: Some(
                "merging_routers_with_fallbacks_panics failed".to_string(),
            ),
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
        let error =
            "Structured agent turn was invalid JSON: expected `,` or `}` at line 1 column 343";

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

        assert_eq!(turn.actions.len(), 1);
        assert!(matches!(
            &turn.actions[0],
            AgentAction::ReadFile { path, .. } if path == "tests/issues/issue_474.rs"
        ));
        assert!(
            turn.parse_warnings.iter().any(|warning| {
                warning.contains("Kept only the legal repair-phase next action")
            })
        );
    }

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
            turn.parse_warnings.iter().any(|warning| {
                warning.contains("Recovered malformed manifest PreviewEdit JSON")
            })
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
            patched
                .contains("pub fn try_compile_intermediates(&self) -> Result<Vec<PathBuf>, Error>")
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

    #[test]
    fn target_lease_redirects_patch_tools_to_implementation_file() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            expected_touch_targets: vec!["src/lib.rs".to_string()],
            owner_files: vec!["src/lib.rs".to_string(), "tests/issues.rs".to_string()],
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                diagnostic_class: Some("test_assertion_failure".to_string()),
                ..BenchmarkValidationDetails::default()
            },
            ..BenchmarkCaseLedger::default()
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/lib.rs".to_string(),
            ..BenchmarkRepairState::default()
        });
        state.sync_benchmark_repair_state_to_ledger();

        let error = state
            .allow_action(&AgentAction::PreviewEdit {
                path: "tests/issues.rs".to_string(),
                edit: crate::agent_protocol::PreviewEditPayload::ReplaceBlock {
                    search_block: "old".to_string(),
                    replace_block: "new".to_string(),
                    range: None,
                },
            })
            .expect_err("test evidence preview should redirect to lease");

        assert!(error.contains("target lease redirect"));
        assert!(error.contains("src/lib.rs"));
    }

    #[test]
    fn benchmark_policy_keeps_owner_test_files_read_only_unless_explicit_touch_targets() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            expected_touch_targets: vec!["src/lib.rs".to_string()],
            owner_files: vec!["src/lib.rs".to_string(), "tests/issues.rs".to_string()],
            ..BenchmarkCaseLedger::default()
        });

        let error = state
            .allow_action(&AgentAction::ReplaceBlock {
                path: "tests/issues.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            })
            .expect_err("owner test file should stay read-only unless explicitly touchable");

        assert!(error.contains("refused test-file edit"));
    }

    #[test]
    fn patch_phase_allows_one_read_only_scaffold_before_write() {
        let ledger = BenchmarkCaseLedger {
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                ..BenchmarkValidationDetails::default()
            },
            ..BenchmarkCaseLedger::default()
        };
        let scaffold = AgentAction::PreviewEdit {
            path: "src/lib.rs".to_string(),
            edit: crate::agent_protocol::PreviewEditPayload::ReplaceBlock {
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            },
        };
        let memory = AgentRepairMemory::default();

        assert!(patch_phase_actions_are_valid(
            &[scaffold],
            "src/lib.rs",
            &ledger,
            &[],
            &memory,
            false,
        ));

        let mut memory_after_scaffold = AgentRepairMemory::default();
        memory_after_scaffold.scorecard.preview_edit_count = 1;
        assert!(!patch_phase_actions_are_valid(
            &[AgentAction::ReadFile {
                path: "src/lib.rs".to_string(),
                range: None,
            }],
            "src/lib.rs",
            &ledger,
            &[],
            &memory_after_scaffold,
            false,
        ));
    }

    #[test]
    fn patch_phase_rejects_apply_preview_for_different_owner() {
        let ledger = BenchmarkCaseLedger {
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                ..BenchmarkValidationDetails::default()
            },
            ..BenchmarkCaseLedger::default()
        };
        let mut memory = AgentRepairMemory::default();
        memory.scorecard.preview_created_count = 1;
        memory.last_preview_id = Some("pv_manifest".to_string());
        memory.last_preview_path = Some("Cargo.toml".to_string());

        assert!(!patch_phase_actions_are_valid(
            &[AgentAction::ApplyPreview {
                preview_id: "pv_manifest".to_string(),
            }],
            "src/features/serde/de_owned.rs",
            &ledger,
            &[],
            &memory,
            true,
        ));

        memory.last_preview_path = Some("src/features/serde/de_owned.rs".to_string());
        assert!(patch_phase_actions_are_valid(
            &[AgentAction::ApplyPreview {
                preview_id: "pv_source".to_string(),
            }],
            "src/features/serde/de_owned.rs",
            &ledger,
            &[],
            &memory,
            true,
        ));
    }

    #[test]
    fn baseline_validation_message_requires_known_fast_loop() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            owner_files: vec!["src/lib.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet issue_123".to_string()],
            ..BenchmarkCaseLedger::default()
        });

        assert!(state.benchmark_needs_baseline_validation());
        let message = state
            .benchmark_baseline_validation_message()
            .expect("baseline message");

        assert!(message.contains("Required next action"));
        assert!(message.contains("cargo test --quiet issue_123"));
        assert!(message.contains("\"RunCommand\""));
    }

    struct NoopEventSink;

    impl RuntimeEventSink for NoopEventSink {
        fn emit(&self, _event: RuntimeEvent) {}
    }

    struct PanicCompletionClient;

    impl CompletionClient for PanicCompletionClient {
        fn request_completion<'a>(
            &'a self,
            _request: &'a CompletionRequest,
        ) -> BoxFuture<'a, Result<CompletionResponse, String>> {
            async move { panic!("completion client should not be called in this test") }.boxed()
        }
    }

    #[derive(Default)]
    struct RecordingEventSink {
        events: Mutex<Vec<RuntimeEvent>>,
    }

    impl RecordingEventSink {
        fn events(&self) -> Vec<RuntimeEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    impl RuntimeEventSink for RecordingEventSink {
        fn emit(&self, event: RuntimeEvent) {
            self.events.lock().expect("events lock").push(event);
        }
    }

    fn test_config() -> AgentConfig {
        AgentConfig {
            validation: crate::agent_context::ValidationCommands {
                fmt_command: Some("cargo fmt --check".to_string()),
                clippy_command: Some(
                    "cargo clippy --all-targets --no-deps -- -D warnings".to_string(),
                ),
                workspace_test_command: Some("cargo test".to_string()),
                targeted_test_prefix: Some("cargo test ".to_string()),
            },
            policy: PolicySettings {
                allow: crate::agent_context::PolicyAllow {
                    mcp_call_tool: true,
                    ..crate::agent_context::PolicyAllow::default()
                },
                ..PolicySettings::default()
            },
            ..AgentConfig::default()
        }
    }

    fn test_request(project_root: &TempDir) -> AgentRunRequest {
        AgentRunRequest {
            session_id: 1,
            goal: "fix the bug".to_string(),
            initial_context: Vec::new(),
            model_id: "test-model".to_string(),
            agent_mode: AgentMode::Act,
            base_url_override: None,
            max_iterations: 8,
            verifier_drain_budget: 4,
            max_total_tokens: None,
            max_seconds: None,
            autonomy_profile: AutonomyProfile::AutonomousSandboxed,
            project_root: project_root.path().to_path_buf(),
            cwd: project_root.path().to_path_buf(),
            enable_rollback_on_validation_failure: true,
            completion_policy: CompletionPolicy::default(),
            parser_recovery_budget: 2,
            run_metadata: serde_json::Value::Null,
            cancellation_flag: None,
        }
    }

    fn render_turn(actions: Vec<AgentAction>, verifier_plan: Option<ValidationPlan>) -> String {
        serde_json::to_string(&AgentTurnResponse {
            assistant_message: "working".to_string(),
            actions,
            task_updates: Vec::new(),
            memory_updates: Vec::new(),
            requested_mode_change: None,
            verifier_plan,
            parse_warnings: Vec::new(),
        })
        .expect("serialize turn")
    }

    fn seed_chrono_needs_patch_state(state: &mut AgentTaskState) {
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Fix duration_round close to epoch behavior".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_round_close_to_epoch' panicked at src/round.rs:789:44:"
                        .to_string(),
                ),
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
    }

    fn seed_chrono_needs_failure_anchor_read_state(state: &mut AgentTaskState) {
        seed_chrono_needs_patch_state(state);
        if let Some(repair_state) = state.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsFailureAnchorRead;
            repair_state.last_owner_slice = None;
            repair_state.failure_anchor_reread_attempted = false;
            repair_state.failure_anchor_reread_honored = false;
            repair_state.invalid_action_count = 0;
        }
        state.sync_benchmark_repair_state_to_ledger();
    }

    #[test]
    fn failure_anchor_phase_accepts_read_only_evidence_actions() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_failure_anchor_read_state(&mut state);

        assert!(state.benchmark_evidence_action_satisfies(
            "src/round.rs",
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            &AgentAction::SearchText {
                query: "duration_round".to_string(),
                limit: 20,
            },
        ));
        assert!(state.benchmark_evidence_action_satisfies(
            "src/round.rs",
            None,
            &AgentAction::SuggestEditAnchors {
                path: "src/round.rs".to_string(),
                range: None,
                search_hint: Some("duration_round".to_string()),
            },
        ));
        assert!(state.benchmark_evidence_action_satisfies(
            "src/round.rs",
            None,
            &AgentAction::ReadFile {
                path: "tests/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 20,
                }),
            },
        ));
    }

    #[test]
    fn controller_injects_required_failure_anchor_read_after_missing_repair_turn() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_failure_anchor_read_state(&mut state);
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 781-813\nhonored_range: 781-813\nfn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "I need to inspect the failure before patching.",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("controller read injection should succeed");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(
            executor.executed_actions(),
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 781,
                    end_line: 813,
                }),
            }]
        );
        assert_eq!(
            state
                .agent_repair_memory
                .scorecard
                .controller_injected_read_count,
            1
        );
        assert_eq!(state.agent_repair_memory.scorecard.parser_recovery_count, 1);
        assert_eq!(state.parser_recovery_failures, 0);
        assert!(state.last_parse_error.is_none());
        assert!(
            state
                .agent_repair_memory
                .scorecard
                .first_valid_write_step
                .is_none()
        );
        assert_eq!(state.agent_repair_memory.observed_slices.len(), 1);
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(BenchmarkRepairPhase::NeedsPatch)
        );
        assert!(
            transcript
                .iter()
                .any(|message| message.content.contains("[Repair Controller]"))
        );
    }

    #[test]
    fn line_oriented_repair_read_updates_memory_without_controller_write() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_failure_anchor_read_state(&mut state);
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 781-813\nhonored_range: 781-813\nfn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "read_file src/round.rs range=[781, 813]",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("line-oriented read should succeed");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(
            state
                .agent_repair_memory
                .scorecard
                .line_oriented_parse_count,
            1
        );
        assert_eq!(
            state
                .agent_repair_memory
                .scorecard
                .controller_injected_read_count,
            0
        );
        assert_eq!(state.agent_repair_memory.observed_slices.len(), 1);
        assert!(
            state
                .agent_repair_memory
                .scorecard
                .first_valid_write_step
                .is_none()
        );
    }

    #[test]
    fn agent_repair_memory_persists_in_checkpoint_snapshot() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.record_line_oriented_parse();
        state.record_controller_injected_read();
        state.record_observed_slice(
            "src/round.rs",
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            Some("needs_failure_anchor_read".to_string()),
            "fn duration_round(value: i64) -> i64 { value }",
            None,
        );

        let snapshot = state.snapshot();
        assert_eq!(snapshot.agent_repair_memory.observed_slices.len(), 1);
        assert_eq!(
            snapshot
                .agent_repair_memory
                .scorecard
                .line_oriented_parse_count,
            1
        );
        assert_eq!(
            snapshot
                .agent_repair_memory
                .scorecard
                .controller_injected_read_count,
            1
        );
    }

    #[test]
    fn verifier_drain_runs_queued_validation_after_model_budget() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.max_iterations = 0;
        request.verifier_drain_budget = 2;
        let checkpoint = AgentCheckpoint {
            snapshot: AgentTaskStateSnapshot {
                current_mode: AgentMode::Act,
                acceptance_criteria: vec!["finish".to_string()],
                working_set: BTreeSet::new(),
                last_tool_summary: None,
                last_failing_verifier: None,
                last_safe_checkpoint: None,
                last_parse_error: None,
                stall_count: 0,
                redundant_inspection_turns: 0,
                recoverable_inspection_failures: 0,
                parser_recovery_failures: 0,
                parser_recovery_validation_fingerprint: None,
                parser_recovery_same_validation_streak: 0,
                has_mutating_change: true,
                verified_green: false,
                validation_queue: VecDeque::from([ValidationPlan {
                    fmt: true,
                    clippy: false,
                    workspace_tests: false,
                    tests: Vec::new(),
                    custom_commands: Vec::new(),
                }]),
                total_billed_tokens: 0,
                last_failed_tool_error: None,
                repair_recovery_turns_remaining: 0,
                benchmark_case_ledger: None,
                repair_requirement: None,
                last_successful_write_action: None,
                benchmark_repair_state: None,
                failed_edit_records: Vec::new(),
                agent_repair_memory: AgentRepairMemory::default(),
            },
            transcript: Vec::new(),
            step: 0,
            request_counter: 1,
        };
        let executor = RecordingToolExecutor::new(vec![Ok("green".to_string())]);
        let sink = RecordingEventSink::default();

        let outcome = futures::executor::block_on(run_agent_task(
            &request,
            &PanicCompletionClient,
            &executor,
            &sink,
            Some(checkpoint),
        ));

        assert_eq!(outcome.stop_reason, StopReason::Success);
        let events = sink.events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::VerifierDrainStarted { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::VerifierDrainFinished {
                verified_green: true,
                ..
            }
        )));
    }

    #[test]
    fn explicit_validation_satisfies_latest_write_boundary() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor =
            RecordingToolExecutor::new(vec![Ok("wrote".to_string()), Ok("green".to_string())]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::WriteFile {
                    path: "README.md".to_string(),
                    content: "updated".to_string(),
                },
                AgentAction::RunValidation {
                    plan: ValidationPlan {
                        fmt: true,
                        clippy: true,
                        workspace_tests: true,
                        tests: Vec::new(),
                        custom_commands: Vec::new(),
                    },
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should succeed");

        assert!(matches!(control_flow, ControlFlow::BreakSuccess));
        assert!(state.validation_queue.is_empty());
        assert!(state.verified_green);
        assert_eq!(executor.executed_actions().len(), 2);
    }

    #[test]
    fn write_only_turn_queues_post_edit_validation_once() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Ok("wrote".to_string())]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::WriteFile {
                path: "README.md".to_string(),
                content: "updated".to_string(),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should succeed");

        let queued = state
            .validation_queue
            .iter()
            .map(ValidationPlan::summary)
            .collect::<Vec<_>>();
        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(executor.executed_actions().len(), 1);
        assert_eq!(
            queued,
            vec!["fmt".to_string(), "workspace_tests".to_string()]
        );
    }

    #[test]
    fn multiple_writes_do_not_duplicate_validation_queue() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Ok("write one".to_string()),
            Ok("write two".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::WriteFile {
                    path: "README.md".to_string(),
                    content: "updated".to_string(),
                },
                AgentAction::ReplaceBlock {
                    path: "README.md".to_string(),
                    search_block: "updated".to_string(),
                    replace_block: "updated again".to_string(),
                    range: None,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should succeed");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(state.validation_queue.len(), 2);
        assert_eq!(executor.executed_actions().len(), 2);
    }

    #[test]
    fn failed_batch_does_not_leave_hidden_validation_queue() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Ok("write one".to_string()),
            Err("replace failed".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::WriteFile {
                    path: "README.md".to_string(),
                    content: "updated".to_string(),
                },
                AgentAction::ReplaceBlock {
                    path: "README.md".to_string(),
                    search_block: "missing".to_string(),
                    replace_block: "replacement".to_string(),
                    range: None,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue after failure");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(state.has_mutating_change);
        assert!(!state.verified_green);
        assert!(state.validation_queue.is_empty());
        assert_eq!(state.repair_recovery_turns_remaining, 1);
        assert!(
            transcript
                .iter()
                .any(|message| { message.content.contains("[Batch execution aborted]") })
        );
        assert!(
            transcript
                .iter()
                .any(|message| { message.content.contains("[Repair Brief]") })
        );
    }

    #[test]
    fn failed_edit_plain_reread_is_rejected_with_range_guidance() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Err("replace failed".to_string()),
            Ok("exact code".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let failed_edit_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            }],
            None,
        );

        let first_control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failed_edit_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failed edit should continue");

        assert!(matches!(first_control_flow, ControlFlow::Continue));
        assert_eq!(state.repair_recovery_turns_remaining, 1);

        let repair_read_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: None,
            }],
            None,
        );

        let second_control_flow = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &repair_read_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("plain reread should be redirected");

        assert!(matches!(second_control_flow, ControlFlow::Continue));
        assert_eq!(state.repair_recovery_turns_remaining, 1);
        assert_eq!(executor.executed_actions().len(), 1);
        assert!(
            state
                .repair_requirement
                .as_ref()
                .is_some_and(|requirement| !requirement.exact_reread_completed)
        );
        assert!(
            transcript
                .iter()
                .any(|message| message.content.contains("requires a focused `ReadFile`"))
        );
    }

    #[test]
    fn failed_edit_blocks_followup_write_until_fresh_reread() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Err("replace failed".to_string())]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let failed_edit_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            }],
            None,
        );

        futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failed_edit_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failed edit should continue");

        let patch_without_reread = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "newer".to_string(),
                range: None,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &patch_without_reread,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("follow-up write should trigger deterministic reread recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(
            executor
                .executed_actions()
                .iter()
                .filter(|action| action.is_write_like())
                .count()
                == 1
        );
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("previous edit failed and the repair target must be reread first")
        }));
    }

    #[test]
    fn rolled_back_validation_failure_requires_reread_of_last_write_target() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.last_successful_write_action = Some(AgentAction::ReplaceBlock {
            path: "src/round.rs".to_string(),
            search_block: "old".to_string(),
            replace_block: "new".to_string(),
            range: None,
        });
        if let Some(repair_state) = state.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsFastLoopRerun;
        }
        let failure = "unexpected closing delimiter\nError writing files: failed to resolve mod `round`: cannot parse /tmp/work/src/round.rs:10:1\n[System] Changes were safely rolled back.";

        state.observe_outcome(&ActionOutcome::Failure {
            action: AgentAction::RunValidation {
                plan: ValidationPlan {
                    fmt: true,
                    ..ValidationPlan::default()
                },
            },
            error: failure.to_string(),
        });

        let requirement = state
            .repair_requirement
            .as_ref()
            .expect("rolled back write should require a focused reread");
        assert_eq!(requirement.path, "src/round.rs");
        assert_eq!(
            requirement.suggested_range,
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 2,
                end_line: 34,
            })
        );
        assert_eq!(state.repair_recovery_turns_remaining, 1);
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(BenchmarkRepairPhase::NeedsPatch)
        );
    }

    #[test]
    fn failed_edit_ranged_reread_unlocks_followup_write() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Err("replace failed at src/round.rs:778:44".to_string()),
            Ok("[read_file]\npath: src/round.rs\nrequested_range: 770-790\nhonored_range: 770-790\nfn excerpt() {}\n".to_string()),
            Ok("patch applied".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let failed_edit_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            }],
            None,
        );

        futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failed_edit_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failed edit should continue");

        let reread_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 770,
                    end_line: 790,
                }),
            }],
            None,
        );
        futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &reread_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("ranged reread should continue");

        assert!(
            state
                .repair_requirement
                .as_ref()
                .is_some_and(|requirement| requirement.exact_reread_completed)
        );

        let followup_patch_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "newer".to_string(),
                range: None,
            }],
            None,
        );

        let result = futures::executor::block_on(handle_model_turn(
            3,
            ModelTurnInput {
                content: &followup_patch_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ));

        assert!(matches!(result, Ok(ControlFlow::Continue)));
    }

    #[test]
    fn failed_patch_phase_edit_suppresses_patch_packet_until_reread() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        let executor = RecordingToolExecutor::new(vec![Err(
            "replace_block: Search block is ambiguous; found 2 matches".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let failed_edit_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "if span < 0".to_string(),
                replace_block: "if span < 0".to_string(),
                range: None,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failed_edit_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failed edit should continue to repair reread");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(state.repair_requirement_needs_reread());
        let joined = transcript
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("[Repair Brief]"));
        assert!(joined.contains("fresh `ReadFile`"));
        assert!(!joined.contains("[Patch Packet]"));
    }

    #[test]
    fn failed_apply_patch_without_line_anchor_gets_fallback_reread_range() {
        let requirement = repair_requirement_from_action(
            &AgentAction::ApplyPatch {
                path: "Cargo.toml".to_string(),
                patch: "not a valid patch".to_string(),
            },
            "apply_patch expects a unified diff patch or SEARCH/REPLACE blocks",
        )
        .expect("apply patch failure should create repair requirement");

        assert_eq!(requirement.path, "Cargo.toml");
        assert_eq!(
            requirement.suggested_range,
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 1,
                end_line: 120,
            })
        );
    }

    #[test]
    fn needs_patch_phase_allows_failed_edit_focused_reread() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.repair_requirement = Some(RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "replace_block: Search block is ambiguous; found 2 matches".to_string(),
            previous_search_block: Some("if span < 0".to_string()),
            suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            exact_reread_completed: false,
        });
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 149-215\nhonored_range: 149-215\nfn duration_round() {}\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let reread_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &reread_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("focused reread should be allowed during failed edit recovery");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(executor.executed_actions().len(), 1);
        assert!(
            state
                .repair_requirement
                .as_ref()
                .is_some_and(|requirement| requirement.exact_reread_completed)
        );
        assert!(transcript.iter().all(|message| {
            !message
                .content
                .contains("does not satisfy the current repair step")
        }));
    }

    #[test]
    fn needs_patch_phase_failed_edit_correction_preserves_reread_requirement() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.repair_requirement = Some(RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "replace_block: Search block is ambiguous; found 2 matches".to_string(),
            previous_search_block: Some("if span < 0".to_string()),
            suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            exact_reread_completed: false,
        });

        let correction = state
            .benchmark_repair_phase_correction_message(&[AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "if span < 0".to_string(),
                replace_block: "if span <= 0".to_string(),
                range: None,
            }])
            .expect("correction")
            .expect("message");

        assert!(correction.contains("fresh `ReadFile`"));
        assert!(correction.contains("Suggested reread range: 149-215"));
        assert!(!correction.contains("[Patch Packet]"));
        assert!(!correction.contains("Do not reread"));
    }

    #[test]
    fn needs_patch_phase_rejects_bare_replace_after_ambiguous_failure() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.failed_edit_records.push(FailedEditRecord {
            action_kind: "replace_block".to_string(),
            path: "src/round.rs".to_string(),
            search_hash: Some("search".to_string()),
            replace_hash: Some("replace".to_string()),
            failure_reason: "Search block is ambiguous; found 2 matches at lines 151, 188"
                .to_string(),
            matching_line_numbers: vec![151, 188],
            attempts: 1,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let correction = state
            .benchmark_repair_phase_correction_message(&[AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "if span < 0".to_string(),
                replace_block: "if span <= 0".to_string(),
                range: None,
            }])
            .expect("correction")
            .expect("bare replace should be rejected");

        assert!(correction.contains("Bare `ReplaceBlock` was rejected"));
        assert!(correction.contains("Failed edit memory:"));

        let ranged = state
            .benchmark_repair_phase_correction_message(&[AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "if span < 0".to_string(),
                replace_block: "if span <= 0".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
            }])
            .expect("ranged correction");
        assert!(ranged.is_none());
    }

    #[test]
    fn failed_write_records_failed_edit_event() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Err(
            "Search block is ambiguous; found 2 matches at lines 12, 20".to_string(),
        )]);
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/lib.rs".to_string(),
                search_block: "old".to_string(),
                replace_block: "new".to_string(),
                range: None,
            }],
            None,
        );

        futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failed edit should stay recoverable");

        assert!(sink.events().iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::FailedEditRecorded {
                    record,
                    ..
                } if record.path == "src/lib.rs"
                    && record.matching_line_numbers == vec![12, 20]
            )
        }));
    }

    #[test]
    fn benchmark_read_file_observation_prefers_compact_anchored_excerpt() {
        let action = AgentAction::ReadFile {
            path: "src/round.rs".to_string(),
            range: None,
        };
        let output = format!(
            "[read_file]\npath: src/round.rs\n{}",
            (1..=80)
                .map(|line| {
                    if line == 35 {
                        "pub fn duration_rounding_mode() {".to_string()
                    } else if line == 36 {
                        "    let epoch = timestamp - 1;".to_string()
                    } else {
                        format!("line {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
        let repair_requirement = RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "replace failed".to_string(),
            previous_search_block: Some("let epoch = timestamp - 1;".to_string()),
            suggested_range: None,
            exact_reread_completed: false,
        };

        let rendered = summarize_tool_observation_for_transcript(
            &action,
            "success",
            &output,
            true,
            Some(&repair_requirement),
            None,
        );

        assert!(rendered.contains("footprint:"));
        assert!(rendered.contains("[anchored excerpt lines"));
        assert!(rendered.contains("let epoch = timestamp - 1;"));
    }

    #[test]
    fn read_only_turn_does_not_finish_autonomous_run() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Ok("read".to_string())]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/lib.rs".to_string(),
                range: None,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(!state.has_mutating_change);
        assert!(!state.verified_green);
    }

    #[test]
    fn read_only_failure_continues_remaining_inspection_actions() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Err("missing directory".to_string()),
            Ok("read succeeded".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::ListDirectory {
                    path: "src".to_string(),
                },
                AgentAction::ReadFile {
                    path: "crates/orders-core/src/lib.rs".to_string(),
                    range: None,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(executor.executed_actions().len(), 2);
        assert!(
            transcript
                .iter()
                .any(|message| { message.content.contains("[Batch execution continued]") })
        );
    }

    #[test]
    fn read_only_path_failure_queues_recovery_turn_without_stall() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Err(
            "list_directory: Path does not exist\nrequest_path: workspace/crates/reconciliation-core\nsuggested_path: crates/reconciliation-core\nreason: redundant_workspace_prefix"
                .to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ListDirectory {
                path: "workspace/crates/reconciliation-core".to_string(),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.stall_count, 0);
        assert_eq!(state.recoverable_inspection_failures, 1);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("Suggested next path: crates/reconciliation-core")
        }));
    }

    #[test]
    fn repeated_recoverable_inspection_failures_exhaust_budget() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![
            Err("read_file: Path does not exist\nrequest_path: workspace/foo-1".to_string()),
            Err("read_file: Path does not exist\nrequest_path: workspace/foo-2".to_string()),
            Err("read_file: Path does not exist\nrequest_path: workspace/foo-3".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        for step in 1..=2 {
            let turn = render_turn(
                vec![AgentAction::ReadFile {
                    path: format!("workspace/foo-{step}"),
                    range: None,
                }],
                None,
            );
            let control_flow = futures::executor::block_on(handle_model_turn(
                step,
                ModelTurnInput {
                    content: &turn,
                    native_turn: None,
                    native_turn_error: None,
                    output_truncated: false,
                },
                &mut state,
                &request,
                &executor,
                &sink,
                &mut transcript,
            ))
            .expect("recoverable failure should continue");
            assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        }

        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "workspace/foo-3".to_string(),
                range: None,
            }],
            None,
        );
        let error = match futures::executor::block_on(handle_model_turn(
            3,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        )) {
            Ok(_) => panic!("third recoverable failure should exhaust budget"),
            Err(error) => error,
        };

        assert!(error.contains("recovery budget exhausted"));
    }

    #[test]
    fn truncated_model_output_requests_compact_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"cut off",
                native_turn: None,
                native_turn_error: None,
                output_truncated: true,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("previous structured JSON was truncated")
        }));
        assert!(executor.executed_actions().is_empty());
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "output_truncated"
        )));
    }

    #[test]
    fn truncated_plain_text_without_complete_json_requests_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "I'll inspect the billing domain first.\n```json\n{\"assistant_message\":\"I'll inspect billing-domain\",",
                native_turn: None,
                native_turn_error: None,
                output_truncated: true,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("previous structured JSON was truncated")
        }));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "output_truncated"
        )));
    }

    #[test]
    fn malformed_control_character_json_requests_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\n  \"assistant_message\": \"bad\u{0001}json\"\n}",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("previous structured JSON was malformed")
        }));
        assert!(executor.executed_actions().is_empty());
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "malformed"
        )));
    }

    #[test]
    fn needs_patch_phase_parse_recovery_message_includes_patch_contract() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some(
                "Patch duration_trunc and immediately rerun the failing tests".to_string(),
            ),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_trunc_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(778),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ],
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:"
                        .to_string(),
                ),
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_trunc_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 188,
                end_line: 254,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_trunc(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"patching src/round.rs\",\"actions\":[",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("patch phase parse recovery should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("You are still in patch phase"));
        assert!(recovery_message.contains("Owner path: src/round.rs"));
        assert!(recovery_message.contains("Recommended rerun command:"));
        assert!(recovery_message.contains("\"ReplaceRange\""));
        assert!(recovery_message.contains("\"RunCommand\""));
    }

    #[test]
    fn needs_fast_loop_rerun_phase_parse_recovery_message_includes_rerun_contract() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Rerun the narrowed fast loop".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_trunc_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(778),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ],
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFastLoopRerun,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_trunc_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 188,
                end_line: 254,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_trunc(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"rerunning tests\",\"actions\":[",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("rerun phase parse recovery should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("fast-loop rerun phase"));
        assert!(recovery_message.contains("Recommended rerun command:"));
        assert!(recovery_message.contains("\"RunCommand\""));
    }

    #[test]
    fn needs_patch_phase_plain_text_response_queues_patch_specific_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Patch duration_round and rerun the fast loop".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_min_max".to_string(),
                ],
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "I will patch src/round.rs now and rerun the fast loop.",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("plain patch-phase prose should queue parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("You are still in patch phase"));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "missing_json_object"
        )));
    }

    #[test]
    fn needs_implementation_read_plain_text_response_queues_focused_read_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        if let Some(repair_state) = state.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsImplementationRead;
            repair_state.last_owner_slice = None;
            repair_state.implementation_reread_attempted = false;
            repair_state.implementation_reread_honored = false;
        }
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "Reading the suggested implementation slice in `src/round.rs`.",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("plain implementation-read prose should queue parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("focused-read phase"));
        assert!(recovery_message.contains("Read one implementation slice now"));
        assert!(recovery_message.contains("Minimal JSON example"));
        assert!(recovery_message.contains("\"ReadFile\""));
        assert!(recovery_message.contains("\"range\""));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "missing_json_object"
        )));
    }

    #[test]
    fn invalid_repair_action_resets_parser_recovery_counter() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        if let Some(repair_state) = state.benchmark_repair_state.as_mut() {
            repair_state.phase = BenchmarkRepairPhase::NeedsFailureAnchorRead;
            repair_state.failure_anchor_reread_attempted = false;
            repair_state.failure_anchor_reread_honored = false;
        }
        state.parser_recovery_failures = 1;
        state.last_parse_error = Some("missing_json_object".to_string());
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ListDirectory {
                path: ".".to_string(),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("wrong repair action should queue correction");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.parser_recovery_failures, 0);
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.invalid_action_count),
            Some(1)
        );
        let correction_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("correction message");
        assert!(correction_message.contains("Rejected turn plan: list_directory"));
        assert!(correction_message.contains("Minimal JSON example"));
        assert!(correction_message.contains("\"ReadFile\""));
    }

    #[test]
    fn benchmark_plain_text_response_queues_owner_file_parser_recovery() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "I should inspect src/round.rs next.",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("plain benchmark prose should queue parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.stall_count, 0);
        assert_eq!(state.parser_recovery_failures, 1);
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("missing_json_object"));
        assert!(recovery_message.contains("read the primary owner file `src/round.rs`"));
        assert!(recovery_message.contains("\"ReadFile\":{\"path\":\"src/round.rs\"}"));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "missing_json_object"
        )));
    }

    #[test]
    fn benchmark_fast_loop_prose_is_recovered_to_known_command() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/features/serde/de_owned.rs".to_string()],
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
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        });
        let executor = RecordingToolExecutor::new(vec![Err(
            "error[E0432]: unresolved import `chrono`".to_string(),
        )]);
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "Running the fast loop test to see the current failure.",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("fast-loop prose should become an executable command");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(state.parser_recovery_failures, 0);
        assert!(transcript.iter().all(|message| {
            !message.content.contains("missing_json_object")
                && !message.content.contains("parser recovery")
        }));
        assert!(
            sink.events()
                .iter()
                .all(|event| { !matches!(event, RuntimeEvent::ParseRecoveryQueued { .. }) })
        );
        assert!(matches!(
            executor.executed_actions().as_slice(),
            [AgentAction::RunCommand { command, timeout_ms }]
                if command == "cargo test --quiet --features serde --test issues issue_474"
                    && *timeout_ms == 120_000
        ));
    }

    #[test]
    fn benchmark_support_write_validation_preserves_manifest_edit_on_expected_failure() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/features/serde/de_owned.rs".to_string()],
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
            last_validation_failure: Some("unresolved imports/crates: chrono, uuid".to_string()),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                diagnostic_class: Some("manifest_dependency_error".to_string()),
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.agent_repair_memory.last_preview_result =
            Some("[preview_edit]\npath: Cargo.toml\nwould_apply: true".to_string());
        state.agent_repair_memory.preview_origin = Some("write_locked_manifest".to_string());
        state.last_successful_write_action = Some(AgentAction::ApplyPreview {
            preview_id: "pv_manifest".to_string(),
        });
        let executor = RecordingToolExecutor::new(vec![Err(
            "thread 'issue_474::test' panicked: CannotBorrowOwnedData".to_string(),
        )]);
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let outcome = futures::executor::block_on(dispatch_action(
            1,
            &mut state,
            AgentAction::RunValidation {
                plan: ValidationPlan {
                    custom_commands: vec![
                        "cargo test --quiet --features serde --test issues issue_474".to_string(),
                    ],
                    ..ValidationPlan::default()
                },
            },
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("validation dispatch should complete");

        assert!(matches!(outcome, DispatchOutcome::Failure));
        assert_eq!(executor.rollback_flags(), vec![false]);
        assert_eq!(
            state.agent_repair_memory.scorecard.support_write_count, 0,
            "the preservation check must not fake write telemetry"
        );
    }

    #[test]
    fn malformed_actions_field_is_recoverable_parse_error() {
        assert!(is_recoverable_structured_parse_error(
            "Structured agent turn `actions` field was invalid: missing field `replace_block`"
        ));
    }

    #[test]
    fn needs_patch_phase_empty_actions_queue_patch_specific_retry() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Patch duration_round and rerun the fast loop".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_min_max".to_string(),
                ],
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: r#"{"assistant_message":"patching src/round.rs","actions":[]}"#,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("empty patch-phase actions should queue parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("Parse error class: missing_tool_call"));
        assert!(recovery_message.contains("You are still in patch phase"));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "missing_tool_call"
        )));
    }

    #[test]
    fn parser_recovery_failure_can_be_followed_by_successful_turn() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(vec![Ok("read".to_string())]);
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"bad\"",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("parser recovery should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.parser_recovery_failures, 1);

        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/lib.rs".to_string(),
                range: None,
            }],
            None,
        );
        let control_flow = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("follow-up turn should succeed");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(state.parser_recovery_failures, 0);
        assert!(
            sink.events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ParseRecoveryQueued { .. }))
        );
    }

    #[test]
    fn repeated_parser_recovery_exhausts_budget() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.parser_recovery_budget = 1;
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let error = match futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"bad\"",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        )) {
            Ok(_) => panic!("parser recovery should exhaust the configured budget"),
            Err(error) => error,
        };

        assert!(error.contains("repeated parser recovery attempts"));
        assert!(
            sink.events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ParseRecoveryExhausted { .. }))
        );
    }

    #[test]
    fn repeated_parser_recovery_during_repair_stalls_when_validation_state_is_unchanged() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.parser_recovery_budget = 3;
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.sync_benchmark_repair_state_to_ledger();
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let first = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "{\"assistant_message\":\"bad\"",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("first repair parser recovery should retry");
        assert!(matches!(first, ControlFlow::ContinueNoBudget));

        let error = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: "{\"assistant_message\":\"bad\"",
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect_err("second identical repair parser recovery should stall");

        assert!(error.contains("parser recovery without changing validation state"));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryExhausted {
                error_class,
                ..
            } if error_class == "parser_recovery_stalled"
        )));
    }

    #[test]
    fn nvidia_qwen_repair_phase_uses_minimal_compaction_and_smaller_completion_cap() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.model_id = "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string();
        request.completion_policy.first_turn_max_completion_tokens = Some(3072);
        request.completion_policy.later_turn_max_completion_tokens = Some(1536);
        request.completion_policy.prompt_compaction_policy =
            Some(PromptCompactionPolicy::Last6Ledger768);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);

        assert_eq!(
            prompt_compaction_policy_for_turn(
                &request.completion_policy,
                &request.model_id,
                &state,
            ),
            Some(PromptCompactionPolicy::BenchmarkRepairMinimal)
        );
        assert_eq!(
            max_completion_tokens_for_turn(
                &request.completion_policy,
                1,
                &request.model_id,
                &state
            ),
            Some(1536)
        );
    }

    #[test]
    fn nvidia_qwen_repair_phase_uses_tighter_caps_and_state_packet_after_patch_failure() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.model_id = "nvidia/qwen/qwen3-coder-480b-a35b-instruct".to_string();
        request.completion_policy.first_turn_max_completion_tokens = Some(4096);
        request.completion_policy.later_turn_max_completion_tokens = Some(3072);
        request.completion_policy.prompt_compaction_policy =
            Some(PromptCompactionPolicy::Last6Ledger768);
        let mut state = AgentTaskState::new(&request, test_config());
        seed_chrono_needs_patch_state(&mut state);
        state.agent_repair_memory.post_patch_diagnostic_class =
            Some("manifest_feature_error".to_string());

        assert_eq!(
            prompt_compaction_policy_for_turn(
                &request.completion_policy,
                &request.model_id,
                &state,
            ),
            Some(PromptCompactionPolicy::BenchmarkStatePacket)
        );
        assert_eq!(
            max_completion_tokens_for_turn(
                &request.completion_policy,
                1,
                &request.model_id,
                &state
            ),
            Some(1536)
        );
    }

    #[test]
    fn native_tool_mode_retries_when_turn_has_no_actions() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.completion_policy.native_tool_calls = true;
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: r#"{"assistant_message":"I will inspect the crate next.","task_updates":[{"title":"inspect","status":"pending"}]}"#,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should trigger parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.parser_recovery_failures, 1);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("omitted the required concrete tool action")
        }));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "missing_tool_call"
        )));
    }

    #[test]
    fn native_tool_missing_required_field_queues_parser_recovery() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.completion_policy.native_tool_calls = true;
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "",
                native_turn: None,
                native_turn_error: Some("native tool `replace_block` was missing `path`"),
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("missing native field should be recoverable");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.parser_recovery_failures, 1);
        assert_eq!(executor.executed_actions().len(), 0);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("Parse error class: malformed_action")
        }));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "malformed_action"
        )));
    }

    #[test]
    fn native_tool_invalid_required_field_queues_parser_recovery() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.completion_policy.native_tool_calls = true;
        let mut state = AgentTaskState::new(&request, test_config());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: "",
                native_turn: None,
                native_turn_error: Some(
                    "native tool `modify_toml` had invalid `operations`: missing field `name`",
                ),
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("invalid native field should be recoverable");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(state.parser_recovery_failures, 1);
        assert_eq!(executor.executed_actions().len(), 0);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("For ModifyToml dependency operations")
        }));
        assert!(sink.events().iter().any(|event| matches!(
            event,
            RuntimeEvent::ParseRecoveryQueued {
                error_class,
                ..
            } if error_class == "malformed_action"
        )));
    }

    #[test]
    fn benchmark_repeated_validation_before_repair_write_is_rejected_recoverably() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: tests".to_string()),
            last_validation_failure: Some("round::tests::epoch failed".to_string()),
            validation_details: BenchmarkValidationDetails {
                repair_required: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state
            .agent_repair_memory
            .validation_failures
            .push(AgentRepairValidationFailure {
                command: "tests(--quiet --lib round::tests::)".to_string(),
                summary: "round::tests::epoch failed".to_string(),
            });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::RunValidation {
                plan: ValidationPlan {
                    fmt: false,
                    clippy: false,
                    workspace_tests: false,
                    tests: vec!["--lib round::tests::".to_string()],
                    custom_commands: Vec::new(),
                },
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("repeated validation should become a repair prompt, not fatal");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(executor.executed_actions().len(), 0);
        assert_eq!(state.agent_repair_memory.rejected_actions.len(), 1);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("validation already exposed the failure")
        }));
    }

    #[test]
    fn benchmark_repeated_fast_loop_command_before_repair_write_is_rejected_recoverably() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast loop".to_string()),
            last_validation_failure: Some("round::tests::epoch failed".to_string()),
            validation_details: BenchmarkValidationDetails::default(),
        });
        state
            .agent_repair_memory
            .validation_failures
            .push(AgentRepairValidationFailure {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                summary: "round::tests::epoch failed".to_string(),
            });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::RunCommand {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                timeout_ms: 30_000,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("repeated fast-loop command should become a repair prompt, not fatal");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(executor.executed_actions().len(), 0);
        assert_eq!(state.agent_repair_memory.rejected_actions.len(), 1);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("validation already exposed the failure")
        }));
    }

    #[test]
    fn benchmark_native_empty_action_recovery_points_at_owner_file() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let mut request = test_request(&project_root);
        request.completion_policy.native_tool_calls = true;
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: r#"{"assistant_message":"I should inspect the owner file now.","actions":[]}"#,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should trigger benchmark-aware parser recovery");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        let recovery_message = transcript
            .iter()
            .find(|message| message.role == TranscriptRole::User)
            .map(|message| message.content.clone())
            .expect("parser recovery message");
        assert!(recovery_message.contains("Required next action"));
        assert!(recovery_message.contains("cargo test --quiet --lib round::tests::"));
        assert!(recovery_message.contains("\"RunCommand\""));
        assert!(sink.events().is_empty());
    }

    #[test]
    fn redundant_inspection_turn_is_blocked_by_loop_guard() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.working_set.insert(".".to_string());
        state
            .working_set
            .insert("crates/orders-core/src/lib.rs".to_string());
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::ListDirectory {
                    path: ".".to_string(),
                },
                AgentAction::ReadFile {
                    path: "crates/orders-core/src/lib.rs".to_string(),
                    range: None,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(executor.executed_actions().len(), 0);
        assert!(
            transcript
                .iter()
                .any(|message| { message.content.contains("[Loop guard]") })
        );
    }

    #[test]
    fn failed_fast_loop_allows_one_same_owner_file_reread() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.working_set.insert("src/round.rs".to_string());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails::default(),
        });
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nfn excerpt() {}\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 760,
                    end_line: 811,
                }),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(executor.executed_actions().len(), 1);
        assert!(
            transcript
                .iter()
                .all(|message| !message.content.contains("[Loop guard]"))
        );
    }

    #[test]
    fn failed_fast_loop_ranged_reread_still_allowed_after_prior_loop_guard() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.working_set.insert("src/round.rs".to_string());
        state.redundant_inspection_turns = 1;
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("repair needs a focused owner-file reread".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFailureAnchorRead,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_trunc_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
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
        state.sync_benchmark_repair_state_to_ledger();

        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 770-802\nhonored_range: 770-802\nfn duration_trunc() {}\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 770,
                    end_line: 802,
                }),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("anchored reread should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(executor.executed_actions().len(), 1);
        assert!(
            transcript
                .iter()
                .all(|message| !message.content.contains("[Loop guard]"))
        );
        assert!(
            state
                .benchmark_repair_state
                .as_ref()
                .is_some_and(|repair_state| repair_state.failure_anchor_reread_attempted)
        );
    }

    #[test]
    fn anchored_repair_requires_patch_next_but_recovers_once() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("epoch truncation arithmetic is off by one".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: false,
                post_fast_loop_validation_rerun_attempted: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.repair_requirement = Some(RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "fast loop failed".to_string(),
            previous_search_block: None,
            suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 760,
                end_line: 811,
            }),
            exact_reread_completed: true,
        });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 750,
                    end_line: 811,
                }),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("repair redirection should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(executor.executed_actions().is_empty());
        assert!(
            transcript
                .iter()
                .any(|message| { message.content.contains("the next step must be a patch") })
        );
    }

    #[test]
    fn benchmark_read_file_observation_prefers_failed_fast_loop_line_hint() {
        let summary = summarize_read_file_observation(
            "src/round.rs",
            None,
            "[read_file]\npath: src/round.rs\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n21\n22\n23\n24\n25\n26\n27\n28\n29\n30\n",
            None,
            Some(&BenchmarkCaseLedger {
                case_class: "narrow-owner-first".to_string(),
                owner_files: vec!["src/round.rs".to_string()],
                fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
                expected_touch_targets: vec!["src/round.rs".to_string()],
                companion_files_required: Vec::new(),
                named_tests: Vec::new(),
                current_hypothesis: None,
                validation_status: Some("failed: fast-loop".to_string()),
                last_validation_failure: Some(
                    "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:18:44:"
                        .to_string(),
                ),
                validation_details: BenchmarkValidationDetails::default(),
            }),
        );

        assert!(summary.contains("[anchored excerpt lines"));
        assert!(summary.contains("18"));
        assert!(summary.contains("19"));
        assert!(!summary.contains("[excerpt lines 1-24"));
    }

    #[test]
    fn benchmark_read_file_observation_preserves_requested_slice_content() {
        let summary = summarize_read_file_observation(
            "src/round.rs",
            Some(crate::agent_protocol::ReadFileRange {
                start_line: 760,
                end_line: 762,
            }),
            "[read_file]\npath: src/round.rs\nrequested_range: 760-762\nhonored_range: 760-762\nlet first = 1;\nlet second = 2;\nlet third = 3;\n",
            None,
            None,
        );

        assert!(summary.contains("[requested excerpt lines 760-762"));
        assert!(summary.contains("let first = 1;"));
        assert!(summary.contains("let third = 3;"));
    }

    #[test]
    fn record_fast_loop_validation_failure_populates_structured_details() {
        let mut ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: vec!["round::tests::test_duration_trunc_close_to_epoch".to_string()],
            current_hypothesis: Some("epoch truncation arithmetic is off by one".to_string()),
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };

        record_fast_loop_validation_failure(
            &mut ledger,
            "---- round::tests::test_duration_trunc_close_to_epoch stdout ----\nthread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44: assertion `left == right` failed",
        );

        assert_eq!(
            ledger.validation_status.as_deref(),
            Some("failed: fast-loop")
        );
        assert_eq!(
            ledger.validation_details.failing_test_names,
            vec!["round::tests::test_duration_trunc_close_to_epoch".to_string()]
        );
        assert_eq!(
            ledger.validation_details.primary_failure_path.as_deref(),
            Some("src/round.rs")
        );
        assert_eq!(ledger.validation_details.primary_failure_line, Some(778));
        assert_eq!(
            ledger
                .validation_details
                .primary_failure_test_name
                .as_deref(),
            Some("round::tests::test_duration_trunc_close_to_epoch")
        );
        assert!(ledger.validation_details.repair_required);
        assert!(
            ledger
                .validation_details
                .assertion_excerpt
                .as_deref()
                .is_some_and(|value| value.contains("assertion"))
        );
    }

    #[test]
    fn record_fast_loop_validation_failure_skips_warning_noise_in_assertion_excerpt() {
        let mut ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: vec!["round::tests::test_duration_trunc_close_to_epoch".to_string()],
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };

        record_fast_loop_validation_failure(
            &mut ledger,
            "Command failed: warning: unexpected `cfg` condition value: `bench`\n---- round::tests::test_duration_trunc_close_to_epoch stdout ----\nthread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:\n",
        );

        assert_eq!(
            ledger.validation_details.assertion_excerpt.as_deref(),
            Some(
                "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:"
            )
        );
    }

    #[test]
    fn record_fast_loop_validation_failure_classifies_manifest_errors_before_warnings() {
        let mut ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["tests/issues/issue_474.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet issue_474".to_string()],
            expected_touch_targets: vec!["Cargo.toml".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };

        record_fast_loop_validation_failure(
            &mut ledger,
            "warning: unexpected `cfg` condition value: `bench`\n --> tests/issues/issue_474.rs:10:1\nerror[E0432]: unresolved import `serde`\n --> Cargo.toml:1:1\n",
        );

        assert_eq!(
            ledger.validation_details.diagnostic_class.as_deref(),
            Some("manifest_dependency_error")
        );
        assert_eq!(
            ledger.validation_details.assertion_excerpt.as_deref(),
            Some("error[E0432]: unresolved import `serde`")
        );
    }

    #[test]
    fn manifest_validation_excerpt_groups_multiple_unresolved_imports() {
        let mut ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["tests/issues/issue_474.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet issue_474".to_string()],
            expected_touch_targets: vec!["Cargo.toml".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };

        record_fast_loop_validation_failure(
            &mut ledger,
            "error[E0432]: unresolved import `chrono`\n --> tests/issues/issue_474.rs:6:5\nerror[E0432]: unresolved import `uuid`\n --> tests/issues/issue_474.rs:10:5\n",
        );

        assert_eq!(
            ledger.validation_details.assertion_excerpt.as_deref(),
            Some("unresolved imports/crates: chrono, uuid")
        );
        assert!(
            ledger
                .last_validation_failure
                .as_deref()
                .is_some_and(|failure| failure.contains("chrono, uuid"))
        );
    }

    #[test]
    fn extract_unresolved_import_names_reads_summary_form() {
        assert_eq!(
            extract_unresolved_import_names("assertion unresolved imports/crates: chrono, uuid"),
            vec!["chrono".to_string(), "uuid".to_string()]
        );
        assert_eq!(
            extract_unresolved_import_names(
                "at tests/issues/issue_474.rs:6 | assertion unresolved imports/crates: chrono, uuid | diagnostic_class manifest_dependency_error"
            ),
            vec!["chrono".to_string(), "uuid".to_string()]
        );
    }

    #[test]
    fn classify_benchmark_diagnostic_detects_manifest_feature_error() {
        let output = "error[E0277]: the trait bound `Uuid: serde::Serialize` is not satisfied\nerror[E0277]: the trait bound `DateTime<Utc>: serde::Deserialize<'de>` is not satisfied\n";
        assert_eq!(
            classify_benchmark_diagnostic(output).as_deref(),
            Some("manifest_feature_error")
        );
        assert_eq!(
            extract_manifest_feature_dependency_names(output),
            vec!["chrono".to_string(), "uuid".to_string()]
        );
    }

    #[test]
    fn benchmark_dependency_candidates_include_case_06_manifest_feature_crates() {
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
        };

        assert_eq!(
            benchmark_dependency_candidates(&ledger),
            vec!["chrono".to_string(), "uuid".to_string()]
        );
        assert_eq!(
            benchmark_manifest_patch_operations(
                &ledger,
                Some("dev-dependencies"),
                &benchmark_dependency_candidates(&ledger),
            )
            .len(),
            2
        );
    }

    #[test]
    fn validation_plan_tests_selector_matches_canonical_fast_loop() {
        let ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: vec!["round::tests::test_duration_trunc_close_to_epoch".to_string()],
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };

        let canonical_plan = ValidationPlan {
            tests: vec!["round::tests::".to_string()],
            ..ValidationPlan::default()
        };
        assert_eq!(
            validation_plan_fast_loop_match_kind(&ledger, &canonical_plan),
            Some(FastLoopMatchKind::ExactCanonical)
        );

        let subset_plan = ValidationPlan {
            tests: vec!["round::tests::test_duration_trunc_close_to_epoch".to_string()],
            ..ValidationPlan::default()
        };
        assert_eq!(
            validation_plan_fast_loop_match_kind(&ledger, &subset_plan),
            Some(FastLoopMatchKind::SubsetFastLoop)
        );

        let explicit_selector_ledger = BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/features/serde/de_owned.rs".to_string()],
            fast_loop_commands: vec![
                "cargo test --quiet --features serde --test issues issue_474".to_string(),
            ],
            expected_touch_targets: vec!["Cargo.toml".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        };
        let selector_plan = ValidationPlan {
            tests: vec!["issue_474".to_string()],
            ..ValidationPlan::default()
        };
        assert_eq!(
            validation_plan_fast_loop_match_kind(&explicit_selector_ledger, &selector_plan),
            Some(FastLoopMatchKind::ExactCanonical)
        );
    }

    #[test]
    fn suggested_reread_range_from_failure_handles_summary_separator() {
        let range = suggested_reread_range_from_failure(
            "src/round.rs",
            "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:778 | assertion thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:",
        )
        .expect("range should parse");

        assert_eq!(range.start_line, 770);
        assert_eq!(range.end_line, 802);
    }

    #[test]
    fn implementation_range_suggestion_prefers_duration_round_candidate() {
        let owner_text = "\
pub fn unrelated() {}\n\
pub fn duration_round(value: i64) -> i64 {\n\
    value\n\
}\n\
pub fn duration_trunc(value: i64) -> i64 {\n\
    value\n\
}\n";

        let range = suggest_implementation_range_from_owner_text(
            owner_text,
            Some("round::tests::test_duration_round_close_to_min_max"),
        )
        .expect("implementation range");

        assert!(range.start_line <= 2);
        assert!(range.end_line >= 2);
    }

    #[test]
    fn implementation_range_suggestion_prefers_real_definition_over_trait_signature() {
        let owner_text = "\
pub trait DurationRound {\n\
    fn duration_trunc(self, duration: TimeDelta) -> Result<Self, Self::Err>;\n\
}\n\
\n\
impl DurationRound for NaiveDateTime {\n\
    fn duration_trunc(self, duration: TimeDelta) -> Result<Self, Self::Err> {\n\
        duration_trunc(self, self, duration)\n\
    }\n\
}\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
\n\
fn duration_trunc<T>(\n\
    naive: NaiveDateTime,\n\
    original: T,\n\
    duration: TimeDelta,\n\
) -> Result<T, RoundingError>\n\
where\n\
    T: Timelike,\n\
{\n\
    Ok(original)\n\
}\n";

        let range = suggest_implementation_range_from_owner_text(
            owner_text,
            Some("round::tests::test_duration_trunc_close_to_epoch"),
        )
        .expect("implementation range");

        assert!(range.start_line > 8);
        assert!(range.end_line >= 28);
    }

    #[test]
    fn fast_loop_failure_injects_repair_phase_brief_into_transcript() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: None,
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails::default(),
        });
        let executor = RecordingToolExecutor::new(vec![Err(
            "Command failed: warning: unexpected `cfg` condition value: `bench`\nthread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:\n"
                .to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::RunCommand {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                timeout_ms: 60_000,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("turn should continue after failed fast loop");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(transcript.iter().any(|message| {
            message.content.contains("[Repair Phase]")
                && message.content.contains("Suggested range: 770-802")
        }));
    }

    #[test]
    fn successful_benchmark_fast_loop_run_command_finishes_without_full_validation() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.has_mutating_change = true;
        state.verified_green = false;
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: vec!["round::tests::test_duration_round_close_to_epoch".to_string()],
            current_hypothesis: Some("patch has been applied".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some("round failure".to_string()),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFastLoopRerun,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            last_owner_slice: None,
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();
        let executor =
            RecordingToolExecutor::new(vec![Ok("test result: ok. 1 passed; 0 failed".to_string())]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::RunCommand {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                timeout_ms: 60_000,
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("successful fast loop should finish the benchmark repair");

        assert!(matches!(control_flow, ControlFlow::BreakSuccess));
        assert!(state.verified_green);
        assert!(state.validation_queue.is_empty());
        assert!(state.last_failing_verifier.is_none());
        assert!(state.benchmark_repair_state.is_none());
        assert!(state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_status.as_deref() == Some("green: fast-loop")
                && !ledger.validation_details.repair_required
                && ledger
                    .validation_details
                    .post_fast_loop_validation_rerun_attempted
        }));
        assert!(
            !transcript.iter().any(|message| {
                message
                    .content
                    .contains("Outstanding edits are still unverified")
            }),
            "fast-loop success should not queue broad final validation"
        );
    }

    #[test]
    fn successful_benchmark_fast_loop_validation_clears_followup_queue() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.has_mutating_change = true;
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
            validation_status: Some("patched: controller exact case04".to_string()),
            last_validation_failure: None,
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.enqueue_post_edit_validation(None);
        assert!(
            state.validation_queue.len() > 1,
            "post-edit queue includes follow-up validation before fast-loop success"
        );
        let executor = RecordingToolExecutor::new(vec![Ok("test result: ok".to_string())]);
        let sink = RecordingEventSink::default();
        let mut transcript = Vec::new();
        let first_validation = state.next_validation_action().expect("queued fast loop");

        let outcome = futures::executor::block_on(dispatch_action(
            1,
            &mut state,
            first_validation,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("validation dispatch should complete");

        assert!(matches!(outcome, DispatchOutcome::Success));
        assert!(state.verified_green);
        assert!(state.validation_queue.is_empty());
        assert!(state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_status.as_deref() == Some("green: fast-loop")
                && !ledger.validation_details.repair_required
        }));
    }

    #[test]
    fn second_read_after_anchored_reread_becomes_recovery_not_fatal() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.working_set.insert("src/round.rs".to_string());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("epoch truncation arithmetic is off by one".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: false,
                post_fast_loop_validation_rerun_attempted: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.repair_requirement = Some(RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "fast loop failed".to_string(),
            previous_search_block: None,
            suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            exact_reread_completed: false,
        });
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 770-802\nhonored_range: 770-802\nline 770\nline 771\nline 772\n".to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![
                AgentAction::ReadFile {
                    path: "src/round.rs".to_string(),
                    range: Some(crate::agent_protocol::ReadFileRange {
                        start_line: 770,
                        end_line: 802,
                    }),
                },
                AgentAction::ReadFile {
                    path: "src/round.rs".to_string(),
                    range: Some(crate::agent_protocol::ReadFileRange {
                        start_line: 750,
                        end_line: 811,
                    }),
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("repair redirection should continue");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert_eq!(executor.executed_actions().len(), 1);
        assert!(transcript.iter().any(|message| {
            message
                .content
                .contains("anchored reread is already complete and the next step must be a patch")
        }));
    }

    #[test]
    fn write_denied_before_required_reread_injects_controller_read() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.working_set.insert("src/round.rs".to_string());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("repair a failed source edit".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: false,
                post_fast_loop_validation_rerun_attempted: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            last_owner_slice: None,
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.repair_requirement = Some(RepairRequirement {
            path: "src/round.rs".to_string(),
            failure_reason: "previous edit failed".to_string(),
            previous_search_block: None,
            suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            exact_reread_completed: false,
        });
        state
            .agent_repair_memory
            .observed_slices
            .push(AgentRepairObservedSlice {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 770,
                    end_line: 802,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 770,
                    end_line: 802,
                }),
                purpose: Some("implementation_anchor".to_string()),
                content_fingerprint: Some("observed".to_string()),
            });

        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 770-802\nhonored_range: 770-802\nfn duration_round() {}\n"
                .to_string(),
        )]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let turn = render_turn(
            vec![AgentAction::WriteFile {
                path: "src/round.rs".to_string(),
                content: "fn duration_round() {}\n".to_string(),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("policy-denied write should become a controller reread");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(
            state
                .repair_requirement
                .as_ref()
                .is_some_and(|requirement| { requirement.exact_reread_completed })
        );
        assert!(matches!(
            executor.executed_actions().as_slice(),
            [AgentAction::ReadFile { path, range: Some(range) }]
                if path == "src/round.rs" && range.start_line == 770 && range.end_line == 802
        ));
        assert_eq!(
            state
                .agent_repair_memory
                .scorecard
                .controller_injected_read_count,
            1
        );
        assert!(
            transcript
                .iter()
                .any(|message| message.content.contains("[Repair Controller]"))
        );
    }

    #[test]
    fn test_only_failure_anchor_slice_enables_one_implementation_read_then_patch_and_rerun() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: vec!["round::tests::test_duration_round_close_to_min_max".to_string()],
            current_hypothesis: Some("rounding logic is off near the min/max boundary".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_min_max` failed | at src/round.rs:800"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                failing_test_names: vec![
                    "round::tests::test_duration_round_close_to_min_max".to_string(),
                ],
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_min_max".to_string(),
                ),
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(800),
                assertion_excerpt: Some("assertion `left == right` failed".to_string()),
                repair_required: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFailureAnchorRead,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_min_max".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 792,
                end_line: 824,
            }),
            implementation_suggested_range: None,
            last_owner_slice: None,
            latest_owner_file_text: Some(
                "\
pub fn duration_round(value: i64) -> i64 {\n\
    value\n\
}\n\
#[test]\n\
fn test_duration_round_close_to_min_max() {\n\
    assert_eq!(1, 2);\n\
}\n"
                .to_string(),
            ),
            failure_anchor_reread_attempted: false,
            failure_anchor_reread_honored: false,
            implementation_reread_allowed: false,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let executor = RecordingToolExecutor::new(vec![
            Ok("[read_file]\npath: src/round.rs\nrequested_range: 792-824\nhonored_range: 792-811\n#[test]\nfn test_duration_round_close_to_min_max() {\n    assert_eq!(1, 2);\n}\n".to_string()),
            Ok("[read_file]\npath: src/round.rs\nrequested_range: 1-40\nhonored_range: 1-20\npub fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string()),
            Ok("patch applied".to_string()),
            Ok("test result: ok. 1 passed; 0 failed".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();

        let failure_anchor_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 792,
                    end_line: 824,
                }),
            }],
            None,
        );
        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failure_anchor_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failure anchor read should continue");
        assert!(matches!(control_flow, ControlFlow::Continue));
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(BenchmarkRepairPhase::NeedsImplementationRead)
        );
        assert!(transcript.iter().any(|message| {
            message.role == TranscriptRole::User
                && message
                    .content
                    .contains("You have the failing test slice. Read one implementation slice now.")
        }));

        let implementation_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 40,
                }),
            }],
            None,
        );
        futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &implementation_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("implementation read should continue");
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(BenchmarkRepairPhase::NeedsPatch)
        );
        assert!(transcript.iter().any(|message| {
            message.role == TranscriptRole::User
                && message
                    .content
                    .contains("emit one write on `src/round.rs` now")
        }));

        let patch_turn = render_turn(
            vec![AgentAction::ReplaceBlock {
                path: "src/round.rs".to_string(),
                search_block: "value".to_string(),
                replace_block: "value + 1".to_string(),
                range: None,
            }],
            None,
        );
        futures::executor::block_on(handle_model_turn(
            3,
            ModelTurnInput {
                content: &patch_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("patch turn should continue");
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.phase),
            Some(BenchmarkRepairPhase::NeedsFastLoopRerun)
        );

        let rerun_turn = render_turn(
            vec![AgentAction::RunCommand {
                command: "cargo test --quiet --lib round::tests::".to_string(),
                timeout_ms: 30_000,
            }],
            None,
        );
        futures::executor::block_on(handle_model_turn(
            4,
            ModelTurnInput {
                content: &rerun_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("fast-loop rerun should continue");
        assert!(state.benchmark_repair_state.is_none());
        assert!(
            state
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| !ledger.validation_details.repair_required)
        );
    }

    #[test]
    fn failure_anchor_read_uses_workspace_owner_text_for_implementation_range() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let source_dir = project_root.path().join("src");
        fs::create_dir_all(&source_dir).expect("create src dir");
        fs::write(
            source_dir.join("round.rs"),
            "\
pub trait DurationRound {\n\
    fn duration_round(self, duration: TimeDelta) -> Result<Self, Self::Err>;\n\
}\n\
\n\
fn helper() {}\n\
\n\
fn duration_round<T>(\n\
    naive: NaiveDateTime,\n\
    original: T,\n\
    duration: TimeDelta,\n\
) -> Result<T, RoundingError>\n\
where\n\
    T: Timelike,\n\
{\n\
    Ok(original)\n\
}\n",
        )
        .expect("write owner file");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_round_close_to_epoch' panicked at src/round.rs:789:44:"
                        .to_string(),
                ),
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFailureAnchorRead,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: None,
            last_owner_slice: None,
            latest_owner_file_text: Some(
                "[excerpt lines 1-24 and 800-811 of 811]\n... [middle lines omitted] ..."
                    .to_string(),
            ),
            failure_anchor_reread_attempted: false,
            failure_anchor_reread_honored: false,
            implementation_reread_allowed: false,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        let executor = RecordingToolExecutor::new(vec![Ok(
            "[read_file]\npath: src/round.rs\nrequested_range: 781-813\nhonored_range: 781-811\n#[test]\nfn test_duration_round_close_to_epoch() {\n    assert_eq!(1, 2);\n}\n".to_string(),
        )]);
        let sink = RecordingEventSink::default();
        let mut transcript = vec![TranscriptMessage {
            role: TranscriptRole::User,
            content: "goal".to_string(),
        }];
        let failure_anchor_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 781,
                    end_line: 813,
                }),
            }],
            None,
        );

        futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &failure_anchor_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("failure anchor read should continue");

        let implementation_range = state
            .benchmark_repair_state
            .as_ref()
            .and_then(|repair_state| repair_state.implementation_suggested_range)
            .expect("implementation range");
        assert!(implementation_range.start_line <= 7);
        assert!(implementation_range.end_line >= 7);
    }

    #[test]
    fn implementation_read_must_overlap_suggested_range_when_available() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Need the implementation body, not the test slice".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_round_close_to_epoch' panicked at src/round.rs:789:44:"
                        .to_string(),
                ),
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsImplementationRead,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 140,
                end_line: 220,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 781,
                    end_line: 813,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 781,
                    end_line: 811,
                }),
                kind: OwnerSliceKind::FailureAnchor,
                test_only: true,
                slice_content: Some(
                    "#[test]\nfn test_duration_round_close_to_epoch() {\n    assert_eq!(1, 2);\n}\n"
                        .to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = RecordingEventSink::default();
        let mut transcript = vec![TranscriptMessage {
            role: TranscriptRole::User,
            content: "goal".to_string(),
        }];
        let invalid_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 100,
                }),
            }],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &invalid_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("invalid implementation reread should queue correction");

        assert!(matches!(control_flow, ControlFlow::ContinueNoBudget));
        assert!(transcript.iter().any(|message| {
            message.role == TranscriptRole::User
                && message
                    .content
                    .contains("overlaps the suggested implementation range")
        }));
        assert_eq!(
            state
                .benchmark_repair_state
                .as_ref()
                .map(|repair_state| repair_state.invalid_action_count),
            Some(1)
        );
    }

    #[test]
    fn needs_patch_phase_allows_one_corrective_retry_then_fails() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("patch the owner file now".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some("test failed at src/round.rs:800".to_string()),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: None,
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 792,
                end_line: 824,
            }),
            implementation_suggested_range: None,
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 792,
                    end_line: 824,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 792,
                    end_line: 811,
                }),
                kind: OwnerSliceKind::FailureAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: false,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let executor = RecordingToolExecutor::new(Vec::new());
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let invalid_turn = render_turn(
            vec![AgentAction::ReadFile {
                path: "src/round.rs".to_string(),
                range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 600,
                    end_line: 700,
                }),
            }],
            None,
        );

        let first = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &invalid_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("first invalid action should be corrected");
        assert!(matches!(first, ControlFlow::ContinueNoBudget));

        let second_error = match futures::executor::block_on(handle_model_turn(
            2,
            ModelTurnInput {
                content: &invalid_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        )) {
            Ok(_) => panic!("second invalid action should fail"),
            Err(error) => error,
        };
        assert!(second_error.contains("write_phase_action_refusal"));
    }

    #[test]
    fn needs_patch_phase_message_includes_patch_packet_slice_and_contract() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Fix duration_round close to epoch behavior".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_round_close_to_epoch' panicked at src/round.rs:789:44:"
                        .to_string(),
                ),
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 120,
                end_line: 180,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 132,
                    end_line: 166,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 132,
                    end_line: 164,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    if value < 0 {\n        value - 1\n    } else {\n        value\n    }\n}\n"
                        .to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });

        let message = state
            .benchmark_repair_phase_message()
            .expect("patch packet message");

        assert!(message.contains("[Patch Packet]"));
        assert!(message.contains("Owner path: src/round.rs"));
        assert!(message.contains("Honored implementation range: 132-164"));
        assert!(message.contains("Allowed actions: prefer `ReplaceRange`"));
        assert!(message.contains("Next-step contract: emit exactly one concrete write turn"));
        assert!(message.contains("Response shape: return one raw JSON object only"));
        assert!(message.contains("Minimal JSON example:"));
        assert!(message.contains("\"ReplaceRange\""));
        assert!(message.contains("fn duration_round(value: i64) -> i64"));
    }

    #[test]
    fn needs_patch_phase_message_uses_leased_target_over_evidence_path() {
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
                "src/features/serde/de_owned.rs".to_string(),
                "Cargo.toml".to_string(),
            ],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Resolve missing benchmark dependency".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "error[E0432]: unresolved import `chrono` at tests/issues/issue_474.rs:6"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
                primary_failure_line: Some(6),
                diagnostic_class: Some("manifest_dependency_error".to_string()),
                assertion_excerpt: Some("error[E0432]: unresolved import `chrono`".to_string()),
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
                path: "tests/issues/issue_474.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 30,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 30,
                }),
                kind: OwnerSliceKind::FailureAnchor,
                test_only: true,
                slice_content: Some("use chrono::Utc;\n#[test]\nfn issue_474() {}\n".to_string()),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let message = state
            .benchmark_repair_phase_message()
            .expect("patch packet message");

        assert!(message.contains("Owner path: tests/issues/issue_474.rs"));
        assert!(message.contains("Patch target: Cargo.toml"));
        assert!(message.contains("Current target lease: Cargo.toml"));
        assert!(message.contains("Required next action: read_file Cargo.toml"));
        assert!(message.contains(
            "Allowed actions: `PreviewEdit` with `modify_toml` on `Cargo.toml` first, then `ApplyPreview`."
        ));
        assert!(message.contains("Target dependency table: [dev-dependencies]"));
        assert!(message.contains("\"path\":\"Cargo.toml\""));
        assert!(message.contains("\"PreviewEdit\""));
        assert!(message.contains("Last honored evidence slice:"));
        assert!(!message.contains("write_patch tests/issues/issue_474.rs"));
        assert!(!message.contains("on the owner path"));
        let requirement = state
            .repair_requirement
            .as_ref()
            .expect("leased manifest should require a reread");
        assert_eq!(requirement.path, "Cargo.toml");
        assert!(matches!(
            state.required_repair_read_action(),
            Some(AgentAction::ReadFile { path, range: None }) if path == "Cargo.toml"
        ));

        state.agent_repair_memory.scorecard.anchor_suggestion_count = 1;
        let recovery_message = state.parser_recovery_message(false, "malformed");
        assert!(recovery_message.contains("Issue exactly one `ReadFile` for `Cargo.toml`"));
        assert!(!recovery_message.contains("first exactly one write on `Cargo.toml`"));
    }

    #[test]
    fn benchmark_policy_requires_observed_leased_target_before_write() {
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
            current_hypothesis: Some("add missing manifest dependencies".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "error[E0432]: unresolved import `chrono` at tests/issues/issue_474.rs:6"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
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
                path: "tests/issues/issue_474.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 30,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 1,
                    end_line: 30,
                }),
                kind: OwnerSliceKind::FailureAnchor,
                test_only: true,
                slice_content: Some("use chrono::Utc;\n".to_string()),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: false,
            implementation_reread_honored: false,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let broad_write = AgentAction::WriteFile {
            path: "Cargo.toml".to_string(),
            content: "[workspace]\n".to_string(),
        };
        let error = state
            .allow_action(&broad_write)
            .expect_err("unobserved target write should be rejected");
        assert!(error.contains("requires observing leased patch target `Cargo.toml`"));
        assert!(error.contains("ReadFile the full manifest first"));

        state.record_observed_slice(
            "Cargo.toml",
            None,
            None,
            Some("patch_scaffold".to_string()),
            "[workspace]\nmembers = []\n",
            None,
        );
        state
            .allow_action(&broad_write)
            .expect("observed target write remains backward-compatible");
    }

    #[test]
    fn needs_fast_loop_rerun_phase_message_includes_minimal_json_example() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Rerun the narrowed fast loop".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_trunc_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(778),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ],
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsFastLoopRerun,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_trunc_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 188,
                end_line: 254,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_trunc(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });

        let message = state
            .benchmark_repair_phase_message()
            .expect("fast-loop rerun message");

        assert!(message.contains("Patch captured. Rerun the fast loop now."));
        assert!(message.contains("Recommended rerun command:"));
        assert!(message.contains("Minimal JSON example:"));
        assert!(message.contains("\"RunCommand\""));
    }

    #[test]
    fn needs_patch_phase_accepts_patch_then_fast_loop_rerun_bundle() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Patch duration_round and rerun the fast loop".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_round_close_to_epoch` failed | at src/round.rs:789"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(789),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                ),
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_round_close_to_epoch' panicked at src/round.rs:789:44:"
                        .to_string(),
                ),
                post_fast_loop_patch_attempted: false,
                post_fast_loop_validation_rerun_attempted: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_round_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 781,
                end_line: 813,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 215,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 149,
                    end_line: 215,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_round(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let executor = RecordingToolExecutor::new(vec![
            Ok("[replace_block]\npath: src/round.rs\nstatus: applied\n".to_string()),
            Ok("[run_command]\nstatus: success\n".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let bundled_turn = render_turn(
            vec![
                AgentAction::ReplaceBlock {
                    path: "src/round.rs".to_string(),
                    search_block: "if span > stamp.abs() {".to_string(),
                    replace_block: "if span == 0 {".to_string(),
                    range: None,
                },
                AgentAction::RunCommand {
                    command: "cargo test --quiet --lib round::tests::".to_string(),
                    timeout_ms: 30_000,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &bundled_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("bundled patch plus fast-loop rerun should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(state.benchmark_repair_state.is_none());
        assert!(
            state
                .benchmark_case_ledger
                .as_ref()
                .is_some_and(|ledger| !ledger.validation_details.repair_required)
        );
        assert!(state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_details.post_fast_loop_patch_attempted
                && ledger
                    .validation_details
                    .post_fast_loop_validation_rerun_attempted
        }));
    }

    #[test]
    fn needs_patch_phase_accepts_narrowed_fast_loop_rerun_bundle() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: Some("Patch duration_trunc and rerun the failing tests".to_string()),
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some(
                "test `round::tests::test_duration_trunc_close_to_epoch` failed | at src/round.rs:778"
                    .to_string(),
            ),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                primary_failure_path: Some("src/round.rs".to_string()),
                primary_failure_line: Some(778),
                primary_failure_test_name: Some(
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                ),
                failing_test_names: vec![
                    "round::tests::test_duration_trunc_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_epoch".to_string(),
                    "round::tests::test_duration_round_close_to_min_max".to_string(),
                ],
                assertion_excerpt: Some(
                    "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44:"
                        .to_string(),
                ),
                post_fast_loop_patch_attempted: false,
                post_fast_loop_validation_rerun_attempted: false,
                ..BenchmarkValidationDetails::default()
            },
        });
        state.benchmark_repair_state = Some(BenchmarkRepairState {
            phase: BenchmarkRepairPhase::NeedsPatch,
            owner_path: "src/round.rs".to_string(),
            primary_failure_test_name: Some(
                "round::tests::test_duration_trunc_close_to_epoch".to_string(),
            ),
            failure_anchor_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 770,
                end_line: 802,
            }),
            implementation_suggested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 188,
                end_line: 254,
            }),
            last_owner_slice: Some(OwnerSliceRecord {
                path: "src/round.rs".to_string(),
                requested_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                honored_range: Some(crate::agent_protocol::ReadFileRange {
                    start_line: 188,
                    end_line: 254,
                }),
                kind: OwnerSliceKind::ImplementationAnchor,
                test_only: false,
                slice_content: Some(
                    "fn duration_trunc(value: i64) -> i64 {\n    value\n}\n".to_string(),
                ),
            }),
            latest_owner_file_text: None,
            failure_anchor_reread_attempted: true,
            failure_anchor_reread_honored: true,
            implementation_reread_allowed: true,
            implementation_reread_attempted: true,
            implementation_reread_honored: true,
            invalid_action_count: 0,
        });
        state.sync_benchmark_repair_state_to_ledger();

        let executor = RecordingToolExecutor::new(vec![
            Ok("[replace_block]\npath: src/round.rs\nstatus: applied\n".to_string()),
            Ok("[run_command]\nstatus: success\n".to_string()),
        ]);
        let sink = NoopEventSink;
        let mut transcript = Vec::new();
        let bundled_turn = render_turn(
            vec![
                AgentAction::ReplaceBlock {
                    path: "src/round.rs".to_string(),
                    search_block: "if span > stamp.abs() {".to_string(),
                    replace_block: "if span > 0 && span > stamp.abs() {".to_string(),
                    range: None,
                },
                AgentAction::RunCommand {
                    command: "cargo test --quiet --lib round::tests::test_duration_trunc_close_to_epoch round::tests::test_duration_round_close_to_epoch round::tests::test_duration_round_close_to_min_max".to_string(),
                    timeout_ms: 30_000,
                },
            ],
            None,
        );

        let control_flow = futures::executor::block_on(handle_model_turn(
            1,
            ModelTurnInput {
                content: &bundled_turn,
                native_turn: None,
                native_turn_error: None,
                output_truncated: false,
            },
            &mut state,
            &request,
            &executor,
            &sink,
            &mut transcript,
        ))
        .expect("bundled patch plus narrowed fast-loop rerun should continue");

        assert!(matches!(control_flow, ControlFlow::Continue));
        assert!(state.benchmark_case_ledger.as_ref().is_some_and(|ledger| {
            ledger.validation_details.post_fast_loop_patch_attempted
                && ledger
                    .validation_details
                    .post_fast_loop_validation_rerun_attempted
                && ledger
                    .validation_details
                    .fast_loop_rerun_match_kind
                    .as_deref()
                    == Some("subset_fast_loop")
        }));
    }

    #[test]
    fn enqueue_post_edit_validation_runs_fast_loop_first_during_repair_phase() {
        let project_root = tempfile::tempdir().expect("tempdir");
        let request = test_request(&project_root);
        let mut state = AgentTaskState::new(&request, test_config());
        state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
            case_class: "narrow-owner-first".to_string(),
            owner_files: vec!["src/round.rs".to_string()],
            fast_loop_commands: vec!["cargo test --quiet --lib round::tests::".to_string()],
            expected_touch_targets: vec!["src/round.rs".to_string()],
            companion_files_required: Vec::new(),
            named_tests: Vec::new(),
            current_hypothesis: None,
            validation_status: Some("failed: fast-loop".to_string()),
            last_validation_failure: Some("round failure".to_string()),
            validation_details: BenchmarkValidationDetails {
                repair_required: true,
                post_fast_loop_patch_attempted: true,
                ..BenchmarkValidationDetails::default()
            },
        });

        state.enqueue_post_edit_validation(None);

        let first_plan = state
            .validation_queue
            .pop_front()
            .expect("first queued plan");
        assert_eq!(
            first_plan.custom_commands,
            vec!["cargo test --quiet --lib round::tests::".to_string()]
        );
    }
