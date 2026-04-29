import { cn } from "@/lib/utils";
import { useRunStore } from "@/store/runStore";
import { useViewStore, type InspectorTab } from "@/store/viewStore";

import { DiffPane } from "@/components/inspector/DiffPane";
import { MemoryPane } from "@/components/inspector/MemoryPane";
import { PermissionsPane } from "@/components/inspector/PermissionsPane";
import { ProofPane } from "@/components/inspector/ProofPane";
import { RulesPane } from "@/components/inspector/RulesPane";
import { TerminalPane } from "@/components/inspector/TerminalPane";
import { useFeatureFlags } from "@/store/featureFlags";

interface Props {
  className?: string;
}

const TABS: { tab: InspectorTab; label: string; accelerator: string }[] = [
  { tab: "status", label: "Status", accelerator: "⌘1" },
  { tab: "tasks", label: "Tasks", accelerator: "⌘2" },
  { tab: "context", label: "Context", accelerator: "⌘3" },
  { tab: "diff", label: "Diff", accelerator: "⌘4" },
  { tab: "proof", label: "Proof", accelerator: "⌘5" },
  { tab: "permissions", label: "Permissions", accelerator: "⌘6" },
  { tab: "memory", label: "Memory", accelerator: "⌘7" },
  { tab: "rules", label: "Rules", accelerator: "⌘8" },
];

export function Inspector({ className }: Props) {
  const tab = useViewStore((s) => s.inspectorTab);
  const setTab = useViewStore((s) => s.setInspectorTab);
  const activeRunId = useRunStore((s) => s.activeRunId);
  const flags = useFeatureFlags((s) => s.flags);
  const summary = useRunStore((s) =>
    activeRunId ? (s.runs[activeRunId] ?? null) : null,
  );
  const pendingCount = useRunStore((s) => s.pendingPermissions.length);

  return (
    <aside
      role="complementary"
      aria-label="Inspector"
      className={cn(
        "flex h-full min-w-0 flex-col overflow-hidden border-l border-border-subtle bg-bg-surface",
        className,
      )}
    >
      <nav
        role="tablist"
        aria-label="Inspector tabs"
        className="flex shrink-0 overflow-x-auto border-b border-border-subtle"
      >
        {TABS.map(({ tab: t, label, accelerator }) => (
          <button
            key={t}
            type="button"
            role="tab"
            aria-selected={tab === t}
            title={`${label} (${accelerator})`}
            onClick={() => setTab(t)}
            className={cn(
              "px-3 py-2 text-xs font-medium text-fg-secondary",
              "border-b-2 border-transparent hover:text-fg-primary",
              tab === t && "border-ring-focus text-fg-primary",
            )}
          >
            {label}
            {t === "permissions" && pendingCount > 0 && (
              <span className="ml-1 rounded-sm bg-status-warning/20 px-1 text-[10px] text-status-warning">
                {pendingCount}
              </span>
            )}
          </button>
        ))}
      </nav>
      <div className="flex-1 overflow-y-auto px-3 py-3 text-sm">
        {tab === "status" && (
          <StatusPane
            runId={activeRunId}
            summaryRow={summary ? `${summary.goal}` : null}
            modelId={summary?.modelId ?? null}
            stopReason={summary?.stopReason ?? null}
            totalSteps={summary?.totalSteps ?? 0}
            totalTokens={summary?.totalBilledTokens ?? 0}
          />
        )}
        {tab === "diff" && <DiffPane />}
        {tab === "proof" && <ProofPane />}
        {tab === "permissions" && <PermissionsPane />}
        {tab === "tasks" && (
          <Placeholder>Task DAG lands in PR9 (agents sidebar).</Placeholder>
        )}
        {tab === "context" && (
          <Placeholder>
            Context packs + compaction history lands in PR8.
          </Placeholder>
        )}
        {tab === "memory" && (
          flags.memory_editor ? (
            <MemoryPane />
          ) : (
            <Placeholder>
              Memory query surface is gated behind Settings → UI →
              Expansive features.
            </Placeholder>
          )
        )}
        {tab === "rules" && (
          flags.rules_editor ? (
            <RulesPane />
          ) : (
            <Placeholder>
              Rules surface is gated behind Settings → UI → Expansive
              features.
            </Placeholder>
          )
        )}
      </div>
    </aside>
  );
}

interface StatusProps {
  runId: string | null;
  summaryRow: string | null;
  modelId: string | null;
  stopReason: string | null;
  totalSteps: number;
  totalTokens: number;
}

function StatusPane({
  runId,
  summaryRow,
  modelId,
  stopReason,
  totalSteps,
  totalTokens,
}: StatusProps) {
  if (!runId) {
    return (
      <p className="text-center text-xs text-fg-muted">
        Start a run to see status here.
      </p>
    );
  }
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12px]">
      <Field label="Run">{runId}</Field>
      <Field label="Goal">{summaryRow ?? "—"}</Field>
      <Field label="Model">{modelId ?? "—"}</Field>
      <Field label="Steps">{totalSteps}</Field>
      <Field label="Tokens">{totalTokens}</Field>
      <Field label="Stop reason">{stopReason ?? "(running)"}</Field>
    </dl>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <>
      <dt className="font-medium uppercase tracking-wider text-fg-muted">
        {label}
      </dt>
      <dd className="font-mono text-fg-primary">{children}</dd>
    </>
  );
}

function Placeholder({ children }: { children: React.ReactNode }) {
  return (
    <p className="rounded-md border border-dashed border-border-subtle px-3 py-4 text-center text-xs text-fg-muted">
      {children}
    </p>
  );
}
