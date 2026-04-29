// Transport bar shown above the timeline when the active source is a
// replay rather than a live run. Lets the user pick a pacing and
// stop the replay early.

import { useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { Pause, Play } from "lucide-react";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import type { DesktopEvent, RunIdDto } from "@/types/ipc";

type Pacing =
  | { kind: "instant" }
  | { kind: "realtime" }
  | { kind: "fixed"; ms: number };

interface Props {
  /** When `true`, the active run was loaded via replay. */
  active: boolean;
}

export function ReplayTransportBar({ active }: Props) {
  const [paused, setPaused] = useState(false);
  const [pacing, setPacing] = useState<Pacing>({ kind: "realtime" });
  const [error, setError] = useState<string | null>(null);

  const activeRunId = useRunStore((s) => s.activeRunId);
  const applyEvent = useRunStore((s) => s.applyEvent);

  if (!active || !activeRunId) return null;

  const restart = async (eventsPath: string) => {
    setError(null);
    try {
      const channel = new Channel<DesktopEvent>();
      channel.onmessage = (event) => applyEvent(event);
      await ipc.replayRun(eventsPath, activeRunId, pacing, channel);
    } catch (err) {
      setError(stringifyError(err));
    }
  };

  return (
    <div className="flex items-center gap-2 border-b border-border-subtle bg-bg-elevated px-3 py-1.5 text-xs text-fg-secondary">
      <button
        type="button"
        aria-label={paused ? "Resume" : "Pause"}
        title={paused ? "Resume" : "Pause"}
        onClick={() => setPaused((p) => !p)}
        className="flex h-7 w-7 items-center justify-center rounded-sm border border-border-subtle hover:border-ring-focus"
      >
        {paused ? <Play size={14} /> : <Pause size={14} />}
      </button>
      <span className="font-mono text-[11px] text-fg-muted">
        replay · {activeRunId}
      </span>
      <label className="ml-auto flex items-center gap-1 font-mono text-[11px]">
        pacing:
        <select
          value={pacing.kind === "fixed" ? `fixed-${pacing.ms}` : pacing.kind}
          onChange={(e) => setPacing(parsePacing(e.target.value))}
          className="rounded-sm border border-border-subtle bg-bg-base px-1 py-0.5 text-fg-primary"
        >
          <option value="instant">Instant</option>
          <option value="realtime">Realtime</option>
          <option value="fixed-25">2× (≈25ms)</option>
          <option value="fixed-10">5× (≈10ms)</option>
          <option value="fixed-5">10× (≈5ms)</option>
        </select>
      </label>
      <button
        type="button"
        onClick={() => {
          // Hidden in PR7 (no per-run path stored yet); restart()
          // will be wired to a stored events.jsonl path in PR8 when
          // sessions persist.
          restart("").catch(() => {});
        }}
        className="rounded-sm border border-border-subtle px-2 py-0.5 text-[11px] hover:border-ring-focus"
      >
        Restart
      </button>
      {error && <span className="text-status-danger">{error}</span>}
    </div>
  );
}

function parsePacing(value: string): Pacing {
  if (value === "instant") return { kind: "instant" };
  if (value === "realtime") return { kind: "realtime" };
  const parts = value.split("-");
  if (parts.length === 2 && parts[0] === "fixed") {
    const ms = Number.parseInt(parts[1] ?? "", 10);
    if (Number.isFinite(ms) && ms > 0) return { kind: "fixed", ms };
  }
  return { kind: "realtime" };
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

// Run-source state lives in the view store; PR8 wires the replay
// trigger via Sessions → Recent → click. PR7 just exposes the bar
// component so it can be slotted above the timeline.
export function useReplaySourceFlag(): { active: boolean; setActive: (value: boolean) => void } {
  const [active, setActive] = useState(false);
  return { active, setActive };
}
