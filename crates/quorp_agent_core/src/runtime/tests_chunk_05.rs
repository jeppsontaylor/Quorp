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
            "[excerpt lines 1-24 and 800-811 of 811]\n... [middle lines omitted] ...".to_string(),
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
fn needs_patch_phase_allows_patch_intent_retries_before_failing() {
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
            post_fast_loop_patch_attempted: true,
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

    let second = match futures::executor::block_on(handle_model_turn(
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
        Ok(outcome) => outcome,
        Err(error) => panic!("second invalid action should still produce correction: {error}"),
    };
    assert!(matches!(second, ControlFlow::ContinueNoBudget));
    assert!(
        transcript
            .iter()
            .any(|message| message.content.contains("[Patch Intent Packet]"))
    );

    let third = futures::executor::block_on(handle_model_turn(
        3,
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
    .expect("third invalid action should still be corrected");
    assert!(matches!(third, ControlFlow::ContinueNoBudget));

    let fourth_error = match futures::executor::block_on(handle_model_turn(
        4,
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
        Ok(_) => panic!("fourth invalid action should fail"),
        Err(error) => error,
    };
    assert!(
        fourth_error.contains("source_patch_refusal")
            || fourth_error.contains("write_phase_action_refusal"),
        "{fourth_error}"
    );
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
            "error[E0432]: unresolved import `chrono` at tests/issues/issue_474.rs:6".to_string(),
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
            "error[E0432]: unresolved import `chrono` at tests/issues/issue_474.rs:6".to_string(),
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
fn required_repair_read_action_strips_ansi_from_controller_path() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.benchmark_case_ledger = Some(BenchmarkCaseLedger {
        case_class: "narrow-owner-first".to_string(),
        owner_files: vec!["\x1b[0maxum/src/lib.rs".to_string()],
        fast_loop_commands: vec![
            "cargo test --quiet -p axum --lib --features headers routing::tests::".to_string(),
        ],
        expected_touch_targets: vec!["axum/src/routing/mod.rs".to_string()],
        companion_files_required: Vec::new(),
        named_tests: Vec::new(),
        current_hypothesis: None,
        validation_status: Some("failed: fast-loop".to_string()),
        last_validation_failure: Some(
            "thread 'routing::tests::fallback' panicked at axum/src/routing/tests/mod.rs:382:9"
                .to_string(),
        ),
        validation_details: BenchmarkValidationDetails {
            repair_required: true,
            diagnostic_class: Some("test_failure".to_string()),
            primary_failure_path: Some("axum/src/routing/tests/mod.rs".to_string()),
            primary_failure_line: Some(382),
            ..BenchmarkValidationDetails::default()
        },
    });
    state.benchmark_repair_state =
        benchmark_repair_state_from_ledger(state.benchmark_case_ledger.as_ref().unwrap());

    let repair_state = state
        .benchmark_repair_state
        .as_mut()
        .expect("repair state");
    assert_eq!(repair_state.owner_path, "axum/src/lib.rs");
    repair_state.phase = BenchmarkRepairPhase::NeedsImplementationRead;
    repair_state.implementation_suggested_range = Some(crate::agent_protocol::ReadFileRange {
        start_line: 361,
        end_line: 393,
    });

    assert!(matches!(
        state.required_repair_read_action(),
        Some(AgentAction::ReadFile { path, range: Some(range) })
            if path == "axum/src/lib.rs" && range.start_line == 361 && range.end_line == 393
    ));
}

#[test]
fn exact_chrono_playbook_handles_test_failure_diagnostics() {
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
        last_validation_failure: Some(
            "thread 'round::tests::test_duration_trunc_close_to_epoch' panicked at src/round.rs:778:44"
                .to_string(),
        ),
        validation_details: BenchmarkValidationDetails {
            repair_required: true,
            diagnostic_class: Some("test_failure".to_string()),
            primary_failure_path: Some("src/round.rs".to_string()),
            primary_failure_line: Some(778),
            ..BenchmarkValidationDetails::default()
        },
    });
    let source_text = r#"fn duration_round() {
        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
}

fn duration_trunc() {
        if span > stamp.abs() {
            return Err(RoundingError::DurationExceedsTimestamp);
        }
}
"#;
    let repair_state = BenchmarkRepairState {
        phase: BenchmarkRepairPhase::NeedsPatch,
        owner_path: "src/round.rs".to_string(),
        latest_owner_file_text: Some(source_text.to_string()),
        last_owner_slice: Some(OwnerSliceRecord {
            path: "src/round.rs".to_string(),
            requested_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 276,
            }),
            honored_range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 149,
                end_line: 276,
            }),
            kind: OwnerSliceKind::ImplementationAnchor,
            test_only: false,
            slice_content: Some(source_text.to_string()),
        }),
        ..BenchmarkRepairState::default()
    };

    let action = exact_benchmark_source_patch_action_from_state(
        &state,
        &repair_state,
        state.benchmark_case_ledger.as_ref().unwrap(),
    )
    .expect("chrono exact patch");

    assert!(matches!(
        action,
        AgentAction::WriteFile { path, content }
            if path == "src/round.rs"
                && !content.contains("DurationExceedsTimestamp")
    ));
}

#[test]
fn test_assertion_failure_prefers_source_over_manifest_support() {
    let ledger = BenchmarkCaseLedger {
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
        last_validation_failure: Some(
            "test `issue_474::test` failed | assertion CannotBorrowOwnedData".to_string(),
        ),
        validation_details: BenchmarkValidationDetails {
            repair_required: true,
            diagnostic_class: Some("test_assertion_failure".to_string()),
            primary_failure_path: Some("tests/issues/issue_474.rs".to_string()),
            primary_failure_line: Some(47),
            post_fast_loop_patch_attempted: true,
            ..BenchmarkValidationDetails::default()
        },
    };

    assert_eq!(
        target_lease_for_ledger(&ledger).as_deref(),
        Some("src/features/serde/de_owned.rs")
    );
}
