// Typed wrapper around Tauri's `invoke()` for the Quorp IPC surface.
// Keeps every command's signature in one place so the call sites can
// just `import { ipc } from "@/lib/invoke"`.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { Channel } from "@tauri-apps/api/core";

import type {
  AppStatus,
  ArtifactWindow,
  BenchmarkFixture,
  BenchmarkOptions,
  DesktopEvent,
  DoctorReport,
  PermissionDecisionDto,
  PermissionRequestId,
  ProviderHealth,
  ProviderSummary,
  RunHandle,
  RunIdDto,
  RunStatusDto,
  StartRunRequest,
  TrustDecision,
  TrustReceipt,
  WorkspaceId,
  WorkspaceSummary,
} from "@/types/ipc";

// Mirrors `ArtifactKind` from quorp_desktop_ipc::artifact_dto.
export type ArtifactKind =
  | "events_jsonl"
  | "summary"
  | "transcript"
  | "checkpoint"
  | "final_diff"
  | "proof_receipt"
  | "request"
  | "metadata"
  | "routing_summary"
  | "other";

export const ipc = {
  // workspace
  addWorkspace: (path: string) =>
    tauriInvoke<WorkspaceSummary>("add_workspace", { path }),
  listWorkspaces: () => tauriInvoke<WorkspaceSummary[]>("list_workspaces"),
  trustWorkspace: (id: WorkspaceId, decision: TrustDecision) =>
    tauriInvoke<TrustReceipt>("trust_workspace", { id, decision }),
  removeWorkspace: (id: WorkspaceId) =>
    tauriInvoke<void>("remove_workspace", { id }),
  openTerminalAt: (path: string) =>
    tauriInvoke<void>("open_terminal_at", { path }),

  // run
  startAgentRun: (request: StartRunRequest, onEvent: Channel<DesktopEvent>) =>
    tauriInvoke<RunHandle>("start_agent_run", { request, onEvent }),
  cancelRun: (runId: RunIdDto) => tauriInvoke<void>("cancel_run", { runId }),
  getRunStatus: (runId: RunIdDto) =>
    tauriInvoke<RunStatusDto>("get_run_status", { runId }),
  listActiveRuns: () => tauriInvoke<RunHandle[]>("list_active_runs"),

  // permission
  respondToPermission: (
    requestId: PermissionRequestId,
    decision: PermissionDecisionDto,
  ) =>
    tauriInvoke<void>("respond_to_permission", { requestId, decision }),
  pendingPermissionCount: () =>
    tauriInvoke<number>("pending_permission_count"),
  cancelAllPermissions: () => tauriInvoke<void>("cancel_all_permissions"),

  // artifacts
  readArtifact: (
    workspaceId: WorkspaceId,
    runId: RunIdDto,
    kind: ArtifactKind,
    offset: number,
    limit: number,
  ) =>
    tauriInvoke<ArtifactWindow>("read_artifact", {
      workspaceId,
      runId,
      kind,
      offset,
      limit,
    }),
  listRunArtifacts: (workspaceId: WorkspaceId, runId: RunIdDto) =>
    tauriInvoke<ArtifactKind[]>("list_run_artifacts", { workspaceId, runId }),
  readEventWindow: (
    workspaceId: WorkspaceId,
    runId: RunIdDto,
    fromSeq: number,
    toSeq: number,
  ) =>
    tauriInvoke<unknown[]>("read_event_window", {
      workspaceId,
      runId,
      fromSeq,
      toSeq,
    }),
  revealPath: (path: string) => tauriInvoke<void>("reveal_path", { path }),
  applyRunDiff: (runId: RunIdDto, targetWorkspaceId: WorkspaceId) =>
    tauriInvoke<{
      run_id: RunIdDto;
      target_workspace_id: WorkspaceId;
      applied_files: number;
      skipped_files: number;
      conflict_files: number;
    }>("apply_run_diff", { runId, targetWorkspaceId }),
  verifyRunAgain: (runId: RunIdDto) =>
    tauriInvoke<{ run_id: RunIdDto; verify_run_id: RunIdDto }>(
      "verify_run_again",
      { runId },
    ),
  rollbackToCheckpoint: (runId: RunIdDto, requestCounter: number) =>
    tauriInvoke<{
      run_id: RunIdDto;
      workspace_id: WorkspaceId;
      request_counter: number;
      restored_files: number;
      backup_filename: string;
      message: string;
    }>("rollback_to_checkpoint", { runId, requestCounter }),

  // benchmark
  listBenchmarkFixtures: () =>
    tauriInvoke<BenchmarkFixture[]>("list_benchmark_fixtures"),
  startBenchmarkRun: (
    options: BenchmarkOptions,
    onEvent: Channel<DesktopEvent>,
  ) => tauriInvoke<RunHandle>("start_benchmark_run", { options, onEvent }),

  // replay
  replayRun: (
    eventsPath: string,
    runId: RunIdDto,
    pacing: { kind: "instant" | "realtime" } | { kind: "fixed"; ms: number },
    onEvent: Channel<DesktopEvent>,
  ) =>
    tauriInvoke<number>("replay_run", {
      eventsPath,
      runId,
      pacing: pacing.kind === "fixed" ? { fixed: { ms: pacing.ms } } : pacing.kind,
      onEvent,
    }),

  // provider
  providerInfo: () => tauriInvoke<ProviderSummary>("provider_info"),
  setNimApiKey: (secret: string) =>
    tauriInvoke<void>("set_nim_api_key", { secret }),
  clearNimApiKey: () => tauriInvoke<void>("clear_nim_api_key"),
  validateNimProvider: () =>
    tauriInvoke<ProviderHealth>("validate_nim_provider"),

  // doctor
  appStatus: () => tauriInvoke<AppStatus>("app_status"),
  doctorReport: () => tauriInvoke<DoctorReport>("doctor_report"),
  wireVersion: () => tauriInvoke<number>("wire_version"),

  // expansive (PR10 + PR12)
  checkForUpdates: () =>
    tauriInvoke<{
      current_version: string;
      latest_known: string;
      update_available: boolean;
      channel: string;
    }>("check_for_updates"),
  applyUpdate: () => tauriInvoke<void>("apply_update"),
  newWindow: (workspaceId: WorkspaceId | null) =>
    tauriInvoke<{ label: string; workspace_id: WorkspaceId | null }>(
      "new_window",
      { workspaceId },
    ),
  queryMemory: (
    workspaceId: WorkspaceId,
    tier: string,
    query: string,
    limit: number,
  ) =>
    tauriInvoke<{
      tier: string;
      query: string;
      total: number;
      items: Array<{
        id: string;
        tier: string;
        summary: string;
        score: number;
        recorded_at: string;
      }>;
    }>("query_memory", { workspaceId, tier, query, limit }),
  pruneMemory: (tier: string, olderThanIso: string) =>
    tauriInvoke<{ tier: string; removed: number }>("prune_memory", {
      tier,
      olderThanIso,
    }),
  listRules: (workspaceId: WorkspaceId | null) =>
    tauriInvoke<
      Array<{
        id: string;
        display_name: string;
        source_path: string;
        lifecycle: string;
        evidence_count: number;
      }>
    >("list_rules", { workspaceId }),
  updateRuleLifecycle: (
    workspaceId: WorkspaceId,
    ruleId: string,
    lifecycle: string,
  ) =>
    tauriInvoke<string>("update_rule_lifecycle", {
      workspaceId,
      ruleId,
      lifecycle,
    }),
  listAgentsInRun: (runId: RunIdDto) =>
    tauriInvoke<string[]>("list_agents_in_run", { runId }),
};

/**
 * Convenience constructor for a typed Tauri channel that the run /
 * benchmark / replay commands stream events through. The channel is
 * one-shot from the frontend's perspective — pass it once into a
 * starting command and listen for events.
 */
export function makeEventChannel(
  onMessage: (event: DesktopEvent) => void,
): Channel<DesktopEvent> {
  const channel = new Channel<DesktopEvent>();
  channel.onmessage = onMessage;
  return channel;
}
