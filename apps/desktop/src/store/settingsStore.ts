// UI-only flag store for the settings modal. The persistent settings
// payload itself lives on the Rust side; this store only tracks
// "is the modal open" + which section is selected.

import { create } from "zustand";

export type SettingsSection =
  | "general"
  | "workspaces"
  | "provider"
  | "models"
  | "permissions"
  | "sandbox"
  | "memory"
  | "rules"
  | "ui"
  | "storage"
  | "updates"
  | "reset";

interface SettingsState {
  open: boolean;
  section: SettingsSection;
  openSettings: (section?: SettingsSection) => void;
  closeSettings: () => void;
  setSection: (section: SettingsSection) => void;
}

export const useSettingsStore = create<SettingsState>((set) => ({
  open: false,
  section: "general",
  openSettings: (section) =>
    set((s) => ({ open: true, section: section ?? s.section })),
  closeSettings: () => set({ open: false }),
  setSection: (section) => set({ section }),
}));
