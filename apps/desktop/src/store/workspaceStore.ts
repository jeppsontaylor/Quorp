import { create } from "zustand";

import { ipc } from "@/lib/invoke";
import type { TrustDecision, WorkspaceId, WorkspaceSummary } from "@/types/ipc";

interface WorkspaceState {
  workspaces: WorkspaceSummary[];
  activeWorkspaceId: WorkspaceId | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  setActive: (id: WorkspaceId | null) => void;
  trust: (id: WorkspaceId, decision: TrustDecision) => Promise<void>;
  remove: (id: WorkspaceId) => Promise<void>;
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  workspaces: [],
  activeWorkspaceId: null,
  loading: false,
  error: null,
  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const list = await ipc.listWorkspaces();
      set({ workspaces: list, loading: false });
    } catch (err) {
      set({ loading: false, error: stringifyError(err) });
    }
  },
  setActive: (id) => set({ activeWorkspaceId: id }),
  trust: async (id, decision) => {
    try {
      await ipc.trustWorkspace(id, decision);
      await get().refresh();
    } catch (err) {
      set({ error: stringifyError(err) });
    }
  },
  remove: async (id) => {
    try {
      await ipc.removeWorkspace(id);
      await get().refresh();
      if (get().activeWorkspaceId === id) {
        set({ activeWorkspaceId: null });
      }
    } catch (err) {
      set({ error: stringifyError(err) });
    }
  },
}));

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
