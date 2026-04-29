// Single source of truth for global keyboard shortcuts. Hooked up by
// `useGlobalKeymap()` in src/lib/useGlobalKeymap.ts.

export type KeymapAction =
  | "focus_composer"
  | "send_message"
  | "cancel_run"
  | "open_palette"
  | "permission_picker"
  | "toggle_left"
  | "toggle_right"
  | "toggle_composer"
  | "open_settings"
  | "new_session"
  | "add_folder"
  | "replay_last"
  | "inspector_status"
  | "inspector_tasks"
  | "inspector_context"
  | "inspector_diff"
  | "inspector_proof"
  | "inspector_permissions"
  | "inspector_memory"
  | "inspector_rules";

export interface KeymapBinding {
  /** Human label used in About / Help and the command palette. */
  label: string;
  /** Display string used by tooltips. */
  display: string;
  /** Match against `(e.metaKey || e.ctrlKey)`, then `e.key`. */
  key: string;
  /** Require shift modifier to fire. */
  shift?: boolean;
  /** Match without Cmd/Ctrl (e.g. Esc-only bindings). */
  modless?: boolean;
}

export const KEYMAP: Record<KeymapAction, KeymapBinding> = {
  focus_composer: { label: "Focus Composer", display: "⌘L", key: "l" },
  send_message: { label: "Send", display: "⌘↩", key: "Enter" },
  cancel_run: { label: "Cancel Run", display: "⌘.", key: "." },
  open_palette: { label: "Command Palette", display: "⌘K", key: "k" },
  permission_picker: {
    label: "Permission Picker",
    display: "⌘⇧P",
    key: "P",
    shift: true,
  },
  toggle_left: { label: "Toggle Left Panel", display: "⌘B", key: "b" },
  toggle_right: { label: "Toggle Right Inspector", display: "⌘J", key: "j" },
  toggle_composer: { label: "Toggle Composer", display: "⌘/", key: "/" },
  open_settings: { label: "Settings", display: "⌘,", key: "," },
  new_session: { label: "New Session", display: "⌘N", key: "n" },
  add_folder: { label: "Add Folder…", display: "⌘O", key: "o" },
  replay_last: {
    label: "Replay Last Run",
    display: "⌘⇧R",
    key: "R",
    shift: true,
  },
  inspector_status: { label: "Inspector ▸ Status", display: "⌘1", key: "1" },
  inspector_tasks: { label: "Inspector ▸ Tasks", display: "⌘2", key: "2" },
  inspector_context: { label: "Inspector ▸ Context", display: "⌘3", key: "3" },
  inspector_diff: { label: "Inspector ▸ Diff", display: "⌘4", key: "4" },
  inspector_proof: { label: "Inspector ▸ Proof", display: "⌘5", key: "5" },
  inspector_permissions: {
    label: "Inspector ▸ Permissions",
    display: "⌘6",
    key: "6",
  },
  inspector_memory: { label: "Inspector ▸ Memory", display: "⌘7", key: "7" },
  inspector_rules: { label: "Inspector ▸ Rules", display: "⌘8", key: "8" },
};

export function matches(binding: KeymapBinding, event: KeyboardEvent): boolean {
  if (!binding.modless && !(event.metaKey || event.ctrlKey)) return false;
  if (binding.shift && !event.shiftKey) return false;
  if (!binding.shift && event.shiftKey && binding.key.length === 1) {
    // For non-shift bindings, ignore shift'd variants unless the key
    // is the shifted symbol itself (`?` for `/`, etc.).
    return false;
  }
  return event.key === binding.key || event.key.toLowerCase() === binding.key.toLowerCase();
}
