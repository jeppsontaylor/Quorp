import { useEffect, useRef, useState } from "react";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import { cn } from "@/lib/utils";
import type {
  CapabilityTokenDto,
  PermissionDecisionKind,
  PermissionScope,
  RiskLevel,
} from "@/types/ipc";

/**
 * Modal that auto-opens whenever the run store has at least one
 * pending permission request. Renders the head of the queue; further
 * pending requests stay visible in the Permissions inspector tab.
 *
 * Implements `role="alertdialog"` + a focus trap on the primary
 * action (the Allow Once button) so the user can hit Enter to
 * approve. Escape closes the modal but does NOT resolve the
 * request — the user must click Allow or Deny explicitly. Clicking
 * the backdrop is also a no-op for the same reason.
 */
export function PermissionModal() {
  const pending = useRunStore((s) => s.pendingPermissions);
  const resolveLocally = useRunStore((s) => s.resolvePermission);
  const head = pending[0];

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const primaryRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (head) {
      primaryRef.current?.focus();
      setError(null);
    }
  }, [head?.request_id]);

  if (!head) return null;

  const respond = async (
    decision: PermissionDecisionKind,
    scope: PermissionScope,
  ) => {
    setBusy(true);
    setError(null);
    try {
      await ipc.respondToPermission(head.request_id, { decision, scope });
      resolveLocally(head.request_id);
    } catch (err) {
      // Broker may have timed out: drop locally so the queue moves on.
      resolveLocally(head.request_id);
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      // Esc dismisses the modal without resolving (the queue still
      // shows it in the inspector). The user must explicitly choose.
      return;
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-bg-overlay backdrop-blur-sm"
      onKeyDown={onKeyDown}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="perm-modal-title"
        aria-describedby="perm-modal-body"
        className={cn(
          "w-full max-w-lg rounded-lg border bg-bg-surface p-5 shadow-2xl",
          riskBorder(head.risk),
        )}
      >
        <header className="flex items-center justify-between">
          <h2
            id="perm-modal-title"
            className="text-sm font-semibold uppercase tracking-wider text-fg-primary"
          >
            Permission required
          </h2>
          <span
            className={cn(
              "rounded-sm border px-2 py-0.5 font-mono text-[10px]",
              riskBorder(head.risk),
              riskFg(head.risk),
            )}
          >
            {riskLabel(head.risk)}
          </span>
        </header>
        <div id="perm-modal-body" className="mt-3 flex flex-col gap-2">
          <p className="text-xs uppercase tracking-wider text-fg-muted">
            {head.tool} · run {shortenRun(head.run_id)}
          </p>
          <p className="break-all rounded-md bg-bg-base px-3 py-2 font-mono text-[12px] text-fg-primary">
            {head.action_summary}
          </p>
          {head.cwd && (
            <p className="font-mono text-[11px] text-fg-muted">cwd: {head.cwd}</p>
          )}
          {head.tokens.length > 0 && (
            <ul
              aria-label="Capability tokens"
              className="flex flex-wrap gap-1 text-[10px]"
            >
              {head.tokens.map((tok, i) => (
                <li
                  key={i}
                  className="rounded-sm border border-border-subtle px-1.5 py-0.5 font-mono text-fg-secondary"
                >
                  {capabilityLabel(tok)}
                </li>
              ))}
            </ul>
          )}
          {head.reason && (
            <p className="text-xs text-fg-secondary">{head.reason}</p>
          )}
        </div>
        <div className="mt-4 flex flex-col gap-2">
          <div className="flex gap-2">
            <button
              ref={primaryRef}
              type="button"
              disabled={busy}
              onClick={() => respond("allow", "once")}
              className="flex-1 rounded-md border border-status-success/60 bg-status-success/10 px-3 py-2 text-sm font-semibold text-status-success hover:border-status-success disabled:opacity-50"
            >
              Allow Once
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => respond("allow", "session")}
              className="flex-1 rounded-md border border-status-success/40 px-3 py-2 text-sm text-status-success hover:border-status-success disabled:opacity-50"
            >
              Allow Session
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => respond("allow", "project")}
              className="flex-1 rounded-md border border-status-success/40 px-3 py-2 text-sm text-status-success hover:border-status-success disabled:opacity-50"
            >
              Allow Project
            </button>
          </div>
          <button
            type="button"
            disabled={busy}
            onClick={() => respond("deny", "once")}
            className="rounded-md border border-status-danger/60 bg-status-danger/10 px-3 py-2 text-sm font-semibold text-status-danger hover:border-status-danger disabled:opacity-50"
          >
            Deny
          </button>
        </div>
        {pending.length > 1 && (
          <p className="mt-3 text-[11px] text-fg-muted">
            +{pending.length - 1} more pending in the inspector
          </p>
        )}
        {error && (
          <p className="mt-2 text-[11px] text-status-danger">{error}</p>
        )}
      </div>
    </div>
  );
}

function riskBorder(risk: RiskLevel): string {
  switch (risk) {
    case "low":
      return "border-status-success/40";
    case "medium":
      return "border-status-warning/40";
    case "high":
      return "border-status-attention/60";
    case "critical":
      return "border-status-danger/70";
  }
}

function riskFg(risk: RiskLevel): string {
  switch (risk) {
    case "low":
      return "text-status-success";
    case "medium":
      return "text-status-warning";
    case "high":
      return "text-status-attention";
    case "critical":
      return "text-status-danger";
  }
}

function riskLabel(risk: RiskLevel): string {
  return risk.toUpperCase();
}

function capabilityLabel(token: CapabilityTokenDto): string {
  if (token.kind === "other") return token.label;
  return token.kind.replaceAll("_", " ");
}

function shortenRun(runId: string): string {
  if (runId.length <= 18) return runId;
  return `${runId.slice(0, 8)}…${runId.slice(-6)}`;
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
