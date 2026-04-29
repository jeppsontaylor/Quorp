import { create } from "zustand";

export type Surface =
  | "workspaces"
  | "sessions"
  | "tasks"
  | "rules"
  | "memory"
  | "benchmarks"
  | "doctor";

export type InspectorTab =
  | "status"
  | "tasks"
  | "context"
  | "diff"
  | "proof"
  | "permissions"
  | "memory"
  | "rules";

export type Theme =
  | "quorp-dark"
  | "quorp-light"
  | "quorp-system"
  | "quorp-high-contrast"
  | "quorp-no-color";

interface ViewState {
  surface: Surface;
  inspectorTab: InspectorTab;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  composerExpanded: boolean;
  theme: Theme;
  setSurface: (surface: Surface) => void;
  setInspectorTab: (tab: InspectorTab) => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  toggleComposer: () => void;
  setTheme: (theme: Theme) => void;
}

export const useViewStore = create<ViewState>((set) => ({
  surface: "workspaces",
  inspectorTab: "status",
  leftCollapsed: false,
  rightCollapsed: false,
  composerExpanded: false,
  theme: "quorp-dark",
  setSurface: (surface) => set({ surface }),
  setInspectorTab: (inspectorTab) => set({ inspectorTab }),
  toggleLeft: () => set((s) => ({ leftCollapsed: !s.leftCollapsed })),
  toggleRight: () => set((s) => ({ rightCollapsed: !s.rightCollapsed })),
  toggleComposer: () => set((s) => ({ composerExpanded: !s.composerExpanded })),
  setTheme: (theme) => {
    set({ theme });
    document.documentElement.dataset.theme = theme;
  },
}));
