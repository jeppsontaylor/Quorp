import { useState } from "react";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import type {
  CapabilityTokenDto,
  PermissionDecisionKind,
  PermissionRequestDto,
  PermissionScope,
  RiskLevel,
} from "@/types/ipc";

const SCOPE_OPTIONS: { value: PermissionScope; label: string; key: string }[] =
  [
    { value: "once", label: "Once", key: "O" },
    { value: "session", label: "Session", key: "S" },
    { value: "project", label: "Project", key: "P" },
  ];

export function PermissionsPane() {
  const pending = useRunStore((s) => s.pendingPermissions);
  const resolveLocally = useRunStore((s) => s.resolvePermission);
  const [busyId, setBusyId] = useState<string | null>(null);

  const respond = async (
    request: PermissionRequestDto,
    decision: PermissionDecisionKind,
    scope: PermissionScope,
  ) => {
    setBusyId(request.request_id);
    try {
      await ipc.respondToPermission(request.request_id, { decision, scope });
      resolveLocally(request.request_id);
    } catch (err) {
      // The broker may have timed out (`Stale`); drop locally either
      // way so the user can move on.
      resolveLocally(request.request_id);
      console.error("respond_to_permission failed", err);
    } finally {
      setBusyId(null);
    }
  };

  if (pending.length === 0) {
    return (
      <p className="rounded-md border border-dashed border-border-subtle px-3 py-4 text-center text-xs text-fg-muted">
        No pending permission requests.
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-3">
      {pending.map((req) => (
        <li
          key={req.request_id}
          className="rounded-md border border-status-warning/40 bg-bg-base p-3"
        >
          <header className="flex items-center justify-between text-xs">
            <span className="uppercase tracking-wider text-status-warning">
              {req.tool} · {riskLabel(req.risk)}
            </span>
            <span className="font-mono text-[10px] text-fg-muted">
              {new Date(req.requested_at).toLocaleTimeString()}
            </span>
          </header>
          <p className="mt-2 font-mono text-[12px] text-fg-primary break-all">
            {req.action_summary}
          </p>
          {req.cwd && (
            <p className="mt-1 font-mono text-[10px] text-fg-muted">
              cwd: {req.cwd}
            </p>
          )}
          {req.tokens.length > 0 && (
            <ul
              aria-label="Capability tokens"
              className="mt-2 flex flex-wrap gap-1 text-[10px]"
            >
              {req.tokens.map((tok, i) => (
                <li
                  key={i}
                  className="rounded-sm border border-border-subtle px-1.5 py-0.5 font-mono text-fg-secondary"
                >
                  {capabilityLabel(tok)}
                </li>
              ))}
            </ul>
          )}
          {req.reason && (
            <p className="mt-2 text-xs text-fg-secondary">{req.reason}</p>
          )}
          <div className="mt-3 flex gap-2 text-xs">
            {SCOPE_OPTIONS.map((s) => (
              <button
                key={`allow-${s.value}`}
                type="button"
                disabled={busyId === req.request_id}
                onClick={() => respond(req, "allow", s.value)}
                title={`Allow [${s.key}]`}
                className="flex-1 rounded-sm border border-status-success/40 px-2 py-1 text-status-success hover:border-status-success disabled:opacity-50"
              >
                Allow {s.label}
              </button>
            ))}
            <button
              type="button"
              disabled={busyId === req.request_id}
              onClick={() => respond(req, "deny", "once")}
              className="rounded-sm border border-status-danger/40 px-2 py-1 text-status-danger hover:border-status-danger disabled:opacity-50"
            >
              Deny
            </button>
          </div>
        </li>
      ))}
    </ul>
  );
}

function riskLabel(risk: RiskLevel): string {
  switch (risk) {
    case "low":
      return "low!";
    case "medium":
      return "med!!";
    case "high":
      return "high!!!";
    case "critical":
      return "crit!!!!";
  }
}

function capabilityLabel(token: CapabilityTokenDto): string {
  if (token.kind === "other") return token.label;
  return token.kind.replaceAll("_", " ");
}
