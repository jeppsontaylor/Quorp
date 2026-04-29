import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Lock, ShieldCheck } from "lucide-react";

import { SessionsPanel } from "@/components/SessionsPanel";
import { ipc } from "@/lib/invoke";
import { cn } from "@/lib/utils";
import { useFeatureFlags } from "@/store/featureFlags";
import { useViewStore } from "@/store/viewStore";
import { useWorkspaceStore } from "@/store/workspaceStore";

interface Props {
  className?: string;
}

export function LeftPanel({ className }: Props) {
  const surface = useViewStore((s) => s.surface);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const refresh = useWorkspaceStore((s) => s.refresh);
  const setActive = useWorkspaceStore((s) => s.setActive);
  const trust = useWorkspaceStore((s) => s.trust);
  const multiSession = useFeatureFlags(
    (s) => s.flags.multi_session_threads,
  );

  if (surface === "sessions" && multiSession) {
    return (
      <aside
        className={cn(
          "flex h-full min-w-0 flex-col overflow-hidden border-r border-border-subtle bg-bg-surface",
          className,
        )}
      >
        <SessionsPanel />
      </aside>
    );
  }

  const onAddFolder = async () => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: "Add a workspace",
    });
    if (typeof picked !== "string") return;
    try {
      await ipc.addWorkspace(picked);
      await refresh();
    } catch (err) {
      console.error("add_workspace failed", err);
    }
  };

  return (
    <aside
      className={cn(
        "flex h-full min-w-0 flex-col overflow-hidden border-r border-border-subtle bg-bg-surface",
        className,
      )}
    >
      <header className="flex items-center justify-between border-b border-border-subtle px-3 py-2">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-fg-muted">
          {surface}
        </h2>
        {surface === "workspaces" && (
          <button
            type="button"
            onClick={onAddFolder}
            className="rounded-sm border border-border-subtle px-2 py-0.5 text-[11px] text-fg-secondary hover:border-ring-focus hover:text-fg-primary"
          >
            + Add Folder
          </button>
        )}
      </header>
      <div className="flex-1 overflow-y-auto px-2 py-2">
        {surface === "workspaces" ? (
          <ul className="flex flex-col gap-1">
            {workspaces.length === 0 && (
              <li className="rounded-md border border-dashed border-border-subtle px-3 py-6 text-center text-xs text-fg-muted">
                No workspaces yet. Click <strong>+ Add Folder</strong> above to register one.
              </li>
            )}
            {workspaces.map((w) => (
              <li
                key={w.id}
                className="group flex flex-col gap-0.5 rounded-md border border-transparent px-2 py-1.5 hover:border-border-subtle hover:bg-bg-elevated"
              >
                <button
                  type="button"
                  className="flex items-center gap-2 text-left text-sm text-fg-primary"
                  onClick={() => setActive(w.id)}
                >
                  {w.trust === "trusted" ? (
                    <ShieldCheck size={14} className="text-status-info" />
                  ) : (
                    <Lock size={14} className="text-fg-muted" />
                  )}
                  <span className="truncate">{w.display_name}</span>
                </button>
                <div className="flex items-center justify-between text-[11px] text-fg-muted">
                  <span className="truncate font-mono">
                    {w.canonical_path}
                  </span>
                </div>
                <div className="hidden items-center gap-2 pt-1 text-[11px] group-hover:flex">
                  <button
                    type="button"
                    onClick={() =>
                      trust(
                        w.id,
                        w.trust === "trusted" ? "untrusted" : "trusted",
                      )
                    }
                    className="rounded-sm border border-border-subtle px-1.5 py-0.5 hover:border-ring-focus"
                  >
                    {w.trust === "trusted" ? "Untrust" : "Trust"}
                  </button>
                  <button
                    type="button"
                    onClick={() => ipc.openTerminalAt(w.canonical_path).catch(() => {})}
                    className="rounded-sm border border-border-subtle px-1.5 py-0.5 hover:border-ring-focus"
                  >
                    Open in CLI
                  </button>
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className="px-3 py-6 text-center text-xs text-fg-muted">
            {capitalize(surface)} surface lands in PR{surface === "benchmarks" ? "6" : "7+"}.
          </p>
        )}
      </div>
    </aside>
  );
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}
