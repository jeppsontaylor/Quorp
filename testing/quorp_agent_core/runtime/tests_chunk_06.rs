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

#[test]
fn enqueue_full_validation_runs_fast_loop_before_broad_validation_during_repair_phase() {
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

    state.enqueue_full_validation();

    let first_plan = state
        .validation_queue
        .pop_front()
        .expect("fast loop queued first");
    assert_eq!(
        first_plan.custom_commands,
        vec!["cargo test --quiet --lib round::tests::".to_string()]
    );
    let second_plan = state
        .validation_queue
        .pop_front()
        .expect("full validation queued second");
    assert!(second_plan.fmt);
    assert!(second_plan.clippy);
    assert!(second_plan.workspace_tests);
    assert!(
        state
            .benchmark_case_ledger
            .as_ref()
            .is_some_and(|ledger| ledger.validation_details.full_validation_before_fast_loop)
    );
}

#[test]
fn repeated_failed_bare_replace_block_increments_retry_count() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    let first_action = AgentAction::ReplaceBlock {
        path: "src/lib.rs".to_string(),
        search_block: "old".to_string(),
        replace_block: "new".to_string(),
        range: None,
    };
    let second_action = AgentAction::ReplaceBlock {
        path: "src/lib.rs".to_string(),
        search_block: "old".to_string(),
        replace_block: "new".to_string(),
        range: None,
    };

    let _ = state.record_failed_edit(&first_action, "replace_block: search block not found");
    let _ = state.record_failed_edit(&second_action, "replace_block: search block not found");

    assert_eq!(
        state
            .agent_repair_memory
            .scorecard
            .bare_replace_block_retry_count,
        1
    );
}

#[test]
fn runtime_event_fanout_delivers_to_all_subscribers() {
    let fanout = RuntimeEventFanout::new();
    let memory_subscriber = fanout.subscribe();
    let rule_subscriber = fanout.subscribe();
    fanout.emit(RuntimeEvent::PhaseChanged {
        phase: "verifying".to_string(),
        detail: Some("l2 targeted".to_string()),
    });

    let memory_events = memory_subscriber.drain();
    let rule_events = rule_subscriber.drain();

    assert_eq!(memory_events.len(), 1);
    assert_eq!(rule_events.len(), 1);
    assert!(matches!(
        &memory_events[0],
        RuntimeEvent::PhaseChanged { phase, detail }
            if phase == "verifying" && detail.as_deref() == Some("l2 targeted")
    ));
    assert!(matches!(
        &rule_events[0],
        RuntimeEvent::PhaseChanged { phase, detail }
            if phase == "verifying" && detail.as_deref() == Some("l2 targeted")
    ));
}

#[test]
fn runtime_event_fanout_records_subscriber_backpressure() {
    let fanout = RuntimeEventFanout::new();
    let proof_recorder = fanout.subscribe_named_with_capacity("proof_recorder", 2);

    fanout.emit(RuntimeEvent::PhaseChanged {
        phase: "first".to_string(),
        detail: None,
    });
    fanout.emit(RuntimeEvent::PhaseChanged {
        phase: "second".to_string(),
        detail: None,
    });
    fanout.emit(RuntimeEvent::PhaseChanged {
        phase: "third".to_string(),
        detail: None,
    });

    let events = proof_recorder.drain();
    assert_eq!(proof_recorder.subscriber_name(), "proof_recorder");
    assert_eq!(proof_recorder.capacity(), 2);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        RuntimeEvent::SubscriberBackpressure {
            subscriber,
            dropped_events: 2,
            capacity: 2,
        } if subscriber == "proof_recorder"
    ));
    assert!(matches!(
        &events[1],
        RuntimeEvent::PhaseChanged { phase, detail }
            if phase == "third" && detail.is_none()
    ));
}

#[test]
fn runtime_event_worker_drains_subscriber_queue() {
    let fanout = RuntimeEventFanout::new();
    let subscription = fanout.subscribe_named_with_capacity("memory_writer", 4);
    let (event_tx, event_rx) = std::sync::mpsc::channel();

    let worker = fanout.spawn_worker(subscription.clone(), move |event| {
        event_tx.send(event).expect("send runtime event");
    });

    fanout.emit(RuntimeEvent::PhaseChanged {
        phase: "planning".to_string(),
        detail: Some("worker test".to_string()),
    });

    let delivered = event_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("worker should receive runtime event");
    assert!(matches!(
        delivered,
        RuntimeEvent::PhaseChanged { phase, detail }
            if phase == "planning" && detail.as_deref() == Some("worker test")
    ));

    worker.stop().expect("worker should stop cleanly");
    assert!(subscription.drain().is_empty());
}

#[test]
fn first_write_governor_requires_targeted_observation_after_failure() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.agent_repair_memory.last_failure_packet = Some(quorp_verify::parse_failure_packet(
        "cargo check",
        r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":12,"column_start":5,"is_primary":true}]}}"#,
    ));

    let write_action = AgentAction::WriteFile {
        path: "src/lib.rs".to_string(),
        content: "fn main() {}".to_string(),
    };
    let error = state
        .allow_action(&write_action)
        .expect_err("write should be blocked before targeted observation");
    assert!(error.contains("first-write governor"));

    state.record_observed_slice(
        "src/lib.rs",
        None,
        None,
        Some("evidence".to_string()),
        "fn main() {}\n",
        Some("stable-hash"),
    );
    state
        .allow_action(&write_action)
        .expect("write should be allowed after targeted observation");
}

#[test]
fn repeated_evidence_reads_are_blocked_on_the_third_attempt() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    state.agent_repair_memory.last_failure_packet = Some(quorp_verify::parse_failure_packet(
        "cargo test",
        "test foo::bar ... FAILED",
    ));
    let signature = canonical_action_signature(
        &AgentAction::ReadFile {
            path: "src/lib.rs".to_string(),
            range: Some(crate::agent_protocol::ReadFileRange {
                start_line: 1,
                end_line: 5,
            }),
        },
        state.benchmark_case_ledger.as_ref(),
    );
    state
        .agent_repair_memory
        .canonical_action_history
        .push(AgentRepairCanonicalAction {
            step: 1,
            kind: "ReadFile".to_string(),
            signature: signature.clone(),
            target_path: Some("src/lib.rs".to_string()),
            validation_like: false,
        });
    state
        .agent_repair_memory
        .canonical_action_history
        .push(AgentRepairCanonicalAction {
            step: 2,
            kind: "ReadFile".to_string(),
            signature,
            target_path: Some("src/lib.rs".to_string()),
            validation_like: false,
        });

    let read_action = AgentAction::ReadFile {
        path: "src/lib.rs".to_string(),
        range: Some(crate::agent_protocol::ReadFileRange {
            start_line: 1,
            end_line: 5,
        }),
    };
    state.record_canonical_action(3, &read_action);
    let error = state
        .allow_action(&read_action)
        .expect_err("third repeated evidence read should be blocked");
    assert!(error.contains("no-progress detector"));
}

#[test]
fn assistant_action_mismatch_triggers_recovery_refresh() {
    let mismatch = assistant_action_mismatch(
        "I patched the file and fixed the bug.",
        &[AgentAction::ReadFile {
            path: "src/lib.rs".to_string(),
            range: None,
        }],
    );

    assert!(mismatch.is_some());
    assert!(
        recovery_refresh_message(mismatch.as_deref().unwrap())
            .contains("Refresh the recovery packet")
    );
}

#[test]
fn large_source_write_requires_an_explicit_lease() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    let huge_content = vec!["line".to_string(); 201].join("\n");
    let write_action = AgentAction::WriteFile {
        path: "src/lib.rs".to_string(),
        content: huge_content,
    };

    let error = state
        .allow_action(&write_action)
        .expect_err("large source write without lease should be rejected");
    assert!(error.contains("semantic edit or attach an explicit lease"));

    state.agent_repair_memory.implementation_target_lease = Some("src/lib.rs".to_string());
    state
        .allow_action(&write_action)
        .expect("leased large source write should be allowed");
}
