// TypeScript mirrors of `quorp_desktop_ipc` DTOs. Hand-written to
// match the Rust definitions; we keep the tags / field names verbatim
// so serde's snake_case output deserializes 1:1.
//
// When extending these, update the Rust source first, then regenerate
// this file (or hand-edit). The wire-version constant on the Rust
// side guards against drift; the desktop refuses to attach if the
// numbers disagree.

export const DESKTOP_WIRE_VERSION = 1;
export const DEFAULT_MODEL_ID = "qwen/qwen3-coder-480b-a35b-instruct";
export const DEFAULT_PROVIDER_NAME = "nvidia-nim";

export type RunIdDto = string;
export type WorkspaceId = string;
export type ArtifactId = string;
export type PermissionRequestId = string;

export type TrustDecision = "untrusted" | "trusted_for_read_only" | "trusted";

export interface WorkspaceSummary {
  id: WorkspaceId;
  canonical_path: string;
  display_name: string;
  trust: TrustDecision;
  last_opened_at: string | null;
  pinned: boolean;
  run_count: number;
}

export interface TrustReceipt {
  workspace_id: WorkspaceId;
  previous: TrustDecision;
  current: TrustDecision;
  decided_at: string;
}

export type PermissionModeDto =
  | "read_only"
  | "ask"
  | "accept_edits"
  | "auto_safe"
  | "yolo_sandbox";

export type SandboxModeDto =
  | "host"
  | "git_worktree"
  | "tmp_copy"
  | "mac_apple_sandbox";

export type RiskLevel = "low" | "medium" | "high" | "critical";

export type CapabilityTokenDto =
  | { kind: "shell_metacharacters" }
  | { kind: "compound_command" }
  | { kind: "filesystem_write" }
  | { kind: "filesystem_delete" }
  | { kind: "network" }
  | { kind: "secrets_read" }
  | { kind: "container" }
  | { kind: "generated_executable" }
  | { kind: "git_remote_mutation" }
  | { kind: "dependency_install" }
  | { kind: "mcp" }
  | { kind: "browser" }
  | { kind: "other"; label: string };

export interface PermissionRequestDto {
  request_id: PermissionRequestId;
  run_id: RunIdDto;
  action_summary: string;
  tool: string;
  cwd: string | null;
  tokens: CapabilityTokenDto[];
  risk: RiskLevel;
  reason: string | null;
  requested_at: string;
}

export type PermissionScope = "once" | "session" | "project";
export type PermissionDecisionKind = "allow" | "deny";

export interface PermissionDecisionDto {
  decision: PermissionDecisionKind;
  scope: PermissionScope;
}

export type RunPhaseDto = "starting" | "running" | "cancelling" | "finished" | "failed";

export type StopReasonDto =
  | "completed"
  | "cancelled"
  | "timeout"
  | "budget_exhausted"
  | "tool_failure"
  | "policy_denied"
  | "fatal_error"
  | "unknown_error";

export interface StartRunRequest {
  workspace_id: WorkspaceId;
  goal: string;
  permission_mode: PermissionModeDto;
  sandbox_mode: SandboxModeDto;
  model_id: string | null;
  wall_clock_budget_seconds: number | null;
}

export interface BenchmarkOptions {
  fixture_id: string;
  permission_mode: PermissionModeDto;
  sandbox_mode: SandboxModeDto;
  model_id: string | null;
  wall_clock_budget_seconds: number | null;
}

export interface RunHandle {
  run_id: RunIdDto;
  started_at: string;
}

export interface RunStatusDto {
  run_id: RunIdDto;
  workspace_id: WorkspaceId;
  phase: RunPhaseDto;
  permission_mode: PermissionModeDto;
  sandbox_mode: SandboxModeDto;
  model_id: string;
  current_step: number;
  total_billed_tokens: number;
  context_pressure: number | null;
  started_at: string;
  finished_at: string | null;
  stop_reason: StopReasonDto | null;
}

export type ValidationStatusDto =
  | "queued"
  | "running"
  | "passed"
  | "failed"
  | "skipped";

export type RunFailureStage =
  | "sandbox_setup"
  | "provider_connect"
  | "tool_execution"
  | "agent_loop"
  | "post_run"
  | "unknown";

export interface TokenUsageDto {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface DiffSummary {
  diff_id: ArtifactId;
  files_changed: number;
  additions: number;
  deletions: number;
  sample_paths: string[];
}

// 1:1 mirror of `RuntimeEventDto`. See
// crates/quorp_desktop_ipc/src/desktop_event.rs for the source.
export type RuntimeEventDto =
  | { event: "status_update"; seq: number; status: string }
  | { event: "phase_changed"; seq: number; phase: string; detail: string | null }
  | {
      event: "assistant_turn_summary";
      seq: number;
      step: number;
      assistant_message: string;
      actions: string[];
      wrote_files: boolean;
      validation_queued: boolean;
      parse_warning_count: number;
    }
  | { event: "fatal_error"; seq: number; error: string }
  | { event: "run_started"; seq: number; goal: string; model_id: string }
  | {
      event: "model_request_started";
      seq: number;
      step: number;
      request_id: number;
      message_count: number;
      prompt_token_estimate: number;
      completion_token_cap: number | null;
      safety_mode: string | null;
    }
  | {
      event: "model_request_finished";
      seq: number;
      step: number;
      request_id: number;
      usage: TokenUsageDto | null;
      watchdog: string | null;
    }
  | { event: "tool_call_started"; seq: number; step: number; action: string }
  | {
      event: "tool_call_finished";
      seq: number;
      step: number;
      action: string;
      status: string;
      action_kind: string;
      target_path: string | null;
      edit_summary: string | null;
    }
  | { event: "validation_started"; seq: number; step: number; summary: string }
  | {
      event: "validation_finished";
      seq: number;
      step: number;
      summary: string;
      status: string;
    }
  | {
      event: "path_resolution_failed";
      seq: number;
      step: number;
      action: string;
      request_path: string;
      suggested_path: string | null;
      reason: string | null;
      error: string;
    }
  | {
      event: "recovery_turn_queued";
      seq: number;
      step: number;
      action: string;
      suggested_path: string | null;
      message: string;
    }
  | {
      event: "policy_denied";
      seq: number;
      step: number;
      action: string;
      reason: string;
    }
  | {
      event: "subscriber_backpressure";
      seq: number;
      subscriber: string;
      dropped_events: number;
      capacity: number;
    }
  | {
      event: "checkpoint_saved";
      seq: number;
      step: number;
      request_counter: number;
    }
  | {
      event: "run_finished";
      seq: number;
      reason: string;
      total_steps: number;
      total_billed_tokens: number;
      duration_ms: number;
    }
  | { event: "other"; seq: number; kind: string; payload: unknown };

export interface IpcError {
  code:
    | "internal"
    | "not_implemented"
    | "invalid_input"
    | "workspace_not_found"
    | "trust_required"
    | "run_not_found"
    | "stale"
    | "runtime_error"
    | "sandbox_error"
    | "provider_error"
    | "provider_unauthorized"
    | "keychain_error"
    | "filesystem_error"
    | "cancelled"
    | "timeout"
    | "wire_version_mismatch";
  message: string;
  cause: string | null;
}

export type DesktopEvent =
  | {
      kind: "run_started";
      run_id: RunIdDto;
      goal: string;
      model_id: string;
      started_at: string;
    }
  | {
      kind: "run_finished";
      run_id: RunIdDto;
      stop_reason: StopReasonDto;
      total_steps: number;
      total_billed_tokens: number;
      duration_ms: number;
    }
  | {
      kind: "run_failed";
      run_id: RunIdDto;
      error: string;
      stage: RunFailureStage;
    }
  | {
      kind: "runtime";
      run_id: RunIdDto;
      batch: RuntimeEventDto[];
      batch_seq: number;
    }
  | {
      kind: "permission";
      run_id: RunIdDto;
      request: PermissionRequestDto;
    }
  | {
      kind: "diff";
      run_id: RunIdDto;
      diff_id: ArtifactId;
      summary: DiffSummary;
    }
  | {
      kind: "validation";
      run_id: RunIdDto;
      step: number;
      status: ValidationStatusDto;
      summary: string;
      proof_receipt_id: ArtifactId | null;
    }
  | {
      kind: "checkpoint";
      run_id: RunIdDto;
      step: number;
      request_counter: number;
      written_at: string;
    }
  | {
      kind: "context_compaction";
      run_id: RunIdDto;
      before_tokens: number;
      after_tokens: number;
      kept_messages: number;
    }
  | {
      kind: "error";
      run_id: RunIdDto | null;
      error: IpcError;
    };

export interface ProviderSummary {
  name: string;
  display_name: string;
  base_url: string;
  default_model: string;
  has_key: boolean;
}

export interface ProviderHealth {
  ok: boolean;
  latency_ms: number;
  model_id_echo: string | null;
  error: string | null;
}

export interface BenchmarkFixture {
  fixture_id: string;
  set: string;
  display_name: string;
  description: string;
  workspace_path: string;
  reference_proof_path: string | null;
  has_reference_proof: boolean;
}

export interface AppStatus {
  desktop_core_version: string;
  desktop_wire_version: number;
  sandbox_exec_present: boolean;
  has_active_runs: boolean;
  workspace_count: number;
  pending_permission_count: number;
  provider_has_key: boolean;
}

export type DoctorStatus = "ok" | "warn" | "fail" | "skipped";

export interface DoctorCheck {
  id: string;
  label: string;
  status: DoctorStatus;
  detail: string;
  remediation: string | null;
}

export interface DoctorReport {
  generated_at: string;
  checks: DoctorCheck[];
  overall: DoctorStatus;
}
