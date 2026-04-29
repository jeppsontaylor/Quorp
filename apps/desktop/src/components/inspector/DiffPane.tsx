import { useEffect, useMemo, useState } from "react";

import { ipc } from "@/lib/invoke";
import { useArtifactStore } from "@/store/artifactStore";
import { useRunStore } from "@/store/runStore";
import { useWorkspaceStore } from "@/store/workspaceStore";
import { cn } from "@/lib/utils";

/**
 * Diff inspector tab.
 *
 * Reads `<run_dir>/final.diff` via the artifact store, parses unified
 * diff hunks lazily, and renders a file tree + line-by-line view.
 * CodeMirror integration lands in PR8; PR7 ships a hand-rolled
 * renderer that's functional, accessible, and fast for the typical
 * benchmark-run footprint (a few hundred lines across <10 files).
 */
export function DiffPane() {
  const activeRunId = useRunStore((s) => s.activeRunId);
  const workspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const window = useArtifactStore((s) =>
    activeRunId && workspaceId
      ? s.windows[
          `${workspaceId}:${activeRunId}:final_diff:0:${1024 * 1024}`
        ]
      : undefined,
  );
  const fetchWindow = useArtifactStore((s) => s.fetchWindow);
  const errors = useArtifactStore((s) => s.errors);

  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [applyResult, setApplyResult] = useState<string | null>(null);

  useEffect(() => {
    if (!activeRunId || !workspaceId) return;
    fetchWindow({
      workspaceId,
      runId: activeRunId,
      kind: "final_diff",
      offset: 0,
      limit: 1024 * 1024,
    }).catch(() => {});
  }, [activeRunId, fetchWindow, workspaceId]);

  const fileGroups = useMemo(() => parseUnifiedDiff(window?.content ?? ""), [
    window?.content,
  ]);

  if (!activeRunId) {
    return <Empty>Select a run to see its diff.</Empty>;
  }

  const errorKey = workspaceId
    ? `${workspaceId}:${activeRunId}:final_diff:0:${1024 * 1024}`
    : "";
  const error = errors[errorKey];
  if (error && fileGroups.length === 0) {
    return (
      <Empty tone="warn">
        No <code>final.diff</code> on disk for this run yet.
      </Empty>
    );
  }
  if (fileGroups.length === 0) {
    return <Empty>Loading diff…</Empty>;
  }

  const summary = summarize(fileGroups);
  const activeFile =
    fileGroups.find((g) => g.path === selectedFile) ?? fileGroups[0]!;

  const onApply = async () => {
    if (!workspaceId || !activeRunId) return;
    setBusy(true);
    setApplyResult(null);
    try {
      const r = await ipc.applyRunDiff(activeRunId, workspaceId);
      setApplyResult(
        `Applied ${r.applied_files} file(s); skipped ${r.skipped_files}; conflicts ${r.conflict_files}.`,
      );
    } catch (err) {
      setApplyResult(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-border-subtle px-3 py-2 text-xs text-fg-secondary">
        <div className="flex items-center justify-between gap-2">
          <span>
            <strong>{summary.fileCount}</strong> files ·{" "}
            <span className="text-diff-add-fg">+{summary.additions}</span>{" "}
            <span className="text-diff-del-fg">−{summary.deletions}</span>
          </span>
          <button
            type="button"
            disabled={busy}
            onClick={onApply}
            className="rounded-sm border border-border-subtle px-2 py-0.5 text-[11px] hover:border-ring-focus disabled:opacity-50"
            title="Apply this diff to the source workspace"
          >
            Apply to source
          </button>
        </div>
        {applyResult && (
          <p className="mt-1 text-[11px] text-fg-muted">{applyResult}</p>
        )}
      </header>
      <div className="grid flex-1 grid-cols-[200px_1fr] overflow-hidden">
        <ul
          className="overflow-y-auto border-r border-border-subtle text-xs"
          aria-label="Changed files"
        >
          {fileGroups.map((g) => (
            <li key={g.path}>
              <button
                type="button"
                onClick={() => setSelectedFile(g.path)}
                className={cn(
                  "flex w-full flex-col gap-0.5 px-2 py-1.5 text-left hover:bg-bg-elevated",
                  activeFile.path === g.path && "bg-bg-elevated",
                )}
              >
                <span className="truncate font-mono text-fg-primary">
                  {g.path}
                </span>
                <span className="font-mono text-[10px] text-fg-muted">
                  <span className="text-diff-add-fg">+{g.additions}</span>{" "}
                  <span className="text-diff-del-fg">−{g.deletions}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
        <div className="overflow-y-auto bg-bg-base font-mono text-[12px] leading-tight">
          {activeFile.lines.map((line, i) => (
            <DiffLine key={i} line={line} />
          ))}
        </div>
      </div>
    </div>
  );
}

interface DiffLineModel {
  kind: "add" | "del" | "context" | "hunk" | "header";
  text: string;
}

interface FileDiffGroup {
  path: string;
  additions: number;
  deletions: number;
  lines: DiffLineModel[];
}

function DiffLine({ line }: { line: DiffLineModel }) {
  const cls =
    line.kind === "add"
      ? "bg-diff-add-bg text-diff-add-fg"
      : line.kind === "del"
        ? "bg-diff-del-bg text-diff-del-fg"
        : line.kind === "hunk"
          ? "bg-diff-gutter text-fg-muted"
          : line.kind === "header"
            ? "bg-bg-elevated text-fg-secondary"
            : "text-fg-secondary";
  const prefix =
    line.kind === "add" ? "+" : line.kind === "del" ? "-" : line.kind === "context" ? " " : "";
  return (
    <div className={`${cls} flex gap-1 px-2 py-[1px]`}>
      <span className="w-3 select-none text-fg-muted">{prefix}</span>
      <span className="whitespace-pre-wrap break-all">{line.text}</span>
    </div>
  );
}

function Empty({
  children,
  tone,
}: {
  children: React.ReactNode;
  tone?: "warn";
}) {
  return (
    <p
      className={cn(
        "p-4 text-center text-xs",
        tone === "warn" ? "text-status-warning" : "text-fg-muted",
      )}
    >
      {children}
    </p>
  );
}

/**
 * Parse a unified-diff body into per-file groups. Tolerant: malformed
 * sections fall through as `header` lines, never throw.
 */
export function parseUnifiedDiff(body: string): FileDiffGroup[] {
  if (!body) return [];
  const groups: FileDiffGroup[] = [];
  let current: FileDiffGroup | null = null;

  const lines = body.split(/\r?\n/);
  for (const raw of lines) {
    if (raw.startsWith("diff --git ")) {
      if (current) groups.push(current);
      const path = extractDiffGitPath(raw) ?? "(unknown)";
      current = { path, additions: 0, deletions: 0, lines: [] };
      current.lines.push({ kind: "header", text: raw });
      continue;
    }
    if (
      raw.startsWith("+++ ") ||
      raw.startsWith("--- ") ||
      raw.startsWith("index ")
    ) {
      ensure(current, () => (current = newGroup(raw)))!;
      current!.lines.push({ kind: "header", text: raw });
      continue;
    }
    if (raw.startsWith("@@ ")) {
      ensure(current, () => (current = newGroup("(unknown)")))!;
      current!.lines.push({ kind: "hunk", text: raw });
      continue;
    }
    if (current === null) {
      // Skip preamble before the first `diff --git`.
      continue;
    }
    if (raw.startsWith("+") && !raw.startsWith("+++")) {
      current.lines.push({ kind: "add", text: raw.slice(1) });
      current.additions += 1;
    } else if (raw.startsWith("-") && !raw.startsWith("---")) {
      current.lines.push({ kind: "del", text: raw.slice(1) });
      current.deletions += 1;
    } else {
      current.lines.push({
        kind: "context",
        text: raw.startsWith(" ") ? raw.slice(1) : raw,
      });
    }
  }
  if (current) groups.push(current);
  return groups;
}

function ensure<T>(value: T | null, init: () => void): T | null {
  if (value === null) init();
  return value;
}

function newGroup(path: string): FileDiffGroup {
  return { path, additions: 0, deletions: 0, lines: [] };
}

function extractDiffGitPath(line: string): string | null {
  // diff --git a/foo b/foo  →  foo
  const match = line.match(/^diff --git a\/(.+) b\/.+$/);
  return match ? (match[1] ?? null) : null;
}

function summarize(groups: FileDiffGroup[]) {
  let additions = 0;
  let deletions = 0;
  for (const g of groups) {
    additions += g.additions;
    deletions += g.deletions;
  }
  return { fileCount: groups.length, additions, deletions };
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
