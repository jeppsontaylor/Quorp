import { useEffect, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { FlaskConical, RefreshCw } from "lucide-react";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import { useViewStore } from "@/store/viewStore";
import { cn } from "@/lib/utils";
import type {
  BenchmarkFixture,
  BenchmarkOptions,
  DesktopEvent,
  PermissionModeDto,
  SandboxModeDto,
} from "@/types/ipc";

/**
 * Benchmarks library. Lists every `benchmark.json` fixture under
 * `benchmark/challenges/rust-swebench-top5/*` (the only fixture set
 * Quorp ships in v1) and offers a one-click "Run in sandbox" launcher.
 */
export function BenchmarksPanel() {
  const [fixtures, setFixtures] = useState<BenchmarkFixture[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<BenchmarkFixture | null>(null);

  const applyEvent = useRunStore((s) => s.applyEvent);
  const setActive = useRunStore((s) => s.setActive);
  const setSurface = useViewStore((s) => s.setSurface);

  const refresh = async () => {
    setError(null);
    try {
      const list = await ipc.listBenchmarkFixtures();
      setFixtures(list);
    } catch (err) {
      setError(stringifyError(err));
    }
  };

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  const start = async (fixture: BenchmarkFixture) => {
    setBusyId(fixture.fixture_id);
    setError(null);
    try {
      const channel = new Channel<DesktopEvent>();
      channel.onmessage = (event) => {
        applyEvent(event);
        if (event.kind === "run_started") setActive(event.run_id);
      };
      const opts: BenchmarkOptions = {
        fixture_id: fixture.fixture_id,
        permission_mode: "yolo_sandbox" as PermissionModeDto,
        sandbox_mode: defaultSandbox(),
        model_id: null,
        wall_clock_budget_seconds: null,
      };
      await ipc.startBenchmarkRun(opts, channel);
      setSurface("sessions");
      setConfirm(null);
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center justify-between border-b border-border-subtle px-4 py-3">
        <div className="flex items-center gap-2">
          <FlaskConical size={18} className="text-fg-secondary" />
          <h2 className="text-sm font-semibold text-fg-primary">Benchmarks</h2>
        </div>
        <button
          type="button"
          onClick={() => refresh().catch(() => {})}
          aria-label="Refresh"
          title="Refresh fixture list"
          className="flex h-7 w-7 items-center justify-center rounded-sm border border-border-subtle hover:border-ring-focus"
        >
          <RefreshCw size={14} />
        </button>
      </header>
      <div className="flex-1 overflow-y-auto px-4 py-3">
        {error && (
          <p className="mb-3 rounded-md border border-status-danger/40 bg-bg-base px-3 py-2 text-xs text-status-danger">
            {error}
          </p>
        )}
        {fixtures === null && <p className="text-xs text-fg-muted">Loading…</p>}
        {fixtures && fixtures.length === 0 && (
          <p className="rounded-md border border-dashed border-border-subtle px-3 py-6 text-center text-xs text-fg-muted">
            No fixtures detected. Make sure
            <code> benchmark/challenges/rust-swebench-top5/</code> exists in
            the repo root.
          </p>
        )}
        {fixtures && fixtures.length > 0 && (
          <ul className="grid grid-cols-2 gap-3">
            {fixtures.map((fx) => (
              <li
                key={fx.fixture_id}
                className="flex flex-col gap-2 rounded-md border border-border-subtle bg-bg-base p-3"
              >
                <header>
                  <p className="font-mono text-[11px] uppercase tracking-wider text-fg-muted">
                    {fx.set}
                  </p>
                  <h3 className="mt-1 text-sm font-semibold text-fg-primary">
                    {fx.display_name}
                  </h3>
                </header>
                <p className="line-clamp-3 text-xs text-fg-secondary">
                  {fx.description}
                </p>
                <p className="font-mono text-[10px] text-fg-muted truncate">
                  {fx.workspace_path}
                </p>
                <footer className="mt-1 flex items-center gap-2">
                  <button
                    type="button"
                    disabled={busyId === fx.fixture_id}
                    onClick={() => setConfirm(fx)}
                    className={cn(
                      "rounded-md border px-2 py-1 text-xs",
                      "border-status-info/40 text-status-info hover:border-status-info",
                      "disabled:opacity-50",
                    )}
                  >
                    Run in sandbox
                  </button>
                  {fx.has_reference_proof && (
                    <span className="text-[10px] text-fg-muted">
                      ✓ has reference proof
                    </span>
                  )}
                </footer>
              </li>
            ))}
          </ul>
        )}
      </div>
      {confirm && (
        <ConfirmRunModal
          fixture={confirm}
          onCancel={() => setConfirm(null)}
          onConfirm={() => start(confirm)}
        />
      )}
    </div>
  );
}

function ConfirmRunModal({
  fixture,
  onCancel,
  onConfirm,
}: {
  fixture: BenchmarkFixture;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-bench-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-bg-overlay backdrop-blur-sm"
    >
      <div className="w-full max-w-md rounded-lg border border-border-subtle bg-bg-surface p-5 shadow-2xl">
        <h2
          id="confirm-bench-title"
          className="text-sm font-semibold uppercase tracking-wider text-fg-primary"
        >
          Run benchmark in sandbox?
        </h2>
        <p className="mt-2 text-sm text-fg-secondary">{fixture.display_name}</p>
        <ul className="mt-3 flex flex-col gap-1 rounded-md bg-bg-base p-3 text-[11px] text-fg-muted">
          <li>
            • Workspace cloned to{" "}
            <code>/private/tmp/quorp/&lt;run-id&gt;/work/</code> via{" "}
            <code>cp -c -R</code>.
          </li>
          <li>• <code>sandbox-exec</code> profile enforces the boundary.</li>
          <li>• Network is denied by default.</li>
          <li>• <code>FullAuto</code> mode — no per-tool prompts.</li>
        </ul>
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-md border border-border-subtle px-3 py-1.5 text-sm hover:border-ring-focus"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded-md border border-status-info/60 bg-status-info/10 px-3 py-1.5 text-sm text-status-info hover:border-status-info"
          >
            Start run
          </button>
        </div>
      </div>
    </div>
  );
}

function defaultSandbox(): SandboxModeDto {
  return typeof navigator !== "undefined" && navigator.platform.includes("Mac")
    ? "mac_apple_sandbox"
    : "tmp_copy";
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
