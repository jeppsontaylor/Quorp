// Lazy fetcher for per-run artifacts. Holds windows in memory for
// the active run; older runs page on demand. The store does not
// persist across reloads — it's a UI-side cache layered on top of
// `quorp_desktop_core::ArtifactStore` (the Rust LRU is the source of
// truth and survives the JS reload).

import { create } from "zustand";

import { ipc, type ArtifactKind } from "@/lib/invoke";
import type { ArtifactWindow, RunIdDto, WorkspaceId } from "@/types/ipc";

interface FetchKey {
  workspaceId: WorkspaceId;
  runId: RunIdDto;
  kind: ArtifactKind;
  offset: number;
  limit: number;
}

function keyOf(k: FetchKey): string {
  return `${k.workspaceId}:${k.runId}:${k.kind}:${k.offset}:${k.limit}`;
}

interface ArtifactState {
  windows: Record<string, ArtifactWindow>;
  kinds: Record<string, ArtifactKind[]>;
  pending: Record<string, Promise<unknown>>;
  errors: Record<string, string>;
  fetchKinds: (
    workspaceId: WorkspaceId,
    runId: RunIdDto,
  ) => Promise<ArtifactKind[]>;
  fetchWindow: (
    key: FetchKey,
  ) => Promise<ArtifactWindow | null>;
  clearForRun: (runId: RunIdDto) => void;
}

export const useArtifactStore = create<ArtifactState>((set, get) => ({
  windows: {},
  kinds: {},
  pending: {},
  errors: {},
  fetchKinds: async (workspaceId, runId) => {
    const key = `${workspaceId}:${runId}:__kinds`;
    const cached = get().kinds[`${workspaceId}:${runId}`];
    if (cached) return cached;
    const inflight = get().pending[key];
    if (inflight) {
      const result = (await inflight) as ArtifactKind[] | undefined;
      return result ?? [];
    }
    const promise = ipc
      .listRunArtifacts(workspaceId, runId)
      .then((kinds) => {
        set((s) => ({
          kinds: { ...s.kinds, [`${workspaceId}:${runId}`]: kinds },
        }));
        return kinds;
      })
      .catch((err) => {
        set((s) => ({
          errors: { ...s.errors, [key]: stringifyError(err) },
        }));
        return [] as ArtifactKind[];
      })
      .finally(() => {
        set((s) => {
          const next = { ...s.pending };
          delete next[key];
          return { pending: next };
        });
      });
    set((s) => ({ pending: { ...s.pending, [key]: promise } }));
    return promise;
  },
  fetchWindow: async (key) => {
    const cacheKey = keyOf(key);
    const cached = get().windows[cacheKey];
    if (cached) return cached;
    const inflight = get().pending[cacheKey];
    if (inflight) {
      const result = (await inflight) as ArtifactWindow | null | undefined;
      return result ?? null;
    }
    const promise = ipc
      .readArtifact(
        key.workspaceId,
        key.runId,
        key.kind,
        key.offset,
        key.limit,
      )
      .then((win) => {
        set((s) => ({ windows: { ...s.windows, [cacheKey]: win } }));
        return win;
      })
      .catch((err) => {
        set((s) => ({
          errors: { ...s.errors, [cacheKey]: stringifyError(err) },
        }));
        return null;
      })
      .finally(() => {
        set((s) => {
          const next = { ...s.pending };
          delete next[cacheKey];
          return { pending: next };
        });
      });
    set((s) => ({ pending: { ...s.pending, [cacheKey]: promise } }));
    return promise;
  },
  clearForRun: (runId) =>
    set((s) => {
      const windows = Object.fromEntries(
        Object.entries(s.windows).filter(
          ([k]) => !k.includes(`:${runId}:`),
        ),
      );
      const kinds = Object.fromEntries(
        Object.entries(s.kinds).filter(([k]) => !k.endsWith(`:${runId}`)),
      );
      return { windows, kinds };
    }),
}));

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
