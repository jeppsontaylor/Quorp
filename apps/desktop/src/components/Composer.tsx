import { useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";

import { ipc } from "@/lib/invoke";
import { useRunStore } from "@/store/runStore";
import { useWorkspaceStore } from "@/store/workspaceStore";
import { useViewStore } from "@/store/viewStore";
import { cn } from "@/lib/utils";
import type {
  DesktopEvent,
  PermissionModeDto,
  SandboxModeDto,
} from "@/types/ipc";

const PERMISSION_MODES: PermissionModeDto[] = [
  "read_only",
  "ask",
  "accept_edits",
  "auto_safe",
  "yolo_sandbox",
];

const SANDBOX_MODES: SandboxModeDto[] = [
  "tmp_copy",
  "git_worktree",
  "mac_apple_sandbox",
  "host",
];

export function Composer() {
  const expanded = useViewStore((s) => s.composerExpanded);
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const applyEvent = useRunStore((s) => s.applyEvent);
  const setActive = useRunStore((s) => s.setActive);

  const [goal, setGoal] = useState("");
  const [permission, setPermission] = useState<PermissionModeDto>("ask");
  const [sandbox, setSandbox] = useState<SandboxModeDto>(
    typeof navigator !== "undefined" && navigator.platform.includes("Mac")
      ? "mac_apple_sandbox"
      : "tmp_copy",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const canSend = !!activeWorkspaceId && goal.trim().length > 0 && !busy;

  const send = async () => {
    if (!canSend || !activeWorkspaceId) return;
    setBusy(true);
    setError(null);
    try {
      const channel = new Channel<DesktopEvent>();
      channel.onmessage = (event) => {
        applyEvent(event);
        if (event.kind === "run_started") setActive(event.run_id);
      };
      await ipc.startAgentRun(
        {
          workspace_id: activeWorkspaceId,
          goal: goal.trim(),
          permission_mode: permission,
          sandbox_mode: sandbox,
          model_id: null,
          wall_clock_budget_seconds: null,
        },
        channel,
      );
      setGoal("");
    } catch (err) {
      setError(stringifyError(err));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown: React.KeyboardEventHandler<HTMLTextAreaElement> = (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      send();
    }
  };

  return (
    <footer
      className={cn(
        "border-t border-border-subtle bg-bg-surface px-3 py-2",
        expanded ? "h-[280px]" : "h-composer",
      )}
    >
      <div className="flex items-center gap-2 text-xs">
        <Chip
          label={`mode: ${permission.replaceAll("_", " ")}`}
          options={PERMISSION_MODES}
          onPick={setPermission}
          format={(m) => m.replaceAll("_", " ")}
        />
        <Chip
          label={`sandbox: ${sandbox.replaceAll("_", " ")}`}
          options={SANDBOX_MODES}
          onPick={setSandbox}
          format={(m) => m.replaceAll("_", " ")}
        />
        <span className="ml-auto font-mono text-fg-muted">
          ⌘↩ send · ⌘. cancel · ⌘K palette
        </span>
      </div>
      <textarea
        ref={textareaRef}
        className={cn(
          "mt-2 block w-full resize-none rounded-md border border-border-subtle bg-bg-base p-2 font-mono text-sm text-fg-primary",
          "outline-none focus:border-ring-focus",
          expanded ? "h-[210px]" : "h-[80px]",
        )}
        placeholder={
          activeWorkspaceId
            ? "Describe what the agent should do…"
            : "Add a workspace from the left panel before starting a run."
        }
        value={goal}
        disabled={!activeWorkspaceId || busy}
        onChange={(e) => setGoal(e.target.value)}
        onKeyDown={onKeyDown}
      />
      {error && (
        <p className="mt-1 text-xs text-status-danger" role="alert">
          {error}
        </p>
      )}
    </footer>
  );
}

interface ChipProps<T extends string> {
  label: string;
  options: T[];
  onPick: (value: T) => void;
  format: (value: T) => string;
}

function Chip<T extends string>({ label, options, onPick, format }: ChipProps<T>) {
  return (
    <details className="group relative">
      <summary
        className="cursor-pointer list-none rounded-sm border border-border-subtle px-2 py-0.5 font-mono text-[11px] text-fg-secondary group-open:border-ring-focus"
      >
        {label}
      </summary>
      <ul className="absolute left-0 z-10 mt-1 min-w-[10rem] rounded-md border border-border-subtle bg-bg-elevated py-1 text-[11px] shadow-lg">
        {options.map((opt) => (
          <li key={opt}>
            <button
              type="button"
              onClick={(e) => {
                onPick(opt);
                (
                  e.currentTarget.closest("details") as HTMLDetailsElement | null
                )?.removeAttribute("open");
              }}
              className="block w-full px-2 py-1 text-left text-fg-secondary hover:bg-bg-surface hover:text-fg-primary"
            >
              {format(opt)}
            </button>
          </li>
        ))}
      </ul>
    </details>
  );
}

function stringifyError(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}
