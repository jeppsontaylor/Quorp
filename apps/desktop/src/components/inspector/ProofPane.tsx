import { useEffect, useState } from "react";

import { ipc, type ArtifactKind } from "@/lib/invoke";
import { useArtifactStore } from "@/store/artifactStore";
import { useRunStore } from "@/store/runStore";
import { useWorkspaceStore } from "@/store/workspaceStore";
import { cn } from "@/lib/utils";

const LADDER: { id: string; label: string; description: string }[] = [
  { id: "L0", label: "L0", description: "Smoke check (compile / lint)" },
  { id: "L1", label: "L1", description: "Unit tests" },
  { id: "L2", label: "L2", description: "Integration tests" },
  { id: "L3", label: "L3", description: "Full suite" },
  { id: "L4", label: "L4", description: "Stress / fuzz" },
];

export function ProofPane() {
  const activeRunId = useRunStore((s) => s.activeRunId);
  const workspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const fetchKinds = useArtifactStore((s) => s.fetchKinds);
  const kinds = useArtifactStore((s) =>
    activeRunId && workspaceId
      ? (s.kinds[`${workspaceId}:${activeRunId}`] ?? [])
      : [],
  );
  const [verifyState, setVerifyState] = useState<string | null>(null);
  const [verifyBusy, setVerifyBusy] = useState(false);

  useEffect(() => {
    if (!activeRunId || !workspaceId) return;
    fetchKinds(workspaceId, activeRunId).catch(() => {});
  }, [activeRunId, fetchKinds, workspaceId]);

  if (!activeRunId) {
    return <Empty>Select a run to see proof artifacts.</Empty>;
  }

  const has = (kind: ArtifactKind) => kinds.includes(kind);

  const onVerifyAgain = async () => {
    if (!activeRunId) return;
    setVerifyBusy(true);
    setVerifyState(null);
    try {
      const r = await ipc.verifyRunAgain(activeRunId);
      setVerifyState(`Re-verification queued · ${r.verify_run_id}`);
    } catch (err) {
      setVerifyState(stringifyError(err));
    } finally {
      setVerifyBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <section aria-label="Verification ladder">
        <h3 className="mb-2 text-xs uppercase tracking-wider text-fg-muted">
          Verification ladder
        </h3>
        <ul className="flex flex-col gap-1">
          {LADDER.map((stage) => (
            <li
              key={stage.id}
              className="flex items-center gap-2 rounded-md border border-border-subtle bg-bg-base px-2 py-1.5"
            >
              <span className="font-mono text-[11px] text-fg-secondary">
                {stage.label}
              </span>
              <span className="text-xs text-fg-primary">{stage.description}</span>
              <span className="ml-auto font-mono text-[10px] text-fg-muted">
                {/* PR8 fills this from a real proof packet. */}
                pending
              </span>
            </li>
          ))}
        </ul>
      </section>
      <section aria-label="Run artifacts">
        <h3 className="mb-2 text-xs uppercase tracking-wider text-fg-muted">
          Artifacts
        </h3>
        <ul className="flex flex-col gap-1 text-xs">
          {has("proof_receipt") ? (
            <ArtifactRow label="proof-receipt.json" present />
          ) : (
            <ArtifactRow label="proof-receipt.json" present={false} />
          )}
          <ArtifactRow label="summary.json" present={has("summary")} />
          <ArtifactRow label="transcript.json" present={has("transcript")} />
          <ArtifactRow label="checkpoint.json" present={has("checkpoint")} />
          <ArtifactRow label="final.diff" present={has("final_diff")} />
          <ArtifactRow label="metadata.json" present={has("metadata")} />
        </ul>
      </section>
      <section>
        <button
          type="button"
          disabled={verifyBusy}
          onClick={onVerifyAgain}
          className="rounded-sm border border-border-subtle px-2 py-1 text-xs hover:border-ring-focus disabled:opacity-50"
        >
          Verify again
        </button>
        {verifyState && (
          <p className="mt-1 text-[11px] text-fg-muted">{verifyState}</p>
        )}
      </section>
    </div>
  );
}

function ArtifactRow({ label, present }: { label: string; present: boolean }) {
  return (
    <li
      className={cn(
        "flex items-center justify-between rounded-sm border border-border-subtle px-2 py-1 font-mono text-[11px]",
        present ? "text-fg-primary" : "text-fg-muted",
      )}
    >
      <span>{label}</span>
      <span>{present ? "✓ on disk" : "—"}</span>
    </li>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return (
    <p className="p-4 text-center text-xs text-fg-muted">{children}</p>
  );
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
