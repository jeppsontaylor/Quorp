import { useEffect, useState } from "react";

import { ipc } from "@/lib/invoke";
import type { AppStatus, ProviderSummary } from "@/types/ipc";
import { useWorkspaceStore } from "@/store/workspaceStore";

export function MissionBar() {
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const active = workspaces.find((w) => w.id === activeWorkspaceId) ?? null;

  const [status, setStatus] = useState<AppStatus | null>(null);
  const [provider, setProvider] = useState<ProviderSummary | null>(null);

  useEffect(() => {
    let alive = true;
    Promise.all([ipc.appStatus(), ipc.providerInfo()])
      .then(([s, p]) => {
        if (alive) {
          setStatus(s);
          setProvider(p);
        }
      })
      .catch(() => {
        /* ignore; mission bar tolerates missing data */
      });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="flex h-mission-bar items-center gap-3 border-b border-border-subtle bg-bg-surface px-4 text-xs text-fg-secondary">
      <div className="flex items-center gap-2 font-mono">
        <span className="rounded-sm bg-bg-elevated px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-fg-muted">
          Quorp
        </span>
        <span className="text-fg-primary">
          {active?.display_name ?? "No workspace"}
        </span>
        {active && (
          <span className="text-fg-muted">{active.canonical_path}</span>
        )}
      </div>
      <div className="ml-auto flex items-center gap-3">
        <Chip
          label={provider ? provider.display_name : "Provider"}
          tone={provider?.has_key ? "ok" : "warn"}
          tooltip={
            provider
              ? provider.has_key
                ? `Key set · ${provider.default_model}`
                : "No API key configured"
              : ""
          }
        />
        <Chip
          label={`Sandbox-exec ${status?.sandbox_exec_present ? "✓" : "✗"}`}
          tone={status?.sandbox_exec_present ? "ok" : "warn"}
          tooltip="macOS Apple sandbox availability"
        />
        <Chip
          label={`Workspaces ${status?.workspace_count ?? 0}`}
          tone="info"
        />
        <Chip
          label={`Wire v${status?.desktop_wire_version ?? "?"}`}
          tone="info"
          tooltip="IPC wire version"
        />
      </div>
    </div>
  );
}

interface ChipProps {
  label: string;
  tone: "ok" | "warn" | "info" | "danger";
  tooltip?: string;
}

function Chip({ label, tone, tooltip }: ChipProps) {
  const toneClass =
    tone === "ok"
      ? "border-status-success/40 text-status-success"
      : tone === "warn"
        ? "border-status-warning/40 text-status-warning"
        : tone === "danger"
          ? "border-status-danger/40 text-status-danger"
          : "border-border-subtle text-fg-secondary";
  return (
    <span
      title={tooltip ?? label}
      className={`rounded-sm border px-2 py-0.5 font-mono text-[11px] ${toneClass}`}
    >
      {label}
    </span>
  );
}
