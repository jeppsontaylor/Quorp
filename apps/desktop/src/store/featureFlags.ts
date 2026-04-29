// Per-user feature flags. Persisted via tauri-plugin-store on the
// Rust side; this store mirrors them in memory so React can read
// without an async hop. Defaults are conservative: every Expansive
// surface ships off-by-default and the user opts in via Settings.

import { create } from "zustand";

export type FeatureFlagId =
  | "terminal_pane"
  | "multi_window"
  | "agents_sidebar"
  | "auto_updater"
  | "memory_editor"
  | "rules_editor"
  | "multi_session_threads"
  | "light_theme";

interface FeatureFlagState {
  flags: Record<FeatureFlagId, boolean>;
  setFlag: (id: FeatureFlagId, enabled: boolean) => void;
  toggle: (id: FeatureFlagId) => void;
  reset: () => void;
}

const DEFAULTS: Record<FeatureFlagId, boolean> = {
  terminal_pane: false,
  multi_window: false,
  agents_sidebar: false,
  auto_updater: false,
  memory_editor: false,
  rules_editor: false,
  // Multi-session is convenient enough to ship default-on; the only
  // surface change is a Sessions panel that's empty for first-time
  // users.
  multi_session_threads: true,
  // Light theme defaults off to match the existing Dark default;
  // users opt in.
  light_theme: false,
};

export const useFeatureFlags = create<FeatureFlagState>((set) => ({
  flags: { ...DEFAULTS },
  setFlag: (id, enabled) =>
    set((s) => ({ flags: { ...s.flags, [id]: enabled } })),
  toggle: (id) =>
    set((s) => ({ flags: { ...s.flags, [id]: !s.flags[id] } })),
  reset: () => set({ flags: { ...DEFAULTS } }),
}));

export const FLAG_LABELS: Record<FeatureFlagId, string> = {
  terminal_pane: "Terminal pane",
  multi_window: "Multi-window",
  agents_sidebar: "Agents sidebar",
  auto_updater: "Auto-updater (manual check)",
  memory_editor: "Memory editor",
  rules_editor: "Rules editor",
  multi_session_threads: "Multi-session threads",
  light_theme: "Light + system theme variants",
};

export const FLAG_DESCRIPTIONS: Record<FeatureFlagId, string> = {
  terminal_pane:
    "Embedded xterm.js terminal in the inspector. Lazy-loaded; off by default.",
  multi_window:
    "File → New Window opens an additional desktop window per workspace.",
  agents_sidebar:
    "Splits the timeline into Main / Verifier / Patch reviewer streams.",
  auto_updater:
    "Surfaces a manual `Check for updates` button in Settings → Updates.",
  memory_editor:
    "Read + prune entries in the 6 Quorp memory tiers from the inspector.",
  rules_editor:
    "List `.rules` files and lifecycle states (Draft / Active / Suspended).",
  multi_session_threads:
    "Sessions panel in the left rail; click to switch the active timeline.",
  light_theme:
    "Adds Light and System (auto) theme options under Settings → UI.",
};
