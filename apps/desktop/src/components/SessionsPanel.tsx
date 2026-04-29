// Multi-session sidebar (PR10).
//
// Lists every run the current desktop instance has seen — active and
// finished — and lets the user click to switch the active timeline.
// The runStore already keys events by `(run_id, seq)`, so switching
// is just `setActive`; nothing else needs to change.

import { Activity, CheckCircle, Clock, XCircle } from "lucide-react";

import { ipc } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import { useRunStore } from "@/store/runStore";

export function SessionsPanel({ className }: { className?: string }) {
  const runs = useRunStore((s) => s.runs);
  const activeRunId = useRunStore((s) => s.activeRunId);
  const setActive = useRunStore((s) => s.setActive);

  const sorted = Object.values(runs).sort((a, b) => {
    // Active first, then most-recent started first.
    if (!a.finishedAt && b.finishedAt) return -1;
    if (a.finishedAt && !b.finishedAt) return 1;
    return b.startedAt.localeCompare(a.startedAt);
  });

  return (
    <div className={cn("flex h-full flex-col", className)}>
      <header className="border-b border-border-subtle px-3 py-2">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-fg-muted">
          Sessions
        </h2>
      </header>
      <ul className="flex-1 overflow-y-auto px-2 py-2">
        {sorted.length === 0 && (
          <li className="rounded-md border border-dashed border-border-subtle px-3 py-6 text-center text-xs text-fg-muted">
            No runs yet. Start one from the composer or the Benchmarks panel.
          </li>
        )}
        {sorted.map((run) => (
          <li key={run.runId}>
            <button
              type="button"
              onClick={() => setActive(run.runId)}
              className={cn(
                "flex w-full flex-col gap-0.5 rounded-md border px-2 py-1.5 text-left",
                activeRunId === run.runId
                  ? "border-ring-focus bg-bg-elevated"
                  : "border-transparent hover:border-border-subtle hover:bg-bg-elevated",
              )}
            >
              <div className="flex items-center gap-2">
                <SessionIcon
                  finished={!!run.finishedAt}
                  stopReason={run.stopReason}
                />
                <span className="truncate text-xs text-fg-primary">
                  {run.goal || "(no goal)"}
                </span>
              </div>
              <div className="flex items-center justify-between text-[10px] text-fg-muted">
                <span className="font-mono">{shortRunId(run.runId)}</span>
                <span>{formatStarted(run.startedAt)}</span>
              </div>
              {activeRunId === run.runId && !run.finishedAt && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation();
                    ipc.cancelRun(run.runId).catch(() => {});
                  }}
                  className="mt-1 self-start rounded-sm border border-status-danger/40 px-1.5 py-0.5 text-[10px] text-status-danger hover:border-status-danger"
                >
                  Cancel
                </button>
              )}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function SessionIcon({
  finished,
  stopReason,
}: {
  finished: boolean;
  stopReason: string | null;
}) {
  const cls = "shrink-0";
  if (!finished) {
    return <Activity size={12} className={`${cls} text-status-info`} />;
  }
  if (stopReason === "completed") {
    return <CheckCircle size={12} className={`${cls} text-status-success`} />;
  }
  if (stopReason === "cancelled") {
    return <XCircle size={12} className={`${cls} text-fg-muted`} />;
  }
  if (stopReason === "fatal_error" || stopReason === "tool_failure") {
    return <XCircle size={12} className={`${cls} text-status-danger`} />;
  }
  return <Clock size={12} className={`${cls} text-fg-muted`} />;
}

function shortRunId(runId: string): string {
  if (runId.length <= 18) return runId;
  return `${runId.slice(0, 8)}…${runId.slice(-6)}`;
}

function formatStarted(iso: string): string {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString();
  } catch {
    return iso;
  }
}
