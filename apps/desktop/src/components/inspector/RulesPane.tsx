import { useEffect, useState } from "react";

import { ipc } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import { useWorkspaceStore } from "@/store/workspaceStore";

interface RuleSummary {
  id: string;
  display_name: string;
  source_path: string;
  lifecycle: string;
  evidence_count: number;
}

const LIFECYCLES = ["draft", "active", "suspended", "archived"] as const;
type Lifecycle = (typeof LIFECYCLES)[number];

export function RulesPane() {
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const [rules, setRules] = useState<RuleSummary[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    try {
      const list = await ipc.listRules(activeWorkspaceId);
      setRules(list);
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    refresh().catch(() => {});
    // refresh when active workspace changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeWorkspaceId]);

  const setLifecycle = async (id: string, lifecycle: Lifecycle) => {
    if (!activeWorkspaceId) {
      setError(
        "Select a workspace before changing lifecycle (the ledger lives under <workspace>/.quorp/rules/).",
      );
      return;
    }
    try {
      await ipc.updateRuleLifecycle(activeWorkspaceId, id, lifecycle);
      await refresh();
    } catch (err) {
      setError(stringifyError(err));
    }
  };

  return (
    <div className="flex flex-col gap-3 text-sm">
      <header>
        <h3 className="text-base font-semibold text-fg-primary">Rules</h3>
        <p className="mt-1 text-xs text-fg-muted">
          Repo-level (<code>.rules</code>), project-level (
          <code>.quorp/rules/*.rules</code>), and global (
          <code>~/.quorp/rules/*.rules</code>) files. Lifecycle
          transitions persist to <code>.quorp/rules/lifecycle.json</code>;{" "}
          <code>rule_forge</code> reads the same ledger when it lands.
        </p>
      </header>
      {error && (
        <p className="rounded-md border border-status-danger/40 bg-bg-base px-2 py-1 text-xs text-status-danger">
          {error}
        </p>
      )}
      {rules === null && !error && (
        <p className="text-xs text-fg-muted">Loading…</p>
      )}
      {rules && rules.length === 0 && (
        <p className="rounded-md border border-dashed border-border-subtle px-3 py-4 text-center text-xs text-fg-muted">
          No rule files surfaced yet. Add a <code>.rules</code> file to
          your workspace or set up a global one in
          <code> ~/.quorp/rules/</code>.
        </p>
      )}
      <ul className="flex flex-col gap-2">
        {rules?.map((rule) => (
          <li
            key={rule.id}
            className="rounded-md border border-border-subtle bg-bg-base p-3"
          >
            <header className="flex items-center justify-between gap-2">
              <h4 className="text-sm font-semibold text-fg-primary">
                {rule.display_name}
              </h4>
              <span
                className={cn(
                  "rounded-sm border px-1.5 py-0.5 font-mono text-[10px] uppercase",
                  lifecycleClass(rule.lifecycle),
                )}
              >
                {rule.lifecycle}
              </span>
            </header>
            <p className="mt-1 font-mono text-[10px] text-fg-muted truncate">
              {rule.source_path}
            </p>
            <p className="mt-1 text-[11px] text-fg-secondary">
              evidence: {rule.evidence_count}
            </p>
            <div className="mt-2 flex gap-1 text-[10px]">
              {LIFECYCLES.map((lc) => (
                <button
                  key={lc}
                  type="button"
                  disabled={busy || rule.lifecycle === lc || !activeWorkspaceId}
                  onClick={() => setLifecycle(rule.id, lc)}
                  className={cn(
                    "rounded-sm border px-1.5 py-0.5",
                    rule.lifecycle === lc
                      ? "border-ring-focus text-fg-primary"
                      : "border-border-subtle text-fg-secondary hover:border-ring-focus",
                    "disabled:opacity-40",
                  )}
                >
                  {lc}
                </button>
              ))}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

function lifecycleClass(state: string): string {
  switch (state.toLowerCase()) {
    case "active":
      return "border-status-success/40 text-status-success";
    case "draft":
      return "border-status-info/40 text-status-info";
    case "suspended":
      return "border-status-warning/40 text-status-warning";
    case "archived":
      return "border-border-subtle text-fg-muted";
    default:
      return "border-border-subtle text-fg-secondary";
  }
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
