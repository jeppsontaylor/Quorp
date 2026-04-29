//! Unit tests for the desktop-core scaffolding (PR2 + PR4).

use tempfile::TempDir;

use crate::workspace_registry::WorkspaceRegistry;
use crate::{DESKTOP_CORE_VERSION, DESKTOP_WIRE_VERSION, TrustStore};
use quorp_desktop_ipc::{TrustDecision, TrustReceipt, WorkspaceId};

#[test]
fn version_is_published() {
    // Surface check that env! macro picked up the package version.
    assert!(!DESKTOP_CORE_VERSION.is_empty());
}

#[test]
fn wire_version_re_export_matches_ipc_crate() {
    assert_eq!(
        DESKTOP_WIRE_VERSION,
        quorp_desktop_ipc::DESKTOP_WIRE_VERSION
    );
}

#[test]
fn app_state_constructs_with_in_memory_secret_store() {
    let state =
        crate::DesktopAppState::with_secret_store(crate::InMemorySecretStore::arc()).unwrap();
    assert!(state.workspaces.list().is_empty());
    assert!(state.trust_log.is_empty());
    assert!(!state.providers.has_api_key());
    assert_eq!(state.permissions.pending_count(), 0);
}

#[test]
fn workspace_registry_adds_new_workspace_with_canonical_path() {
    let temp = TempDir::new().unwrap();
    let registry = WorkspaceRegistry::new();
    let record = registry.add(temp.path()).unwrap();
    assert_eq!(record.trust, TrustDecision::Untrusted);
    assert!(!record.canonical_path.is_empty());
    assert!(!record.id.as_str().is_empty());
}

#[test]
fn workspace_registry_returns_existing_record_on_duplicate_add() {
    let temp = TempDir::new().unwrap();
    let registry = WorkspaceRegistry::new();
    let first = registry.add(temp.path()).unwrap();
    let second = registry.add(temp.path()).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(registry.list().len(), 1);
}

#[test]
fn workspace_registry_canonicalizes_tmp_vs_private_tmp_on_macos() {
    // On macOS, /tmp is a symlink to /private/tmp. Both forms should
    // resolve to the same workspace id. We use an actual temp dir
    // (which lives under /var/folders/... on macOS) and confirm the
    // canonical path is what we'd expect dunce to return.
    let temp = TempDir::new().unwrap();
    let registry = WorkspaceRegistry::new();
    let record = registry.add(temp.path()).unwrap();
    let canonical_again = dunce::canonicalize(temp.path())
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(record.canonical_path, canonical_again);
}

#[test]
fn workspace_registry_rejects_missing_path() {
    let registry = WorkspaceRegistry::new();
    let result = registry.add("/this/path/does/not/exist/anywhere");
    assert!(result.is_err());
}

#[test]
fn workspace_registry_rejects_file() {
    let temp = TempDir::new().unwrap();
    let file_path = temp.path().join("a.txt");
    std::fs::write(&file_path, b"x").unwrap();
    let registry = WorkspaceRegistry::new();
    let result = registry.add(&file_path);
    assert!(result.is_err());
}

#[test]
fn workspace_registry_set_trust_returns_receipt() {
    let temp = TempDir::new().unwrap();
    let registry = WorkspaceRegistry::new();
    let record = registry.add(temp.path()).unwrap();
    let receipt = registry
        .set_trust(&record.id, TrustDecision::Trusted)
        .unwrap();
    assert_eq!(receipt.previous, TrustDecision::Untrusted);
    assert_eq!(receipt.current, TrustDecision::Trusted);
    let updated = registry.get(&record.id).unwrap();
    assert_eq!(updated.trust, TrustDecision::Trusted);
}

#[test]
fn workspace_registry_remove_works() {
    let temp = TempDir::new().unwrap();
    let registry = WorkspaceRegistry::new();
    let record = registry.add(temp.path()).unwrap();
    registry.remove(&record.id).unwrap();
    assert!(registry.get(&record.id).is_none());
}

#[test]
fn workspace_registry_set_trust_unknown_returns_error() {
    let registry = WorkspaceRegistry::new();
    let result = registry.set_trust(&WorkspaceId::new("ws-bogus"), TrustDecision::Trusted);
    assert!(result.is_err());
}

#[test]
fn trust_store_records_and_truncates() {
    let store = TrustStore::with_capacity(2);
    for i in 0..5 {
        store.record(TrustReceipt {
            workspace_id: WorkspaceId::new(format!("ws-{i}")),
            previous: TrustDecision::Untrusted,
            current: TrustDecision::Trusted,
            decided_at: format!("2026-04-28T15:00:0{i}Z"),
        });
    }
    let snap = store.snapshot();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].workspace_id.as_str(), "ws-3");
    assert_eq!(snap[1].workspace_id.as_str(), "ws-4");
}

mod event_bridge_tests {
    use crate::event_bridge::{
        run_failure_stage_from_label, run_started_event, stop_reason_dto_from_label, translate,
        validation_status_from_label,
    };
    use quorp_agent_core::{RuntimeEvent, TokenUsage, UsageSource};
    use quorp_desktop_ipc::{
        DesktopEvent, RunFailureStage, RunIdDto, RuntimeEventDto, StopReasonDto,
        ValidationStatusDto,
    };

    #[test]
    fn run_started_event_has_rfc3339_timestamp() {
        let evt = run_started_event(
            RunIdDto::new("run-1"),
            "fix bug".into(),
            "qwen/qwen3-coder-480b-a35b-instruct".into(),
        );
        if let DesktopEvent::RunStarted { started_at, .. } = evt {
            // Trivial parse check: must contain a 'T' and 'Z' or an offset.
            assert!(started_at.contains('T'));
        } else {
            panic!("expected RunStarted");
        }
    }

    #[test]
    fn translate_preserves_run_started_payload() {
        let evt = RuntimeEvent::RunStarted {
            goal: "smoke".into(),
            model_id: "m".into(),
        };
        let dto = translate(0, evt);
        match dto {
            RuntimeEventDto::RunStarted {
                seq,
                goal,
                model_id,
            } => {
                assert_eq!(seq, 0);
                assert_eq!(goal, "smoke");
                assert_eq!(model_id, "m");
            }
            _ => panic!("expected RunStarted DTO"),
        }
    }

    #[test]
    fn translate_model_request_finished_carries_usage() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_billed_tokens: 150,
            reasoning_tokens: None,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
            provider_request_id: None,
            latency_ms: 0,
            finish_reason: None,
            usage_source: UsageSource::Reported,
        };
        let evt = RuntimeEvent::ModelRequestFinished {
            step: 1,
            request_id: 42,
            usage: Some(usage),
            watchdog: None,
        };
        let dto = translate(7, evt);
        match dto {
            RuntimeEventDto::ModelRequestFinished {
                seq,
                step,
                request_id,
                usage,
                watchdog,
            } => {
                assert_eq!(seq, 7);
                assert_eq!(step, 1);
                assert_eq!(request_id, 42);
                let usage = usage.unwrap();
                assert_eq!(usage.prompt_tokens, 100);
                assert_eq!(usage.completion_tokens, 50);
                assert_eq!(usage.total_tokens, 150);
                assert!(watchdog.is_none());
            }
            _ => panic!("expected ModelRequestFinished DTO"),
        }
    }

    #[test]
    fn translate_unknown_variant_falls_through_to_other() {
        // TurnCompleted has no dedicated DTO; it should land in Other.
        let evt = RuntimeEvent::TurnCompleted {
            transcript: Vec::new(),
        };
        let dto = translate(11, evt);
        match dto {
            RuntimeEventDto::Other { seq, kind, .. } => {
                assert_eq!(seq, 11);
                assert_eq!(kind, "turn_completed");
            }
            _ => panic!("expected Other DTO for unmapped variant"),
        }
    }

    #[test]
    fn stop_reason_label_mappings() {
        assert_eq!(
            stop_reason_dto_from_label("Completed"),
            StopReasonDto::Completed
        );
        assert_eq!(
            stop_reason_dto_from_label("Cancelled"),
            StopReasonDto::Cancelled
        );
        assert_eq!(
            stop_reason_dto_from_label("BudgetExhausted"),
            StopReasonDto::BudgetExhausted
        );
        assert_eq!(
            stop_reason_dto_from_label("not-real"),
            StopReasonDto::UnknownError
        );
    }

    #[test]
    fn run_failure_stage_label_mappings() {
        assert_eq!(
            run_failure_stage_from_label("sandbox_setup"),
            RunFailureStage::SandboxSetup
        );
        assert_eq!(
            run_failure_stage_from_label("agent_loop"),
            RunFailureStage::AgentLoop
        );
        assert_eq!(
            run_failure_stage_from_label("unknown-thing"),
            RunFailureStage::Unknown
        );
    }

    #[test]
    fn validation_status_label_mappings() {
        assert_eq!(
            validation_status_from_label("PASSED"),
            ValidationStatusDto::Passed
        );
        assert_eq!(
            validation_status_from_label("queued"),
            ValidationStatusDto::Queued
        );
        assert_eq!(
            validation_status_from_label("noop"),
            ValidationStatusDto::Skipped
        );
        // Unknown labels default to Running so cards keep flowing.
        assert_eq!(
            validation_status_from_label("???"),
            ValidationStatusDto::Running
        );
    }
}

mod hooks_smoke_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use quorp_session::quorp::agent_runner::HeadlessRunHooks;

    #[test]
    fn default_hooks_have_no_effects() {
        let hooks = HeadlessRunHooks::default();
        assert!(hooks.progress_tx.is_none());
        assert!(hooks.extra_event_sink.is_none());
        assert!(hooks.cancellation_flag.is_none());
    }

    #[test]
    fn hooks_can_carry_cancellation_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let hooks = HeadlessRunHooks {
            cancellation_flag: Some(flag.clone()),
            ..HeadlessRunHooks::default()
        };
        assert!(hooks.cancellation_flag.is_some());
        flag.store(true, Ordering::SeqCst);
        assert!(hooks.cancellation_flag.unwrap().load(Ordering::SeqCst));
    }
}

mod secret_keychain_tests {
    use crate::secret_keychain::{InMemorySecretStore, SecretStore};

    #[test]
    fn in_memory_store_round_trips() {
        let store = InMemorySecretStore::new();
        assert!(!store.has("nim"));
        assert!(matches!(store.get("nim"), Ok(None)));
        store.set("nim", "secret-key").unwrap();
        assert!(store.has("nim"));
        assert_eq!(store.get("nim").unwrap().as_deref(), Some("secret-key"));
        store.clear("nim").unwrap();
        assert!(!store.has("nim"));
    }

    #[test]
    fn in_memory_clear_missing_account_is_idempotent() {
        let store = InMemorySecretStore::new();
        store.clear("nope").unwrap();
        assert!(!store.has("nope"));
    }

    #[test]
    fn in_memory_set_overwrites() {
        let store = InMemorySecretStore::new();
        store.set("nim", "a").unwrap();
        store.set("nim", "b").unwrap();
        assert_eq!(store.get("nim").unwrap().as_deref(), Some("b"));
    }
}

mod provider_registry_tests {
    use std::sync::Arc;

    use crate::provider_registry::{NIM_KEYCHAIN_ACCOUNT, ProviderRegistry};
    use crate::secret_keychain::{InMemorySecretStore, SecretStore};
    use quorp_desktop_ipc::{DEFAULT_MODEL_ID, DEFAULT_PROVIDER_NAME};

    fn registry() -> (ProviderRegistry, Arc<InMemorySecretStore>) {
        let store = Arc::new(InMemorySecretStore::new());
        let registry = ProviderRegistry::new(store.clone());
        (registry, store)
    }

    #[test]
    fn summary_reports_default_provider() {
        let (registry, _) = registry();
        let summary = registry.summary();
        assert_eq!(summary.name, DEFAULT_PROVIDER_NAME);
        assert_eq!(summary.default_model, DEFAULT_MODEL_ID);
        assert!(!summary.has_key);
    }

    #[test]
    fn summary_flips_has_key_after_set() {
        let (registry, _) = registry();
        registry.set_api_key("nvapi-fake-test-key").unwrap();
        let summary = registry.summary();
        assert!(summary.has_key);
        // Frontend never sees the key — the summary carries no
        // `api_key` field at all.
    }

    #[test]
    fn set_api_key_rejects_empty() {
        let (registry, _) = registry();
        let err = registry.set_api_key("   ").unwrap_err();
        assert!(matches!(
            err,
            crate::provider_registry::ProviderError::Invalid(_)
        ));
    }

    #[test]
    fn set_api_key_trims_whitespace() {
        let (registry, store) = registry();
        registry.set_api_key("  abc  ").unwrap();
        assert_eq!(
            store.get(NIM_KEYCHAIN_ACCOUNT).unwrap().as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn clear_api_key_removes_credential() {
        let (registry, store) = registry();
        registry.set_api_key("secret").unwrap();
        registry.clear_api_key().unwrap();
        assert!(!registry.has_api_key());
        assert!(!store.has(NIM_KEYCHAIN_ACCOUNT));
    }
}

mod permission_broker_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::permission_broker::{
        BrokerOutcome, PendingAction, PermissionBroker, PermissionBrokerError,
    };
    use quorp_desktop_ipc::{
        CapabilityTokenDto, DesktopEvent, PermissionDecisionDto, PermissionDecisionKind,
        PermissionRequestId, PermissionScope, RiskLevel, RunIdDto,
    };

    fn pending(run: &str) -> PendingAction {
        PendingAction {
            run_id: RunIdDto::new(run),
            action_summary: "cargo install bandage".into(),
            tool: "shell".into(),
            cwd: Some("/tmp/quorp/run-1/work".into()),
            tokens: vec![CapabilityTokenDto::Network],
            risk: RiskLevel::High,
            reason: None,
        }
    }

    fn allow_session() -> PermissionDecisionDto {
        PermissionDecisionDto {
            decision: PermissionDecisionKind::Allow,
            scope: PermissionScope::Session,
        }
    }

    #[tokio::test]
    async fn request_resolves_when_user_allows() {
        let broker = Arc::new(PermissionBroker::new(Duration::from_secs(2)));
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let broker_for_resolve = broker.clone();
        let request_task = tokio::spawn(async move {
            let outcome = broker.request(pending("run-1"), &sink).await.unwrap();
            assert!(outcome.is_allow());
        });
        // The broker emits exactly one event before awaiting the
        // oneshot. Pull it and resolve via the request id it carried.
        let evt = events.recv().await.expect("permission event");
        let request_id = match evt {
            DesktopEvent::Permission { request, .. } => request.request_id,
            other => panic!("expected Permission event, got {other:?}"),
        };
        broker_for_resolve
            .resolve(&request_id, allow_session())
            .unwrap();
        request_task.await.unwrap();
    }

    #[tokio::test]
    async fn request_times_out_to_deny() {
        let broker = Arc::new(PermissionBroker::new(Duration::from_millis(100)));
        let (sink, _events) = tokio::sync::mpsc::unbounded_channel();
        let outcome = broker.request(pending("run-2"), &sink).await.unwrap();
        assert!(matches!(outcome, BrokerOutcome::TimedOut));
        assert_eq!(broker.pending_count(), 0);
    }

    #[tokio::test]
    async fn resolve_unknown_id_returns_stale() {
        let broker = PermissionBroker::default();
        let err = broker
            .resolve(&PermissionRequestId::new("perm-bogus"), allow_session())
            .unwrap_err();
        assert!(matches!(err, PermissionBrokerError::Stale(_)));
    }

    #[tokio::test]
    async fn cancel_all_unblocks_pending_requests() {
        let broker = Arc::new(PermissionBroker::new(Duration::from_secs(60)));
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let broker_for_cancel = broker.clone();
        let waiter =
            tokio::spawn(async move { broker.request(pending("run-3"), &sink).await.unwrap() });
        // Drain the emitted permission event so the channel doesn't
        // back up; we don't need the request id for cancel_all.
        let _ = events.recv().await;
        broker_for_cancel.cancel_all();
        let outcome = waiter.await.unwrap();
        assert!(matches!(outcome, BrokerOutcome::TimedOut));
    }
}

mod replay_service_tests {
    use std::time::Duration;

    use crate::replay_service::{ReplayPacing, ReplayService};
    use quorp_agent_core::{RuntimeEvent, StopReason};
    use quorp_desktop_ipc::{DesktopEvent, RunIdDto};

    fn write_events_jsonl(path: &std::path::Path, events: &[RuntimeEvent]) {
        let mut body = String::new();
        for event in events {
            body.push_str(&serde_json::to_string(event).unwrap());
            body.push('\n');
        }
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn replay_emits_runtime_batches() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        write_events_jsonl(
            &path,
            &[
                RuntimeEvent::RunStarted {
                    goal: "demo".into(),
                    model_id: "m".into(),
                },
                RuntimeEvent::ToolCallStarted {
                    step: 1,
                    action: "shell".into(),
                },
                RuntimeEvent::RunFinished {
                    reason: StopReason::Success,
                    total_steps: 1,
                    total_billed_tokens: 0,
                    duration_ms: 5,
                },
            ],
        );
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let service = ReplayService::new();
        let count = service
            .replay(
                &path,
                RunIdDto::new("run-rep"),
                sink,
                ReplayPacing::Instant,
                2,
            )
            .await
            .unwrap();
        assert_eq!(count, 3);
        let mut batches = 0;
        let mut total = 0;
        while let Ok(evt) = events.try_recv() {
            if let DesktopEvent::Runtime { batch, .. } = evt {
                batches += 1;
                total += batch.len();
            }
        }
        assert!(batches >= 2);
        assert_eq!(total, 3);
    }

    #[tokio::test]
    async fn replay_missing_path_errors() {
        let service = ReplayService::new();
        let (sink, _) = tokio::sync::mpsc::unbounded_channel();
        let err = service
            .replay(
                std::path::Path::new("/this/path/does/not/exist.jsonl"),
                RunIdDto::new("run-x"),
                sink,
                ReplayPacing::Instant,
                4,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::replay_service::ReplayError::Missing(_)
        ));
    }

    #[tokio::test]
    async fn replay_pacing_does_not_lose_events() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let evts: Vec<_> = (0..5)
            .map(|i| RuntimeEvent::ToolCallStarted {
                step: i,
                action: format!("step-{i}"),
            })
            .collect();
        write_events_jsonl(&path, &evts);
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let service = ReplayService::new();
        let count = service
            .replay(
                &path,
                RunIdDto::new("run-paced"),
                sink,
                ReplayPacing::Fixed(Duration::from_millis(1)),
                2,
            )
            .await
            .unwrap();
        assert_eq!(count, 5);
        let mut total = 0;
        while let Ok(evt) = events.try_recv() {
            if let DesktopEvent::Runtime { batch, .. } = evt {
                total += batch.len();
            }
        }
        assert_eq!(total, 5);
    }
}

mod run_service_tests {
    use std::sync::Arc;

    use crate::run_service::{RunError, RunService};
    use crate::workspace_registry::WorkspaceRegistry;
    use quorp_desktop_ipc::{
        DesktopEvent, PermissionModeDto, RunPhaseDto, SandboxModeDto, StartRunRequest,
        StopReasonDto, TrustDecision,
    };

    fn add_workspace(registry: &WorkspaceRegistry) -> quorp_desktop_ipc::WorkspaceId {
        let temp = tempfile::tempdir().unwrap();
        let record = registry.add(temp.path()).unwrap();
        // Leak the tempdir so it survives for the test lifetime.
        std::mem::forget(temp);
        record.id
    }

    #[test]
    fn sanitize_rejects_yolo_on_untrusted_workspace() {
        let registry = WorkspaceRegistry::new();
        let id = add_workspace(&registry);
        let runs = RunService::new();
        let req = StartRunRequest {
            workspace_id: id,
            goal: "x".into(),
            permission_mode: PermissionModeDto::YoloSandbox,
            sandbox_mode: SandboxModeDto::TmpCopy,
            model_id: None,
            wall_clock_budget_seconds: None,
        };
        let err = runs.sanitize_request(&registry, req).unwrap_err();
        assert!(matches!(err, RunError::TrustRequired(_)));
    }

    #[test]
    fn sanitize_rewrites_host_to_confined_on_untrusted() {
        let registry = WorkspaceRegistry::new();
        let id = add_workspace(&registry);
        let runs = RunService::new();
        let req = StartRunRequest {
            workspace_id: id,
            goal: "x".into(),
            permission_mode: PermissionModeDto::Ask,
            sandbox_mode: SandboxModeDto::Host,
            model_id: None,
            wall_clock_budget_seconds: None,
        };
        let sanitized = runs.sanitize_request(&registry, req).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(sanitized.sandbox_mode, SandboxModeDto::MacAppleSandbox);
        } else {
            assert_eq!(sanitized.sandbox_mode, SandboxModeDto::TmpCopy);
        }
    }

    #[test]
    fn sanitize_passes_trusted_request_unchanged() {
        let registry = WorkspaceRegistry::new();
        let id = add_workspace(&registry);
        registry.set_trust(&id, TrustDecision::Trusted).unwrap();
        let runs = RunService::new();
        let req = StartRunRequest {
            workspace_id: id,
            goal: "x".into(),
            permission_mode: PermissionModeDto::YoloSandbox,
            sandbox_mode: SandboxModeDto::Host,
            model_id: None,
            wall_clock_budget_seconds: None,
        };
        let sanitized = runs.sanitize_request(&registry, req).unwrap();
        assert_eq!(sanitized.permission_mode, PermissionModeDto::YoloSandbox);
        assert_eq!(sanitized.sandbox_mode, SandboxModeDto::Host);
    }

    #[test]
    fn sanitize_rejects_unknown_workspace() {
        let registry = WorkspaceRegistry::new();
        let runs = RunService::new();
        let req = StartRunRequest {
            workspace_id: quorp_desktop_ipc::WorkspaceId::new("ws-bogus"),
            goal: "x".into(),
            permission_mode: PermissionModeDto::Ask,
            sandbox_mode: SandboxModeDto::TmpCopy,
            model_id: None,
            wall_clock_budget_seconds: None,
        };
        let err = runs.sanitize_request(&registry, req).unwrap_err();
        assert!(matches!(err, RunError::WorkspaceNotFound(_)));
    }

    #[test]
    fn cancel_unknown_run_errors() {
        let runs = RunService::new();
        let err = runs
            .cancel_run(&quorp_desktop_ipc::RunIdDto::new("nope"))
            .unwrap_err();
        assert!(matches!(err, RunError::NotFound(_)));
    }

    #[tokio::test]
    async fn demo_run_emits_lifecycle() {
        let registry = WorkspaceRegistry::new();
        let id = add_workspace(&registry);
        let runs = Arc::new(RunService::new());
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let req = StartRunRequest {
            workspace_id: id,
            goal: "demo".into(),
            permission_mode: PermissionModeDto::Ask,
            sandbox_mode: SandboxModeDto::TmpCopy,
            model_id: None,
            wall_clock_budget_seconds: None,
        };
        let handle = runs.start_demo_run(req, sink).unwrap();
        // Drain events with a generous total timeout. We expect at
        // least one RunStarted, at least one Runtime batch, and one
        // RunFinished.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut started = false;
        let mut finished = false;
        let mut runtime_batches = 0;
        while std::time::Instant::now() < deadline && !finished {
            if let Ok(Some(evt)) =
                tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await
            {
                match evt {
                    DesktopEvent::RunStarted { run_id, .. } => {
                        assert_eq!(run_id, handle.run_id);
                        started = true;
                    }
                    DesktopEvent::Runtime { run_id, .. } => {
                        assert_eq!(run_id, handle.run_id);
                        runtime_batches += 1;
                    }
                    DesktopEvent::RunFinished {
                        run_id,
                        stop_reason,
                        ..
                    } => {
                        assert_eq!(run_id, handle.run_id);
                        assert_eq!(stop_reason, StopReasonDto::Completed);
                        finished = true;
                    }
                    _ => {}
                }
            }
        }
        assert!(started, "RunStarted not emitted");
        assert!(runtime_batches >= 1, "no Runtime batches");
        assert!(finished, "RunFinished not emitted");

        let status = runs.status(&handle.run_id).expect("status");
        assert_eq!(status.phase, RunPhaseDto::Finished);
        assert_eq!(status.stop_reason, Some(StopReasonDto::Completed));
    }
}

mod artifact_store_tests {
    use std::path::Path;

    use crate::artifact_store::ArtifactStore;
    use quorp_desktop_ipc::{ArtifactKind, RunIdDto};

    fn write_run(workspace_root: &Path, run_id: &str, files: &[(&str, &str)]) {
        let dir = workspace_root.join(".quorp").join("runs").join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
    }

    #[tokio::test]
    async fn read_kind_returns_full_text_for_small_files() {
        let temp = tempfile::tempdir().unwrap();
        write_run(
            temp.path(),
            "r1",
            &[("summary.json", "{\"stop_reason\":\"Success\"}")],
        );
        let store = ArtifactStore::default();
        let win = store
            .read_kind(temp.path(), &RunIdDto::new("r1"), ArtifactKind::Summary, 0, 4096)
            .await
            .unwrap();
        assert_eq!(win.content, "{\"stop_reason\":\"Success\"}");
        assert!(!win.binary_encoded);
        assert!(!win.is_truncated);
        assert!(win.total_size > 0);
    }

    #[tokio::test]
    async fn read_kind_pages_truncated_files() {
        let temp = tempfile::tempdir().unwrap();
        let body: String = (0..2048).map(|_| 'x').collect();
        write_run(temp.path(), "r2", &[("summary.json", &body)]);
        let store = ArtifactStore::default();
        let win = store
            .read_kind(
                temp.path(),
                &RunIdDto::new("r2"),
                ArtifactKind::Summary,
                0,
                512,
            )
            .await
            .unwrap();
        assert_eq!(win.content.len(), 512);
        assert!(win.is_truncated);
    }

    #[tokio::test]
    async fn read_kind_missing_artifact_errors() {
        let temp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::default();
        let err = store
            .read_kind(
                temp.path(),
                &RunIdDto::new("missing"),
                ArtifactKind::Summary,
                0,
                100,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, crate::ArtifactError::Missing(_)));
    }

    #[tokio::test]
    async fn read_event_window_filters_by_seq_range() {
        let temp = tempfile::tempdir().unwrap();
        let body = (0..5)
            .map(|i| format!("{{\"seq\": {i}, \"event\":\"x\"}}\n"))
            .collect::<String>();
        write_run(temp.path(), "r3", &[("events.jsonl", &body)]);
        let store = ArtifactStore::default();
        let events = store
            .read_event_window(temp.path(), &RunIdDto::new("r3"), 1, 4)
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        let first = events[0]["seq"].as_u64().unwrap();
        assert_eq!(first, 1);
    }

    #[tokio::test]
    async fn read_event_window_skips_blank_and_malformed_lines() {
        let temp = tempfile::tempdir().unwrap();
        let body = "\n{\"seq\": 0,\"event\":\"a\"}\n   \nnot-json\n{\"seq\": 1,\"event\":\"b\"}\n";
        write_run(temp.path(), "r4", &[("events.jsonl", body)]);
        let store = ArtifactStore::default();
        let events = store
            .read_event_window(temp.path(), &RunIdDto::new("r4"), 0, 100)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn list_kinds_finds_only_present_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        write_run(
            temp.path(),
            "r5",
            &[("summary.json", "{}"), ("transcript.json", "[]")],
        );
        let store = ArtifactStore::default();
        let kinds = store
            .list_kinds(temp.path(), &RunIdDto::new("r5"))
            .await
            .unwrap();
        assert!(kinds.contains(&ArtifactKind::Summary));
        assert!(kinds.contains(&ArtifactKind::Transcript));
        assert!(!kinds.contains(&ArtifactKind::EventsJsonl));
    }

    #[tokio::test]
    async fn list_kinds_missing_run_dir_errors() {
        let temp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::default();
        let err = store
            .list_kinds(temp.path(), &RunIdDto::new("nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, crate::ArtifactError::Missing(_)));
    }
}

mod diff_applier_tests {
    use std::path::Path;

    use quorp_desktop_ipc::{RunIdDto, WorkspaceId};

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// Build a minimal `<workspace>/.quorp/runs/<run_id>/final.diff`
    /// scaffold and return `(workspace_root, run_dir)`.
    fn scaffold(
        body: Option<&str>,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().to_path_buf();
        let run_dir = workspace.join(".quorp/runs/run-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        if let Some(b) = body {
            std::fs::write(run_dir.join("final.diff"), b).unwrap();
        }
        (temp, workspace, run_dir)
    }

    #[tokio::test]
    async fn missing_diff_returns_zero_applied() {
        let (_temp, workspace, run_dir) = scaffold(None);
        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 0);
        assert_eq!(receipt.conflict_files, 0);
        assert!(receipt.message.contains("no final.diff"));
    }

    #[tokio::test]
    async fn empty_diff_returns_zero_applied() {
        let (_temp, workspace, run_dir) = scaffold(Some(""));
        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 0);
        assert!(receipt.message.contains("empty"));
    }

    #[tokio::test]
    async fn in_process_applier_modifies_existing_file() {
        let (_temp, workspace, run_dir) = scaffold(None);
        write(&workspace.join("greet.txt"), "hello\nworld\n");
        let diff = "diff --git a/greet.txt b/greet.txt\n--- a/greet.txt\n+++ b/greet.txt\n@@ -1,2 +1,2 @@\n hello\n-world\n+earth\n";
        std::fs::write(run_dir.join("final.diff"), diff).unwrap();

        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 1);
        assert_eq!(receipt.conflict_files, 0);
        let body = std::fs::read_to_string(workspace.join("greet.txt")).unwrap();
        assert_eq!(body, "hello\nearth\n");
    }

    #[tokio::test]
    async fn in_process_applier_creates_new_file() {
        let (_temp, workspace, run_dir) = scaffold(None);
        let diff = "diff --git a/notes.md b/notes.md\nnew file mode 100644\n--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1,2 @@\n+title\n+body\n";
        std::fs::write(run_dir.join("final.diff"), diff).unwrap();

        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 1);
        let body = std::fs::read_to_string(workspace.join("notes.md")).unwrap();
        assert_eq!(body, "title\nbody\n");
    }

    #[tokio::test]
    async fn in_process_applier_reports_conflicts_atomically() {
        let (_temp, workspace, run_dir) = scaffold(None);
        // The file's actual content disagrees with the diff's
        // expected context line — apply must abort with no on-disk
        // changes.
        write(&workspace.join("file.txt"), "actual content\n");
        let diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n-expected content\n+new content\n";
        std::fs::write(run_dir.join("final.diff"), diff).unwrap();

        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 0);
        assert_eq!(receipt.conflict_files, 1);
        // File untouched.
        let body = std::fs::read_to_string(workspace.join("file.txt")).unwrap();
        assert_eq!(body, "actual content\n");
    }

    #[tokio::test]
    async fn missing_workspace_root_errors() {
        let temp = tempfile::tempdir().unwrap();
        let bogus = temp.path().join("does-not-exist");
        let result = crate::apply_run_diff(
            &temp.path().join("run"),
            &bogus,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::ApplyDiffError::WorkspaceMissing(_))
        ));
    }

    #[tokio::test]
    async fn unused_placeholder_for_diff_applier() {
        // (Placeholder so the next test's anchor stays unique.)
        let _ = ();
    }

    #[tokio::test]
    async fn multi_file_in_process_apply_succeeds() {
        let (_temp, workspace, run_dir) = scaffold(None);
        write(&workspace.join("a.txt"), "x\n");
        write(&workspace.join("b.txt"), "y\n");
        let diff = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1,1 +1,1 @@\n",
            "-x\n",
            "+x2\n",
            "diff --git a/b.txt b/b.txt\n",
            "--- a/b.txt\n",
            "+++ b/b.txt\n",
            "@@ -1,1 +1,1 @@\n",
            "-y\n",
            "+y2\n",
        );
        std::fs::write(run_dir.join("final.diff"), diff).unwrap();
        let receipt = crate::apply_run_diff(
            &run_dir,
            &workspace,
            RunIdDto::new("run-1"),
            WorkspaceId::new("ws-1"),
        )
        .await
        .unwrap();
        assert_eq!(receipt.applied_files, 2);
        assert_eq!(receipt.conflict_files, 0);
        assert_eq!(std::fs::read_to_string(workspace.join("a.txt")).unwrap(), "x2\n");
        assert_eq!(std::fs::read_to_string(workspace.join("b.txt")).unwrap(), "y2\n");
    }
}

mod rules_adapter_tests {
    use std::path::PathBuf;

    use crate::rules_adapter::{list_rules, update_lifecycle};

    fn write(path: &PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn empty_workspace_returns_empty_list() {
        let temp = tempfile::tempdir().unwrap();
        let rules = list_rules(temp.path()).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn repo_level_rules_file_surfaces_as_active() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join(".rules"),
            "* always test\n* never delete prod\n",
        );
        let rules = list_rules(temp.path()).unwrap();
        assert_eq!(rules.len(), 1);
        let row = &rules[0];
        assert_eq!(row.lifecycle, "active");
        assert_eq!(row.evidence_count, 2);
        assert!(row.display_name.contains("repo"));
        assert!(row.id.starts_with("rule-"));
    }

    #[test]
    fn project_rules_default_to_draft() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join(".quorp/rules/style.rules"),
            "- two-space indent\n- single-quoted strings\n- trailing commas\n",
        );
        let rules = list_rules(temp.path()).unwrap();
        assert_eq!(rules.len(), 1);
        let row = &rules[0];
        assert_eq!(row.lifecycle, "draft");
        assert_eq!(row.evidence_count, 3);
    }

    #[test]
    fn update_lifecycle_persists_via_ledger() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join(".quorp/rules/style.rules"),
            "- keep types\n",
        );
        let initial = list_rules(temp.path()).unwrap();
        let id = initial[0].id.clone();

        let new_state = update_lifecycle(temp.path(), &id, "active").unwrap();
        assert_eq!(new_state, "active");

        let after = list_rules(temp.path()).unwrap();
        assert_eq!(after[0].lifecycle, "active");

        // Ledger file exists on disk.
        let ledger = temp.path().join(".quorp/rules/lifecycle.json");
        assert!(ledger.exists());

        // Round-trip another change.
        let new_state2 = update_lifecycle(temp.path(), &id, "suspended").unwrap();
        assert_eq!(new_state2, "suspended");
        let final_state = list_rules(temp.path()).unwrap();
        assert_eq!(final_state[0].lifecycle, "suspended");
    }

    #[test]
    fn update_lifecycle_rejects_unknown_state() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".rules"), "* x\n");
        let initial = list_rules(temp.path()).unwrap();
        let err = update_lifecycle(temp.path(), &initial[0].id, "weird").unwrap_err();
        assert!(matches!(err, crate::RulesAdapterError::Ledger(_)));
    }

    #[test]
    fn evidence_count_handles_mixed_bullet_styles() {
        let temp = tempfile::tempdir().unwrap();
        write(
            &temp.path().join(".rules"),
            "Header\n\n* one\n- two\n+ three\nNot a bullet\n  * indented also counts\n",
        );
        let rules = list_rules(temp.path()).unwrap();
        assert_eq!(rules[0].evidence_count, 4);
    }

    #[test]
    fn lifecycle_ledger_survives_truncation() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join(".rules"), "* x\n");
        // Empty / truncated ledger file shouldn't fail the load.
        let ledger = temp.path().join(".quorp/rules/lifecycle.json");
        write(&ledger, "");
        let rules = list_rules(temp.path()).unwrap();
        assert_eq!(rules[0].lifecycle, "active");
    }
}

mod memory_adapter_tests {
    use crate::MemoryAdapter;

    #[tokio::test]
    async fn unknown_tier_is_invalid_input() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = MemoryAdapter::new();
        let err = adapter
            .query(temp.path(), "no-such-tier", String::new(), 10)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::MemoryAdapterError::UnknownTier(_)
        ));
    }

    #[tokio::test]
    async fn empty_workspace_returns_no_hits() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = MemoryAdapter::new();
        let result = adapter
            .query(temp.path(), "working", String::new(), 10)
            .await
            .unwrap();
        assert_eq!(result.tier, "working");
        assert_eq!(result.total, 0);
        assert!(result.items.is_empty());
    }

    #[tokio::test]
    async fn tier_label_round_trips_each_supported_tier() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = MemoryAdapter::new();
        for tier in [
            "working",
            "episodic",
            "semantic",
            "procedural",
            "negative",
            "rule",
        ] {
            let result = adapter
                .query(temp.path(), tier, String::new(), 5)
                .await
                .unwrap();
            assert_eq!(result.tier, tier);
        }
    }

    #[tokio::test]
    async fn forget_drops_cached_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let adapter = MemoryAdapter::new();
        // Acquire once so the cache populates, then forget; the next
        // call must succeed without panicking on a stale handle.
        let _ = adapter
            .query(temp.path(), "working", String::new(), 1)
            .await
            .unwrap();
        adapter.forget(temp.path());
        let _ = adapter
            .query(temp.path(), "working", String::new(), 1)
            .await
            .unwrap();
    }
}

mod rollback_tests {
    use std::path::Path;

    use quorp_desktop_ipc::{RunIdDto, WorkspaceId};

    use crate::rollback::{RollbackError, rollback_to_checkpoint};

    /// Build a run dir with `events.jsonl` carrying a fixed sequence
    /// of `CheckpointSaved` payloads at the supplied counters.
    fn write_run_with_checkpoints(dir: &Path, counters: &[(u64, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut body = String::new();
        for (counter, marker) in counters {
            body.push_str(&format!(
                "{{\"event\":\"checkpoint_saved\",\"checkpoint\":{{\"request_counter\":{counter},\"step\":{counter},\"marker\":\"{marker}\"}}}}\n"
            ));
        }
        std::fs::write(dir.join("events.jsonl"), body).unwrap();
    }

    #[test]
    fn missing_run_dir_errors() {
        let temp = tempfile::tempdir().unwrap();
        let bogus = temp.path().join("nope");
        let err = rollback_to_checkpoint(
            &bogus,
            7,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap_err();
        assert!(matches!(err, RollbackError::RunDirMissing(_)));
    }

    #[test]
    fn missing_events_jsonl_errors() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let err = rollback_to_checkpoint(
            &run_dir,
            7,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap_err();
        assert!(matches!(err, RollbackError::EventsMissing(_)));
    }

    #[test]
    fn unknown_counter_returns_checkpoint_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        write_run_with_checkpoints(&run_dir, &[(1, "first"), (2, "second")]);
        let err = rollback_to_checkpoint(
            &run_dir,
            999,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap_err();
        assert!(matches!(err, RollbackError::CheckpointNotFound(999)));
    }

    #[test]
    fn rollback_writes_checkpoint_json_when_none_exists() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        write_run_with_checkpoints(&run_dir, &[(1, "first"), (2, "second")]);
        let receipt = rollback_to_checkpoint(
            &run_dir,
            2,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap();
        assert_eq!(receipt.restored_files, 1); // only the new checkpoint.json
        assert!(receipt.backup_filename.is_empty());
        let written =
            std::fs::read_to_string(run_dir.join("checkpoint.json")).unwrap();
        assert!(written.contains("\"second\""));
        assert!(written.contains("\"request_counter\""));
    }

    #[test]
    fn rollback_backs_up_existing_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        write_run_with_checkpoints(&run_dir, &[(1, "first"), (5, "fifth")]);
        // Pre-existing checkpoint.json — this is what we expect to
        // back up before overwriting.
        std::fs::write(run_dir.join("checkpoint.json"), b"{\"original\":true}")
            .unwrap();

        let receipt = rollback_to_checkpoint(
            &run_dir,
            1,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap();

        assert_eq!(receipt.restored_files, 2); // backup + new checkpoint
        assert!(!receipt.backup_filename.is_empty());
        assert!(receipt.backup_filename.starts_with("checkpoint-pre-rollback-"));
        assert!(receipt.backup_filename.ends_with(".json"));

        // Backup carries the original payload verbatim.
        let backup = std::fs::read_to_string(run_dir.join(&receipt.backup_filename))
            .unwrap();
        assert_eq!(backup, "{\"original\":true}");

        // New checkpoint.json carries the matched event.
        let new_body =
            std::fs::read_to_string(run_dir.join("checkpoint.json")).unwrap();
        assert!(new_body.contains("\"first\""));
    }

    #[test]
    fn duplicate_counter_uses_most_recent_match() {
        // Sometimes a checkpoint is re-saved at the same counter
        // (e.g. on resume). The most recent line wins.
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        write_run_with_checkpoints(
            &run_dir,
            &[(1, "old"), (2, "between"), (1, "new")],
        );
        let receipt = rollback_to_checkpoint(
            &run_dir,
            1,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap();
        let body =
            std::fs::read_to_string(run_dir.join("checkpoint.json")).unwrap();
        assert!(body.contains("\"new\""));
        assert!(!body.contains("\"old\""));
        assert!(receipt.message.contains("request_counter=1"));
    }

    #[test]
    fn malformed_event_lines_are_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let run_dir = temp.path().join("run-1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let body = concat!(
            "not-json\n",
            "{\"event\":\"phase_changed\",\"phase\":\"warming\"}\n",
            "\n",
            "{\"event\":\"checkpoint_saved\",\"checkpoint\":{\"request_counter\":3,\"step\":3}}\n",
        );
        std::fs::write(run_dir.join("events.jsonl"), body).unwrap();
        let receipt = rollback_to_checkpoint(
            &run_dir,
            3,
            RunIdDto::new("r1"),
            WorkspaceId::new("ws-1"),
        )
        .unwrap();
        assert_eq!(receipt.restored_files, 1);
    }
}
