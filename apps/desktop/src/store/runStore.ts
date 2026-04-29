// Per-run timeline reducer. Live runs and replays both feed events
// into the same store; the store keys events by `(run_id, seq)` so
// out-of-order batches still sort correctly.

import { create } from "zustand";

import type {
  DesktopEvent,
  DiffSummary,
  PermissionRequestDto,
  RunIdDto,
  RuntimeEventDto,
  StopReasonDto,
  ValidationStatusDto,
} from "@/types/ipc";

const TIMELINE_HARD_CAP = 5_000;

export interface NormalizedEvent {
  runId: RunIdDto;
  seq: number;
  event: RuntimeEventDto;
  receivedAt: number;
}

export interface RunSummary {
  runId: RunIdDto;
  goal: string;
  modelId: string;
  startedAt: string;
  finishedAt: string | null;
  stopReason: StopReasonDto | null;
  totalSteps: number;
  totalBilledTokens: number;
  durationMs: number;
}

export interface DiffEntry {
  runId: RunIdDto;
  diffId: string;
  summary: DiffSummary;
  receivedAt: number;
}

export interface ValidationEntry {
  runId: RunIdDto;
  step: number;
  status: ValidationStatusDto;
  summary: string;
  proofReceiptId: string | null;
  receivedAt: number;
}

interface RunState {
  runs: Record<RunIdDto, RunSummary>;
  // Timeline kept per-run; we display only the active run's slice but
  // keep finished runs around so the UI can revisit them.
  timeline: Record<RunIdDto, NormalizedEvent[]>;
  pendingPermissions: PermissionRequestDto[];
  diffs: Record<RunIdDto, DiffEntry[]>;
  validations: Record<RunIdDto, ValidationEntry[]>;
  activeRunId: RunIdDto | null;
  setActive: (id: RunIdDto | null) => void;
  applyEvent: (event: DesktopEvent) => void;
  resolvePermission: (requestId: string) => void;
  reset: () => void;
}

export const useRunStore = create<RunState>((set) => ({
  runs: {},
  timeline: {},
  pendingPermissions: [],
  diffs: {},
  validations: {},
  activeRunId: null,
  setActive: (id) => set({ activeRunId: id }),
  applyEvent: (event) =>
    set((state) => applyDesktopEvent(state, event)),
  resolvePermission: (requestId) =>
    set((state) => ({
      pendingPermissions: state.pendingPermissions.filter(
        (p) => p.request_id !== requestId,
      ),
    })),
  reset: () =>
    set({
      runs: {},
      timeline: {},
      pendingPermissions: [],
      diffs: {},
      validations: {},
      activeRunId: null,
    }),
}));

function applyDesktopEvent(
  state: RunState,
  event: DesktopEvent,
): Partial<RunState> {
  switch (event.kind) {
    case "run_started": {
      const summary: RunSummary = {
        runId: event.run_id,
        goal: event.goal,
        modelId: event.model_id,
        startedAt: event.started_at,
        finishedAt: null,
        stopReason: null,
        totalSteps: 0,
        totalBilledTokens: 0,
        durationMs: 0,
      };
      return {
        runs: { ...state.runs, [event.run_id]: summary },
        timeline: { ...state.timeline, [event.run_id]: [] },
        activeRunId: state.activeRunId ?? event.run_id,
      };
    }
    case "run_finished": {
      const existing = state.runs[event.run_id];
      const summary: RunSummary = {
        ...(existing ?? {
          runId: event.run_id,
          goal: "",
          modelId: "",
          startedAt: "",
          finishedAt: null,
          stopReason: null,
          totalSteps: 0,
          totalBilledTokens: 0,
          durationMs: 0,
        }),
        finishedAt: new Date().toISOString(),
        stopReason: event.stop_reason,
        totalSteps: event.total_steps,
        totalBilledTokens: event.total_billed_tokens,
        durationMs: event.duration_ms,
      };
      return { runs: { ...state.runs, [event.run_id]: summary } };
    }
    case "run_failed": {
      const existing = state.runs[event.run_id];
      if (!existing) return {};
      return {
        runs: {
          ...state.runs,
          [event.run_id]: {
            ...existing,
            finishedAt: new Date().toISOString(),
            stopReason: "fatal_error",
          },
        },
      };
    }
    case "runtime": {
      const existing = state.timeline[event.run_id] ?? [];
      const incoming: NormalizedEvent[] = event.batch.map((rt) => ({
        runId: event.run_id,
        seq: rt.seq,
        event: rt,
        receivedAt: Date.now(),
      }));
      // Append + sort + cap. For dense seq numbers this is O(n log n)
      // but n is bounded by the cap so it stays cheap in practice.
      const merged = [...existing, ...incoming]
        .sort((a, b) => a.seq - b.seq)
        .slice(-TIMELINE_HARD_CAP);
      return {
        timeline: { ...state.timeline, [event.run_id]: merged },
      };
    }
    case "permission": {
      // De-dupe by request_id so a re-emit doesn't queue twice.
      const next = state.pendingPermissions.filter(
        (p) => p.request_id !== event.request.request_id,
      );
      next.push(event.request);
      return { pendingPermissions: next };
    }
    case "diff": {
      const entry: DiffEntry = {
        runId: event.run_id,
        diffId: event.diff_id,
        summary: event.summary,
        receivedAt: Date.now(),
      };
      const existing = state.diffs[event.run_id] ?? [];
      // De-dupe by diff_id; later events for the same diff replace.
      const next = existing
        .filter((d) => d.diffId !== event.diff_id)
        .concat(entry);
      return { diffs: { ...state.diffs, [event.run_id]: next } };
    }
    case "validation": {
      const entry: ValidationEntry = {
        runId: event.run_id,
        step: event.step,
        status: event.status,
        summary: event.summary,
        proofReceiptId: event.proof_receipt_id ?? null,
        receivedAt: Date.now(),
      };
      const existing = state.validations[event.run_id] ?? [];
      return {
        validations: {
          ...state.validations,
          [event.run_id]: [...existing, entry],
        },
      };
    }
    case "checkpoint":
    case "context_compaction":
    case "error":
      // These surface through dedicated inspector tabs (Status,
      // Context, Error log). The reducer doesn't need extra
      // bookkeeping for them yet — we read directly from the
      // `timeline` slice.
      return {};
  }
}
