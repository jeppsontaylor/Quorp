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

#[test]
fn repair_source_tracks_highest_severity_usage() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());

    assert_eq!(
        state.agent_repair_memory.scorecard.repair_source,
        Some(RepairSource::Model)
    );

    state.record_controller_injected_read();
    assert_eq!(
        state.agent_repair_memory.scorecard.repair_source,
        Some(RepairSource::Controller)
    );

    state.record_repair_source(RepairSource::EvalOracle);
    assert_eq!(
        state.agent_repair_memory.scorecard.repair_source,
        Some(RepairSource::EvalOracle)
    );

    state.record_repair_source(RepairSource::Skill);
    assert_eq!(
        state.agent_repair_memory.scorecard.repair_source,
        Some(RepairSource::EvalOracle)
    );
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
            clippy_command: Some("cargo clippy --all-targets --no-deps -- -D warnings".to_string()),
            workspace_test_command: Some("cargo test".to_string()),
            targeted_test_prefix: Some("cargo test ".to_string()),
        },
        policy: PolicySettings {
            mode: crate::agent_context::PolicyMode::BenchmarkAutonomous,
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
        failure_reason: "Search block is ambiguous; found 2 matches at lines 151, 188".to_string(),
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
fn needs_patch_phase_rejects_bare_replace_after_any_failed_replace_block() {
    let project_root = tempfile::tempdir().expect("tempdir");
    let request = test_request(&project_root);
    let mut state = AgentTaskState::new(&request, test_config());
    seed_chrono_needs_patch_state(&mut state);
    state.failed_edit_records.push(FailedEditRecord {
        action_kind: "replace_block".to_string(),
        path: "src/round.rs".to_string(),
        search_hash: Some("search".to_string()),
        replace_hash: Some("replace".to_string()),
        failure_reason: "replace_block: search block was not found at the recorded location"
            .to_string(),
        matching_line_numbers: vec![151],
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
