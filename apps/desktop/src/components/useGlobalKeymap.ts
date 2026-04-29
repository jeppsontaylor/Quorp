import { useEffect } from "react";

import { KEYMAP, matches, type KeymapAction } from "@/lib/keymap";
import { useRunStore } from "@/store/runStore";
import { useSettingsStore } from "@/store/settingsStore";
import { useViewStore } from "@/store/viewStore";
import { ipc } from "@/lib/invoke";

export function useGlobalKeymap() {
  const toggleLeft = useViewStore((s) => s.toggleLeft);
  const toggleRight = useViewStore((s) => s.toggleRight);
  const toggleComposer = useViewStore((s) => s.toggleComposer);
  const setInspectorTab = useViewStore((s) => s.setInspectorTab);
  const activeRunId = useRunStore((s) => s.activeRunId);
  const openSettings = useSettingsStore((s) => s.openSettings);

  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      const action = matchAction(event);
      if (!action) return;
      event.preventDefault();
      switch (action) {
        case "cancel_run":
          if (activeRunId) ipc.cancelRun(activeRunId).catch(() => {});
          return;
        case "open_settings":
          openSettings();
          return;
        case "toggle_left":
          toggleLeft();
          return;
        case "toggle_right":
          toggleRight();
          return;
        case "toggle_composer":
          toggleComposer();
          return;
        case "inspector_status":
          setInspectorTab("status");
          return;
        case "inspector_tasks":
          setInspectorTab("tasks");
          return;
        case "inspector_context":
          setInspectorTab("context");
          return;
        case "inspector_diff":
          setInspectorTab("diff");
          return;
        case "inspector_proof":
          setInspectorTab("proof");
          return;
        case "inspector_permissions":
          setInspectorTab("permissions");
          return;
        case "inspector_memory":
          setInspectorTab("memory");
          return;
        case "inspector_rules":
          setInspectorTab("rules");
          return;
        default:
          // Other shortcuts are handled by their own components
          // (composer focus, palette, settings, etc.). Falling
          // through is fine.
          return;
      }
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [
    activeRunId,
    openSettings,
    setInspectorTab,
    toggleComposer,
    toggleLeft,
    toggleRight,
  ]);
}

function matchAction(event: KeyboardEvent): KeymapAction | null {
  for (const [name, binding] of Object.entries(KEYMAP)) {
    if (matches(binding, event)) return name as KeymapAction;
  }
  return null;
}
