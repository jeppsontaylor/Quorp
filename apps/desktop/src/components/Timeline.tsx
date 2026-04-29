import { useState } from "react";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import { useViewStore } from "@/store/viewStore";
import type { NormalizedEvent } from "@/store/runStore";

export function Timeline() {
  const activeRunId = useRunStore((s) => s.activeRunId);
  const events = useRunStore((s) =>
    activeRunId ? (s.timeline[activeRunId] ?? []) : [],
  );

  return (
    <section
      role="log"
      aria-live="polite"
      aria-relevant="additions"
      className="flex-1 overflow-y-auto bg-bg-base px-4 py-3"
    >
      {!activeRunId ? (
        <EmptyState />
      ) : events.length === 0 ? (
        <p className="text-center text-xs text-fg-muted">
          Waiting for events…
        </p>
      ) : (
        <ul className="flex flex-col gap-2">
          {events.map((evt) => (
            <li key={`${evt.runId}:${evt.seq}`}>
              <Card event={evt} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function EmptyState() {
  return (
    <div className="mx-auto mt-12 max-w-md rounded-lg border border-dashed border-border-subtle p-6 text-center text-sm text-fg-muted">
      <p className="text-fg-secondary">No active run.</p>
      <p className="mt-2 text-xs">
        Add a workspace from the left panel, then start a run from the
        composer below to see semantic events stream here.
      </p>
    </div>
  );
}

function Card({ event }: { event: NormalizedEvent }) {
  const e = event.event;
  const seqLabel = `#${event.seq}`;
  const cls =
    "rounded-md border border-border-subtle bg-bg-surface px-3 py-2 text-sm";
  switch (e.event) {
    case "run_started":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            run started · {seqLabel}
          </header>
          <p className="mt-1 text-fg-primary">{e.goal}</p>
          <p className="mt-0.5 font-mono text-[11px] text-fg-muted">
            {e.model_id}
          </p>
        </article>
      );
    case "assistant_turn_summary":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            assistant turn · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 whitespace-pre-wrap text-fg-primary">
            {e.assistant_message}
          </p>
          {e.actions.length > 0 && (
            <p className="mt-1 font-mono text-[11px] text-fg-muted">
              actions: {e.actions.join(", ")}
            </p>
          )}
        </article>
      );
    case "tool_call_started":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            ▸ tool · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 font-mono text-[12px] text-fg-primary">{e.action}</p>
        </article>
      );
    case "tool_call_finished":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            ✓ tool finished · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 font-mono text-[12px] text-fg-primary">
            {e.action} → {e.status}
          </p>
          {e.edit_summary && (
            <p className="mt-1 text-xs text-fg-secondary">{e.edit_summary}</p>
          )}
        </article>
      );
    case "validation_started":
    case "validation_finished":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            validation · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 text-fg-primary">{e.summary}</p>
        </article>
      );
    case "model_request_started":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            model request · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 font-mono text-[11px] text-fg-muted">
            request_id={e.request_id} · msgs={e.message_count} · est_tokens=
            {e.prompt_token_estimate}
          </p>
        </article>
      );
    case "model_request_finished":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            ✓ model finished · step {e.step} · {seqLabel}
          </header>
          {e.usage && (
            <p className="mt-1 font-mono text-[11px] text-fg-muted">
              prompt={e.usage.prompt_tokens} completion=
              {e.usage.completion_tokens} total={e.usage.total_tokens}
            </p>
          )}
        </article>
      );
    case "phase_changed":
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            phase · {seqLabel}
          </header>
          <p className="mt-1 text-fg-primary">{e.phase}</p>
          {e.detail && (
            <p className="mt-0.5 text-xs text-fg-muted">{e.detail}</p>
          )}
        </article>
      );
    case "checkpoint_saved":
      return <CheckpointCard step={e.step} requestCounter={e.request_counter} seqLabel={seqLabel} runId={event.runId} />;
    case "context_compaction" as never:
      // Reserved for the dedicated context-pack card in PR8.
      return null;
    case "policy_denied":
      return (
        <article
          className={`${cls} border-status-danger/60`}
        >
          <header className="text-xs uppercase tracking-wider text-status-danger">
            policy denied · step {e.step} · {seqLabel}
          </header>
          <p className="mt-1 font-mono text-[12px] text-fg-primary">{e.action}</p>
          <p className="mt-1 text-xs text-fg-secondary">{e.reason}</p>
        </article>
      );
    case "subscriber_backpressure":
      return (
        <article className={`${cls} border-status-warning/60`}>
          <header className="text-xs uppercase tracking-wider text-status-warning">
            backpressure · {seqLabel}
          </header>
          <p className="mt-1 font-mono text-[11px] text-fg-secondary">
            {e.subscriber}: dropped {e.dropped_events} of {e.capacity}
          </p>
        </article>
      );
    case "fatal_error":
      return (
        <article
          className={`${cls} border-status-danger/60 bg-bg-surface`}
        >
          <header className="text-xs uppercase tracking-wider text-status-danger">
            fatal · {seqLabel}
          </header>
          <p className="mt-1 text-fg-primary">{e.error}</p>
        </article>
      );
    default:
      return (
        <article className={cls}>
          <header className="text-xs uppercase tracking-wider text-fg-muted">
            {e.event} · {seqLabel}
          </header>
        </article>
      );
  }
}

function CheckpointCard({
  step,
  requestCounter,
  seqLabel,
  runId,
}: {
  step: number;
  requestCounter: number;
  seqLabel: string;
  runId: string;
}) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const setInspectorTab = useViewStore((s) => s.setInspectorTab);

  const onRollback = async () => {
    setBusy(true);
    setResult(null);
    try {
      const r = await ipc.rollbackToCheckpoint(runId, requestCounter);
      const backup = r.backup_filename
        ? ` · backup ${r.backup_filename}`
        : "";
      setResult(
        `Rolled back to #${r.request_counter} · ${r.restored_files} file(s)${backup}`,
      );
    } catch (err) {
      setResult(stringifyError(err));
    } finally {
      setBusy(false);
      setInspectorTab("status");
    }
  };

  return (
    <article className="rounded-md border border-status-info/40 bg-bg-surface px-3 py-2 text-sm">
      <header className="text-xs uppercase tracking-wider text-status-info">
        checkpoint · step {step} · {seqLabel}
      </header>
      <p className="mt-1 font-mono text-[11px] text-fg-secondary">
        request_counter={requestCounter}
      </p>
      <button
        type="button"
        disabled={busy}
        onClick={onRollback}
        className="mt-2 rounded-sm border border-border-subtle px-2 py-0.5 text-[11px] hover:border-ring-focus disabled:opacity-50"
      >
        Rollback to this checkpoint
      </button>
      {result && (
        <p className="mt-1 text-[11px] text-fg-muted">{result}</p>
      )}
    </article>
  );
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
