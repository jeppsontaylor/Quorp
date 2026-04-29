import { useEffect } from "react";

import { Layout } from "@/components/Layout";
import { PermissionModal } from "@/components/PermissionModal";
import { SettingsDialog } from "@/components/SettingsDialog";
import { useViewStore } from "@/store/viewStore";
import { useWorkspaceStore } from "@/store/workspaceStore";
import { useGlobalKeymap } from "@/components/useGlobalKeymap";
import { useMenuRouter } from "@/components/useMenuRouter";

export function App() {
  const refresh = useWorkspaceStore((s) => s.refresh);
  const theme = useViewStore((s) => s.theme);

  // Apply theme to <html data-theme="..."> on mount and on every change.
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  // Initial workspace load. The Tauri runtime might not be ready on
  // the very first paint (the preload bundle hits ~50ms after mount);
  // we surface IPC errors in the store, never throw from the view.
  useEffect(() => {
    refresh().catch(() => {
      /* logged in store */
    });
  }, [refresh]);

  useGlobalKeymap();
  useMenuRouter();

  return (
    <>
      <Layout />
      <PermissionModal />
      <SettingsDialog />
    </>
  );
}
