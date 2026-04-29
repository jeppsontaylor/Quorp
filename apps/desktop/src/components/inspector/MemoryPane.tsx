import { useState } from "react";

import { ipc } from "@/lib/invoke";
import { useWorkspaceStore } from "@/store/workspaceStore";

const TIERS = [
  "working",
  "episodic",
  "semantic",
  "procedural",
  "negative",
  "rule",
] as const;

type Tier = (typeof TIERS)[number];

interface MemoryItem {
  id: string;
  tier: string;
  summary: string;
  score: number;
  recorded_at: string;
}

export function MemoryPane() {
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const [tier, setTier] = useState<Tier>("working");
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<MemoryItem[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const search = async () => {
    if (!activeWorkspaceId) {
      setError("Select a workspace first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await ipc.queryMemory(activeWorkspaceId, tier, query, 50);
      setItems(result.items);
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-3 text-sm">
      <header>
        <h3 className="text-base font-semibold text-fg-primary">Memory</h3>
        <p className="mt-1 text-xs text-fg-muted">
          Search across the six Quorp memory tiers backed by{" "}
          <code>quorp_memory</code>. Pruning lands once
          <code> Memory::prune_older_than </code>is added upstream.
        </p>
      </header>
      {!activeWorkspaceId && (
        <p className="rounded-md border border-status-warning/40 bg-bg-base px-3 py-2 text-xs text-status-warning">
          Pick a workspace from the sidebar to enable memory queries.
        </p>
      )}
      <div className="flex gap-2">
        <select
          value={tier}
          onChange={(e) => setTier(e.target.value as Tier)}
          className="rounded-md border border-border-subtle bg-bg-base px-2 py-1 text-xs"
          disabled={!activeWorkspaceId || busy}
        >
          {TIERS.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") search();
          }}
          placeholder="Query…"
          className="flex-1 rounded-md border border-border-subtle bg-bg-base px-2 py-1 font-mono text-sm outline-none focus:border-ring-focus"
          disabled={!activeWorkspaceId || busy}
        />
        <button
          type="button"
          onClick={search}
          disabled={busy || !activeWorkspaceId}
          className="rounded-md border border-border-subtle px-3 py-1 text-xs hover:border-ring-focus disabled:opacity-50"
        >
          Search
        </button>
      </div>
      {error && <p className="text-xs text-status-danger">{error}</p>}
      <ul className="flex flex-col gap-1.5">
        {items === null && (
          <li className="rounded-md border border-dashed border-border-subtle px-3 py-4 text-center text-xs text-fg-muted">
            Run a query to see results.
          </li>
        )}
        {items && items.length === 0 && (
          <li className="rounded-md border border-dashed border-border-subtle px-3 py-4 text-center text-xs text-fg-muted">
            No entries in this tier yet.
          </li>
        )}
        {items?.map((it) => (
          <li
            key={it.id}
            className="rounded-md border border-border-subtle bg-bg-base p-2 text-xs"
          >
            <div className="flex items-baseline justify-between gap-2 text-[10px] text-fg-muted">
              <span className="font-mono uppercase">{it.tier}</span>
              <span className="font-mono">score {it.score.toFixed(2)}</span>
            </div>
            <p className="mt-1 text-fg-primary">{it.summary}</p>
          </li>
        ))}
      </ul>
    </div>
  );
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
