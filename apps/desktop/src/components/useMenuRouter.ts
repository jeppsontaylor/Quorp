import { useEffect } from "react";

import { listen } from "@tauri-apps/api/event";

import { useSettingsStore } from "@/store/settingsStore";
import { useViewStore } from "@/store/viewStore";

/**
 * Listens for `menu://event` payloads emitted by `src-tauri/menu.rs`
 * and routes them to the matching store mutation. Keeps the menu bar
 * in sync with the keymap so `Cmd+B` and `View → Toggle Left Panel`
 * fire the same code path.
 */
export function useMenuRouter() {
  const toggleLeft = useViewStore((s) => s.toggleLeft);
  const toggleRight = useViewStore((s) => s.toggleRight);
  const setTheme = useViewStore((s) => s.setTheme);
  const theme = useViewStore((s) => s.theme);
  const setSurface = useViewStore((s) => s.setSurface);
  const openSettings = useSettingsStore((s) => s.openSettings);

  useEffect(() => {
    const unsubscribePromise = listen<string>("menu://event", (event) => {
      const id = event.payload;
      switch (id) {
        case "toggle_left":
          toggleLeft();
          break;
        case "toggle_right":
          toggleRight();
          break;
        case "toggle_high_contrast":
          setTheme(theme === "quorp-high-contrast" ? "quorp-dark" : "quorp-high-contrast");
          break;
        case "toggle_no_color":
          setTheme(theme === "quorp-no-color" ? "quorp-dark" : "quorp-no-color");
          break;
        case "doctor":
          setSurface("doctor");
          break;
        case "benchmarks":
          setSurface("benchmarks");
          break;
        case "settings":
          openSettings();
          break;
        default:
          break;
      }
    });
    return () => {
      unsubscribePromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [openSettings, setSurface, setTheme, theme, toggleLeft, toggleRight]);
}
