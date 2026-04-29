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
    assert_eq!(
        state
            .benchmark_case_ledger
            .as_ref()
            .map(|ledger| ledger.validation_details.prose_only_recovery_count),
        Some(1)
    );
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
    assert_eq!(
        state
            .agent_repair_memory
            .scorecard
            .prose_only_recovery_count,
        1
    );
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
        prompt_compaction_policy_for_turn(&request.completion_policy, &request.model_id, &state,),
        Some(PromptCompactionPolicy::BenchmarkRepairMinimal)
    );
    assert_eq!(
        max_completion_tokens_for_turn(&request.completion_policy, 1, &request.model_id, &state),
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
        prompt_compaction_policy_for_turn(&request.completion_policy, &request.model_id, &state,),
        Some(PromptCompactionPolicy::BenchmarkStatePacket)
    );
    assert_eq!(
        max_completion_tokens_for_turn(&request.completion_policy, 1, &request.model_id, &state),
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

