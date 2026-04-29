import { useEffect, useState } from "react";
import { CheckCircle, AlertTriangle, XCircle, MinusCircle, RefreshCw } from "lucide-react";

import { ipc } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import type { DoctorCheck, DoctorReport, DoctorStatus } from "@/types/ipc";

/**
 * Doctor surface. Runs every probe in `quorp_desktop_core::doctor`
 * and renders one row per check with status + remediation. Used both
 * as the left-rail "Doctor" surface and via Tools → Open Doctor.
 */
export function DoctorPanel() {
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.doctorReport();
      setReport(r);
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between border-b border-border-subtle px-4 py-3">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-fg-primary">Doctor</h2>
          {report && <OverallBadge status={report.overall} />}
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={() => refresh().catch(() => {})}
          aria-label="Refresh"
          title="Refresh"
          className="flex h-7 w-7 items-center justify-center rounded-sm border border-border-subtle hover:border-ring-focus disabled:opacity-50"
        >
          <RefreshCw size={14} className={busy ? "animate-spin" : undefined} />
        </button>
      </header>
      <div className="flex-1 overflow-y-auto px-4 py-3">
        {error && (
          <p className="mb-3 rounded-md border border-status-danger/40 bg-bg-base px-3 py-2 text-xs text-status-danger">
            {error}
          </p>
        )}
        {!report && !error && (
          <p className="text-xs text-fg-muted">Running probes…</p>
        )}
        {report && (
          <ul className="flex flex-col gap-2">
            {report.checks.map((check) => (
              <li key={check.id}>
                <CheckRow check={check} />
              </li>
            ))}
          </ul>
        )}
        {report && (
          <p className="mt-4 text-[10px] text-fg-muted">
            Generated at {new Date(report.generated_at).toLocaleString()}
          </p>
        )}
      </div>
    </div>
  );
}

function CheckRow({ check }: { check: DoctorCheck }) {
  return (
    <article
      className={cn(
        "flex items-start gap-3 rounded-md border bg-bg-base p-3 text-sm",
        borderForStatus(check.status),
      )}
    >
      <StatusIcon status={check.status} />
      <div className="flex-1 min-w-0">
        <header className="flex items-baseline justify-between gap-2">
          <h3 className="font-semibold text-fg-primary">{check.label}</h3>
          <span className="font-mono text-[10px] uppercase tracking-wider text-fg-muted">
            {check.id}
          </span>
        </header>
        <p className="mt-0.5 text-xs text-fg-secondary break-words">
          {check.detail}
        </p>
        {check.remediation && check.status !== "ok" && (
          <p className="mt-1 text-[11px] text-fg-muted">
            <span className="font-semibold text-fg-secondary">Fix: </span>
            {check.remediation}
          </p>
        )}
      </div>
    </article>
  );
}

function StatusIcon({ status }: { status: DoctorStatus }) {
  const cls = "shrink-0";
  switch (status) {
    case "ok":
      return <CheckCircle size={18} className={`${cls} text-status-success`} />;
    case "warn":
      return <AlertTriangle size={18} className={`${cls} text-status-warning`} />;
    case "fail":
      return <XCircle size={18} className={`${cls} text-status-danger`} />;
    case "skipped":
      return <MinusCircle size={18} className={`${cls} text-fg-muted`} />;
  }
}

function OverallBadge({ status }: { status: DoctorStatus }) {
  const label = status.toUpperCase();
  const cls =
    status === "ok"
      ? "border-status-success/40 text-status-success"
      : status === "warn"
        ? "border-status-warning/40 text-status-warning"
        : status === "fail"
          ? "border-status-danger/40 text-status-danger"
          : "border-border-subtle text-fg-muted";
  return (
    <span
      className={`rounded-sm border px-1.5 py-0.5 font-mono text-[10px] ${cls}`}
    >
      {label}
    </span>
  );
}

function borderForStatus(status: DoctorStatus): string {
  switch (status) {
    case "ok":
      return "border-status-success/30";
    case "warn":
      return "border-status-warning/40";
    case "fail":
      return "border-status-danger/50";
    case "skipped":
      return "border-border-subtle";
  }
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
