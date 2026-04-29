//! Round-trip and shape-stability tests for the desktop IPC wire types.

use crate::permission_dto::{PermissionDecisionKind, PermissionScope, RiskLevel};
use crate::{
    ArtifactId, ArtifactKind, ArtifactWindow, CapabilityTokenDto, DEFAULT_MODEL_ID,
    DEFAULT_PROVIDER_NAME, DESKTOP_WIRE_VERSION, DesktopEvent, DiffSummary, IpcError, IpcErrorCode,
    PermissionDecisionDto, PermissionModeDto, PermissionRequestDto, PermissionRequestId,
    RunFailureStage, RunHandle, RunIdDto, RuntimeEventDto, SandboxModeDto, StartRunRequest,
    StopReasonDto, TokenUsageDto, TrustDecision, ValidationStatusDto, WorkspaceId,
    WorkspaceSummary,
};

#[test]
fn wire_version_is_one() {
    assert_eq!(DESKTOP_WIRE_VERSION, 1);
}

#[test]
fn default_provider_constants_match_spec() {
    assert_eq!(DEFAULT_PROVIDER_NAME, "nvidia-nim");
    assert_eq!(DEFAULT_MODEL_ID, "qwen/qwen3-coder-480b-a35b-instruct");
}

fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let bytes = serde_json::to_vec(value).expect("serialize");
    serde_json::from_slice(&bytes).expect("deserialize")
}

#[test]
fn run_id_serializes_transparently() {
    let id = RunIdDto::new("run-2026-04-28-001");
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"run-2026-04-28-001\"");
    let back: RunIdDto = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn workspace_summary_roundtrips() {
    let value = WorkspaceSummary {
        id: WorkspaceId::new("ws-1"),
        canonical_path: "/Users/me/Code/quorp".into(),
        display_name: "quorp".into(),
        trust: TrustDecision::Untrusted,
        last_opened_at: Some("2026-04-28T15:00:00Z".into()),
        pinned: false,
        run_count: 7,
    };
    let back = round_trip(&value);
    assert_eq!(back.id, value.id);
    assert_eq!(back.canonical_path, value.canonical_path);
    assert_eq!(back.trust, value.trust);
    assert_eq!(back.run_count, value.run_count);
}

#[test]
fn start_run_request_with_defaults_roundtrips() {
    let value = StartRunRequest {
        workspace_id: WorkspaceId::new("ws-1"),
        goal: "fix the decoder bug".into(),
        permission_mode: PermissionModeDto::Ask,
        sandbox_mode: SandboxModeDto::MacAppleSandbox,
        model_id: None,
        wall_clock_budget_seconds: Some(1800),
    };
    let back = round_trip(&value);
    assert_eq!(back.workspace_id, value.workspace_id);
    assert_eq!(back.goal, value.goal);
    assert_eq!(back.sandbox_mode, value.sandbox_mode);
}

#[test]
fn run_handle_roundtrips() {
    let value = RunHandle {
        run_id: RunIdDto::new("run-x"),
        started_at: "2026-04-28T15:00:00Z".into(),
    };
    let back = round_trip(&value);
    assert_eq!(back.run_id, value.run_id);
    assert_eq!(back.started_at, value.started_at);
}

#[test]
fn permission_request_carries_capability_tokens() {
    let value = PermissionRequestDto {
        request_id: PermissionRequestId::new("perm-1"),
        run_id: RunIdDto::new("run-1"),
        action_summary: "cargo install bandage".into(),
        tool: "shell".into(),
        cwd: Some("/tmp/quorp/run-1/work".into()),
        tokens: vec![
            CapabilityTokenDto::Network,
            CapabilityTokenDto::DependencyInstall,
            CapabilityTokenDto::Other {
                label: "writes_cargo_home".into(),
            },
        ],
        risk: RiskLevel::High,
        reason: Some("downloads from the network".into()),
        requested_at: "2026-04-28T15:00:00Z".into(),
    };
    let back = round_trip(&value);
    assert_eq!(back.tokens.len(), 3);
    assert!(matches!(back.tokens[0], CapabilityTokenDto::Network));
    assert!(matches!(
        back.tokens[1],
        CapabilityTokenDto::DependencyInstall
    ));
    if let CapabilityTokenDto::Other { label } = &back.tokens[2] {
        assert_eq!(label, "writes_cargo_home");
    } else {
        panic!("expected Other variant");
    }
}

#[test]
fn permission_decision_roundtrips() {
    let value = PermissionDecisionDto {
        decision: PermissionDecisionKind::Allow,
        scope: PermissionScope::Session,
    };
    let back = round_trip(&value);
    assert_eq!(back.decision, value.decision);
    assert_eq!(back.scope, value.scope);
}

#[test]
fn desktop_event_runtime_batch_uses_kind_tag() {
    let value = DesktopEvent::Runtime {
        run_id: RunIdDto::new("run-2"),
        batch: vec![RuntimeEventDto::StatusUpdate {
            seq: 0,
            status: "running".into(),
        }],
        batch_seq: 0,
    };
    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(json["kind"], "runtime");
    assert!(json["batch"].is_array());
}

#[test]
fn desktop_event_run_started_serializes() {
    let value = DesktopEvent::RunStarted {
        run_id: RunIdDto::new("run-3"),
        goal: "smoke".into(),
        model_id: DEFAULT_MODEL_ID.into(),
        started_at: "2026-04-28T15:00:00Z".into(),
    };
    let back = round_trip(&value);
    if let DesktopEvent::RunStarted { goal, .. } = back {
        assert_eq!(goal, "smoke");
    } else {
        panic!("expected RunStarted");
    }
}

#[test]
fn desktop_event_run_failed_carries_stage() {
    let value = DesktopEvent::RunFailed {
        run_id: RunIdDto::new("run-4"),
        error: "boom".into(),
        stage: RunFailureStage::SandboxSetup,
    };
    let json = serde_json::to_value(&value).unwrap();
    assert_eq!(json["kind"], "run_failed");
    assert_eq!(json["stage"], "sandbox_setup");
}

#[test]
fn runtime_event_dto_other_carries_payload() {
    let value = RuntimeEventDto::Other {
        seq: 9,
        kind: "future_thing".into(),
        payload: serde_json::json!({"some": "data"}),
    };
    let back = round_trip(&value);
    if let RuntimeEventDto::Other { kind, .. } = back {
        assert_eq!(kind, "future_thing");
    } else {
        panic!("expected Other");
    }
}

#[test]
fn token_usage_default_is_zero() {
    let usage = TokenUsageDto::default();
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn artifact_window_roundtrips() {
    let value = ArtifactWindow {
        run_id: RunIdDto::new("run-5"),
        kind: ArtifactKind::EventsJsonl,
        offset: 0,
        limit: 1024,
        total_size: 2048,
        content_hash: "deadbeef".into(),
        binary_encoded: false,
        content: "{\"event\":\"status_update\"}\n".into(),
        is_truncated: true,
    };
    let back = round_trip(&value);
    assert_eq!(back.kind, ArtifactKind::EventsJsonl);
    assert!(back.is_truncated);
}

#[test]
fn diff_summary_keeps_sample_paths() {
    let value = DiffSummary {
        diff_id: ArtifactId::new("diff-1"),
        files_changed: 4,
        additions: 12,
        deletions: 3,
        sample_paths: vec!["src/decoder.rs".into(), "src/limits.rs".into()],
    };
    let back = round_trip(&value);
    assert_eq!(back.sample_paths.len(), 2);
    assert_eq!(back.additions, 12);
}

#[test]
fn ipc_error_redaction_helpers() {
    let err = IpcError::not_implemented("multi-window").with_cause("scheduled for PR10");
    assert!(matches!(err.code, IpcErrorCode::NotImplemented));
    assert_eq!(err.cause.as_deref(), Some("scheduled for PR10"));
}

#[test]
fn validation_status_serializes_snake_case() {
    let json = serde_json::to_string(&ValidationStatusDto::Failed).unwrap();
    assert_eq!(json, "\"failed\"");
}

#[test]
fn stop_reason_serializes_snake_case() {
    let json = serde_json::to_string(&StopReasonDto::BudgetExhausted).unwrap();
    assert_eq!(json, "\"budget_exhausted\"");
}
